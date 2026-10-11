-- Short-lived, single-use tickets for the Google login round trip (A48).
--
-- The callback runs in the system browser, and the tokens it produces must reach
-- the app. Putting an access or refresh token in a deep link would write a
-- credential into browser history, referrer logs and the OS URL handler, so the
-- callback redirects with an opaque ticket instead and the app exchanges it over
-- HTTPS for the real pair.
--
-- Not a `+sync` table: a ticket is a credential, and a client must never see or
-- replay one through the sync log.
create table login_tickets (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  -- Only the hash. Same reasoning as refresh tokens: a database leak must not
  -- hand over usable credentials.
  ticket_hash text not null unique,
  -- The device the app asked for, carried through the browser round trip.
  device_name text not null,
  device_platform text not null,
  app_version text,
  expires_at timestamptz not null,
  used_at timestamptz,
  created_at timestamptz default now()
);
create index on login_tickets (user_id, expires_at);
