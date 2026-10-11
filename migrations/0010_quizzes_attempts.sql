create table quizzes (                   -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  difficulty text,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on quizzes (user_id, workspace_id);

create table quiz_questions (            -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  quiz_id uuid not null references quizzes on delete cascade,
  kind text not null,              -- mcq | short
  prompt text not null,
  options jsonb,
  answer text not null,
  explanation text,
  chunk_id uuid references chunks on delete set null,
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on quiz_questions (quiz_id);

-- APPEND-ONLY, client-generated id.
create table quiz_attempts (             -- +sync
  id uuid primary key,
  user_id uuid not null references users on delete cascade,
  quiz_id uuid not null references quizzes on delete cascade,
  question_id uuid not null references quiz_questions on delete cascade,
  user_answer text,
  correct boolean,
  answered_at timestamptz default now(),
  created_at timestamptz default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on quiz_attempts (user_id, quiz_id);
create index on quiz_attempts (question_id);
