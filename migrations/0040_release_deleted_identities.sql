-- The identity of an already-marked account is released now (A362).
--
-- `account::mark_deleted` releases the address, the password hash, the Google subject, the
-- student email and the feed token in the same statement that sets `deleted_at`, so a marked
-- account is unfindable by anything a student can type and the address is free the moment the
-- deletion is requested. Rows marked **before** that shipped still hold their real address,
-- and the only thing that would have released it is `purge_deleted_accounts` — up to
-- `ACCOUNT_DELETION_GRACE_DAYS` later. Between the two, such a row is a live row for every
-- lookup that keys on identity: `auth::routes::signup` would answer 409 for the address, and
-- `auth::google::resolve_user` would match the row by `google_sub` and hand out a session for
-- an account the student deleted.
--
-- So this is a cutover, not a backfill of something optional: it makes the assumption the new
-- code reads true for the rows that predate it, which is why the statement is a copy of the
-- release rather than a different, weaker form of it.
--
-- Idempotent and additive. Every column exists (`0003`, `0036`, `0037`); the tombstone address
-- is derived from the row's own id, so a second run rewrites the same value and cannot collide
-- with a live account's address; and rows already purged hold exactly these values, so
-- matching them is a no-op that costs one comparison. No schema change, so no trigger set and
-- no `crates/server/tests/schema.rs` row move — `users` carries no sync trigger.
update users
   set email = 'deleted-' || id::text || '@deleted.invalid',
       password_hash = null,
       google_sub = null,
       student_email = null,
       ics_feed_token = null
 where deleted_at is not null;
