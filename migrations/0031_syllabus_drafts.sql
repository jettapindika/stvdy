-- Syllabus extraction drafts.
--
-- Server-owned, and deliberately NOT a `+sync` table (A251): the model produced the
-- extraction, the student confirms it, and only the confirmed items become events, tasks
-- and goals. There is no `Entity` variant for a draft, and adding one would mean attaching
-- a trigger from `0017_sync_triggers.sql`, which must stay the last migration that attaches
-- one. The precedent is `0020_search_cache` and `0023_report_generation`, neither of which
-- touches the trigger array.
--
-- The unique index on `source_id` is what makes the extraction idempotent: a retried
-- `ingest_source` updates the same draft rather than appending a second one, and
-- `GET /syllabus/{id}/draft` has exactly one row to read.
create table syllabus_drafts (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  source_id uuid not null references sources on delete cascade,
  -- extracting | ready | failed | confirmed. `confirmed` is terminal: a second confirm
  -- answers 409 rather than writing the events, tasks and goals a second time.
  status text not null default 'extracting',
  -- The model's reply, stored verbatim so the review list is recomputed from it on read
  -- rather than stored twice: one extraction, one place a field can be lost.
  extraction jsonb,
  prompt_version text,
  error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create unique index on syllabus_drafts (source_id);
create index on syllabus_drafts (user_id, status);
