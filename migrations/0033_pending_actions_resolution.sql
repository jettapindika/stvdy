-- Resolution state for a proposed action (A273).
--
-- `pending_actions` is an existing `+sync` table, so this is an ALTER after
-- 0017_sync_triggers.sql and NOT a new migration inserted above it: the rule in that
-- file is for a *new* +sync table, and `pending_actions` is already in its `id_tables`
-- array. Its triggers therefore already exist and this migration attaches nothing —
-- 0017 stays the last trigger-attaching migration. `0021`, `0022` and `0023` altered
-- existing tables after it and said the same thing in their own headers.
--
-- No new row in `crates/server/tests/schema.rs`'s `documented` list either: that list is
-- for tables the spec's DDL lacks, and this adds columns to a table the spec has.
alter table pending_actions
  add column expires_at timestamptz,
  add column resolved_at timestamptz,
  add column resolved_by_device_id uuid references devices on delete set null,
  add column result jsonb,
  add column error text,
  add column message_id uuid references messages on delete set null;

-- The backfill for rows that predate the column, and the only place a TTL number appears
-- in SQL. It is a one-time value for existing rows, not a default: `PENDING_ACTION_TTL_MIN`
-- is a Rust constant (`crates/server/src/actions/mod.rs`) and every writer sets the column
-- explicitly, so a SQL default here would be a second source of truth for the same number
-- that drifts the first time the constant changes.
update pending_actions
   set expires_at = coalesce(created_at, now()) + interval '30 minutes'
 where expires_at is null;

alter table pending_actions alter column expires_at set not null;

-- The lazy expiry (A274) and the pending list both read this shape: one account's rows by
-- status, ordered by when they stop being actionable.
create index on pending_actions (user_id, status, expires_at);
