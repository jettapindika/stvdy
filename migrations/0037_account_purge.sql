-- Account purge bookkeeping (A355).
--
-- `users.deleted_at` (in the spec's DDL, `0003`) records that the student asked for deletion.
-- What it cannot answer is whether the data has actually gone, and the scheduler needs that:
-- a purged account keeps its row — it is anonymized rather than deleted, because
-- `payments.user_id` and `credit_ledger.user_id` are `not null references users on delete
-- cascade` and removing the row would take the payment ledger with it — so `purged_at is null`
-- is the only thing that distinguishes "marked" from "done".
--
-- Without it the scheduler's backstop pass would re-examine every purged account on every
-- pass, forever, touching a `jobs` row each time.
--
-- Additive and nullable, so existing rows are unaffected and no backfill is needed: a row with
-- `deleted_at` set and `purged_at` null is an account that is either inside its window or was
-- marked before this shipped, and both are things the backstop is supposed to pick up.
--
-- `users` is in the spec's DDL and carries no sync trigger, so `0017_sync_triggers.sql` stays
-- the last trigger-attaching migration and `crates/server/tests/schema.rs` needs no new row.
alter table users add column purged_at timestamptz;

-- The pass's own index: "marked, not yet purged", which is the only query that reads this
-- column. Partial on `purged_at is null` so it stays the size of the accounts still owed a
-- purge rather than the size of every account ever deleted.
create index on users (deleted_at) where purged_at is null;
