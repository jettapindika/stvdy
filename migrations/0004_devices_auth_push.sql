-- Multi-device sessions.

create table devices (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  name text not null,                    -- e.g. "Jetta's MacBook"
  platform text not null,                -- macos | windows | ios | android | web
  app_version text,
  last_seen_at timestamptz,
  last_sync_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz default now()
);
create index on devices (user_id, revoked_at);

-- Rotating refresh tokens, one per device login. Only the hash is stored, so a
-- database leak does not hand over usable tokens; `family_id` groups a rotation
-- chain so reuse of a rotated token revokes the whole family.
create table auth_sessions (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  device_id uuid not null references devices on delete cascade,
  refresh_token_hash text not null,
  family_id uuid not null,
  expires_at timestamptz not null,
  revoked_at timestamptz,
  created_at timestamptz default now()
);
-- Refresh looks up by token hash; reuse detection scans the family.
create index on auth_sessions (refresh_token_hash);
create index on auth_sessions (user_id, revoked_at);
create index on auth_sessions (family_id);

create table push_tokens (
  device_id uuid primary key references devices on delete cascade,
  provider text not null,                -- apns | fcm
  token text not null,
  updated_at timestamptz default now()
);
