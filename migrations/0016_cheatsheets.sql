-- Print-ready cheatsheet documents (PDF export).
--
-- Only metadata syncs: the rendered HTML and PDF are large, never edited
-- on-device, and live in object storage behind the keys below.
create table cheatsheets (               -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  scope text not null,                   -- e.g. "UTS Jaringan Komputer"
  exam_date date,
  depth text not null default 'normal',  -- brief | normal | thorough
  source_ids uuid[] not null default '{}',
  status text not null default 'pending',-- pending | generating | ready | failed
  error text,
  -- The validated JSON blocks, in object storage rather than a jsonb column:
  -- this table is +sync, so a jsonb body would ship tens of KB of document text
  -- to every device on every change. The blocks are the source of truth for the
  -- document, so a template fix can re-render every existing cheatsheet
  -- deterministically, for free and without re-invoking the model (which would
  -- cost credits and produce different content for a document the student may
  -- already have printed).
  blocks_key text,
  template_version text,                 -- which templates/cheatsheet.*.html rendered it
  html_key text,                         -- object storage key of the rendered HTML
  pdf_key text,                          -- object storage key of the rendered PDF
  page_count int,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on cheatsheets (user_id, workspace_id);
create index on cheatsheets (user_id, status);
create index on cheatsheets (user_id, created_at desc);
