-- Billing and usage. QRIS prepaid periods: no auto-renew, no stored payment
-- methods, at most ONE active subscription per user.

create table subscriptions (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  plan_id text not null references plans,
  status text not null,                -- active | grace | expired | canceled
  billing_period text not null,        -- monthly | yearly
  current_period_start timestamptz not null,
  current_period_end timestamptz not null,
  provider text not null default 'qris_gateway', -- qris_gateway | manual
  created_at timestamptz default now()
);
create index on subscriptions (user_id, status);
-- "At most one active subscription per user" is a data invariant, not a hope.
create unique index on subscriptions (user_id)
  where status in ('active', 'grace');

create table payments (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  subscription_id uuid references subscriptions,
  kind text not null,                  -- subscription | credit_pack
  plan_id text references plans,       -- what to activate on payment (subscription kind)
  billing_period text,                 -- monthly | yearly
  credits int,                         -- credit_pack amount
  provider text not null default 'qris_gateway',
  order_id text not null,              -- our unique reference sent to the gateway
  provider_payment_id text,            -- gateway transaction id
  amount_idr int not null,             -- expected amount (after discounts)
  discount_code text,
  qr_string text,                      -- QRIS payload / QR image URL returned by gateway
  expires_at timestamptz not null,     -- invoice expiry
  status text not null default 'pending', -- pending | paid | expired | failed | refunded | review
  paid_at timestamptz,
  gateway_fee_idr int,                 -- for margin reporting
  created_at timestamptz default now(),
  unique (provider, order_id)
);
create index on payments (user_id, created_at);
create index on payments (status, expires_at);
-- The polling fallback scans pending payments by expiry.
create index on payments (expires_at) where status = 'pending';

-- Idempotency + audit for gateway callbacks. The unique key is what makes a
-- duplicate webhook a no-op instead of a double activation.
create table webhook_events (
  id uuid primary key default gen_random_uuid(),
  provider text not null,
  event_id text not null,
  payload jsonb not null,
  processed_at timestamptz,
  unique (provider, event_id)
);

-- Balance = sum(delta). Spend happens inside a transaction holding
-- pg_advisory_xact_lock(hashtext(user_id::text)), so a bare SELECT sum + insert
-- cannot race; there is no balance column to keep consistent.
create table credit_ledger (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  delta int not null,                    -- negative = spend, positive = grant/top-up
  bucket text not null default 'monthly',-- monthly | purchased (purchased never expires)
  reason text not null,                  -- monthly_grant | credit_pack | usage:<action> | refund | admin
  ref_id uuid,
  created_at timestamptz default now()
);
create index on credit_ledger (user_id, created_at);
create index on credit_ledger (user_id, bucket);

create table usage_events (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  action text not null,
  credits int not null default 0,
  provider_cost_usd numeric(10,6),
  tokens_in int,
  tokens_out int,
  byok boolean not null default false,
  -- Which prompt module version produced this action, so a quality or cost
  -- regression can be attributed to a prompt change rather than guessed at.
  -- Nullable: not every action calls a model.
  prompt_version text,
  created_at timestamptz default now()
);
create index on usage_events (user_id, created_at);
create index on usage_events (user_id, action, created_at);

create table discount_codes (
  code text primary key,
  percent_off int not null,
  valid_until timestamptz,
  max_redemptions int,
  redeemed int not null default 0
);

-- BYOK (power plan): encrypted at rest, never logged, never stored on a device
-- unencrypted.
create table user_api_keys (
  user_id uuid primary key references users on delete cascade,
  provider text not null,
  key_enc bytea not null,
  created_at timestamptz default now()
);
