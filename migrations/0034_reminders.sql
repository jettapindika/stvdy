-- Reminders: server-owned scheduling state (A283).
--
-- Deliberately NOT `+sync`. The client's local notifications are computed from *synced*
-- events, tasks and settings; this table exists so a reminder still fires when no client is
-- running, and the server's copy is a scheduling fact rather than a document the student
-- edits. There is no `Entity` variant for either table, and `0017_sync_triggers.sql` must stay
-- the last trigger-attaching migration — so adding one here would move that boundary for a
-- table no client ever pushes. `crates/server/tests/schema.rs` therefore gets two
-- `documented` rows, the `search_cache` precedent.
--
-- `kind` is free text and deliberately carries **no** check constraint. The engine's own
-- values are `task_due | event_soon | cards_due` (`stvdy_core::reminders::ReminderKind`), but
-- the table is the delivery queue for *any* producer: A300 routes Phase 13's renewal
-- reminders through it, and a closed list here would make a new producer a migration. The
-- Rust enum is the closed set where it matters — in the engine that derives the rows.
--
-- `status` and `skip_reason` **are** constrained, because they are engine-owned vocabulary
-- and the pair is load-bearing: a skipped reminder must say why (a reminder that silently
-- never arrives is what the student reports as "reminders are broken"), and a pending or sent
-- one must not carry a reason for a skip that did not happen. The paired check makes the
-- half-specified row impossible rather than relying on every reader to notice.
create table reminders (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  kind text not null,                     -- task_due | event_soon | cards_due, or a producer's own
  subject_id uuid,                        -- the task/event id; null for cards_due
  subject_key text not null,              -- the id, or the local date for cards_due
  local_date date not null,               -- the dedupe axis, in the user's zone
  fire_at timestamptz not null,
  dedupe_key text not null,               -- reminders::dedupe_key (A284)
  status text not null default 'pending',
  skip_reason text,
  origin text not null default 'server',
  created_at timestamptz not null default now(),
  sent_at timestamptz,
  constraint reminders_status_check check (status in ('pending', 'sent', 'skipped')),
  constraint reminders_origin_check check (origin in ('server', 'client')),
  constraint reminders_skip_reason_check check (
    (status = 'skipped') = (skip_reason is not null)
  )
);

-- The one key, so the second device's identical reminder loses (A285). Not a plain unique
-- constraint, because a *partial* uniqueness rule would be the shape to reach for if a
-- skipped reminder were ever allowed to be replaced; today every row is keyed.
create unique index reminders_dedupe_key_key on reminders (dedupe_key);

-- The sweep's own index: "this account's pending reminders due by now", which is the query
-- `send_reminder` and `GET /reminders` both run.
create index reminders_user_status_fire_idx on reminders (user_id, status, fire_at);

create table reminder_deliveries (
  id uuid primary key default gen_random_uuid(),
  reminder_id uuid not null references reminders on delete cascade,
  channel text not null,                  -- web | push | email | telegram | discord
  device_id uuid references devices on delete set null,
  status text not null,
  reason text,                            -- channel_not_implemented, quiet_hours, …
  error text,
  created_at timestamptz not null default now(),
  constraint reminder_deliveries_status_check check (status in ('sent', 'failed', 'skipped'))
);

-- One delivery per channel per device, and one per channel when there is no device (email,
-- in-app). Two partial indexes rather than one with a coalesced sentinel: a sentinel uuid
-- would be a value that means "no device" sitting in a column that references `devices`, and
-- the two cases are genuinely different rows — a push delivery is per device, an email one is
-- per account.
create unique index reminder_deliveries_per_device_key
  on reminder_deliveries (reminder_id, channel, device_id)
  where device_id is not null;

create unique index reminder_deliveries_per_account_key
  on reminder_deliveries (reminder_id, channel)
  where device_id is null;
