-- Background jobs. No Redis: a polling worker in the server binary claims rows
-- with FOR UPDATE SKIP LOCKED, ordered by run_at.
create table jobs (
  id uuid primary key default gen_random_uuid(),
  kind text not null,                    -- ingest_source | generate_flashcards | generate_quiz
                                         -- | generate_report_section | generate_cheatsheet
                                         -- | render_cheatsheet_pdf | plan_goal | sync_calendar
                                         -- | send_reminder | send_push | poll_payment
                                         -- | reconcile_payments | monthly_grant
                                         -- | renewal_reminder | reset_counters
                                         -- | compact_change_log | purge_deleted_accounts
  payload jsonb not null,
  status text not null default 'queued', -- queued | running | done | failed
  attempts int not null default 0,
  error text,
  -- Singleton recurring jobs (monthly_grant, compact_change_log, poll_payment,
  -- reconcile_payments) must not double-run across restarts or instances: a
  -- duplicate monthly_grant would double-grant credits.
  dedupe_key text,
  run_at timestamptz not null default now(),
  created_at timestamptz default now()
);
-- The worker's claim query.
create index on jobs (status, run_at);
create unique index on jobs (dedupe_key) where dedupe_key is not null;
