create table workspaces (                -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  name text not null,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on workspaces (user_id);

-- Metadata only; chunks stay server-side.
create table sources (                   -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  kind text not null,              -- pdf | pptx | docx | url | text | web | syllabus
  title text not null,
  url text,
  file_key text,                   -- object storage key of the original file
  file_size bigint,
  mime text,
  status text not null default 'pending', -- pending | processing | ready | failed
  error text,
  read_only boolean not null default false, -- set when over plan limits after downgrade
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on sources (user_id, workspace_id);
create index on sources (status);

-- Server-only, never synced: embeddings and full text stay on the server.
create table chunks (
  id uuid primary key default gen_random_uuid(),
  source_id uuid not null references sources on delete cascade,
  page int,
  idx int not null,
  content text not null,
  content_hash text not null,
  -- Which embedding model produced `embedding`. Part of the dedupe key: without
  -- it, switching EMBEDDER would reuse vectors from a different vector space and
  -- silently corrupt retrieval.
  embedder text not null,
  tsv tsvector generated always as (to_tsvector('simple', content)) stored,
  embedding vector(768)            -- must match the configured embedder dimension
);
create index on chunks using hnsw (embedding vector_cosine_ops);
create index on chunks using gin (tsv);
-- NON-unique on purpose. `chunks` has no user_id (it is scoped through
-- source_id -> sources.workspace_id -> workspaces.user_id), so a unique index
-- would be global across all accounts: the first user to upload a widely shared
-- textbook would make ingestion FAIL for every other user uploading the same
-- file, and the constraint error would leak content existence across tenants.
-- Dedupe is a per-user lookup that copies the existing embedding into the new
-- row; this index just makes that lookup cheap.
create index on chunks (embedder, content_hash);
create index on chunks (source_id, idx);
