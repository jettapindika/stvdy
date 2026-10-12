-- Compaction's per-user watermark.
--
-- A cursor is compared against `min_retained_seq` to decide 410 resync_required
-- (docs/SYNC.md). It cannot be compared against `min(seq)`: seq is a bigserial, so a
-- brand-new account's first change can be seq 500 and a device at cursor 0 would be
-- told to resync on its first request (A165).
--
-- Not a `+sync` table, so it must not appear in 0017_sync_triggers.sql's array. 0017
-- stays the last trigger-attaching migration: this file creates no triggers.
create table change_log_watermark (
  user_id uuid primary key references users on delete cascade,
  min_retained_seq bigint not null default 0,
  compacted_at timestamptz
);
