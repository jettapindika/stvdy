-- Billing reference data. Created first because `users.plan_id` references it.
--
-- `config/plans.toml` is the authority for entitlements; this table mirrors it
-- at startup (`db::seed_plans`, an upsert) so billing rows keep a valid FK and
-- the entitlement snapshot stays auditable. Entitlement checks read the typed
-- config, never this table.
create table plans (
  id text primary key,                 -- free | pro | power
  name text not null,
  price_idr_monthly int not null,
  price_idr_yearly int not null,
  entitlements jsonb not null          -- mirrors plans.toml for auditing
);

-- Seed the three plans here, not only at application startup. `users.plan_id`
-- is `not null default 'free'`, so the default must be satisfiable by the schema
-- alone: without these rows any INSERT into `users` fails with
-- `users_plan_id_fkey`, including in tests and in any psql session. Values are
-- the spec's; `seed_plans` overwrites them from plans.toml at boot, so this
-- remains a floor rather than a second source of truth.
insert into plans (id, name, price_idr_monthly, price_idr_yearly, entitlements) values
  ('free',  'Free',  0,     0,      '{}'::jsonb),
  ('pro',   'Pro',   10000, 100000, '{}'::jsonb),
  ('power', 'Power', 25000, 250000, '{}'::jsonb);
