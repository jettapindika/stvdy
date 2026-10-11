-- Email verification tokens.
--
-- Deliberately NOT a `+sync` table: it has no `user_id`-scoped `updated_at`
-- trigger and no `change_row` logging. A verification token is a server-side
-- secret; a client must never be able to sync it down, read it, or replay a
-- token that was already issued. Only the SHA-256 hash of the token is stored
-- (same rule as refresh tokens, plan decision 8): a database leak must not hand
-- an attacker a usable verification link. `used_at` marks single-use; `expires_at`
-- bounds the window. 0017_sync_triggers.sql stays the last migration that
-- attaches triggers and the total stays at 34.

create table email_verification_tokens (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  token_hash text not null unique,
  expires_at timestamptz not null,
  used_at timestamptz,
  created_at timestamptz default now()
);

-- Issuing and consuming both scan the live tokens of one user; the composite
-- index also serves the cleanup of expired rows.
create index on email_verification_tokens (user_id, expires_at);
