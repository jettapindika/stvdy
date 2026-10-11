# Phase 13. QRIS payments. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`
> (**§ 1**'s `PaymentProvider` trait, **§ 3** entitlements, **§ 5** the HTTP error mapping,
> **§ 8** the job-kind list, the Risk Register's B1 row) and in `docs/SPEC.md`
> (§ Payments: QRIS via my gateway, § Plans, § Credit costs, § Entitlements engine,
> § AUTH AND SECURITY, § Screens 14/16, § Honesty rules). This plan implements them; it
> never redefines them. Where this plan and the roadmap disagree, the roadmap wins and
> this file is the bug. Every place this plan *does* deviate from a contract is listed in
> "Decisions this plan pins" with a number and a one-line justification.

**Goal:** a student on the web or desktop picks a plan, sees the exact rupiah amount
(including the student price when their `.ac.id` address is verified), pays it with QRIS
by scanning one code before it expires, and the plan is active on their phone before the
payment screen has finished polling. The money path is boring in the way money paths must
be: the webhook is signature-verified, idempotent, and refuses to activate anything whose
amount does not match; a lost webhook is recovered by polling; and the admin margin report
says — per month, per plan — what the free tier costs separately from what paying
customers cost, which is the only number A46's tripwires can be read from.

**Architecture:** the split the rest of the product already uses.

- `crates/core/src/traits/payment.rs` is the **trait**, and it is the whole point of this
  phase being buildable at all. **B1 is unresolved**: the gateway's docs do not exist, so
  the only honest design is one that a documented field mapping can be dropped into. The
  trait is `PaymentProvider` exactly as contract § 1 writes it; the QRIS adapter is the
  one implementation; and **every gateway-specific value is env**, so the day the docs
  arrive, the diff is `mapping.rs` and one signature arm — not a redesign.
- `crates/core/src/billing/` is **pure**: `price()`, `apply_payment()`, `upgrade_delta()`.
  No database, no clock, no IO. The amount a student is quoted and the amount the webhook
  is checked against come out of the same function, so they cannot disagree.
- `crates/server/src/billing/` owns the `payments` row, the checkout route, the webhook,
  and the period rules; `crates/server/src/integrations/payment/` owns the wire.
- The **money moves in one transaction**: the webhook marks the payment paid, extends the
  subscription, grants the credits, records the fee and enqueues the notification, or none
  of it happened. A webhook that half-applied is a student charged with no plan.

**Tech Stack:** Rust (core + server), Postgres/sqlx, `hmac` + `sha2` (already in the
workspace for the JWT and the local storage signer), `aes-gcm` (already used for the
Google refresh token, reused for the BYOK key), `reqwest` (already the LLM and search
client), React + Vite + TypeScript + Tailwind, TanStack Query. **No new crate
dependency**, with one qualification: `crates/core` gains `http` (the types crate only —
`HeaderMap` is the contract's own signature in § 1, and `http` is already in the tree
under `axum` 0.8; it is a data structure, not a server).

**Exit gate (observable, from the roadmap):** sandbox invoice → verified webhook → plan
active on the second device; duplicate/mismatch/late/expired cases tested; mobile checkout
blocked; **the margin report shows free-tier and paid-tier provider cost separately, with
free-tier utilization and conversion derived from it**.

---

## This phase's real risk, and the defence

> **B1 is unresolved, and it is the largest single execution risk in the doc set.** The
> gateway has no identity, no base URL, no auth scheme, no invoice-create response shape,
> no status-query shape, no callback signature scheme and no fee rates. The spec's own
> line is `GATEWAY = <FILL IN>`. Everything in this phase that touches the wire is
> therefore written against a **documented internal contract** — an interface *we* define
> and *we* test — with the mapping from that contract to the gateway's fields isolated in
> one file and every value read from env.
>
> **The defence is structural, not a promise.** (1) `PaymentProvider` lives in
> `crates/core` and is implemented by a mock that signs bodies the way a real gateway
> would, so **every webhook test in this phase runs against the trait, not against the
> vendor** — the suite that proves idempotency, amount-checking and the period rules keeps
> passing on the day the docs arrive, because none of it knows the vendor's field names.
> (2) The vendor's field names live in exactly one module,
> `crates/server/src/integrations/payment/mapping.rs`, with a test that asserts the
> request we send and the response we parse, so a mapping change is a test change and not
> a silent one. (3) The signature scheme is `QRIS_SIGNATURE_SCHEME`, a closed set of three
> shapes (`hmac_sha256_hex`, `hmac_sha256_base64`, `shared_token`), each unit-tested, and
> an unknown value **fails at startup** rather than defaulting to something that would
> verify everything. **What changes when B1's docs arrive:** `mapping.rs` (field names and
> the fee field), one arm of `signature.rs` if the scheme is not one of the three, and the
> env values. Nothing else in this phase, and no test outside those two files.
>
> **The second risk, with its own defence: a webhook that is not idempotent
> double-activates.** A gateway that retries a callback — because our 200 was lost, or
> because its own retry policy fired — must not grant a second period or a second credit
> grant. There are **two** ways this goes wrong and they need two defences: the *same*
> event replayed (defended by `webhook_events`' `unique (provider, event_id)` plus
> `on conflict do nothing`), and the *same payment* reported under a **new** event id
> (defended by the state transition itself being `update payments set status='paid' …
> where id = $1 and status = 'pending'`, so a second report updates zero rows and the
> handler takes the no-op path). Both are tested, because a defence against the first
> does nothing against the second and only the second happens when a gateway re-sends
> with a fresh id.
>
> **The third risk: the free/paid split is the thing A46's tripwires are read from, so a
> wrong split is a wrong business decision.** The roadmap's sketch is "join
> `usage_events` → `users.plan_id`", and that join is **wrong in exactly the month it
> matters**: the month a user converts, their earlier spend that month is attributed to
> the paid tier, understating the free tier's cost and overstating its utilization's
> denominator — and conversion months are the ones the report is about. The tree cannot
> do better from `users.plan_id` alone because it holds only the *current* plan. So the
> plan adds `usage_events.plan_id` and `credit_ledger.plan_id`, written at write time from
> the effective plan the spend already has in hand (A283), and the split reads those with
> a documented fallback for pre-existing rows. The defence is a test that seeds a user who
> was free in June and paid in July and asserts June's cost stays in the free column.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes. Prompts
  live in versioned modules, never inline. `tracing` structured logs with request ids;
  log latency for retrieval, LLM calls, sync pull/push, jobs, **and webhooks**.
- **Server is the only source of truth.** Clients never call the gateway. The client
  never reports a payment as paid; `docs/SPEC.md` § Payments says it in those words. No
  limit or cost is hardcoded: the matrix is `config/plans.toml`, loaded once at startup.
- **The client is untrusted (A42).** The platform that decides whether checkout is allowed
  comes from `devices.platform`, never the request body (A144). The amount comes from the
  server's own pricing function, never from the client. The webhook's verdict comes from
  the signature and the amount, never from the body's own claim.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
  Local-date math only through `chrono-tz` (A6). Invoice expiry and period ends are
  absolute instants; a "15:00 countdown" is the client rendering `expires_at`.
- **One transaction for the money.** Mark paid → activate/extend → grant credits → record
  the fee → notify: all of it, or none of it. A partially applied webhook is a charged
  student with no plan.
- **Secrets only in env, never logged, never in a response body.** `QRIS_API_KEY` and
  `QRIS_CALLBACK_TOKEN` are `SecretString`; a test drives the swept routes and asserts no
  body contains them (the pattern `tests/tamper.rs` already uses for the LLM key).
- **Plan limits and credit costs are copied, never re-derived** (from
  `config/plans.toml`, which is the authority). The values this phase's tests assert are
  in the table below.
- **Errors serialize to** `{ code, message, min_plan?, limit?, used? }` — every field
  always present, `null` when not applicable. This phase adds no new `code`; it uses
  `validation` 400, `not_found` 404, `conflict` 409, `plan_required`/`feature_locked` 403,
  `rate_limited` 429, `provider_error` 502 (roadmap § 5).
