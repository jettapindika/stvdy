create table users (
  id uuid primary key default gen_random_uuid(),
  email text unique not null,
  password_hash text not null,
  timezone text not null default 'Asia/Jakarta',
  plan_id text not null default 'free' references plans,
  email_verified boolean not null default false,
  student_verified boolean not null default false,   -- verified .ac.id email
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
