-- A cascade-tombstoned child has to be distinguishable from one the student deleted
-- deliberately, or restoring a parent cannot know which children to bring back.
--
-- `tasks.parent_id` has no depth constraint (`0012`), so the tree may be deeper than one
-- level: the flag applies to every descendant the cascade reaches, and both walks are
-- recursive.
--
-- This migration adds no `+sync` table and attaches no trigger: `tasks` is already in
-- `0017_sync_triggers.sql`'s array, so that file stays the last migration that attaches a
-- trigger and `crates/server/tests/schema.rs` needs no new row. The precedent is 0021-0023
-- and 0028, which each say the same thing in their header.
--
-- The column is deliberately **server-owned**: it is not in `Entity::Task.payload_columns`,
-- so a push cannot set it and a pull does not ship it. A client that could set it could
-- resurrect a subtree it never deleted; the client learns about restored children from the
-- upserts the restore emits, which is the same way it learns about everything else.
alter table tasks
  add column deleted_with_parent boolean not null default false;
