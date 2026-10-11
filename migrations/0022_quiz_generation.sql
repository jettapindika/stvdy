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
