-- Chat history: one thread per workspace.
--
-- Server-owned in the sense that messages are written by POST /workspaces/:id/chat,
-- never by a client push: accepting a push would skip the credit spend and let a
-- client forge assistant turns with fabricated citations.
create table messages (                  -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  role text not null,              -- user | assistant
  content text not null,
  citations jsonb not null default '[]',  -- [{type:"chunk",chunk_id,source_id,page} | {type:"web",url,title}]
  used_web boolean not null default false,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on messages (user_id, workspace_id, created_at);
