-- Generation is asynchronous with visible progress, so the quiz carries the state the
-- client polls, exactly as a deck does. `credits_spent` exists so a generation that
-- fails for good can refund exactly what was charged, once; `usage_event_id` carries
-- the row the provider's cost and token counts are attached to, which the margin
-- report needs (A41) and which only the spending route can create.
--
-- This migration only alters an existing table. It adds no `+sync` table, so
-- `0017_sync_triggers.sql` stays last and its trigger array is untouched: `quizzes`
-- is already in that array, and `crates/server/tests/schema.rs` already asserts it
-- carries both triggers.
alter table quizzes
  add column status text not null default 'ready',
  -- The sources the quiz is generated from. `decks` carries this and the generation
  -- job needs it: without it the job has nothing to build a prompt from, and every
  -- quiz would fail with an empty prompt rather than generating. `default '{}'` is
  -- for the rows that already exist; the route always binds a validated, deduplicated
  -- list.
  add column source_ids uuid[] not null default '{}',
  add column error text,
  add column requested_count int not null default 0,
  -- The requested question types (mcq | short | mixed). Stored for the same reason
  -- `requested_count` is: the generation job runs later and must rebuild the same
  -- prompt, and `{{types}}` is one of its settings. The per-question `kind` on
  -- `quiz_questions` is a different fact and is what the client renders.
  add column question_types text not null default 'mixed',
  add column credits_spent int not null default 0,
  add column usage_event_id uuid;

-- A quiz is created `generating`; the default is `ready` so any row written by a path
-- that does not generate (a future hand-made quiz) is usable immediately.

-- The ideas a grader should look for in a short answer, as the module's schema
-- documents them. The spec's sketch omits this column, and the module cannot work
-- without it: a student marking their own short answer is shown the model answer and
-- these points, and "did I say what mattered" is not answerable from a bare answer
-- sentence. `jsonb` because it is a list the model returns, like `options`.
alter table quiz_questions
  add column key_points jsonb;

-- The question's position in the quiz, as the model returned it. Without it the order
-- is whatever `created_at, id` happens to give: every question of one generation is
-- inserted in one transaction, so they share a `created_at` to the microsecond and the
-- order falls through to a random uuid. The model's order is meaningful (it is the
-- sequence the student answers in), so it is stored rather than re-derived.
alter table quiz_questions
  add column idx int not null default 0;

-- The device that wrote the attempt, as `flashcard_reviews` records it. An attempt is
-- one of the five entity kinds a client may write offline, so which device produced it
-- is worth keeping for the same reason it is on a review.
alter table quiz_attempts
  add column device_id uuid references devices on delete set null;
