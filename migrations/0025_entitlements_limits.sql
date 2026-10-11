-- Phase 7: the columns and the one new table the entitlements work needs.

-- A workspace over the new plan's limit after a downgrade is read-only, not deleted.
-- `sources.read_only` has existed since 0007; these are its two siblings.
alter table workspaces add column read_only boolean not null default false;
alter table devices    add column read_only boolean not null default false;

-- The monthly grant has to be idempotent by construction, not by scheduling: the
-- signup grant (A64) and the recurring `monthly_grant` job can both run inside one
-- month, and `jobs.dedupe_key` only stops the *job* from being enqueued twice. This
-- key plus the partial unique index is what makes a second grant a no-op (A152).
-- Existing rows keep `period_key = null` and are unaffected by a partial index.
alter table credit_ledger add column period_key text;
create unique index on credit_ledger (user_id, period_key) where period_key is not null;

-- A43's signup limiter. Database-backed on purpose: an in-process bucket resets on
-- restart, which is exactly what a farm waits for. Every attempt is recorded, including
-- the ones that fail, because a farm's failures are the signal (A154).
create table signup_attempts (
  id bigserial primary key,
  ip text not null,
  email_domain text not null,
  created_at timestamptz not null default now()
);
create index on signup_attempts (ip, created_at);
create index on signup_attempts (email_domain, created_at);
