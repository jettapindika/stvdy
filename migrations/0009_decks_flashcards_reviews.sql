create table decks (                     -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  source_ids uuid[] not null,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on decks (user_id, workspace_id);

-- ease/interval/due are DERIVED by replaying flashcard_reviews (see
-- crates/core/src/sm2.rs); clients treat them as read-only, which is what makes
-- reviews from multiple devices conflict-free.
create table flashcards (                -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  deck_id uuid not null references decks on delete cascade,
  front text not null,
  back text not null,
  chunk_id uuid references chunks on delete set null,
  ease real not null default 2.5,
  interval_days int not null default 0,
  due_at timestamptz not null default now(),
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on flashcards (user_id, due_at);
create index on flashcards (deck_id);

-- APPEND-ONLY, client-generated id. Never updated or deleted, so a duplicate id
-- is ignored rather than overwriting.
create table flashcard_reviews (         -- +sync
  id uuid primary key,
  user_id uuid not null references users on delete cascade,
  flashcard_id uuid not null references flashcards on delete cascade,
  rating text not null,            -- again | hard | good | easy
  reviewed_at timestamptz not null,
  device_id uuid references devices on delete set null,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on flashcard_reviews (flashcard_id, reviewed_at);
create index on flashcard_reviews (user_id, reviewed_at);
