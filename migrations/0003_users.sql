create table users (
  id uuid primary key default gen_random_uuid(),
  email text unique not null,
  -- Nullable, not `not null`: a Google (OIDC) sign-in creates an account with no
  -- password at all (A48). A Google-only user must verify their email before
  -- setting a password, or the reset path becomes an account-takeover vector.
  password_hash text,
  timezone text not null default 'Asia/Jakarta',
  plan_id text not null default 'free' references plans,
  email_verified boolean not null default false,
  -- When verification happened, for auditing. Distinct from the boolean: the
  -- boolean is the entitlement check (A43 requires it before any AI action),
  -- this is the record of when.
  email_verified_at timestamptz,
  student_verified boolean not null default false,   -- verified .ac.id email
  -- Google's stable subject claim. Accounts link by `sub`, never by email alone
  -- (A48): an email address can be reassigned, a `sub` cannot.
  google_sub text unique,
  -- Deviation from the spec's DDL: admin routes need an authority, and there is
  -- no roles table in the MVP. Seeded from ADMIN_EMAIL at startup.
  is_admin boolean not null default false,
  ics_feed_token text unique,                        -- revocable secret for the ICS feed
  deleted_at timestamptz,                            -- account deletion (scheduled hard delete)
  created_at timestamptz default now()
);

-- Login and email verification look up by email (the unique constraint already
-- covers equality); this partial index keeps admin listing cheap.
create index on users (created_at desc);
