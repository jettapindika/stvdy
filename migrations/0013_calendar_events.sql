-- Calendar. This file must run AFTER 0012 because `events.task_id` references
-- `tasks(id)`.

-- Server-side only; devices never hold Google tokens. The refresh token is
-- encrypted at rest with aes-gcm using a key from env.
create table calendar_connections (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  provider text not null,                -- google | ics
  account_email text,
  refresh_token_enc bytea,
  sync_token text,
  ics_url text,
  last_synced_at timestamptz
);
create index on calendar_connections (user_id, provider);

create table events (                    -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid references workspaces on delete set null,
  kind text not null,                    -- class | exam | study_block | deadline | external
  title text not null,
  starts_at timestamptz not null,
  ends_at timestamptz not null,
  external_id text,
  etag text,
  connection_id uuid references calendar_connections on delete set null,
  source text not null default 'app',    -- app | google | ics
  -- The spec declares this as a bare uuid. A link without a FK drifts, so it
  -- gets a real reference; planner- and agent-written events would otherwise
  -- orphan silently when a task is deleted.
  task_id uuid references tasks(id) on delete set null,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on events (user_id, starts_at);
create index on events (task_id);
-- Incremental Google sync matches on (connection, external_id), so two rows for
-- the same remote event must not exist.
create unique index on events (connection_id, external_id)
  where external_id is not null;
