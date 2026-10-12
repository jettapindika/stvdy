-- Phase 13: payments hardening.
--
-- Additive only, and deliberately NOT a `+sync` migration: every column and table here
-- is billing internals, which `docs/SPEC.md` line 137 says never reach a client
-- ("NOT synced to clients: … billing internals, `credit_ledger`, `usage_events`,
-- `payments`"). `0017_sync_triggers.sql` therefore stays the last trigger-attaching
-- migration and its array is untouched.
--
-- This file carries the whole column set Phase 13's tasks name, not just the four this
-- slice (Tasks 2/4/5/6) reads, because it is one migration and one writer: splitting it
-- across slices would mean three peers editing one append-only file. Nothing here is
-- used by a route that does not exist yet — an unused nullable column costs nothing,
-- while a second migration for the same table costs a merge conflict.

-- What the student was quoted, what they were charged, and why the two differ.
--
-- `amount_before_discount_idr` and `discount_percent` are the itemisation the honesty
-- rule needs (gate 7(b)): the amount on the screen traces to `payments.amount_idr`, and
-- a support question about an unexpected amount is answered from the row rather than by
-- re-deriving the arithmetic (A298). `student_discount` records *which* discount was
-- applied, because the student promo and a code do not stack.
--
-- `platform` is the device row the checkout was made from, kept for the same reason the
-- entitlement decision reads `devices.platform`: a body cannot claim a platform (A144),
-- and after the fact there is otherwise no way to see which device asked.
--
-- `review_reason` is A294's whole point: a mismatched amount is not silently applied and
-- not silently dropped, it is a row a human reads. `reconciled_at` is when the poller or
-- the sweep last confirmed the row against the gateway, and `provider_status` is the
-- gateway's own word for it, so "our status says paid, theirs says refunded" is visible.
alter table payments
  add column platform text,
  add column amount_before_discount_idr int,
  add column discount_percent int not null default 0,
  add column student_discount boolean not null default false,
  add column review_reason text,
  add column reconciled_at timestamptz,
  add column provider_status text;

-- Which payment a callback was about.
--
-- Nullable because a callback that names an order we do not have still leaves a row
-- (that is the audit trail for a forged order id), and `on delete set null` rather than
-- cascade because the webhook event is evidence: deleting the payment it refers to must
-- not delete the record that the callback arrived.
alter table webhook_events
  add column payment_id uuid references payments on delete set null;

-- A294/A301 feed this queue, and it is read by an admin looking for work.
create index on payments (review_reason) where review_reason is not null;

-- A303: the plan each spend and each grant happened under, written at write time.
--
-- The roadmap's margin sketch joins `usage_events` to `users.plan_id`, and that join is
-- wrong in exactly the month the report exists to examine: the month a user converts,
-- their earlier spend that month is attributed to the paid tier, understating the free
-- tier's cost. `users` holds only the *current* plan, so the per-event plan has to be
-- recorded when the spend happens. Both columns are nullable (rows written before this
-- migration) and the report reads `coalesce(<table>.plan_id, users.plan_id)`.
alter table usage_events add column plan_id text references plans;
alter table credit_ledger add column plan_id text references plans;

-- A299: the address a student proved they hold, kept beside the boolean so the proof can
-- be re-read. The suffix list is `STUDENT_EMAIL_SUFFIXES` in config, never a literal in
-- code: a second campus suffix is an env change, not a deploy.
alter table users add column student_email text;

-- A296: a downgrade is recorded, not applied. The rollover applies it at the period end,
-- so a downgrade never shortens time already paid for.
alter table subscriptions
  add column pending_plan_id text references plans,
  add column pending_billing_period text;

-- A302: BYOK is a key plus an optional endpoint and model, because a compatible provider
-- is not always the default one.
alter table user_api_keys
  add column base_url text,
  add column model text;

-- A299: one token table serves both verification purposes, so a token minted for one
-- cannot be spent on the other. Existing rows are email verifications, which is what the
-- default says; the check is the backstop for a value the consumer's `match` would
-- otherwise treat as "not student_verify".
alter table email_verification_tokens
  add column purpose text not null default 'email_verify';

alter table email_verification_tokens
  add constraint email_verification_tokens_purpose
  check (purpose in ('email_verify', 'student_verify'));

-- A298: a discount code is once per user, and this row is what makes it so.
--
-- The unique key is `(code, user_id)`: the redemption is a fact about a pair, not about a
-- payment, so a second attempt by the same account is refused by the database rather than
-- by a read-then-write in the route. `payment_id` names the payment that redeemed it, so
-- "why was this code counted" is answerable, and `on delete cascade` on the payment is
-- right here (unlike `webhook_events`): a redemption with no payment is not evidence, it
-- is a row that can never be justified.
--
-- Deliberately NOT `+sync`: it is a billing internal like `discount_codes` itself.
create table discount_redemptions (
  id uuid primary key default gen_random_uuid(),
  code text not null references discount_codes on delete cascade,
  user_id uuid not null references users on delete cascade,
  payment_id uuid not null references payments on delete cascade,
  created_at timestamptz not null default now(),
  unique (code, user_id)
);

create index on discount_redemptions (payment_id);
