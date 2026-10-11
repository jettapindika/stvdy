create table reports (                   -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  outline jsonb,
  body_md text,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on reports (user_id, workspace_id);

-- Holds the losing body when the same report is edited on two devices: the spec's
-- conflict rule says to keep the latest and store the losing version "as a
-- recoverable copy". Server-side recovery data: not synced, no triggers.
create table report_revisions (
  id uuid primary key default gen_random_uuid(),
  report_id uuid not null references reports on delete cascade,
  user_id uuid not null references users on delete cascade,
  body_md text not null,
  replaced_at timestamptz not null default now()
);
create index on report_revisions (report_id, replaced_at desc);