- **Quality gate:** `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `npm --prefix app test`, `npm --prefix app run build`,
  `node scripts/check-no-cdn.mjs`, `cargo sqlx prepare --workspace --check`, the
  `src-tauri` trio (`just check-shell`), and `cargo sqlx prepare --workspace` re-run after
  any query changes.
- **UI language English**, but Indonesian documents and queries must work end to end, and
  every new string this phase adds goes through `t()` with both `en.json` and `id.json`
  entries — including the invoice, the reminder and the refusal copy.
- **Honesty rules are a copy contract.** The countdown is a real `expires_at`, not a
  decoration. The amount shown traces to `payments.amount_idr`. No number on the margin
  report is invented: a month with no data is `null`, never `0`, and the FX rate used is
  printed as the input it is.

### Values this phase charges and refuses (copied from `config/plans.toml`)

Prices (IDR), read through `crates/core/src/config/plans.rs`: free `0/0`, pro
`10000/100000`, power `25000/250000`, `student_percent_off` free `0`, pro `50`, power
`50`. Credit packs are a config table this phase adds (`[credit_packs]`), because the spec
says "one-time credit packs" without naming an amount, and an amount invented in code is
the thing A21 forbids.

| Plan | `price_idr_monthly` | `price_idr_yearly` | student monthly | `byok` | checkout on ios/android |
|---|---|---|---|---|---|
| free | `0` | `0` | `0` | false | n/a (nothing to sell) |
| pro | `10000` | `100000` | `5000` | false | **refused** 403 `feature_locked` (A24/A151) |
| power | `25000` | `250000` | `12500` | true | **refused** 403 `feature_locked` |

Gateway fee rates are **not** a plan value: they are `QRIS_FEE_PERCENT` /
`QRIS_FEE_FIXED_IDR` in env, defaulting to `0`/`0` (i.e. "unknown, record nothing") rather
than to a plausible-looking rate, because B1 has not told us the rate and a made-up fee
would silently corrupt the margin report.

---

## What already exists (checked in this tree, not assumed)

- **`migrations/0014_billing.sql` already has the whole billing schema and needs no new
  table**: `subscriptions` (with the partial unique index on `(user_id)` for
  `status in ('active','grace')` — "at most one active subscription per user" is a data
  invariant, not a hope), `payments` (`kind`, `plan_id`, `billing_period`, `credits`,
  `provider`, `order_id`, `provider_payment_id`, `amount_idr`, `discount_code`,
  `qr_string`, `expires_at`, `status` incl. `review`, `paid_at`, `gateway_fee_idr`,
  `unique (provider, order_id)`), `webhook_events` (`unique (provider, event_id)`),
  `credit_ledger`, `usage_events` (`provider_cost_usd numeric(10,6)`, `byok`),
  `discount_codes` (`code`, `percent_off`, `valid_until`, `max_redemptions`, `redeemed`)
  and `user_api_keys` (`key_enc bytea`, aes-gcm). The two indexes the polling fallback
  needs already exist (`payments (status, expires_at)` and the partial
  `payments (expires_at) where status = 'pending'`).
- `migrations/0017_sync_triggers.sql` is the **last** trigger-attaching migration and this
  phase adds **no `+sync` table**, so it is untouched. Billing is explicitly *not synced*
  (`docs/SPEC.md` line 137: "NOT synced to clients: … billing internals, `credit_ledger`,
  `usage_events`, `payments`").
- `crates/server/src/credits.rs` has `grant`, `grant_in`, `spend`, `spend_in`, `refund`,
  `refund_in`, `record_usage`, `latest_usage_event`, `RefundTarget` (+ `refund_pending`),
  `costs`, `month_provider_cost`, `ceiling_paused`, `search_cost`, `chat_message_cost`,
  `daily_chat_cap`, `daily_search_cap`, `record_search`. **What is missing for this phase
  is one function: a grant into the `purchased` bucket** (A277) — `grant` writes the
  monthly bucket only.
- `crates/server/src/entitlements.rs` has `snapshot`, `snapshot_for`, `snapshot_in`,
  `require`, `require_for`, `reserve_in`, `recompute_read_only`, `effective_plan`,
  `facts`, `limits_view`, `refusal`. `effective_plan` is where a paid plan becomes real:
  **`users.plan_id` grants nothing unless the subscription status is `active`/`grace`
  (A146)**, which is why the webhook's job is to make a subscription live, not to write
  `plan_id`.
- `crates/server/src/jobs/`: `worker.rs::dispatch` (a `match` on `kind`, unknown kinds fail
  permanently), `schedule.rs` (the only time-based producer: per-account local-boundary
  rows with `dedupe_key = "<kind>:<user_id>:<local YYYY-MM>"`, `SCHEDULE_INTERVAL` 900s,
  `SCHEDULE_BATCH` 500), `handlers/grants.rs` (`monthly_grant`, period key
  `"sub:<subscription_id>:<period start>"` for paid and `"monthly:<local YYYY-MM>"` for
  free, `insert … on conflict do nothing` against `credit_ledger`'s partial unique index),
  `handlers/counters.rs` (`reset_counters`). **`poll_payment`, `reconcile_payments` and
  `renewal_reminder` are named in roadmap § 8 and have no arm.**
- `crates/server/src/config.rs` builds and validates every provider config at boot and
  fails startup on a broken one (`validate_encryption_key` is the shape a new validator
  copies). **There is no `QrisConfig`.**
- `crates/core/src/traits/` has `clock`, `embedder`, `llm`, `mailer`, `search`, `storage`
  — **there is no `payment.rs`**, and `testing.rs` has `FixedClock`, `HashEmbedder`,
  `InMemoryStorage`, `RecordingMailer`, `ScriptedLlm` — **there is no mock gateway**,
  which roadmap § 1 says lives there.
- `crates/server/src/integrations/` has `llm`, `search`, `fetch`, `embedder`, `storage`,
  `mailer`, `clock`, `documents` and `Providers::build`, which performs the boot
  assertions. `crates/server/src/integrations/llm.rs` is the [OI]-compatible client
  (`base_url` + `api_key` + `default_model`), and `ChatRequest` **carries no per-request
  key** — so BYOK needs a seam this phase adds (A282).
- `crates/core/src/config/plans.rs` has `PlanConfig { price_idr_monthly, price_idr_yearly,
  student_percent_off, … }`, `Credits`, `PlanId`, `PlansFile::get`, `PlansFile::validate`.
  `crates/core/src/config/billing_policy.rs` has `BillingPolicy`, `Platform`,
  `PlatformPolicy { allow_purchase_ui, show_prices, allow_purchase_cta_in_locked_messages }`,
  `PlatformPolicy::DENY`, `for_platform`, `allows_purchase_ui`, and `validate()` which
  **rejects a mobile entry that enables purchase UI, prices or a CTA** — the file cannot be
  edited into an app-store violation.
- `crates/server/src/auth/extract.rs` has `AuthUser { user_id, email, email_verified,
  student_verified, plan_id, timezone, platform, is_admin, subscription_status, … }` and
  refuses an unverified account at the spending extractor (`EmailNotVerified`). The
  platform comes from the device row (A144).
- `crates/server/src/ratelimit.rs` has `auth_layer()` (a `GovernorLayer` with the
  documented error shape). Phase 7's plan adds `signup_limiter()` and `spend_layer()`; this
  phase reuses whatever those land as and adds no limiter of its own beyond wiring
  checkout through one.
- **Phase 7's plan has not landed in this tree yet, and this phase consumes two of its
  outputs by name:** `billing::checkout_allowed(&BillingPolicy, &str) -> AppResult<()>`
  (Task 9, A151) and `admin::margin_report(&PgPool, from, to) -> AppResult<MarginReport>`
  with `MarginReport { per_plan: Vec<PlanMargin>, provider_cost: CostSplit, revenue:
  Option<Revenue> }` (Task 11). Both are written down in
  `docs/superpowers/plans/2026-10-11-phase-7-entitlements-credits-limits.md` and are
  **inherited, not re-planned**: this phase completes `margin_report`'s `revenue` and adds
  the utilization/conversion/tripwire fields, and calls `checkout_allowed` as the first
  statement of the checkout handler. If Phase 7's signatures differ when they land, this
  plan adapts to them; it does not fork them.
- **Phases 8–12 land first, and this phase re-plans none of them.** It inherits: the sync
  transport and `sync::push::decide` (8 — billing is not a synced entity, so nothing here
  the push allowlist); `crates/server/src/settings/` (9 — the BYOK *routes* are added into
  the settings module it lands, because `/settings/api-key` is a settings path, while the
  encryption and the provider seam live in `billing/byok.rs`; if Phase 9's settings module
  is shaped differently this plan adapts to it rather than forking it);
  `crates/core/src/calendar/` (10 — irrelevant here except that a downgrade's read-only
  recompute already runs); `crates/core/src/planner/` (11); and from **12**: the reminder
  engine, the `send_reminder`/`send_push` jobs and the `PushProvider`/`Mailer` delivery
  path. **`renewal_reminder` (Task 8) is a producer into Phase 12's delivery, not a second
  delivery path** — it enqueues the same `send_push`/`send_reminder` rows Phase 12 built,
  and its own test asserts the *enqueue*, not the transport.
- **The SPA** already has `app/src/features/billing/{plans.ts,policy.ts}` (the
  `config/plans.toml` and `config/billing_policy.toml` readers, with `DENY` as the
  parse-failure direction), `app/src/components/ui/LockedFeature.tsx` (`whatKey`,
  `minPlan`, `reasonKey` — keys, not strings, A116), `app/src/api/errors.ts` (the
  exhaustive `ApiErrorCode` → copy map) and Phase 7's `PlanUsageScreen.tsx` +
  `CreditMeter.tsx` + `app/src/api/queries/usage.ts`. **`app/src/api/queries/` has no
  `billing.ts`.**
- **MISSING, and created by this phase:** `crates/core/src/traits/payment.rs`,
  `crates/core/src/traits/testing.rs`'s mock gateway, `crates/core/src/billing/`,
  `crates/server/src/integrations/payment/`, `crates/server/src/billing/`'s
  `checkout`/`webhook`/`student`/`byok` modules and route table,
  `crates/server/src/jobs/handlers/{poll_payment,reconcile_payments,renewal}.rs` and their
  three dispatch arms, the `QrisConfig` + its boot validation, the BYOK override seam in
  the chat path, `admin.rs`'s payment tools and the completed `margin_report`, the SPA's
  pricing/billing/checkout screens, and `scripts/smoke-phase13.sh`.

---

## Decisions this plan pins (so parallel work cannot diverge)

Each is a place the spec, the roadmap and the tree leave a choice. They are appended to
`docs/ASSUMPTIONS.md` as rows **A270–A289** in the same commits as the code.

1. **A270 — `PaymentProvider` lives in `crates/core/src/traits/payment.rs`, its DTOs are
   `InvoiceRequest`/`Invoice`/`GatewayStatus`/`WebhookEvent`/`WebhookKind`/`PaymentError`,
   and `crates/core` gains the `http` dependency for the contract's own `HeaderMap`
   parameter.** Contract § 1 writes the signature as `parse_and_verify_webhook(&self,
   headers: &HeaderMap, raw_body: &[u8])`; `http` is a types crate with no IO and is
   already in the tree under `axum` 0.8, so taking it in core is cheaper than inventing a
   second header representation and converting at the boundary. `PaymentError` is
   `{ Transport(String), Invalid(String), Signature, UnknownOrder, Provider(String),
   NotConfigured }` — `Signature` and `UnknownOrder` are separate variants because the
   webhook route maps them to different responses (401 vs 404) and collapsing them would
   make that mapping a string match.
2. **A271 — The gateway's field mapping is one file, and it is the only file B1 changes.**
   `crates/server/src/integrations/payment/mapping.rs` holds the request body we send, the
   response fields we read and the fee field, each as a named function with a test. The
   rest of the adapter (`qris.rs`) is transport and the trait's shape. **When B1's docs
   arrive, the diff is `mapping.rs`, one arm of `signature.rs` if the scheme is new, and
   the env values — no test outside those two files changes, because every behavioural
   test in this phase runs against the trait's mock.**
3. **A272 — `QRIS_SIGNATURE_SCHEME` is a closed set of three schemes, an unknown value
   fails startup, and a failed verification is 401 `unauthorized` that never says which
   check failed.** `hmac_sha256_hex` (HMAC-SHA259 over the raw body, hex, in
   `QRIS_SIGNATURE_HEADER`, default `X-Callback-Signature`), `hmac_sha256_base64` (the
   same, base64), `shared_token` (a constant-time compare of the header against
   `QRIS_CALLBACK_TOKEN`, for a gateway with no signature at all). The comparison is
   `hmac`'s `verify_slice`/`subtle`-style constant time in every arm; a scheme the code
   does not know is a **startup** error, because a default would be a scheme that verifies
   everything. `QRIS_WEBHOOK_IPS`, when set, is an allowlist checked before the signature.
4. **A273 — Idempotency is the `webhook_events` unique key *and* a guarded state
   transition, because they defend different replays.** `insert into webhook_events
   (provider, event_id, payload) values (…) on conflict (provider, event_id) do nothing
   returning id` — no row returned means this exact event was already handled, so the
   handler answers `200 { "duplicate": true }` and does nothing. Separately, the payment's
   own transition is `update payments set status = 'paid', paid_at = $2 … where id = $1 and
   status = 'pending'`, so a gateway that re-reports the same payment under a **new** event
   id updates zero rows, and the handler takes the same no-op path. The second is what
   happens when a real gateway retries; the first is what happens when it does not change
   the id.
5. **A274 — The amount check is equality against `payments.amount_idr` in IDR, and any
   mismatch is `review`, never an activation.** `paid > amount` and `paid < amount` are the
   same outcome (over- and under-payment both need a human), the row goes to `status =
   'review'` with a `review_reason` naming the two numbers, the raw payload is kept in
   `webhook_events.payload`, one `tracing::error!` with the order id is emitted, and
   `admin::margin_report`'s payment list surfaces it. A currency that is not `IDR` is a
   mismatch for the same reason. This is the one place the spec's "never activate
   automatically" is a hard rule and not a preference.
6. **A275 — A payment confirmed after its invoice expiry is honoured when the gateway
   says it was paid *before* the expiry, and otherwise goes to `review`.** The spec leaves
   it "configurable"; `QRIS_HONOR_LATE_PAYMENT` (default `true`) decides the first case
   only. The distinction matters because the two cases are different facts: "the student
   paid in time and our webhook was late" is our problem and must not cost the student
   their plan; "the student paid after the code expired" is a gateway-side race that a
   human should look at. `paid_at < expires_at` is the test, and both branches are tested.
7. **A276 — The period rules are: extend from `current_period_end` when the subscription is
   `active`/`grace`, start at `paid_at` when it is not, upgrade immediately with the
   prorated difference, downgrade at period end.** The extension rule is what makes paying
   early not lose days; the upgrade is `upgrade_delta()` in `crates/core/src/billing`
   (charged as its own `payments` row with `kind = 'subscription'` and the same
   `subscription_id`); the downgrade writes `subscriptions.pending_plan_id` +
   `pending_billing_period` and the rollover applies it, so **a downgrade never shortens a
   paid period** and never deletes anything (`recompute_read_only` then flags what the new
   plan puts over its limit, which is Phase 7's existing path).
8. **A277 — A credit pack is a `payments` row with `kind = 'credit_pack'` and a grant into
   the `purchased` bucket with `reason = 'credit_pack'` and `ref_id = payments.id`.** The
   webhook's credit step branches on `kind`: a subscription grants the plan's
   `credits_per_month` through the **existing** `grant()` with the period key
   `"sub:<subscription_id>:<period start>"` (so a renewal grants once per period, which
   Phase 7's `monthly_grant` already established), and a credit pack calls the new
   `credits::grant_purchased(pool, user_id, credits, ref_id)`. `ref_id` is what makes a
   double-credit-pack bug visible in the ledger instead of looking like an ordinary grant.
9. **A278 — Discounts do not stack: the student promo and a discount code are applied
   with `max(percent)`, and a code is once per user and bounded by `max_redemptions`
   atomically.** `discount_codes.redeemed` is incremented in the same transaction that
   writes the payment, guarded by `and (max_redemptions is null or redeemed <
   max_redemptions)`, and a new `discount_redemptions (code, user_id, payment_id,
   created_at, unique (code, user_id))` row is what makes it once per user. Stacking two
   percentages is the kind of generosity nobody notices until the margin report does.
10. **A279 — Student verification is `users.student_email` plus a purpose-scoped token, and
    the suffix list is `STUDENT_EMAIL_SUFFIXES` (default `.ac.id`).** `POST
    /billing/student/verify` mails a link to the supplied address (rate-limited, and only
    for a suffix in the list — otherwise the endpoint is a mail amplifier), and `POST
    /billing/student/confirm` sets `users.student_verified = true` and
    `users.student_email`. The token store is `email_verification_tokens` (0018) extended
    with a `purpose` column (`email_verify | student_verify`) so one table serves both and
    a token minted for one purpose cannot be spent on the other. The discount then applies
    at checkout from `plans.<id>.student_percent_off`, which is already in the config.
11. **A280 — Renewal reminders are four enqueues at 7/3/1/0 days before
    `current_period_end`, deduped by `dedupe_key = "renewal_reminder:<user_id>:<period_end
    date>:<days>"`.** The producer is the existing per-account scheduler pass (extended
    with the four boundaries), and delivery is **Phase 12's** `send_reminder`/`send_push`
    path, so this phase adds a producer and a test that asserts one row per boundary per
    period. On `ios`/`android` the copy is `reminder.planEnding` ("Your plan ends on
    {{date}}.") with **no price and no link** — `billing_policy` decides that, the same
    flag `LockedFeature` reads.
12. **A281 — `poll_payment` and `reconcile_payments` are separate kinds with separate
    schedules.** `poll_payment` runs every 5 minutes over pending payments inside their
    expiry window (the partial index exists for exactly this), asks `get_status`, and
    applies the *same* handler the webhook calls when the gateway says paid — one code
    path, so polling and the webhook cannot diverge. `reconcile_payments` runs hourly,
    lists recent gateway transactions (a window bounded by `QRIS_RECONCILE_HOURS`, default
    48) and flags two mismatches: a gateway payment we have no row for, and one of ours the
    gateway does not know. Both are `review` rows with a `review_reason`, never silent
    edits.
13. **A282 — BYOK is a per-user key plus optional `base_url`/`model`, and the seam is a
    provider the chat path builds for one request.** `user_api_keys` gains `base_url text`
    and `model text` (nullable); `PUT /settings/api-key` encrypts with the same
    `aes-gcm` key and the same `ENCRYPTION_KEY_HEX` the Google refresh token uses,
    `DELETE` removes the row, and `GET /settings/api-key` returns
    `{ set, provider, base_url, model }` — **never the key**. `crates/server/src/billing/byok.rs`
    exposes `provider_for(state, user_id) -> AppResult<Option<Arc<dyn LlmProvider>>>`, and the
    chat and generation paths call it **once per request**; a call made with a user key
    writes `usage_events.byok = true` and its provider cost, consumes **no credits**, and is
    excluded from the ceiling (A156). Power only (`Feature::Byok`), web/desktop only
    (a mobile device cannot set one, the same `billing_policy` rule that refuses checkout).
14. **A283 — `usage_events.plan_id` and `credit_ledger.plan_id` are written at write time,
    and the margin split reads them with a documented fallback.** This is the deviation the
    roadmap's own sketch needs: "join `usage_events` → `users.plan_id`" attributes a
    converted user's earlier same-month spend to the paid tier, which is wrong in exactly
    the month the report exists to examine. `credits::spend_in` and `credits::grant`/`grant_in`
    already receive the effective `PlanId`, so the column is one bind each. Both are
    nullable (rows written before this phase), and the query uses
    `coalesce(<table>.plan_id, users.plan_id)` so the report is never wrong-by-crash, only
    wrong-by-approximation on pre-Phase-13 history, which it labels.
15. **A284 — The margin report's definitions are explicit and printed, and its tripwires
    are computed booleans.** Per month, per plan: `revenue_idr` (sum of `payments.amount_idr`
    where `status='paid'` and `paid_at` in the month), `provider_cost_usd` split
    `free`/`pro`/`power` and, inside each, `byok` vs ours (A156), `gateway_fee_idr`, and
    `gross_margin_idr` at the month's `IDR_PER_USD` (env, default `17889`, the A41 rate —
    **printed as the input it is**). Derived: `free_utilization = free credits spent in the
    month ÷ free credits granted in the month` (A46's "28% ≈ 17 of 60 credits"), and
    `conversion` as **both** `paying ÷ free` and `paying ÷ total` at the report's `to` date,
    because A46's "4%" is `paying ÷ free` and a report that shows one ratio while the
    tripwire names the other is a false alarm waiting to happen. `tripwires` is a struct of
    four booleans: `free_utilization_over_30`, `conversion_under_3`,
    `free_spend_over_quarter_of_revenue`, `plan_loss_two_months_running` (the last is `null`
    when the range has no previous month, because "two months running" cannot be computed
    from one).
16. **A285 — `GET /plans` is authenticated and platform-aware, and it omits prices where
    `show_prices` is false.** The platform is the device row (A144), so a phone gets the
    plan names, the limits and the features and **no prices**, and the SPA's
    `policy.ts` `showPrices` is a second reading of the same file rather than a second
    decision. The pricing page itself renders on web/desktop only.
17. **A286 — The checkout screen polls `GET /billing/payments/:id` and renders five
    states, and the countdown is the server's `expires_at` rendered against the client's
    clock.** The states are the spec's: `waiting`, `paid`, `expired` (with "new QR"), a
    mismatch (`review` → "under review"), `failed`, and offline. A `paid` state never
    renders a QR again, and the poll stops on any terminal status. The server re-checks
    expiry on every poll, so a client whose clock is wrong cannot extend its own invoice.
18. **A287 — Checkout requires a verified email (A43) and is rate-limited, and a second
    checkout for the same plan and period reuses the pending invoice instead of minting a
    second QR.** "User paying twice for the same period" is one of the spec's edge cases;
    the reuse rule is `where user_id = $1 and kind = $2 and plan_id = $3 and billing_period
    = $4 and status = 'pending' and expires_at > now()`, and a second *payment* of the same
    period is handled by the extension rule (A276), not by refusing the money.
19. **A288 — Every admin payment action is idempotent and audited, and none of them
    deletes anything.** `POST /admin/payments/:id/mark-paid` runs the **same** handler the
    webhook runs (one code path, A281's rule) with `provider = 'manual'` recorded;
    `POST /admin/payments/:id/refund` sets `status = 'refunded'` and writes the gateway fee
    as `0` rather than deleting the row; `PATCH /admin/subscriptions/:user_id` extends
    `current_period_end` and calls `recompute_read_only`; `GET /admin/payments?status=review`
    is the queue A274/A281 feed. Each is a second call that changes nothing.
20. **A289 — A forged webhook is a `docs/THREAT_MODEL.md` §5 row and the phase's residual
    risk is `R7`.** §5 gains three rows ("Forge a webhook to activate a plan without
    paying" → Fails: signature + amount + idempotency; "Replay a webhook to double-activate"
    → Fails: the unique key *and* the guarded transition; "Confirm another account's order"
    → Fails: `order_id` resolves to a pending payment scoped by `user_id`). R7 records the
    one thing this design **cannot** verify — the gateway's own claim that money arrived —
    with its revisit trigger, because a signature proves the message came from the gateway
    and proves nothing about the gateway. §9's Phase 13 row names the test that carries it.

---

## File structure (new this phase)

```
migrations/0034_payments_hardening.sql     # payments.platform/amount_before_discount_idr/
                                           # discount_percent/student_discount/review_reason/
                                           # reconciled_at/provider_status; webhook_events.payment_id;
                                           # usage_events.plan_id; credit_ledger.plan_id;
                                           # users.student_email; subscriptions.pending_plan_id +
                                           # pending_billing_period; user_api_keys.base_url + model;
                                           # email_verification_tokens.purpose; discount_redemptions
config/plans.toml                          # + [credit_packs]
crates/core/Cargo.toml                     # + http
crates/core/src/traits/payment.rs          # NEW — the contract § 1 trait and its DTOs
crates/core/src/traits/mod.rs              # + pub mod payment; re-exports
crates/core/src/traits/testing.rs          # + MockPayment (signs bodies the way a gateway does)
crates/core/src/billing/mod.rs             # NEW — module root, re-exports
crates/core/src/billing/price.rs           # NEW — price(), PriceBreakdown, DiscountCode
crates/core/src/billing/period.rs          # NEW — BillingPeriod, apply_payment(), upgrade_delta()
crates/core/src/lib.rs                     # + pub mod billing;
crates/core/tests/payment_contract.rs      # NEW — the trait, the mock, object safety
crates/core/tests/billing_math.rs          # NEW — pricing, extension, proration, downgrade
crates/server/src/config.rs                # + QrisConfig + BYOK/checkout/student env + validation
crates/server/src/state.rs                 # + payment: Arc<dyn PaymentProvider>
crates/server/src/integrations/mod.rs      # + pub mod payment; Providers.payment
crates/server/src/integrations/payment/mod.rs      # NEW — QrisGateway, from_config
crates/server/src/integrations/payment/qris.rs     # NEW — the HTTP client
crates/server/src/integrations/payment/mapping.rs  # NEW — THE file B1 changes
crates/server/src/integrations/payment/signature.rs# NEW — the three schemes
crates/server/src/billing/mod.rs           # Phase 7's checkout_allowed() + this phase's re-exports
crates/server/src/billing/checkout.rs      # NEW — POST /billing/checkout, the invoice, reuse
crates/server/src/billing/webhook.rs       # NEW — the verified, idempotent, amount-checked apply
crates/server/src/billing/apply.rs         # NEW — the ONE transaction both the webhook and
                                           #       the poller call (A273/A281)
crates/server/src/billing/student.rs       # NEW — the .ac.id verification flow
crates/server/src/billing/byok.rs          # NEW — provider_for(), key encrypt/decrypt
crates/server/src/billing/routes.rs        # NEW — /plans, /billing/*, the public webhook
crates/server/src/settings/mod.rs          # Phase 9's module + the three /settings/api-key routes
crates/server/src/admin.rs                 # Phase 7's + payment tools + the completed margin_report
crates/server/src/jobs/handlers/poll_payment.rs     # NEW
crates/server/src/jobs/handlers/reconcile_payments.rs # NEW
crates/server/src/jobs/handlers/renewal.rs          # NEW — the 7/3/1/0 producer
crates/server/src/jobs/{worker,schedule,handlers/mod}.rs # + three dispatch arms, four boundaries
crates/server/src/routes/mod.rs            # + .merge(billing::routes()), the public webhook route
crates/server/src/lib.rs                   # + pub mod billing; the two new handlers
crates/server/tests/{qris_gateway,billing,webhooks,payment_jobs,billing_periods,renewals,byok,admin_margin}.rs # NEW
crates/server/tests/{route_sweep,schema}.rs # + a row per new route; + discount_redemptions
crates/server/tests/common/mod.rs          # + stub_http_json (the helper search.rs already
                                           #   hand-rolls twice; it moves here, one copy)
crates/server/tests/billing_jakarta.rs     # NEW — gate 6 for this phase's copy
app/src/api/queries/billing.ts             # NEW
app/src/features/billing/{PricingScreen,BillingScreen,CheckoutScreen,CheckoutStatus}.tsx # NEW
app/src/features/billing/{checkout.ts,checkout.test.ts}   # NEW — the pure half
app/src/features/billing/PlanUsageScreen.tsx              # read-only on mobile (A285)
app/src/features/billing/plans.ts          # + prices, credit packs, student price
app/src/router.tsx                         # /pricing, /billing, /billing/checkout/:id
app/src/i18n/locales/{en,id}.json          # + pricing.*, billing.*, checkout.*, reminder.*
scripts/smoke-phase13.sh                   # NEW — the sandbox end-to-end run
docs/{API,ARCHITECTURE,RELEASE,THREAT_MODEL,ASSUMPTIONS}.md
```

**Migration numbers.** Phases 8–11 claim `0027`–`0031`; Phase 12 claims `0032` and `0033`;
this phase claims `0034_payments_hardening.sql`. **Re-read `migrations/` before writing it**
rather than trusting this line, the way Phase 11's plan says to. Whatever the number,
`0017_sync_triggers.sql` stays the last trigger-attaching migration and this phase appends
nothing to its array, because no table this phase touches is `+sync`.

---

## Task 1: `PaymentProvider`, its DTOs, and the mock that signs like a gateway

**Files:**
- Create: `crates/core/src/traits/payment.rs`, `crates/core/tests/payment_contract.rs`
- Modify: `crates/core/src/traits/mod.rs`, `crates/core/src/traits/testing.rs`,
  `crates/core/Cargo.toml`, `Cargo.toml` (the `http` workspace dep)

**Interfaces:**
- Consumes: `chrono::{DateTime, Utc}`, `serde_json::Value`, `hmac` + `sha2` (already core
  deps), `http::HeaderMap`.
- Produces: the contract § 1 trait and its types, plus `MockPayment`.

```rust
// crates/core/src/traits/payment.rs
pub struct InvoiceRequest { pub order_id: String, pub amount_idr: i64,
                            pub description: String, pub expires_at: DateTime<Utc> }
pub struct Invoice { pub order_id: String, pub qr_string: String,
                     pub expires_at: DateTime<Utc>, pub provider_payment_id: Option<String> }
pub enum GatewayStatus { Pending,
    Paid { paid_at: DateTime<Utc>, amount_idr: i64, provider_payment_id: Option<String>,
           fee_idr: Option<i64> },
    Expired, Failed, Refunded }
pub enum WebhookKind { Paid, Expired, Failed, Refunded }
pub struct WebhookEvent { pub event_id: String, pub kind: WebhookKind, pub order_id: String,
                          pub amount_idr: i64, pub currency: String,
                          pub paid_at: Option<DateTime<Utc>>, pub provider_payment_id: Option<String>,
                          pub fee_idr: Option<i64>, pub raw: serde_json::Value }
#[derive(Debug, thiserror::Error)]
pub enum PaymentError { Transport(String), Invalid(String), Signature, UnknownOrder,
                        Provider(String), NotConfigured }

#[async_trait::async_trait]
pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn create_invoice(&self, req: InvoiceRequest) -> Result<Invoice, PaymentError>;
    async fn get_status(&self, order_id: &str) -> Result<GatewayStatus, PaymentError>;
    fn parse_and_verify_webhook(&self, headers: &http::HeaderMap, raw_body: &[u8])
        -> Result<WebhookEvent, PaymentError>;
    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError>;
}
```

`MockPayment` (in `testing.rs`) implements the trait with a real HMAC: it holds a
`Mutex<VecDeque<GatewayStatus>>` for `get_status`, a `Mutex<Vec<InvoiceRequest>>` so a test
can assert what was asked for, and

```rust
impl MockPayment {
    pub fn new() -> Self;                          // secret = b"mock-secret"
    pub fn with_secret(secret: &[u8]) -> Self;
    /// The headers a real gateway would send for this body, so a test signs the
    /// same bytes it posts and the route's verification is exercised for real.
    pub fn sign(&self, body: &[u8]) -> http::HeaderMap;
    pub fn push_status(&self, status: GatewayStatus);
    pub fn invoices(&self) -> Vec<InvoiceRequest>;
    pub fn refund_notes(&self) -> Vec<(String, i64)>;
}
```

- [ ] **Step 1: Write the failing test**

```rust
// crates/core/tests/payment_contract.rs
/// Contract § 1's trait is what every later task calls, and a mock gateway is what
/// makes the whole phase testable while B1 is unresolved. This asserts both: the
/// trait is object-safe behind `Arc<dyn PaymentProvider>`, and the mock's signature
/// is a real one rather than a flag.
#[test]
fn the_payment_trait_is_object_safe_and_a_double_drives_every_method() { … }

/// The mock signs the bytes it is given and rejects any other signature, so a
/// webhook test that passes is a webhook test that actually verified something.
#[test]
fn the_mock_verifies_its_own_signature_and_rejects_a_forged_one() {
    // sign(body) -> headers; parse_and_verify_webhook(&headers, body) == Ok(..);
    // then a body swapped under the same headers == Err(PaymentError::Signature).
    // Swapping the BODY and keeping the HEADERS is the assertion that matters: a
    // mock that signed nothing would pass the first half and fail this one.
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-core --test payment_contract` → `error[E0433]` for
`stvdy_core::traits::payment`.

- [ ] **Step 3: Implement**

The trait exactly as contract § 1 writes it, with the DTOs above. `MockPayment`'s
`parse_and_verify_webhook` verifies `HMAC-SHA259(secret, raw_body)` against
`x-mock-signature` (hex), then deserialises the body into the `WebhookEvent` fields; a
body that does not parse is `PaymentError::Invalid`, not a panic.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-core --test payment_contract` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/traits/payment.rs crates/core/src/traits/{mod,testing}.rs \
        crates/core/Cargo.toml Cargo.toml crates/core/tests/payment_contract.rs
git commit -m "feat(core): the payment trait, its DTOs, and a mock gateway that signs"
```

---

## Task 2: `qris_gateway` — the wire, the mapping, and the three signature schemes

**Files:**
- Create: `crates/server/src/integrations/payment/{mod,qris,mapping,signature}.rs`,
  `crates/server/tests/qris_gateway.rs`
- Modify: `crates/server/src/config.rs`, `crates/server/src/integrations/mod.rs`,
  `crates/server/src/state.rs`, `crates/server/src/testing.rs`,
  `crates/server/tests/common/mod.rs`

**Interfaces:**
- Consumes: `PaymentProvider`, `InvoiceRequest`, `Invoice`, `GatewayStatus`,
  `WebhookEvent`, `PaymentError`, `SecretString`, `reqwest`, `hmac`, `sha2`, `hex`.
- Produces: `QrisGateway::from_config(&QrisConfig) -> anyhow::Result<QrisGateway>`,
  `signature::verify(scheme, headers, body, token, header_name) -> Result<(), PaymentError>`,
  `mapping::{invoice_body, invoice_from_response, status_from_response, webhook_from_body}`,
  `AppState.payment: Arc<dyn PaymentProvider>`, and `QrisConfig` + its boot validation.

```rust
// crates/server/src/config.rs
pub struct QrisConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub callback_token: SecretString,
    pub signature_scheme: SignatureScheme,   // closed enum, parsed at boot
    pub signature_header: String,            // default "X-Callback-Signature"
    pub webhook_ips: Vec<IpAddr>,            // empty = no allowlist
    pub fee_percent: f64,                    // default 0.0 = "unknown, record nothing"
    pub fee_fixed_idr: i64,
    pub invoice_ttl_minutes: i64,            // default 15 (docs/SPEC.md)
    pub honor_late_payment: bool,            // default true (A275)
    pub reconcile_hours: i64,                // default 48
}
pub enum SignatureScheme { HmacSha256Hex, HmacSha256Base64, SharedToken }
```

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/qris_gateway.rs
/// The request we send is the internal contract. B1 changes this test's fixtures
/// and nothing else, which is the whole point of isolating the mapping.
#[sqlx::test(migrations = "../../migrations")]
async fn the_gateway_sends_the_order_id_and_amount_the_contract_names(pool: PgPool) { … }

/// The response shape we accept, against a stub HTTP server, so a vendor whose
/// field is named differently fails here rather than in production.
#[sqlx::test(migrations = "../../migrations")]
async fn the_gateway_parses_a_sandbox_shaped_invoice_and_status_response(pool: PgPool) { … }

/// Each scheme verifies its own shape and rejects the other two. Three schemes
/// with one test between them would let a copy-paste make two of them the same.
#[test]
fn every_signature_scheme_accepts_its_own_shape_and_rejects_the_others() { … }

/// A scheme we do not know must stop the process, because the alternative is a
/// default that verifies everything.
#[test]
fn an_unknown_signature_scheme_fails_startup() { … }

/// An unset QRIS_BASE_URL is a supported deployment: the server boots, checkout
/// answers provider_error, and every other route is unaffected.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deployment_with_no_gateway_boots_and_refuses_checkout(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test qris_gateway` → `error[E0433]` for
`stvdy_server::integrations::payment`.

- [ ] **Step 3: Implement**

`qris.rs` is a `reqwest` client (`POST {base_url}/invoices`, `GET {base_url}/invoices/{order_id}`)
with `Authorization: Bearer` from `api_key` — **the paths and the auth header are
themselves in `mapping.rs`**, because those are exactly the things B1 names.
`signature.rs::verify` is pure and takes the raw body, so the route can verify before it
parses (a body that fails the signature is never deserialised). `Providers::build` builds
the gateway when `QRIS_BASE_URL` is set and leaves `AppState.payment` as the mock
otherwise — with `QrisGateway` absent, checkout answers 502 `provider_error` and says so,
which is the honest shape for a deployment with no gateway (the `renderer: Option` pattern,
A130). `common/mod.rs` gains `stub_http_json(status, body) -> (String, JoinHandle<()>)`
lifted from `search.rs`, and `search.rs` uses it too (two copies of a listener is the
second convention this repo's rules forbid).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test qris_gateway` → green.
`cargo test -p stvdy-server --test search` → still green (the helper moved).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/integrations/payment crates/server/src/integrations/mod.rs \
        crates/server/src/{config,state,testing}.rs crates/server/tests/qris_gateway.rs \
        crates/server/tests/common/mod.rs crates/server/tests/search.rs
git commit -m "feat(server): the qris gateway behind one mapping file and three signature schemes"
```

---

## Task 3: `crates/core/src/billing/` — the amount and the period, pure

**Files:**
- Create: `crates/core/src/billing/{mod,price,period}.rs`, `crates/core/tests/billing_math.rs`
- Modify: `crates/core/src/lib.rs`, `config/plans.toml` (`[credit_packs]`),
  `crates/core/src/config/plans.rs` (`CreditPacks`)

**Interfaces:**
- Consumes: `config::{PlanConfig, PlanId, Credits}`, `chrono::{DateTime, Utc, Duration}`.
- Produces:

```rust
pub enum BillingPeriod { Monthly, Yearly }
impl BillingPeriod { pub fn days(self) -> i64; pub fn as_str(self) -> &'static str;
                     pub fn parse(s: &str) -> Option<Self>; }

pub struct DiscountCode { pub code: String, pub percent_off: u8 }

pub struct PriceBreakdown { pub list_idr: i64, pub percent_off: u8, pub discount_idr: i64,
                            pub due_idr: i64, pub student_applied: bool,
                            pub code_applied: Option<String> }

/// The one function that computes what a student owes. `code` and `student` do not
/// stack (A278): the larger percentage wins.
pub fn price(plan: &PlanConfig, period: BillingPeriod, student: bool,
             code: Option<&DiscountCode>) -> PriceBreakdown;

pub struct PeriodChange { pub start: DateTime<Utc>, pub end: DateTime<Utc>, pub extends: bool }

/// Extension from `current_period_end` when there is live paid time, a fresh
/// period at `paid_at` otherwise (A276).
pub fn apply_payment(active_end: Option<DateTime<Utc>>, paid_at: DateTime<Utc>,
                     period: BillingPeriod) -> PeriodChange;

/// The prorated difference for the remaining days of the current period, rounded
/// to the rupiah. Zero when the target plan is not more expensive.
pub fn upgrade_delta(from: &PlanConfig, to: &PlanConfig, period: BillingPeriod,
                     remaining_days: i64) -> i64;
```

- [ ] **Step 1: Write the failing tests**

```rust
// crates/core/tests/billing_math.rs
/// Paying early must not lose days: the new period starts where the old one ends.
#[test]
fn a_purchase_while_active_extends_from_the_period_end_not_from_today() { … }

/// Paying after expiry starts at the payment, not at the old end (or the student
/// pays for the gap they did not have access in).
#[test]
fn an_expired_subscription_restarts_at_paid_at() { … }

/// The student promo and a code are `max(percent)`, never a sum, and the code is
/// named in the breakdown so the invoice can print it.
#[test]
fn a_student_discount_and_a_code_do_not_stack() { … }

/// The upgrade charges the difference for the remaining days only.
#[test]
fn an_upgrade_charges_the_prorated_difference_for_the_remaining_days() { … }

/// A yearly price is not twelve monthlies; it is the config's number, and the
/// monthly equivalent is never computed (an invented number is the defect).
#[test]
fn the_yearly_price_is_the_config_number_and_nothing_is_derived_from_it() { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-core --test billing_math` → `error[E0433]` for `stvdy_core::billing`.

- [ ] **Step 3: Implement**

`price()` reads `price_idr_monthly`/`price_idr_yearly` and `student_percent_off` from the
`PlanConfig` it is handed; nothing is a literal and nothing is derived. `upgrade_delta` is
`max(0, (to_price - from_price) * remaining_days / period.days())` in integer rupiah.
`config/plans.toml` gains `[credit_packs]` with three entries (`credits`, `price_idr`) and
`plans.rs` gains `CreditPacks` + validation (positive credits, positive price, unique
amounts) so a pack price is config and not code (A21).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-core --test billing_math` → green.
`cargo test -p stvdy-core --test plans_matrix` → green (the new table validates).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/billing crates/core/src/lib.rs crates/core/src/config/plans.rs \
        config/plans.toml crates/core/tests/billing_math.rs
git commit -m "feat(core): the pricing function and the period rules, as pure functions"
```

---

## Task 4: Checkout — the payment row, the discount, the student price, the reuse rule

**Files:**
- Create: `crates/server/src/billing/{checkout,routes,student}.rs`,
  `crates/server/tests/billing.rs`, `migrations/0034_payments_hardening.sql`
- Modify: `crates/server/src/billing/mod.rs`, `crates/server/src/routes/mod.rs`,
  `crates/server/src/lib.rs`, `crates/server/tests/route_sweep.rs`

**Interfaces:**
- Consumes: `billing::checkout_allowed` (Phase 7), `stvdy_core::billing::{price,
  PriceBreakdown, BillingPeriod}`, `AppState.payment`, `AuthUser`, `credits::costs`.
- Produces: `billing::routes() -> Router<AppState>`, `checkout::create(state, user, body)
  -> AppResult<PaymentView>`, `student::{request, confirm}`, and the `PaymentView` DTO:

```rust
pub struct PaymentView { pub id: Uuid, pub order_id: String, pub kind: String,
    pub plan_id: Option<String>, pub billing_period: Option<String>, pub credits: Option<i32>,
    pub amount_idr: i64, pub amount_before_discount_idr: i64, pub discount_percent: u8,
    pub discount_code: Option<String>, pub student_discount: bool,
    pub qr_string: Option<String>, pub expires_at: DateTime<Utc>, pub status: String,
    pub paid_at: Option<DateTime<Utc>> }
```

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/billing.rs
/// Phase 7's boundary, called first, with the platform from the device row: a body
/// that says `platform: "web"` from an `ios` device still refuses (A144/A151).
#[sqlx::test(migrations = "../../migrations")]
async fn checkout_is_refused_on_a_mobile_device_and_a_body_cannot_claim_otherwise(pool: PgPool) { … }

/// A43's rule reaches checkout too: an unverified account is 403 `email_not_verified`.
#[sqlx::test(migrations = "../../migrations")]
async fn checkout_requires_a_verified_email(pool: PgPool) { … }

/// One pending payment, one order id, a 15-minute expiry, and the amount the
/// webhook will later be checked against.
#[sqlx::test(migrations = "../../migrations")]
async fn checkout_writes_one_pending_payment_with_the_quoted_amount(pool: PgPool) { … }

/// Asking twice for the same plan and period reuses the invoice instead of
/// minting a second QR (A287) — the spec's "paying twice" case, half of it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_checkout_for_the_same_period_reuses_the_pending_invoice(pool: PgPool) { … }

/// The student price is applied from `student_verified`, and a code does not stack.
#[sqlx::test(migrations = "../../migrations")]
async fn a_verified_student_gets_the_student_price_and_a_code_does_not_stack(pool: PgPool) { … }

/// A code past `max_redemptions`, or used twice by the same account, is refused
/// with 400 `validation` and no payment row.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exhausted_or_reused_discount_code_is_refused(pool: PgPool) { … }

/// The amount the client is shown is the amount in the row, from the one pricing
/// function — the honesty rule (gate 7(b)) as an assertion.
#[sqlx::test(migrations = "../../migrations")]
async fn the_quoted_amount_is_the_stored_amount(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test billing` → `error[E0433]` for `stvdy_server::billing::checkout`.

- [ ] **Step 3: Implement**

The route's first statement is `billing::checkout_allowed(&state.config.billing_policy,
&user.platform)?`. Then the pricing function, the `payments` insert (with
`amount_before_discount_idr`, `discount_percent`, `student_discount`, `discount_code`,
`platform`), the discount redemption inside the same transaction, the gateway call, and the
`qr_string`/`provider_payment_id` written back. A gateway failure rolls the whole
transaction back and answers 502 `provider_error` — a pending row with no QR is worse than
no row. `student.rs` mints a `purpose = 'student_verify'` token in
`email_verification_tokens` and mails it through the existing `Mailer`; the confirm route
sets `student_verified` + `student_email`. Migration `0034` adds every column listed in the
file-structure block.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test billing` → green.
`cargo test -p stvdy-server --test route_sweep` → green (the new routes are classified).

- [ ] **Step 5: Commit**

```bash
git add migrations/0034_payments_hardening.sql crates/server/src/billing \
        crates/server/src/{routes/mod,lib}.rs crates/server/tests/{billing,route_sweep}.rs
git commit -m "feat(server): checkout with the student price, discounts, and one QR per period"
```

---

## Task 5: The webhook — verified, idempotent, amount-checked, one transaction

**Files:**
- Create: `crates/server/src/billing/{webhook,apply}.rs`, `crates/server/tests/webhooks.rs`
- Modify: `crates/server/src/routes/mod.rs` (a public route), `crates/server/src/lib.rs`

**Interfaces:**
- Consumes: `AppState.payment`, `stvdy_core::billing::apply_payment`, `credits::{grant,
  grant_purchased}`, `jobs::enqueue`.
- Produces: `webhook::handle(state, headers, body) -> AppResult<WebhookOutcome>` and
  `apply::apply_paid(state, payment_id, event) -> AppResult<Applied>` — **the one function
  the webhook and `poll_payment` both call** (A281).

```rust
pub struct WebhookOutcome { pub duplicate: bool, pub status: String }
pub enum Applied { Activated { plan: PlanId, period_end: DateTime<Utc> },
                   Extended { plan: PlanId, period_end: DateTime<Utc> },
                   Credits { granted: i32 },
                   AlreadyPaid,
                   Review { reason: String } }
```

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/webhooks.rs
/// The exit gate's happy path, and the second device: after the webhook, a token
/// minted on another device sees the paid plan, because the plan is not a JWT
/// claim (A22) and entitlements load per request.
#[sqlx::test(migrations = "../../migrations")]
async fn a_signed_paid_webhook_activates_the_plan_and_the_second_device_sees_it(pool: PgPool) { … }

/// The same event id twice: one activation, one credit grant, and the second
/// answer says `duplicate` (A273, the `webhook_events` half).
#[sqlx::test(migrations = "../../migrations")]
async fn the_same_event_id_twice_activates_once(pool: PgPool) { … }

/// The same payment under a NEW event id: still one activation (A273, the guarded
/// transition half). This is the case a real gateway produces on retry, and the
/// one a unique-key-only defence misses.
#[sqlx::test(migrations = "../../migrations")]
async fn the_same_payment_under_a_new_event_id_still_activates_once(pool: PgPool) { … }

/// Over- and under-payment both land in `review` with the two numbers recorded,
/// and the subscription is untouched (A274).
#[sqlx::test(migrations = "../../migrations")]
async fn a_mismatched_amount_lands_in_review_and_activates_nothing(pool: PgPool) { … }

/// Paid before expiry, webhook after it: honoured, because the lateness is ours.
#[sqlx::test(migrations = "../../migrations")]
async fn a_payment_made_before_expiry_is_honoured_after_it(pool: PgPool) { … }

/// Paid after expiry: `review`, never automatic (A275).
#[sqlx::test(migrations = "../../migrations")]
async fn a_payment_made_after_expiry_goes_to_review(pool: PgPool) { … }

/// An order id we do not have is 404 `not_found` and writes no webhook event, so a
/// forged order id cannot even leave a row behind.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_order_id_is_not_found_and_writes_nothing(pool: PgPool) { … }

/// A bad signature is 401 `unauthorized`, writes no `webhook_events` row and
/// touches no payment — the §5 "forge a webhook" row, as a case (A289).
#[sqlx::test(migrations = "../../migrations")]
async fn a_forged_signature_is_unauthorized_and_changes_nothing(pool: PgPool) { … }

/// The webhook arriving before the client polls: the poll reads `paid`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_webhook_that_arrives_first_is_visible_to_the_poll(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test webhooks` → 404 for `/webhooks/qris` (the route is not
registered), which is the *observed* status a test-first pass is for.

- [ ] **Step 3: Implement**

`handle` verifies (IP allowlist, then `signature::verify`), then parses, then calls
`apply::apply_paid` **inside one transaction**: `insert into webhook_events … on conflict do
nothing returning id` → no row, return `duplicate`; `update payments set status='paid' …
where id = $1 and status = 'pending'` → zero rows, return `AlreadyPaid`; the amount and
currency check → `review`; then `apply_payment` (Task 3), the subscription upsert, the
credit grant, `gateway_fee_idr`, `reconciled_at`, and `jobs::enqueue` for the
"Pro is active" notification. The route sits in `public_routes` (no bearer token) and
`route_sweep`'s `EXEMPT` gains the row with its reason.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test webhooks` → green, 9 cases.
`cargo test -p stvdy-server --test route_sweep` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/billing/{webhook,apply}.rs crates/server/src/{routes/mod,lib}.rs \
        crates/server/tests/{webhooks,route_sweep}.rs
git commit -m "feat(server): a verified, idempotent, amount-checked webhook in one transaction"
```

---

## Task 6: `poll_payment` and `reconcile_payments`

**Files:**
- Create: `crates/server/src/jobs/handlers/{poll_payment,reconcile_payments}.rs`,
  `crates/server/tests/payment_jobs.rs`
- Modify: `crates/server/src/jobs/{worker,schedule,handlers/mod}.rs`, `crates/server/src/lib.rs`

**Interfaces:**
- Consumes: `billing::apply::apply_paid` (the same function the webhook calls),
  `AppState.payment`, `jobs::{enqueue, Failure}`.
- Produces: `poll_payment::run`, `reconcile_payments::run`, and the two dispatch arms.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/payment_jobs.rs
/// The spec's fallback: a webhook that was lost is recovered by asking the gateway.
#[sqlx::test(migrations = "../../migrations")]
async fn poll_payment_activates_a_payment_whose_webhook_was_lost(pool: PgPool) { … }

/// Still pending at the gateway → still pending here, and `expired` once the
/// invoice's own deadline passes. The row is never left `pending` forever.
#[sqlx::test(migrations = "../../migrations")]
async fn poll_payment_expires_an_invoice_the_gateway_says_is_dead(pool: PgPool) { … }

/// The poller and the webhook run the SAME apply, so a poll that lands after the
/// webhook is a no-op rather than a second activation.
#[sqlx::test(migrations = "../../migrations")]
async fn a_poll_after_a_webhook_changes_nothing(pool: PgPool) { … }

/// A gateway transaction we have no row for is a `review` flag, not silence.
#[sqlx::test(migrations = "../../migrations")]
async fn reconcile_flags_a_gateway_payment_we_never_recorded(pool: PgPool) { … }

/// One of ours the gateway does not know is the other flag.
#[sqlx::test(migrations = "../../migrations")]
async fn reconcile_flags_a_payment_the_gateway_does_not_know(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test payment_jobs` → `error[E0433]` for
`stvdy_server::jobs::handlers::poll_payment`.

- [ ] **Step 3: Implement**

`poll_payment` is a **singleton** job (`dedupe_key = "poll_payment"`) the scheduler enqueues
every 5 minutes; it scans `payments` where `status = 'pending'` and `expires_at` is within
the window (the partial index exists for this), calls `get_status`, and on `Paid` calls
`apply::apply_paid` with a synthetic event id `"poll:<order_id>:<paid_at>"` — a stable id,
so two polls of the same payment collide on the unique key exactly like two webhooks.
`reconcile_payments` (hourly, `dedupe_key = "reconcile_payments"`) compares the gateway's
recent list against `payments` and writes `review_reason` on both mismatch kinds. Neither
retries a permanent failure into a loop: an unknown order at the gateway is `review`, not a
`Failure`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test payment_jobs` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/jobs/handlers/{poll_payment,reconcile_payments}.rs \
        crates/server/src/jobs/{worker,schedule,handlers/mod}.rs crates/server/src/lib.rs \
        crates/server/tests/payment_jobs.rs
git commit -m "feat(server): the polling fallback and the reconciliation sweep"
```

---

## Task 7: Period extension, upgrade proration, downgrade at period end, credit packs

**Files:**
- Create: `crates/server/tests/billing_periods.rs`
- Modify: `crates/server/src/billing/{apply,checkout}.rs`, `crates/server/src/credits.rs`
  (`grant_purchased`), `crates/server/src/entitlements.rs` (the rollover applies a pending
  downgrade), `crates/server/src/jobs/handlers/counters.rs`

**Interfaces:**
- Consumes: `stvdy_core::billing::{apply_payment, upgrade_delta, BillingPeriod}`.
- Produces: `credits::grant_purchased(pool, user_id, credits, ref_id) -> AppResult<()>`,
  `apply::apply_pending_plan(pool, user_id) -> AppResult<Option<PlanId>>`,
  `checkout::upgrade(state, user, target_plan, period) -> AppResult<PaymentView>`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/billing_periods.rs
#[sqlx::test(migrations = "../../migrations")]
async fn paying_while_active_extends_from_the_period_end(pool: PgPool) { … }

#[sqlx::test(migrations = "../../migrations")]
async fn paying_after_expiry_starts_a_fresh_period_at_paid_at(pool: PgPool) { … }

/// An upgrade switches immediately and writes a proration payment row; the
/// subscription's end does not move.
#[sqlx::test(migrations = "../../migrations")]
async fn an_upgrade_switches_immediately_and_charges_the_prorated_difference(pool: PgPool) { … }

/// A downgrade is recorded, not applied: the plan changes at the period end and
/// the paid time is never shortened (A276).
#[sqlx::test(migrations = "../../migrations")]
async fn a_downgrade_waits_for_the_period_end_and_never_shortens_paid_time(pool: PgPool) { … }

/// The purchased bucket: a credit pack lands there, is never expired by the
/// rollover, and is spent after the monthly bucket (A9/A277).
#[sqlx::test(migrations = "../../migrations")]
async fn a_credit_pack_lands_in_the_purchased_bucket_and_survives_the_rollover(pool: PgPool) { … }

/// Two packs are two grants, and the ledger names each payment so a double grant
/// is visible rather than looking like an ordinary top-up.
#[sqlx::test(migrations = "../../migrations")]
async fn two_credit_packs_are_two_grants_each_naming_its_payment(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test billing_periods` → `error[E0599]` for
`credits::grant_purchased`.

- [ ] **Step 3: Implement**

`grant_purchased` is `insert into credit_ledger (user_id, delta, bucket, reason, ref_id)
values ($1, $2, 'purchased', 'credit_pack', $3)` inside the caller's transaction, so a
credit pack and the payment row that bought it commit together. The rollover in
`reset_counters` calls `apply_pending_plan`, which — when `current_period_end <= now()` and
`pending_plan_id` is set — moves `plan_id`, clears the pending columns and calls
`recompute_read_only` (Phase 7's path, unchanged). `checkout::upgrade` prices with
`upgrade_delta` and writes a second `payments` row with the same `subscription_id`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test billing_periods` → green.
`cargo test -p stvdy-server --test limits_periods` → still green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/billing/{apply,checkout}.rs crates/server/src/credits.rs \
        crates/server/src/entitlements.rs crates/server/src/jobs/handlers/counters.rs \
        crates/server/tests/billing_periods.rs
git commit -m "feat(server): extension, prorated upgrade, period-end downgrade and credit packs"
```

---

## Task 8: Renewal reminders at 7/3/1/0 days

**Files:**
- Create: `crates/server/src/jobs/handlers/renewal.rs`, `crates/server/tests/renewals.rs`
- Modify: `crates/server/src/jobs/{worker,schedule}.rs`

**Interfaces:**
- Consumes: `jobs::enqueue`, the scheduler's per-account pass, Phase 12's reminder delivery
  kinds (`send_reminder`/`send_push`), `config::BillingPolicy`.
- Produces: `renewal::run(state, job)`, and four boundary rows from `schedule::pass`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/renewals.rs
/// Four boundaries, four reminders, once each — asserted by the enqueued rows, so
/// this test does not depend on Phase 12's transport.
#[sqlx::test(migrations = "../../migrations")]
async fn the_seven_three_one_and_zero_day_reminders_each_fire_once(pool: PgPool) { … }

/// A second scheduler pass in the same day enqueues nothing: the dedupe key is
/// the watermark (A280), exactly as the grant's period key is (A152).
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_pass_sends_nothing(pool: PgPool) { … }

/// The mobile copy names the date and nothing to buy: no price, no link, no CTA
/// (billing_policy), and the same key renders the same sentence in Indonesian.
#[sqlx::test(migrations = "../../migrations")]
async fn the_reminder_copy_follows_billing_policy_on_a_phone(pool: PgPool) { … }

/// Renewing moves the boundary, so the old period's reminders never fire.
#[sqlx::test(migrations = "../../migrations")]
async fn renewing_moves_the_boundary_and_cancels_the_old_reminders(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test renewals` → `error[E0433]` for
`stvdy_server::jobs::handlers::renewal`.

- [ ] **Step 3: Implement**

`schedule::pass` gains four boundaries per live subscription (`period_end - 7/3/1/0 days`,
in the account's own zone through `chrono-tz`), each enqueued with
`dedupe_key = "renewal_reminder:<user_id>:<period_end date>:<days>"` and `run_at` at the
local 09:00 (a reminder at 03:00 is a reminder nobody reads). `renewal::run` enqueues the
delivery through Phase 12's path with the plan, the date and the channel list from the
plan matrix; on `ios`/`android` the payload carries `planEnding` and no price. A job for a
boundary whose period already moved is a no-op (it re-reads `current_period_end` and
compares).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test renewals` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/jobs/handlers/renewal.rs crates/server/src/jobs/{worker,schedule}.rs \
        crates/server/tests/renewals.rs
git commit -m "feat(server): renewal reminders at 7, 3, 1 and 0 days, deduped by period"
```

---

## Task 9: BYOK — the key, the seam, and the two rules that keep it honest

**Files:**
- Create: `crates/server/src/billing/byok.rs`, `crates/server/tests/byok.rs`
- Modify: `crates/server/src/settings/mod.rs` (Phase 9's module: register the three
  routes there), `crates/server/src/chat/routes.rs`,
  `crates/server/src/jobs/handlers/{generate,generate_quiz,report,cheatsheet}.rs`,
  `crates/server/src/config.rs` (the `ENCRYPTION_KEY_HEX` reader is reused, not re-derived)

**Interfaces:**
- Consumes: `aes-gcm`, `ENCRYPTION_KEY_HEX` (already validated at boot),
  `entitlements::require(Feature::Byok)`, `AuthUser.platform`, `LlmProvider`.
- Produces: `byok::{set, clear, describe, provider_for}`, and the three routes.

```rust
pub struct ByokDescription { pub set: bool, pub provider: Option<String>,
                             pub base_url: Option<String>, pub model: Option<String> }

/// The provider a single request should use: the user's key when they have one and
/// their plan grants BYOK, otherwise `None` (the caller uses the server provider).
pub async fn provider_for(state: &AppState, user: &AuthUser) -> AppResult<Option<Arc<dyn LlmProvider>>>;
```

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/byok.rs
#[sqlx::test(migrations = "../../migrations")]
async fn a_free_or_pro_account_cannot_store_a_key(pool: PgPool) { … }   // 403 plan_required

/// The response never carries the key, and the column is ciphertext: a
/// distinctive key string is absent from every body and from `key_enc`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_key_is_encrypted_at_rest_and_never_returned(pool: PgPool) { … }

/// A BYOK call spends no credits and is recorded with `byok = true` and its
/// provider cost, so the margin report can exclude it (A156/A282).
#[sqlx::test(migrations = "../../migrations")]
async fn a_byok_call_consumes_no_credits_and_is_recorded_as_byok(pool: PgPool) { … }

/// The ceiling does not pause a BYOK account: it is spending someone else's money.
#[sqlx::test(migrations = "../../migrations")]
async fn the_ceiling_does_not_pause_a_byok_account(pool: PgPool) { … }

/// A phone cannot set a key: web/desktop only, the same rule as checkout.
#[sqlx::test(migrations = "../../migrations")]
async fn a_mobile_device_cannot_set_a_key(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test byok` → 404 for `/settings/api-key`.

- [ ] **Step 3: Implement**

`set` encrypts with aes-gcm under `ENCRYPTION_KEY_HEX` and a fresh nonce stored with the
ciphertext, upserts `user_api_keys`, and refuses on a mobile device and on a plan without
`Feature::Byok`. `provider_for` decrypts and builds an `OpenAiCompatLlm` from the user's
`base_url`/`model` (falling back to the configured ones), and returns `None` when there is
no key. The chat and generation paths call it once per request and pass the resulting
provider down; when it is `Some`, the spend is skipped, `usage_events.byok` is `true`, and
the cost is still recorded through `record_usage`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test byok` → green.
`cargo test -p stvdy-server --test chat` → still green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/billing/byok.rs crates/server/src/settings/mod.rs \
        crates/server/src/chat/routes.rs \
        crates/server/src/jobs/handlers crates/server/tests/byok.rs
git commit -m "feat(server): BYOK for Power, with no credits spent and the cost still recorded"
```

---

## Task 10: Admin payment tools, and the margin report finished

**Files:**
- Create: `crates/server/tests/admin_margin.rs`
- Modify: `crates/server/src/admin.rs` (Phase 7's file), `crates/server/src/routes/mod.rs`,
  `crates/server/src/credits.rs` (write `plan_id` on spend and grant — A283),
  `crates/server/src/billing/apply.rs` (write `usage_events.plan_id`)

**Interfaces:**
- Consumes: `AdminUser`, `billing::apply::apply_paid` (**the same function**, A288),
  `entitlements::recompute_read_only`, `credits::grant`.
- Produces: the completed `MarginReport` and the four admin routes.

```rust
pub struct MarginReport { pub from: DateTime<Utc>, pub to: DateTime<Utc>,
    pub idr_per_usd: i64, pub months: Vec<MonthMargin>, pub tripwires: Tripwires }
pub struct MonthMargin { pub month: String,           // "2026-10"
    pub per_plan: Vec<PlanMargin>, pub revenue_idr: i64, pub gateway_fee_idr: i64,
    pub free_utilization: Option<f64>, pub paying: i64, pub free_accounts: i64,
    pub conversion_of_free: Option<f64>, pub conversion_of_total: Option<f64>,
    pub as_of: DateTime<Utc> }
pub struct PlanMargin { pub plan_id: String, pub provider_cost_usd: f64,
    pub byok_cost_usd: f64, pub revenue_idr: i64, pub gross_margin_idr: Option<i64> }
pub struct Tripwires { pub free_utilization_over_30: Option<bool>,
    pub conversion_under_3: Option<bool>, pub free_spend_over_quarter_of_revenue: Option<bool>,
    pub plan_loss_two_months_running: Option<bool> }
```

- [ ] **Step 1: Write the failing tests**

```rust
// crates/server/tests/admin_margin.rs
/// The headline acceptance: free and paid provider cost are separate columns, per
/// plan, per month — the thing A46's tripwires cannot be read from a blend.
#[sqlx::test(migrations = "../../migrations")]
async fn the_margin_report_splits_free_and_paid_provider_cost_per_plan_and_month(pool: PgPool) { … }

/// **The attribution case.** A user who was free in June and paid in July keeps
/// June's cost in the free column (A283). Seeded with `usage_events.plan_id`, this
/// is the test that fails if the query falls back to `users.plan_id`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_user_who_converted_in_july_keeps_junes_cost_in_the_free_column(pool: PgPool) { … }

/// Utilization and conversion are derived from that split, and the two conversion
/// ratios are both present because A46's 4% is one of them (A284).
#[sqlx::test(migrations = "../../migrations")]
async fn free_tier_utilization_and_conversion_are_derived_from_the_split(pool: PgPool) { … }

/// A month with no rows is `null`, never `0`: a zero would be an invented number
/// and a tripwire would read it as a real measurement (gate 7(a)).
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_month_is_null_and_the_fx_rate_used_is_printed(pool: PgPool) { … }

/// BYOK cost is its own column, and it is not inside either tier's cost (A156).
#[sqlx::test(migrations = "../../migrations")]
async fn byok_cost_is_its_own_column(pool: PgPool) { … }

/// The tripwires are booleans computed from the numbers, and the two-month one is
/// null when the range has one month.
#[sqlx::test(migrations = "../../migrations")]
async fn the_tripwires_are_computed_and_the_two_month_one_is_null_without_a_previous(pool: PgPool) { … }

/// Marking paid runs the webhook's own apply, so it activates once and a second
/// call changes nothing (A288).
#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_payment_paid_activates_once(pool: PgPool) { … }

/// A refund records the row and leaves the subscription alone.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refund_records_the_row_and_does_not_delete_the_subscription(pool: PgPool) { … }

/// Every admin payment route is 404 for a non-admin (A7).
#[sqlx::test(migrations = "../../migrations")]
async fn a_non_admin_sees_not_found(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test admin_margin` → the Phase 7 tests fail with
`revenue: None` where the new fields are expected, and `plan_id` is not yet written.

- [ ] **Step 3: Implement**

`credits::spend_in`/`grant`/`grant_in` bind the `PlanId` they already receive into the new
`plan_id` columns. The margin query groups `usage_events` by
`(date_trunc('month', created_at), coalesce(usage_events.plan_id, users.plan_id))` and sums
`provider_cost_usd`, splitting `byok` into its own column; revenue groups `payments` by
`(month, plan_id)` for `status='paid'`; gateway fees sum `gateway_fee_idr`; utilization is
`-sum(delta) where delta < 0 and bucket='monthly'` over `sum(delta) where reason =
'monthly_grant'` for the free plan's rows. `IDR_PER_USD` is read once and printed.
`admin::routes()` gains `GET /admin/payments`, `POST /admin/payments/:id/mark-paid`,
`POST /admin/payments/:id/refund`, `PATCH /admin/subscriptions/:user_id`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test admin_margin` → green, 9 cases.
`cargo test -p stvdy-server --test admin` → green (Phase 7's suite, now with a real split).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/{admin,credits}.rs crates/server/src/billing/apply.rs \
        crates/server/src/routes/mod.rs crates/server/tests/admin_margin.rs
git commit -m "feat(server): the margin report splits free from paid, with the tripwires it feeds"
```

---

## Task 11: The SPA — pricing, billing, and the QRIS checkout screen

**Files:**
- Create: `app/src/api/queries/billing.ts`,
  `app/src/features/billing/{PricingScreen,BillingScreen,CheckoutScreen,CheckoutStatus}.tsx`,
  `app/src/features/billing/{checkout.ts,checkout.test.ts}`,
  `app/src/features/billing/{PricingScreen.test.tsx,CheckoutScreen.test.tsx}`
- Modify: `app/src/features/billing/plans.ts` (prices, packs, the student price),
  `app/src/router.tsx`, `app/src/i18n/locales/{en,id}.json`

**Interfaces:**
- Consumes: `@/features/billing/policy` (`lockedFeaturePolicy`, `policyFor`), `PLANS`,
  `@/api/errors`, TanStack Query, `@/platform`.
- Produces: `checkout.ts` (pure) and the three screens.

```ts
// app/src/features/billing/checkout.ts
/** The five states the spec's checkout screen has, from one payment view. */
export type CheckoutState = 'waiting' | 'paid' | 'expired' | 'review' | 'failed' | 'offline'
export function checkoutState(view: PaymentView, now: number): CheckoutState
/** mm:ss to the server's expiry, clamped at 0. Never negative, never invented. */
export function countdown(expiresAt: string, now: number): { mm: number; ss: number; done: boolean }
/** The order summary lines, from the server's own breakdown. */
export function orderLines(view: PaymentView): { label: string; value: string }[]
```

- [ ] **Step 1: Write the failing test**

```ts
// app/src/features/billing/checkout.test.ts
it('the countdown is derived from the server expiry, not the local clock', () => {
  // `countdown(expiresAt, now)` with `now` injected; a client whose clock is an
  // hour fast still shows the server's remaining time, and `done` is true only
  // when `now >= expiresAt`.
})
it('a paid payment never renders a new QR', () => {
  // checkoutState(view, now) === 'paid' for every `expires_at`, including a past one.
})
it('an expired invoice offers a new QR and a review does not', () => {
  // 'expired' → the retry affordance; 'review' → "under review" and no retry.
})
it('no price is rendered when billing policy hides prices', () => {
  // orderLines is only called by a screen behind `policyFor(kind).showPrices`.
})
```

- [ ] **Step 2: Run it and watch it fail**

`npm --prefix app test` → the module does not exist.

- [ ] **Step 3: Implement**

`PricingScreen` (3 plan cards, yearly default, the student banner when
`student_verified`, a comparison table from `limits_view`, an FAQ) renders only where
`policyFor(kind).allowPurchaseUi`; `BillingScreen` (current plan, renewal date, credits,
payment history from `GET /billing/invoices`, buy credits); `CheckoutScreen` renders the
order summary from `payments.amount_idr`, the QR from `qr_string`, the countdown, and the
five states through `CheckoutStatus`. Polling is a TanStack Query with an interval that
stops on a terminal state. The amount is never computed in the client.

- [ ] **Step 4: Run**

`npm --prefix app test && npm --prefix app run build`

- [ ] **Step 5: Commit**

```bash
git add app/src/api/queries/billing.ts app/src/features/billing app/src/router.tsx \
        app/src/i18n/locales
git commit -m "feat(app): the pricing page, the billing page and the QRIS checkout screen"
```

---

## Task 12: The phone's read-only plan screen, and both locales

**Files:**
- Create: `app/src/features/billing/mobile.test.tsx`
- Modify: `app/src/features/billing/PlanUsageScreen.tsx` (Phase 7's),
  `app/src/i18n/locales/{en,id}.json`, `app/src/api/errors.ts` (if a code needs copy)

**The failing test first.**

- [ ] **Step 1: Write the failing test**

```tsx
// app/src/features/billing/mobile.test.tsx
it('renders no price, no checkout and no upgrade cta on a mobile platform', () => {
  // Render PlanUsageScreen with the platform adapter forced to 'ios'; assert no
  // currency string, no button whose label mentions an upgrade, and no link out.
  // The assertion is on the rendered text, because that is what a store reviewer
  // sees — a component that renders a price "but not for that plan" still fails.
})
it('the locked copy is neutral and names no plan', () => {
  // The locked message on mobile is `lock.notAvailableOnPlan` and carries no
  // `minPlan`; the web variant names the plan. Both are keys (A116).
})
```

- [ ] **Step 2: Run it and watch it fail**

`npm --prefix app test` → the test renders a price on `ios`.

- [ ] **Step 3: Implement**

`PlanUsageScreen` branches on `lockedFeaturePolicy(kind)`: with `showPrices === false` it
renders the plan name, the renewal date, the credit meter and the usage, and **nothing
else**. Every new string (pricing, billing, checkout, reminder, student) lands in `en.json`
and `id.json` with placeholder parity, and the pseudo-locale test covers the new keys.

- [ ] **Step 4: Run**

`npm --prefix app test && npm --prefix app run build && node scripts/check-no-cdn.mjs`

- [ ] **Step 5: Commit**

```bash
git add app/src/features/billing/PlanUsageScreen.tsx app/src/features/billing/mobile.test.tsx \
        app/src/i18n/locales
git commit -m "feat(app): the phone's plan screen sells nothing, and both locales say so"
```

---

## Task 13: Docs, the three gates, and the sandbox smoke run

**Files:**
- Create: `crates/server/tests/billing_jakarta.rs`, `scripts/smoke-phase13.sh`
- Modify: `docs/API.md`, `docs/ARCHITECTURE.md`, `docs/RELEASE.md`,
  `docs/THREAT_MODEL.md`, `docs/ASSUMPTIONS.md`

**The failing test first.**

- [ ] **Step 1: Gate 6's test, before the docs.**

```rust
// crates/server/tests/billing_jakarta.rs
#[sqlx::test(migrations = "../../migrations")]
async fn the_money_copy_is_correct_in_indonesia(pool: PgPool) {
    // The product is sold in Indonesia, so the invoice, the reminder and the
    // checkout states are the copy that matters most:
    //   * a payment's `expires_at` renders as the same instant a Jakarta user's
    //     clock shows, read back through `chrono_tz`, not through a +07:00 offset;
    //   * a period end 30 days out lands on the same local calendar day the UI
    //     claims, including across a Jakarta month boundary;
    //   * every new key renders in Indonesian with its placeholders filled —
    //     `checkout.amount`, `checkout.expiresIn`, `reminder.planEnding`,
    //     `pricing.studentBanner` — asserted by scanning both locale files for the
    //     keys this phase added and rendering each with a value.
}
```

`cargo test -p stvdy-server --test billing_jakarta` — fails until Task 12's locale keys
exist, which is why it is written here.

- [ ] **Step 2: `docs/API.md`.** Replace `## Billing (Phase 13)` (`:819`) and
      `## Admin (Phase 13)` (`:829`) with the real thing in the surrounding style: every
      route, its body, its statuses, its refusal codes, the webhook's
      signature/idempotency/amount rules, and the note that `GET /plans` omits prices on
      mobile. `## Settings (Phase 9)`'s two `api-key` lines gain the Phase 13 tag and the
      third (`GET`) route.

- [ ] **Step 3: `docs/ARCHITECTURE.md`.** A `## Payments (Phase 13)` section after
      `## Quiz (Phase 5)`: the trait-first design and exactly what B1 changes; the
      documented internal contract and the one mapping file; the two idempotency defences
      and why one is not enough; the single-transaction apply; polling and reconciliation
      as one code path; the period rules; and the margin split with A283's attribution and
      why the roadmap's literal join is wrong in the conversion month.

- [ ] **Step 4: `docs/RELEASE.md`.** The store-policy section ("read before Phase 13/14")
      becomes the confirmed Phase 13 position: the three enforcement points are live, the
      **residual store risk is stated as accepted with its trigger** (confirm Indonesia's
      current rules before submission), and a new operational note that the **webhook URL
      must not be hosted on the staging VPS** (A45: it is a one-month trial) — the webhook,
      the OAuth redirect URI and the updater manifests all need a host with the product's
      lifetime.

- [ ] **Step 5: `docs/THREAT_MODEL.md`.** §5 gains the three rows A289 names, each with its
      enforcement; §6 gains **R7** (the gateway's own claim of payment is unverifiable by
      us: a signature proves provenance, not settlement; revisit if the gateway exposes a
      settlement API or if a mismatch rate above `X` appears in reconciliation); §9's
      Phase 13 row names `crates/server/tests/webhooks.rs`.

- [ ] **Step 6: `docs/ASSUMPTIONS.md`.** A270–A289 in the "Pinned decisions" table; the
      schema-deviation rows for `0034` (the `plan_id` columns with their rationale, the
      payment breakdown columns, `discount_redemptions`, `email_verification_tokens.purpose`,
      `user_api_keys.base_url`/`model`); the B1 row updated to record that the trait-first
      work landed and that **the only remaining items are the mapping file, one signature
      arm if the scheme is new, and the PJP/perorangan lead-time work, which is not code**;
      and `crates/server/tests/schema.rs`'s `documented` list gains `discount_redemptions`.

- [ ] **Step 7: Gate 5 (accessibility), measured, on the three new screens.**
  (a) keyboard only: tab through the three plan cards, the yearly toggle, the comparison
  table, the QR screen's states and the "new QR" affordance, each with a visible
  `outline-ring`; (b) contrast on both themes with the token test extended to the new
  components' class strings, **including the QR on its white quiet-zone** (a QR whose
  contrast fails is a QR that does not scan); (c) at 375px width every control is ≥44px;
  (d) the checkout state is never colour alone — each of the five states carries a text
  label; (e) 200% text scale with no clipping of the amount or the countdown. Record the
  check and the result; "looks fine" is not a gate.

- [ ] **Step 8: Gate 7 (honesty), measured.** State which rules this phase touched: the
  countdown is a real `expires_at` (asserted by `checkout.test.ts`); the amount shown is
  `payments.amount_idr` (Task 4's assertion); the margin report prints its FX rate as an
  input and returns `null` for a month with no data (Task 10); no confidence, match
  percentage or predicted number appears on any screen; the reminder copy carries no price
  on mobile; and the credit cost is still shown before every generative action (unchanged
  from Phase 7).

- [ ] **Step 9: `scripts/smoke-phase13.sh`,** in the shape the earlier phases' smoke
  scripts use: migrate, boot, seed a verified student, `POST /billing/checkout`, assert a
  QR string and a 15-minute expiry, sign and post a webhook **as the gateway would**, assert
  the plan is active from a second device's token, post the same body twice and assert one
  activation, post a mismatched amount and assert `review`, run `poll_payment` against a
  stub gateway, and print the margin report's free/paid split. Against the **sandbox** when
  `QRIS_BASE_URL` is set; against the mock otherwise, and it prints which it used.

- [ ] **Step 10: Run everything.**
      `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
      && cargo test --workspace && npm --prefix app test && npm --prefix app run build &&
      node scripts/check-no-cdn.mjs && cargo sqlx prepare --workspace --check`

- [ ] **Step 11: Commit.** `docs(phase 13): the payments contract, the store-policy note, and the gate results`

---

## Phase 13 exit checklist

| # | Gate | Command | Result |
|---|---|---|---|
| 1 | Formatting | `cargo fmt --all -- --check` | |
| 2 | Lints | `cargo clippy --workspace --all-targets -- -D warnings` | |
| 3 | Rust tests | `cargo test --workspace` | |
| 4 | Web tests | `npm --prefix app test` | |
| 5 | Web build | `npm --prefix app run build` | |
| 6 | No CDN | `node scripts/check-no-cdn.mjs` | |
| 7 | Offline SQL | `cargo sqlx prepare --workspace --check` | |
| 8 | Shell trio | `just check-shell` | |
| 9 | **Exit gate, part 1** — sandbox invoice → verified webhook → plan active on the second device | `bash scripts/smoke-phase13.sh` | |
| 10 | **Exit gate, part 2** — duplicate / mismatch / late / expired | `cargo test -p stvdy-server --test webhooks` | |
| 11 | **Exit gate, part 3** — mobile checkout blocked | `cargo test -p stvdy-server --test billing` | |
| 12 | **Exit gate, part 4** — the margin report splits free from paid, with utilization and conversion derived | `cargo test -p stvdy-server --test admin_margin -- --nocapture` | |
| 13 | The trait and the mock | `cargo test -p stvdy-core --test payment_contract` | |
| 14 | The pricing and period math | `cargo test -p stvdy-core --test billing_math` | |
| 15 | The gateway mapping and the three schemes | `cargo test -p stvdy-server --test qris_gateway` | |
| 16 | Polling and reconciliation | `cargo test -p stvdy-server --test payment_jobs` | |
| 17 | Period rules and credit packs | `cargo test -p stvdy-server --test billing_periods` | |
| 18 | Renewal reminders | `cargo test -p stvdy-server --test renewals` | |
| 19 | BYOK | `cargo test -p stvdy-server --test byok` | |
| 20 | Route sweep (every new route classified) | `cargo test -p stvdy-server --test route_sweep` | |
| 21 | Schema deviations | `cargo test -p stvdy-server --test schema` | |
| 22 | Threat model §5 rows (Phase 7's suite, extended) | `cargo test -p stvdy-server --test tamper` | |
| 23 | Gate 5 — accessibility | the five measured checks in Task 13 Step 7 | |
| 24 | Gate 6 — Indonesian, invoice and reminder copy | `cargo test -p stvdy-server --test billing_jakarta` | |
| 25 | Gate 7 — honesty | the statements in Task 13 Step 8 | |

## Execution notes

To be filled in as the phase is built, in the shape Phase 5, 6 and 11 use: what the plan
did not anticipate, each with the file that proved it; the gate measurements' actual
numbers; what B1's docs changed when they arrived (the diff should be `mapping.rs`, one
signature arm, and env — if it is more, this plan was wrong and the notes say where); and
anything a later phase inherits that this one could not know.

## Self-Review

**Spec coverage, item by item.** `docs/SPEC.md` § Payments: "trait `PaymentProvider`
(create_invoice, get_status, parse_and_verify_webhook, refund_note)" → Task 1, with the
contract's exact signature; "billing is PREPAID PERIODS only: 30 days or 365 days" → Task 3
(`BillingPeriod::days`) and Task 7; "Server computes the amount (plan price, discount,
student promo), creates a `payments` row (status=pending, unique order_id, expires_at,
default 15 minutes) and calls the gateway" → Task 4; "Return the QR payload/image URL and
expiry … renders the QR with a countdown … waits via `GET /billing/payments/:id` polling" →
Task 11; "verifies the signature/callback token, checks order_id exists and belongs to a
pending payment, and that the paid amount EQUALS the expected amount_idr and currency is
IDR; then, in ONE transaction and idempotently (`webhook_events` unique key): marks paid,
activates/extends, grants credits, records the gateway fee, and notifies" → Task 5, every
clause; "a `poll_payment` job … a reconciliation job compares recent gateway transactions"
→ Task 6; "duplicate webhooks, amount mismatch, payment after expiry, expired/failed
invoices, paying twice, webhook before the poll" → Task 5's nine cases plus Task 4's reuse
rule; "paying while active EXTENDS … after expiry starts at payment time … upgrade switches
immediately and charges the prorated difference … downgrade at period end" → Task 7;
"Renewal reminders at 7, 3, and 1 day before expiry and on expiry day" → Task 8; "monthly/
yearly toggle (default yearly), one-time credit packs, discount codes, student discount
(auto-apply when `.ac.id` is verified), admin tools, require verified email before
checkout" → Tasks 3, 4, 11 and 10; "Never trust client-reported payment status … gateway
secrets only in env … restrict the webhook endpoint (signature verification, plus IP
allowlist)" → A272 and Task 5; "`config/billing_policy.toml` … mobile builds must contain
no checkout screen, no purchase buttons, and no links or text directing users to buy
elsewhere" → Task 12; § Plans' prices and `student_percent_off` → Task 3 and the values
table; § Entitlements engine's "admin margin report (revenue minus provider cost minus
gateway fees, per plan and per user)" and § Honesty rules → Task 10; § Screens 14 and 16 →
Tasks 11, 12 and 10; § QUALITY BAR's "Payment tests: webhook signature verification,
idempotent duplicate webhooks, amount mismatch → review, late payment after expiry,
extension stacking vs fresh start, prorated upgrade, polling fallback activates a payment
when the webhook is missing, billing_policy hides purchase UI/endpoints on ios/android" →
the exit checklist rows 10–12 and 14–19, one test each. Roadmap Phase 13's paragraph and
its margin-report paragraph → the whole plan, with A283 correcting the attribution and A284
naming every definition.

**Placeholders.** None. Every task names its files, its exact test, its run command and its
commit. The four genuine unknowns the spec leaves open are pinned rather than waved at:
the credit-pack amounts (a config table, A21), the signature scheme (a closed set, A272),
the late-payment rule (config, A275), and the conversion definition (both ratios, A284).
**B1 is not a placeholder** — it is the phase's stated risk, with the exact blast radius
written down.

**Type consistency.** `PaymentProvider`, `InvoiceRequest`, `Invoice`, `GatewayStatus`,
`WebhookEvent`, `WebhookKind`, `PaymentError`, `BillingPeriod`, `PriceBreakdown`,
`PeriodChange`, `DiscountCode`, `PaymentView`, `Applied`, `WebhookOutcome`, `MarginReport`,
`MonthMargin`, `PlanMargin`, `Tripwires`, `ByokDescription`, `CheckoutState` are each
defined once and referenced by name in the server, the tests and the SPA.
`billing::checkout_allowed` and `admin::margin_report` are **Phase 7's**, consumed by name
and not redefined. `credits::grant`, `grant_in`, `spend_in`, `costs`, `record_usage`,
`ceiling_paused` and `entitlements::{require, recompute_read_only, effective_plan}` are
reused, never re-implemented. `Entity`/`Mutation`/`MutationResult` are untouched: billing
is not a synced entity and this phase adds nothing to the push allowlist.

**The bug this plan exists to avoid.** The **amount the client is quoted and the amount the
webhook checks** come from one function (`stvdy_core::billing::price`) and one column
(`payments.amount_idr`), asserted in Task 4. A second computation anywhere is a student
charged one number and activated against another, and nothing about it looks wrong in
review.

**Deviations from the contract, each recorded:** A270 (`crates/core` takes `http` for the
contract's own `HeaderMap`), A272 (three signature schemes rather than the spec's unnamed
one), A275 (the late-payment rule made configurable with a stated default), A280 (renewal
reminders are a *producer* into Phase 12's delivery, not a second delivery path), A282
(BYOK's `base_url`/`model` columns beyond the spec's `provider`), **A283 (the `plan_id`
columns, which correct the roadmap's literal `users.plan_id` join)**, and A284 (the
conversion definition given twice because A46's figure is the `paying ÷ free` one).

**Open items.**
1. **B1 (gateway identity + API docs) is still unresolved.** This plan reduces what is left
   to `mapping.rs`, one signature arm if the scheme is not one of the three, and the env
   values — but the **PJP identification and the perorangan KYC (KTP + NPWP + matching bank
   account) are lead-time work that is not code and are not in any task here.** They must be
   started in parallel with this phase, not after it.
2. **The model-tier fallback (A148).** Phase 7 implements the ceiling's *pause*; the
   spec's other half ("falls back to the cheap model") still has no reader. The margin
   report now produces the calibration that decision needs, so it belongs to whichever
   phase acts on it next — not to this one, which is about money in, not cost out.
3. **Tax and PPN (B2) are Phase 15.** Invoices this phase generates carry the amount and
   the discount and **no tax line**, and `docs/RELEASE.md` says so.
4. **`storage_mb` still counts uploaded originals only (A157).** The margin report's
   provider-cost split does not include storage; whether the report should also carry a
   per-account storage cost is left to the phase that measures it, because the operator's
   storage bill is a hosting fact (A45) and not yet a per-user number.
5. **Apple IAP / Google Play Billing remain out of scope.** `PaymentProvider` is generic
   enough to add them (A24, `docs/RELEASE.md`), and the store-policy risk is now stated as
   accepted with its trigger rather than left implicit.
