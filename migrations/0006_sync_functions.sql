-- Sync plumbing. Three functions, attached to every +sync table by 0017.
--
--   1. touch_updated_at    : maintain updated_at on UPDATE (server receive time,
--                            which is what the LWW conflict rule compares).
--   2. log_change          : append a change_log row in the same transaction, so
--                            a pull cursor is a strict per-user clock.
--   3. log_change_by_user  : the same for tables keyed by user_id (user_settings),
--                            which has no `id` and no `deleted_at` column.
--
-- The device that caused a change is read from a transaction-local setting, so
-- no per-table device column is needed:
--     select set_config('app.device_id', $1, true);
--
-- WHY THE ADVISORY LOCK IS REQUIRED (correctness, not tuning):
--   change_log.seq is a bigserial, so nextval() runs at INSERT time, but the row
--   only becomes visible at COMMIT time. Two concurrent transactions for the same
--   user can therefore be assigned seq 10 and seq 11 and commit in the opposite
--   order. A client that pulls `seq > cursor`, sees 11, and advances its cursor to
--   11 would never receive 10 once it commits — a permanently skipped change on
--   that device.
--
--   Taking a per-user transaction-scoped advisory lock BEFORE inserting the
--   change_log row serialises nextval() against commit order, so for any committed
--   row of a user every lower seq is already committed. That is exactly the
--   property the cursor depends on: a reader can only observe a contiguous prefix.
--
--   The lock must precede the insert. Taking it after is useless, because seq has
--   already been assigned.
--
--   Cost: writes for one user serialise against each other. At this write rate
--   that is free. Scale-out path if it ever matters: add a `txid` column and only
--   return rows below pg_snapshot_xmin(pg_current_snapshot()).
--
--   The key is deliberately the same as the credit-spend lock (docs/ASSUMPTIONS.md
--   A9): one user's sync writes and credit spends serialise against each other,
--   which also removes any lock-ordering deadlock between the two paths.
--
-- LOCK ORDER: advisory-then-rows. Because this lock lives in an AFTER trigger, a
-- multi-row transaction would otherwise acquire it *after* its row locks, and two
-- devices editing overlapping rows could deadlock (40P01). POST /sync/push
-- therefore takes the same lock as its FIRST statement, before applying any
-- mutation, so the order is uniform everywhere. See the roadmap's sync contract.

create or replace function touch_updated_at() returns trigger
language plpgsql as $$
begin
  new.updated_at := now();
  return new;
end $$;

-- For tables whose primary key is `id` and which carry a `deleted_at` tombstone.
create or replace function log_change() returns trigger
language plpgsql as $$
declare
  uid uuid;
  eid uuid;
  op  text;
begin
  if tg_op = 'DELETE' then
    uid := old.user_id;
    eid := old.id;
    op  := 'delete';

    -- If the owner is gone, this delete came from the users cascade (account
    -- purge), not from a device. Two reasons to skip:
    --   1. Correctness: `change_log.user_id` references users, so inserting a
    --      row for a user being deleted violates the FK and aborts the purge.
    --   2. Semantics: no device needs a sync record for a purged account — the
    --      change_log rows are removed by the same cascade.
    -- Only DELETEs need this check: for INSERT/UPDATE the child table's own FK
    -- already guarantees the user exists.
    if not exists (select 1 from users where id = uid) then
      return null;
    end if;
  elsif tg_op = 'INSERT' then
    uid := new.user_id;
    eid := new.id;
    op  := case when new.deleted_at is null then 'upsert' else 'delete' end;
  else
    uid := new.user_id;
    eid := new.id;
    -- A tombstone wins over a concurrent edit: once deleted, further updates
    -- keep logging `delete` until the row is explicitly restored (deleted_at
    -- set back to null), which logs an `upsert`.
    op  := case
             when new.deleted_at is not null then 'delete'
             else 'upsert'
           end;
  end if;

  perform pg_advisory_xact_lock(hashtext(uid::text));

  insert into change_log (user_id, entity, entity_id, op, origin_device_id)
  values (uid, tg_table_name, eid, op,
          nullif(current_setting('app.device_id', true), '')::uuid);

  return null;
end $$;

-- For `user_settings`: the primary key IS user_id, there is no `id` column and
-- no tombstones, so updates only, always `upsert`. A separate function is
-- required because referencing `new.id` / `new.deleted_at` on that table raises
-- "record new has no field id".
create or replace function log_change_by_user() returns trigger
language plpgsql as $$
begin
  perform pg_advisory_xact_lock(hashtext(new.user_id::text));

  insert into change_log (user_id, entity, entity_id, op, origin_device_id)
  values (new.user_id, tg_table_name, new.user_id, 'upsert',
          nullif(current_setting('app.device_id', true), '')::uuid);

  return null;
end $$;
