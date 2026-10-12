-- Calendar connections. `calendar_connections` and `events` come from 0013; this adds
-- the state a poll and a two-way sync need. No new table, so 0017 is untouched: `events`
-- is already in its trigger array, and `calendar_connections` is deliberately not a
-- synced entity (devices never hold a Google token, docs/SPEC.md § Calendar integration).

alter table calendar_connections
  add column created_at timestamptz not null default now(),
  add column updated_at timestamptz not null default now(),
  -- `connected | error | revoked`. `error` is not a failure of the row: the events the
  -- connection did import stay, and the screen shows the reason beside it (A247).
  add column status text not null default 'connected',
  add column error text,
  -- The ICS conditional GET's validators (A239). Without them every 15-minute poll
  -- re-upserts every row, and because `events` is +sync that writes a change_log entry
  -- per row per poll and every device pulls the whole calendar every 15 minutes.
  add column ics_etag text,
  add column ics_last_modified text,
  -- The dedicated "Study Plan" calendar (A237), found or created at connect.
  add column study_calendar_id text,
  -- Which mode the last pass ran in (A236): "an incremental sync that silently stopped
  -- being incremental" is otherwise invisible, because every poll still succeeds.
  add column last_sync_was_full boolean not null default true,
  -- Reserved for the watch channels the spec allows as an optimisation. Polling is the
  -- fallback the spec names and the one this phase implements; the columns exist so
  -- adding a channel is not a migration in a later phase.
  add column watch_channel_id text,
  add column watch_expires_at timestamptz;

-- The poll's query is "live connections whose last_synced_at is older than the
-- interval", so the index has to carry the watermark.
create index on calendar_connections (provider, last_synced_at);

-- Two Google accounts for one user are two rows, which the existing
-- `index on (user_id, provider)` already allows. The cap is `google_accounts_max`
-- through the entitlements engine (A245), not a unique index: a plan can change, and a
-- constraint that refuses the row would be a second source of truth for what a plan grants.
