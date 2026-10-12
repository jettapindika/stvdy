-- The planner's proposal, the goal link a planned block needs, and the two charges a
-- failed generation must be able to give back.
--
-- This migration adds no `+sync` table and attaches no trigger: `events` and `goals` are
-- already in `0017_sync_triggers.sql`'s array, so that file stays the last migration that
-- attaches a trigger and `crates/server/tests/schema.rs` needs no new row for it. The
-- precedent is 0021-0023, 0028 and 0029, which each say the same thing in their header.
--
-- `plan_proposals` is deliberately NOT a `+sync` table (A252). The roadmap's conflict table
-- lists "planner output" among the entities that are "read-only for clients except via
-- their dedicated endpoints", which is what `GET /plans/{id}` is. The accepted blocks reach
-- every device the way the spec says planner results do — as `events` — and the proposal
-- itself never syncs.
--
-- `proposal` holds the serialized `PlanProposal`; `deficit_min` and `warnings` are lifted
-- out of it because the list screen reads them without parsing the whole thing, and because
-- a query for "which proposals could not fit" is a query.
create table plan_proposals (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  goal_id uuid not null references goals on delete cascade,
  status text not null default 'planning',  -- planning | ready | failed | accepted | superseded
  proposal jsonb,
  deficit_min int,
  warnings jsonb,
  prompt_version text,
  error text,
  -- The two credits `POST /goals/{id}/plan` reserved. It is the same guard `quizzes` and
  -- `decks` carry: a permanent generation failure refunds exactly this, once, and the
  -- claim that zeroes it is what makes a second refund (a retried job, the enqueue-failure
  -- path in the route, or the boot sweep) a no-op.
  credits_spent int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index on plan_proposals (user_id, goal_id, created_at desc);

-- A planned block belongs to the goal it was planned for (A263). The same argument `0013`
-- already made for `task_id`: a link without a FK drifts, and a replan or a workload meter
-- that reads a dangling id is a bug that only shows up later.
alter table events add column goal_id uuid references goals(id) on delete set null;
create index on events (goal_id);

-- The eight credits `POST /workspaces/{id}/syllabus` reserved. `0031` created the draft
-- before the refund arm existed, and migrations here are append-only, so the column is
-- added here rather than edited into the committed file. Same shape and same purpose as
-- `plan_proposals.credits_spent` above.
alter table syllabus_drafts add column credits_spent int not null default 0;
