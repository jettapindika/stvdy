-- The sync clock and idempotency log.

-- `seq` is a bigserial, so it is assigned at INSERT time but rows become
-- visible at COMMIT time. The trigger that writes this table (0006) therefore
-- takes a per-user advisory lock before inserting, which makes seq order equal
-- commit order for a given user. Without that, a pull cursor could observe seq
-- 11 while seq 10 is still uncommitted, advance past 10, and lose that change
-- permanently. See migrations/0006_sync_functions.sql.
create table change_log (
  seq bigserial primary key,
  user_id uuid not null references users on delete cascade,
  entity text not null,                  -- table name
  entity_id uuid not null,
  op text not null,                      -- upsert | delete
  origin_device_id uuid,
  changed_at timestamptz not null default now()
);
create index on change_log (user_id, seq);
-- Compaction deletes by age, so it needs an index on changed_at alone.
create index on change_log (changed_at);

-- Idempotency for offline outbox replays: replaying a mutation_id returns the
-- stored result instead of applying the mutation twice.
create table mutation_log (
  mutation_id uuid primary key,
  user_id uuid not null references users on delete cascade,
  device_id uuid references devices on delete set null,
  result jsonb not null,
  created_at timestamptz default now()
);
create index on mutation_log (user_id, created_at);
