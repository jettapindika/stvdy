-- Study OS: tasks, goals, sessions, settings, agent proposals.

create table tasks (                     -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid references workspaces on delete set null,
  parent_id uuid references tasks on delete cascade,
  title text not null,
  notes text,
  due_at timestamptz,
  estimate_min int,
  priority int not null default 2,       -- 1 high, 2 normal, 3 low
  status text not null default 'todo',   -- todo | doing | done
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on tasks (user_id, status, due_at);
create index on tasks (parent_id);

-- Exams / big targets that drive the planner.
create table goals (                     -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  target_date timestamptz not null,
  source_ids uuid[],
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on goals (user_id, target_date);

-- APPEND-ONLY, client-generated id.
create table study_sessions (            -- +sync
  id uuid primary key,
  user_id uuid not null references users on delete cascade,
  workspace_id uuid references workspaces on delete set null,
  task_id uuid references tasks on delete set null,
  started_at timestamptz not null,
  duration_min int not null,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on study_sessions (user_id, started_at);

-- Synced, but keyed by user_id: no `id`, no tombstones. Uses
-- log_change_by_user() rather than log_change().
create table user_settings (
  user_id uuid primary key references users on delete cascade,
  study_start_hour int not null default 8,
  study_end_hour int not null default 22,
  max_study_min_per_day int not null default 240,
  block_len_min int not null default 50,
  break_min int not null default 10,
  reminder_channels jsonb not null default '["web"]',
  updated_at timestamptz not null default now()
);

-- Agent proposals awaiting confirmation; surfaced on every device.
-- The spec's DDL omits the `-- +sync` marker here, but the spec also requires
-- pending actions be "visible and confirmable from any signed-in device", so it
-- carries the sync columns and triggers like every other synced entity.
create table pending_actions (           -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  tool text not null,
  args jsonb not null,
  status text not null default 'pending',-- pending | confirmed | rejected | expired
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on pending_actions (user_id, status);
