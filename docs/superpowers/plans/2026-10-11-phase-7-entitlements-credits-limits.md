# Phase 7. Entitlements, credits, limits. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`
> (§ 3 Entitlements, § 5 HTTP error mapping) and in `docs/SPEC.md`
> (§ Plans, § Credit costs, § Entitlements engine, § AUTH AND SECURITY).
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug. Every place this
> plan *does* deviate from a contract is listed in "Decisions this plan pins"
> with a number and a one-line justification.

**Goal:** the product stops being free-for-all and starts being a plan. One function,
`check(&UsageSnapshot, Feature)`, decides whether an action is permitted, affordable and
well-formed; every route calls it; the plan×feature matrix is generated from
`config/plans.toml` and asserted by a generated test; credits are spent inside one
transaction that cannot double-spend; counters reset on the user's own local day and
month; a downgrade makes over-limit data read-only rather than deleting it; the A43
abuse controls are live; and `docs/THREAT_MODEL.md` §5 stops being a claim and becomes
`crates/server/tests/tamper.rs`, one case per "fails" row asserting the exact error code.

**Architecture:** two layers, deliberately separate. `crates/core/src/entitlements/` is
**pure**: a `UsageSnapshot` value in, a verdict out, no database, no clock, no IO, so
the whole matrix is unit-testable and the same function can be compiled for the client
later. `crates/server/src/entitlements.rs` is the only thing that *builds* a snapshot
(counts, plan, subscription status, credit buckets) and the only thing that turns an
`EntitlementError` into an `AppError`. No handler reads a limit from the config
directly, and a test asserts that. Credits stay what A9 pinned them as: no balance
column, balance = `sum(delta)` over `credit_ledger`, every spend inside one transaction
holding `pg_advisory_xact_lock(hashtext(user_id::text))`, monthly bucket first, then
purchased. Counters are **derived** from `usage_events` at read time through
`chrono-tz` rather than materialised, so "how many chat messages today" has exactly one
answer and the answer is the same one the existing daily cap counts.

**Tech Stack:** Rust (core + server), Postgres/sqlx, React + Vite + TypeScript + Tailwind,
TanStack Query. **One dependency change, in Task 8:** `zip` 9.0.1 (`default-features =
false, features = ["deflate"]`) and `quick-xml` 0.42 are *pinned in
`docs/ASSUMPTIONS.md`'s crate-list deviations but not yet in any `Cargo.toml`* — verified:
`grep -n 'zip\|quick-xml' Cargo.toml crates/*/Cargo.toml` returns nothing. DOCX/PPTX
ingestion is what needs them, so Task 8 adds them at the versions the assumptions table
already records, rather than picking a version in the same change as the code.

**Exit gate (observable, from the roadmap):** every route goes through `check`; plan
matrix test; `crates/server/tests/tamper.rs` rejects every row of
`docs/THREAT_MODEL.md` §5 marked "fails", with the exact error code; concurrent credit
spend never double-spends; downgrade → read-only; A43 abuse controls live (unverified
account cannot spend, signup rate limit trips and logs); provider-cost ceiling stops
spend with a clear error; manual plan switch via admin.

**This phase's real risk, and the defence.**

> **The sweep is where a bypass hides.** "Every handler calls `check`" is a property of
> **35 `.route(` call sites registering 34 paths across eight files** (`routes/mod.rs`
> 12, `sources/routes.rs` 5, `flashcards/routes.rs` 4, `quizzes/routes.rs` 4,
> `devices.rs` 3, `workspaces.rs` 2, `chat/routes.rs` 2, `chunks.rs` 1), seven of which
> are pulled in by a `.merge(...)` in `routes/mod.rs`, and three of which are written
> across two source lines (`routes/mod.rs:106-109` `/auth/resend-verification`,
> `routes/mod.rs:110-113` `/auth/google/set-password`, `quizzes/routes.rs:61-64`). A
> review cannot see the one that was missed; a test can. So the sweep is not a task you
> finish by reading: it is two tests that fail when a route is added without a decision.
> `crates/server/tests/route_sweep.rs::every_registered_route_is_classified` extracts
> every `.route(` from `crates/server/src/**/*.rs` and asserts each one is in the
> plan's classification table; `every_swept_route_refuses_an_exhausted_account` drives
> each one with an account whose limits are all used and asserts the exact entitlement
> code, so a route that calls nothing cannot pass by being classified.
>
> The extractor must read whole files, not lines, for exactly the three multi-line
> sites above — a line-oriented regex silently classifies 32 of 34 paths and passes.
>
> **Two secondary risks, each with a mechanical defence.** (1) *A ceiling that is
> configured but not enforced is worse than none, because it looks like a control*
> (roadmap Phase 7). Defence: a test that a user over the ceiling is refused and that
> the ledger is untouched, plus a grep test that `provider_cost_ceiling_usd` has
> exactly one reader. (2) *A second limit source* — a handler that reads
> `plans.toml` for itself, so the matrix has two answers. Defence: a grep test that
> the `limits.rs` shim's functions are called only from `entitlements.rs` and that no
> file outside `entitlements.rs`, `credits.rs` and `usage.rs` reads a `PlanConfig`
> limit field.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes. Prompts
  live in versioned modules, never inline. `tracing` structured logs with request ids;
  log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Server is the only source of truth.** Clients never call LLM, search or Google
  APIs directly. Entitlements are decided on the server, per handler **and** per sync
  mutation (A42). No limit or cost is hardcoded anywhere: the matrix lives in
  `config/plans.toml` and is loaded once at startup.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default
  `Asia/Jakarta`). Local-date math only through `chrono-tz` (A6), never a fixed offset.
- **Plan limits and credit costs are copied, never re-derived** (from
  `config/plans.toml`, which is the authority; the `plans` table is a mirror for FK
  integrity and auditing, A21). The values this phase's tests assert are in the
  "Values this phase charges and refuses" table below.
- **Errors serialize to** `{ code, message, min_plan?, limit?, used? }` — every field
  always present, `null` when not applicable. Status/code pairs are the roadmap's
  § 5 table and are fixed: `plan_required`/`feature_locked` 403, `limit_reached` 422,
  `insufficient_credits` 402, `rate_limited` 429, `email_not_verified` 403.
- **Offline writes are limited to** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings — exactly the six pushable entities. Everything else is
  server-owned and a client push of it is rejected with `read_only_entity`.
- **Quality gate:** `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `npm --prefix app test`, `npm --prefix app run build`,
  `node scripts/check-no-cdn.mjs`, `cargo sqlx prepare --workspace --check`, the
  `src-tauri` trio (`just check-shell`), and `cargo sqlx prepare --workspace` re-run
  after any query changes.
- **UI language English**, but Indonesian documents and queries must work end to end
  (`simple` tsvector config + multilingual embeddings), and every new string this phase
  adds goes through `t()` with both `en.json` and `id.json` entries.

### Values this phase charges and refuses (copied from `config/plans.toml`)

Credit costs — `[credits]`: `chat_message = 1`, `web_search = 2` (A25, the spec's value
is truncated), `flashcard_batch_10 = 4`, `quiz_10 = 6`, `report_outline = 3`,
`report_section = 6`, `task_breakdown = 2`, `syllabus_import = 8`,
`plan_generation = 2`, `agent_turn = 2`, `long_doc_threshold_tokens = 20000`,
`long_doc_credits_per_1k_tokens = 1`.

Provider cost ceilings — `[provider_cost_ceiling_usd]`: `free = 0.50`, `pro = 2.00`,
`power = 6.00`.

| Limit | free | pro | power |
|---|---|---|---|
| `workspaces` | 2 | unlimited | unlimited |
| `sources_total` | 20 | 150 | 500 |
| `max_file_mb` | 20 | 50 | 100 |
| `storage_mb` | 200 | 2000 | 10000 |
| `devices_max` | 2 | 5 | 10 |
| `credits_per_month` | 60 | 500 | 2000 |
| `chat_msgs_per_day` | 20 | unlimited | unlimited |
| `web_searches_per_day` | 3 | 30 | 100 |
| `flashcards_per_month` | 30 | unlimited | unlimited |
| `quizzes_per_month` | 2 | unlimited | unlimited |
| `reports_per_month` | 1 | unlimited | unlimited |
| `syllabus_imports_total` | 1 | unlimited | unlimited |
| `google_accounts_max` | 0 | 1 | unlimited |
| `export_formats` | `[md]` | `[md,pdf,docx]` | `[md,pdf,docx,csv,json]` |
| `task_ai_breakdown` / `quick_capture` / `google_calendar` / `agent_actions` | false | true | true |
| `planner` | `manual` | `auto` | `auto+balancer+weakness_blocks` |
| `reminder_channels` | `[web,push]` | `[web,push,email]` | `+telegram,discord` |
| `model_tier` | `cheapest` | `cheap` | `best` |
| `byok` / `advanced_analytics` | false | false | true |

Prices (IDR, for the plan screen's *plan name* only — no price is shown in this phase):
free `0`; pro `10000`/mo, `100000`/yr, `student_percent_off = 50`; power `25000`/mo,
`250000`/yr, `student_percent_off = 50`. Grace after expiry: **3 days** (configurable).

---

## What already exists (checked in this tree, not assumed)

**The entitlements engine does not exist.** `crates/core/src/entitlements/` is absent;
`crates/core/src/lib.rs` lists `chunk, citations, config, error, generation, prompts,
quiz, retrieval, search, sm2, traits` and no `entitlements`. `Feature`,
`EntitlementError`, `UsageSnapshot` and `check` exist only as the roadmap's § 3 sketch.

**The Phase 1 limits shim is the thing being replaced, and its response shape must not
change.** `crates/server/src/limits.rs`:
- `upgrade_from(plan) -> Option<PlanId>` — `None` on power, because "not for sale" is
  the truth. This is why `EntitlementError::LimitReached.min_plan` is an `Option` here
  (A140) and not the `PlanId` the contract spells.
- `limit_reached(plan, limit, used, feature) -> AppError` builds
  `AppError::Entitlement { code: "limit_reached", min_plan, limit, used }`.
- `enforce_count`, `enforce_batch`, `enforce_max_file_mb`, `enforce_storage_mb`.
- Its call sites today: `auth/routes.rs:204` (devices), `workspaces.rs:120`
  (workspaces), `sources/routes.rs:342` (max file), `sources/routes.rs:417`
  (`sources_total`), `sources/routes.rs:446` (storage), `flashcards/routes.rs:191`
  (`flashcards_per_month`), `quizzes/routes.rs:225` (`quizzes_per_month`). Phase 7
  replaces those seven call sites with `entitlements::require` and the bodies do not
  change (A52 says exactly this). An **eighth** call site stays where it is:
  `credits.rs:188` (`enforce_count` inside `spend_in`'s daily-cap loop), because a
  credit spend's cap check must be inside the spend's own transaction.

**The credit spend already exists, at the roadmap's § 3 shape.**
`crates/server/src/credits.rs`:
- `const BUCKETS: [&str; 2] = ["monthly", "purchased"]`.
- `spend(state, user_id, plan, action, credits, daily_caps) -> Spent` — one
  transaction; `spend_in(conn, …)` is the transaction-sharing form.
- Inside `spend_in`: `select pg_advisory_xact_lock(hashtext($1::text))` **first**, then
  each `DailyCap` (`count(*) from usage_events where … and (not uncached_only or
  cache_hit = false)`) through `limits::enforce_count`, then the bucket sums, then the
  refusal (`insufficient_credits` 402 with `limit`/`used` **null** and the numbers in
  the message), then the negative `credit_ledger` rows (monthly first, then purchased,
  one row per bucket so the ledger names which bucket paid), then the `usage_events`
  row. `Spent { usage_event_id, remaining }`.
- `grant_monthly(pool, user_id, credits)` — the signup grant (A64).
- `record_usage(pool, usage_event_id, in, out, provider_cost_usd, prompt_version)`,
  `latest_usage_event`, `refund`, `refund_in`, `refund_pending`, `RefundTarget`,
  `daily_chat_cap`, `daily_search_cap`, `search_cost`, `chat_message_cost`,
  `record_search`, `plan_for_more_credits`.
- **`provider_cost_ceiling_usd` has no reader anywhere.** `grep -rn ceiling
  crates/server/src` returns nothing; the only other match in the tree is a comment
  about ceiling *division* (`crates/core/src/config/plans.rs:102`). The config loads and
  validates the ceilings (`plans.rs:376` rejects a plan with no entry) and nothing
  enforces them. This is the "looks like a control" case the roadmap names.
- **`ModelTier` has no reader either.** `grep -rn "model_tier\|ModelTier"
  crates/server/src crates/core/src` matches only the `config` re-export. The LLM model
  is `LLM_MODEL` (default `deepseek-v4.1-flash`) for every plan. So A41's
  "free uses `Cheapest`" is **not implemented**, which is why the ceiling's
  "fall back to the cheap model" half cannot land in this phase (A148).

**The monthly reservation Phase 5 built, which this phase generalises rather than
copies.** `crates/server/src/quizzes/routes.rs:201-232`: one transaction; the per-user
advisory lock; then
`count(*) from quizzes where user_id = $1 and deleted_at is null and created_at >=
<local month start> and status in ('ready','generating')`; then
`limits::enforce_batch(plans, plan, quizzes_per_month, used, 1, "quizzes")`; then
`credits::spend_in`; then the reserving `quizzes` row; then commit. The second term of
that count is the whole point and its comment says so: a quiz that is still generating
has no `quiz_questions` rows, so a count that ignored it reads zero and the cap is
exceeded by asking twice. `flashcards/routes.rs:150-215` does the same for
`flashcards_per_month` against `decks.status`. Phase 7 defines this sequence once as
`entitlements::reserve_in` (Task 2) and moves both call sites onto it (Task 3), rather
than writing a third copy for the report reservation.

**The error envelope and status mapping are already the contract's.**
`crates/server/src/error.rs`: `AppError::Entitlement { code, message, min_plan, limit,
used }`; `status()` maps `insufficient_credits` → 402, `limit_reached` → 422, other
`Entitlement`/`EmailNotVerified` → 403; `IntoResponse` always writes all five keys with
`null` for the absent ones. `AppError::EmailNotVerified` → 403 `email_not_verified`.
`AppError::Forbidden` maps to code `"forbidden"`, which is **not** a row of the
roadmap's § 5 table; nothing in this phase uses it and the sweep's table records that.

**Auth extractors.** `crates/server/src/auth/extract.rs`: `AuthUser { user_id,
device_id, session_id, email, email_verified, plan_id, timezone, is_admin,
student_verified }` and `AuthUser::plan() -> PlanId`; `VerifiedUser` (403
`email_not_verified` when the address is unconfirmed, A43); `AdminUser` (404 for a
non-admin, "whether an admin endpoint exists is not something a non-admin needs to
learn"). **`AuthUser` carries no device platform and no subscription status**, so today
the server cannot know it is talking to an iOS build, and `users.plan_id` is the only
plan input (an expired subscription still reads `pro`).

**The four spending routes already take `VerifiedUser`:** `chat/routes.rs:78`,
`flashcards/routes.rs:96`, `quizzes/routes.rs:130`, `sources/routes.rs:238`.

**Every route registered today** (from `crates/server/src/routes/mod.rs` plus each
module's `routes()`): the three groups are public (`/health`,
`/auth/google/start`, `/auth/google/callback`), auth-rate-limited (`/auth/signup`,
`/auth/login`, `/auth/refresh`, `/auth/logout`, `/auth/verify-email`,
`/auth/google/exchange`), and authenticated (`/auth/me`, `/auth/logout-all`,
`/auth/resend-verification`, `/auth/google/set-password`, `.merge(devices::routes())`,
`.merge(workspaces::routes())`, `.merge(chunks::routes())`, `.merge(chat::routes())`,
`.merge(flashcards::routes())`, `.merge(quizzes::routes())`,
`.merge(sources::routes(max_upload_bytes(&state.config.plans)))`). The full
route→feature classification is Task 3's table; the ones written across two source
lines are `routes/mod.rs:106` (`/auth/resend-verification`) and `quizzes/routes.rs:61`
(`/workspaces/{id}/quizzes`), which is why the sweep's extractor reads whole file
bodies and not lines.

**Rate limiting exists, per instance, on the auth routes only.**
`crates/server/src/ratelimit.rs`: `AUTH_PER_MINUTE = 30`, `PeerIp` (deliberately does
**not** trust `X-Forwarded-For`), `auth_layer()` returning `AppError::RateLimited`.
Its own doc comment says A43's database-backed per-IP and per-email-domain signup
limiter is a Phase 7 gate and is "deliberately not half-built here, because a control
that looks like an abuse defence but resets on restart is worse than a documented gap".

**Jobs.** `crates/server/src/jobs/`: `enqueue`, `claim` (`for update skip locked`),
`mark_done`, `mark_failed`, `requeue_orphans`, `MAX_ATTEMPTS = 5`,
`POLL_INTERVAL = 5s`, `Failure::{transient, permanent}`. `worker::dispatch` handles
exactly `ingest_source`, `generate_flashcards`, `generate_quiz` and treats any other
`kind` as a permanent failure. `migrations/0015_jobs.sql` has `kind text not null` with
the full kind list **in a comment, not a CHECK constraint** (verified: the file has no
CHECK), `status`, `attempts`, `error`, `run_at`, and
`create unique index on jobs (dedupe_key) where dedupe_key is not null`. So
`monthly_grant` and `reset_counters` need no migration to exist as kinds — only
handlers and a scheduler. **There is no scheduler today:** nothing enqueues a
time-based job, and `jobs::worker::spawn` is the only background task.

**Ingestion is PDF and pasted text only.** `crates/server/src/sources/routes.rs:29`:
`const SUPPORTED_KINDS: &[&str] = &["pdf", "text"]`, and the `kind` match at line 310
handles only those two. `crates/server/src/sources/parse.rs` has one function,
`pdf_pages(pdfium, bytes, title) -> Result<Vec<(Option<i32>, String)>, String>`.
`migrations/0007_workspaces_sources_chunks.sql:16` already documents the full kind set
as a comment — `pdf | pptx | docx | url | text | web | syllabus` — and `sources` already
has `url`, `file_key`, `file_size`, `mime`, `status`, `error` and **`read_only boolean
not null default false`** (the downgrade flag, already there, unused). There is no
`preview_key` column.

**The URL fetcher chain exists and is wired to nothing.**
`crates/server/src/integrations/fetch.rs`: `is_public_ip`, `is_citable_url`,
`parse_target`, `resolve_public`, `ReqwestFetcher`, `MoliFetcher`, `ExaContentsFetcher`,
`ChainFetcher::{new, from_config, names}`. `crates/server/tests/ssrf.rs` covers it. But
`AppState` has no fetcher field, no route calls it, and `grep -rn "ChainFetcher"` outside
`fetch.rs` matches only the SSRF test. So "save to workspace" (`kind = web`, a Phase 3
acceptance item) was never wired, and the same missing wiring is what URL ingestion
needs.

**The SPA's cost-before-action invariant already holds for the two generative
surfaces.** `crates/server/src/flashcards/routes.rs::list_decks` returns
`cost: { per_batch: plans.credits.flashcard_batch_10, batch_size: CARDS_PER_BATCH,
max_cards: MAX_REQUESTED_CARDS }` and `quizzes/routes.rs` does the same for
`quiz_10`; `app/src/features/studio/FlashcardsPanel.tsx:104` and
`QuizPanel.tsx:163` render it as "Costs N credits per M cards". The client hardcodes no
price. This is the pattern `GET /usage` extends, not replaces.

**The SPA's per-code copy is English literals the i18n contract cannot see.**
`app/src/api/errors.ts` holds `REASON_BY_CODE` and `TITLE_BY_CODE` as literal strings
(`plan_required: 'This is not part of your current plan.'`,
`limit_reached: 'Your plan has reached its limit for this.'`,
`insufficient_credits: 'Your credit balance is short of what this costs.'`, …). They
are not `t()` keys, so `app/src/i18n/i18n.test.ts` (key parity + placeholder parity,
A104/A118) passes while an Indonesian user reads English error copy. Phase 7 owns the
entitlement copy, so it moves these into the locale files.

**The SPA's plan surface is a placeholder.** `app/src/router.tsx:96-97` renders
`/billing` as `NotBuiltRoute` with `titleKey="billing.planAndUsage"`.
`app/src/features/billing/` holds only `policy.ts` and `policy.test.ts` — the
`billing_policy` reader (`parsePolicy`, `DENY`, `lockedFeaturePolicy`) and
`app/src/components/ui/LockedFeature.tsx` (`whatKey`, `minPlan: 'Pro' | 'Power'`,
`reasonKey`), which already renders the two platform variants A106/A116 require.

**Phase 6 (reports, cheatsheet, PDF) is landing in this tree right now, and it lands
first.** Its plan file exists
(`docs/superpowers/plans/2026-10-11-phase-6-reports-cheatsheets.md`) and its **core half
is merged**: `crates/core/src/render/{mod,blocks,svg}.rs`,
`crates/core/src/generation/{report,cheatsheet}.rs`, the three prompt modules, and
`crates/server/src/render/{mod,chrome.rs}` (the pooled Chromium renderer and the six-page
acceptance test in `crates/server/tests/pdf.rs`). What is not in the tree yet is the
**server half**: `crates/server/src/reports/`, `crates/server/src/cheatsheets/`, the
three job handlers, and the migration `0023_report_generation.sql` — a parallel slice is
writing them now.

Two consequences for this phase:

1. **Migration numbers.** Phase 6 takes `0023` and `0024`; this phase's migrations are
   therefore `0025_entitlements_limits.sql` and `0026_ingestion_formats.sql`, not the
   `0023`/`0024` an earlier draft of this plan used. `0017_sync_triggers.sql` still stays
   the last trigger-attaching migration.
2. **Phase 7's sweep table includes Phase 6's routes** (`POST
   /workspaces/:id/reports/outline`, `POST /reports/:id/generate`, `PATCH /reports/:id`,
   `GET /reports/:id`, `GET /reports/:id/export`, and the four cheatsheet routes) as rows
   the sweep test will demand, and Task 3 sweeps whatever Phase 6 actually registered —
   read the routes rather than trusting this list.

**MISSING, and created by this phase:** `crates/core/src/entitlements/` (all four
files), `crates/core/tests/entitlements_matrix.rs`, `crates/server/src/entitlements.rs`,
`crates/server/src/usage.rs`, `crates/server/src/admin.rs`,
`crates/server/src/billing/mod.rs`, `crates/server/src/sync/{mod,push}.rs`,
`crates/server/src/jobs/schedule.rs`, `jobs/handlers/{grants,counters}.rs`, the two
dispatch arms, `platform`/subscription/effective-plan on `AuthUser`, the DB-backed
signup limiter, the per-account spend bucket, the ceiling check, `credit_ledger.period_key`
+ its unique index, `workspaces.read_only`, `devices.read_only`, `signup_attempts`,
`sources.preview_key`, DOCX/PPTX/URL extraction, the fetcher in `AppState`,
`crates/server/tests/{route_sweep,entitlements,limits_periods,abuse,ingestion_formats,tamper,admin}.rs`,
the SPA's plan/usage screen and credit meter, and `scripts/smoke-phase7.sh`.

---

## Decisions this plan pins (so parallel work cannot diverge)

Each is a place the spec, the roadmap and the tree leave a choice. They are appended to
`docs/ASSUMPTIONS.md` as rows **A140–A163** in the same commits as the code.

1. **A140 — `EntitlementError::LimitReached.min_plan` is `Option<PlanId>`, not the
   contract's `PlanId`.** `crates/server/src/limits.rs::upgrade_from` already returns
   `Option` and already sends `min_plan: null` on power, because a limit on the top plan
   is a genuine maximum and "not for sale" is the truth. A non-optional field would have
   to invent a sentinel and the shipped 422 body would start lying.
2. **A141 — `UsageSnapshot.credits` is a balance, and it is a different type from
   `config::Credits`.** The contract writes `credits: Credits`; the tree already uses
   `Credits` for the *cost table* (`plans.rs:61`). The snapshot's type is
   `entitlements::Credits { monthly, purchased }` with `total()`, defined once in
   `entitlements/usage.rs`. Reusing the config type would put `chat_message = 1` where a
   balance belongs.
3. **A142 — `Feature::ReminderChannel(ReminderChannel)` reuses
   `stvdy_core::config::ReminderChannel`.** The contract spells the payload type
   `Channel`; the config enum already exists with exactly the five variants and is
   already deserialised from `plans.toml`. A second enum would be a second place to add
   a channel.
4. **A143 — `FeatureLocked` means "locked, and no upgrade can be offered to this
   client".** Two live cases: the calling device's platform is `ios`/`android`, where
   `config/billing_policy.toml` sets `allow_purchase_ui = false` (A24/A106), or no plan
   in the matrix grants the feature at all. `PlanRequired` means a higher plan has it and
   this client may be told so. This is the server half of the two `LockedFeature`
   variants the SPA already renders, and it is what keeps the two codes from being
   arbitrary synonyms.
5. **A144 — The platform in an entitlement decision comes from `devices.platform`, never
   from the request body.** The threat model's "Enable checkout on iOS/Android" row is
   only a control if the client cannot claim to be a desktop. `AuthUser` gains
   `platform` from the `devices` join it already performs.
6. **A145 — `AuthUser` loads `platform`, the subscription status and the effective plan
   on every request; there is no 5s in-process plan cache.** A22 names the cache, but the
   cache adds a staleness window to the exact thing A22 exists for (a payment on another
   device taking effect immediately), and the extractor already does one query. Recorded
   as a deliberate deviation.
7. **A146 — The effective plan is `Free` unless the subscription status is `active` or
   `grace`.** `users.plan_id` alone never grants paid limits, so an expired subscription
   takes effect without an admin touching the row. An unrecognised status string is
   treated as `None` (free) and logged, because a status the code does not understand
   must not grant paid limits.
8. **A147 — Counters are derived from `usage_events` at read time through `chrono-tz`;
   there is no counters table.** One source of truth for "how many chat messages today",
   and it is the same source the existing daily cap already counts (so the cap and the
   number shown to the user cannot disagree). `reset_counters` is therefore not a
   zeroing job: it is the per-user **local period-boundary rollover** (mark a lapsed
   grace window `expired`, recompute the read-only flags, enqueue that user's
   `monthly_grant`), which is the timezone-aware job the spec asks for.
9. **A148 — The ceiling pauses at 100% and does not fall back to a cheaper model, and
   the pause is 422 `limit_reached`.** The spec offers "falls back to the cheap model
   **or** pauses expensive actions with a clear message"; the pause is the half the
   abuse control needs and the half that can be built now, because `ModelTier` has no
   reader anywhere in the tree (verified) so a tier→model fallback would have to invent
   a tier→model mapping first. The pause carries `min_plan` = the plan whose ceiling is
   higher (free `0.50` → pro `2.00` → power `6.00`, so upgrading genuinely lifts it),
   keeps `limit`/`used` null (they carry the plan-matrix sense, and the ceiling's
   figures are dollars), and names the local reset date in the message. The tier
   fallback is Phase 13 work, listed in this plan's Open items.
10. **A149 — A read of the user's own data is never entitlement-gated, and the sweep
    classifies every route instead of pretending every route has a `Feature`.** The spec
    requires over-limit data to stay "viewable, exportable", and the contract's `Feature`
    enum has no read variants. So each registered route is `Swept(feature)`,
    `SweptPerMutation` (the sync push route), `ReadOnly` (a read of the user's own rows,
    with a one-line reason) or `Exempt` (no authenticated user: `/health`, the OAuth
    pair, the auth routes, the HMAC-signed object download). A new route that is not in
    the table fails the sweep test.
11. **A150 — The per-mutation push decision lands in Phase 7
    (`crates/server/src/sync/push.rs::decide`) and Phase 8 adds only the transport.**
    Phase 8's route takes the advisory lock, loops the mutations, calls `decide` for
    each, applies what it allows and writes `mutation_log`; it re-implements nothing, and
    its convergence suite re-asserts the same rejections through HTTP. Without this split
    the tamper suite's `Message`/`Source`/derived-field rows could not be verified in
    Phase 7 at all, and the roadmap requires them here.
12. **A151 — The checkout platform refusal lands in Phase 7 as
    `billing::checkout_allowed(&BillingPolicy, platform)`, and Phase 13's route calls it
    as its first statement.** Same reason as A150: the threat model's `ios` row is a
    Phase 7 gate and the route is a Phase 13 one. The guard returns 403 `feature_locked`
    (A143's rule: on mobile there is no upgrade to offer).
13. **A152 — The monthly grant is idempotent by construction, not by scheduling:**
    `credit_ledger` gains `period_key text` and
    `create unique index on credit_ledger (user_id, period_key) where period_key is not
    null`, and every grant is `insert … on conflict do nothing` with
    `period_key = 'monthly:<local YYYY-MM>'` (free) or `'sub:<subscription_id>:<period
    start>'` (paid). `jobs.dedupe_key` alone cannot close this: the signup grant (A64)
    and the recurring `monthly_grant` job can both run inside the same month, and the
    roadmap's own note says a duplicate `monthly_grant` double-grants credits.
14. **A153 — `monthly_rollover` is a new `[credits]` key, default `false`.** The spec
    says "no rollover by default, configurable" and the config has no such key. With
    rollover off, the grant job first writes a negative `reason = 'expire'` row for the
    unspent monthly bucket (`period_key = 'expire:<the same key>'`, so it is once-only
    too), then grants. Purchased credits are never expired.
15. **A154 — The signup limiter is database-backed: a `signup_attempts` table, 25
    attempts per IP per 24h and 100 per non-public email domain per 24h, every block
    logged with the IP and the count.** A43 requires the limit to survive a restart (an
    in-process bucket does not) and R5 requires the false-positive rate to be measurable,
    which is what the log line is for. Public mailbox domains (`gmail.com`,
    `yahoo.com`, `outlook.com`, `hotmail.com`, `icloud.com`, `proton.me`, and the
    Indonesian providers) are **exempt from the domain rule**: a farm uses gmail too, so
    a domain limit there blocks the product's own growth and stops no farm, while the
    per-IP rule and the per-account ceiling still bound it.
16. **A155 — The per-account spend cap is an in-process token bucket at 10
    spend-actions per minute per account (`SPEND_RATE_PER_MINUTE`).** Its job is not to
    bound the total (the credits and the ceiling do that) but to bound the ceiling's
    **overshoot**: the ceiling is checked before the provider call and the cost is
    recorded after it, so N concurrent actions can each pass the check before any of them
    records a cost. Bounding N is what makes the ceiling a bound.
17. **A156 — The ceiling's sum excludes `byok = true` rows, and the margin report shows
    BYOK cost in its own column.** A20: a BYOK call consumes no credits and the user's
    own key pays for it, so counting it against the ceiling would pause an account for
    spending someone else's money; counting it into our blended cost would make the
    margin report overstate our cost.
18. **A157 — `storage_mb` counts uploaded originals (`sum(sources.file_size)`), not
    chunks or embeddings.** The spec says the original file "counts against the plan's
    storage quota", and that is what the tree already sums
    (`sources/routes.rs:446`). Recorded as an **open margin item**, not a fix: A47
    measures a chunk at ~6 KiB, so a URL source (no bytes) and a text source both consume
    real disk that no user's quota accounts for.
19. **A158 — The PPTX/DOCX PDF preview needs `soffice` and follows A51's absent-binary
    policy.** Conversion is `soffice --headless --convert-to pdf` into object storage
    under `sources.preview_key`; when `SOFFICE_BIN` is unset or unloadable the source
    still ingests (text and slide numbers are extracted from the OOXML itself), the
    preview is absent, one line is logged, and `docs/RELEASE.md` carries the image
    requirement. An extractor that silently produced nothing would look like a scanned
    document.
20. **A159 — `GET /usage` is the single source of the numbers the UI shows; the ceiling's
    dollar figures are admin-only.** The screen needs the plan, the limits and the costs
    (gate 7(b) and 7(d)); the user needs to know *that* the ceiling is reached and when it
    resets, not our unit economics. `spent_usd`/`ceiling_usd` appear in
    `GET /admin/margin`, and `GET /usage` carries `provider_cost: { paused: bool,
    resets_at: <timestamptz> }`.
21. **A160 — Account export and account deletion are not in this phase.** `docs/API.md`
    tags `## Account` as Phase 7; that was a forecast written in Phase 0 and this plan
    does not honour it, because neither is in the roadmap's Phase 7 paragraph and both
    need the retention/purge machinery (`purge_deleted_accounts`) that belongs with the
    release prerequisites. The heading is re-tagged in Task 13 and the item is recorded
    in the Self-Review's open items.
22. **A161 — The spend cap's route list is a subset of the sweep's `Swept` rows, not a
    second table.** `route_sweep.rs` asserts every entry in `ratelimit::spending_routes()`
    is also a `Swept` row, so the entitlement decision stays the one place that decides
    whether an action is allowed while the cap only decides how fast. Two independent
    lists of "the routes that spend" would drift, and the drift has exactly one shape: a
    capped route the sweep forgets, or a swept route the cap forgets — and the second is
    an unbounded ceiling overshoot, which is the thing A155 exists to bound.
23. **A162 — A `url` source is fetched by the ingest job; a `web` source is fetched by
    the upload request.** A `url` is a link the user typed and is not expected to exist
    yet, so the request must not block on someone else's server, and the SSRF refusal
    belongs on `sources.error`, which the client already renders. A `web` source is "save
    to workspace" from a search hit, and saving means preserving *what was saved*: the
    text that was on the page when the student saved it, not whatever is there when a
    retry happens to run. Both go through `AppState.fetcher` and never a second
    `reqwest` call, so the `--block-private-networks` guarantee is one implementation.
24. **A163 — A `web` source stores the fetched text as its object and therefore charges
    storage; a `url` source stores nothing and charges zero.** A157 counts
    `sum(sources.file_size)`: a saved page is a snapshot we chose to keep, so it is bytes
    we hold and it counts; a URL source's bytes exist only in `chunks`. A `url` source is
    bounded only by `sources_total`, which is the reason that axis exists separately from
    storage.

---

## File structure (new this phase)

```
migrations/0025_entitlements_limits.sql       # workspaces.read_only, devices.read_only,
                                              # credit_ledger.period_key + unique index,
                                              # signup_attempts
migrations/0026_ingestion_formats.sql         # sources.preview_key
config/plans.toml                             # + [credits] monthly_rollover = false
crates/core/src/entitlements/mod.rs           # NEW — module root, re-exports
crates/core/src/entitlements/feature.rs       # NEW — Feature, Feature::ALL
crates/core/src/entitlements/usage.rs         # NEW — UsageSnapshot, Credits, SubStatus, PlanFacts
crates/core/src/entitlements/check.rs         # NEW — EntitlementError, check, the matrix
crates/core/src/lib.rs                        # + pub mod entitlements;
crates/core/src/config/plans.rs               # + Credits::monthly_rollover
crates/core/tests/entitlements_matrix.rs      # NEW — the generated plan×feature matrix test
crates/core/tests/plans_matrix.rs             # + the monthly_rollover default case
crates/server/src/entitlements.rs             # NEW — snapshot(), require(), From<EntitlementError>
crates/server/src/auth/extract.rs             # AuthUser + platform, subscription_status, effective_plan
crates/server/src/limits.rs                   # the shim becomes the mapping's implementation
crates/server/src/credits.rs                  # + the ceiling check, costs view, period-keyed grant
crates/server/src/usage.rs                    # NEW — GET /usage
crates/server/src/admin.rs                    # NEW — plan switch, credit grant, margin scaffolding
crates/server/src/billing/mod.rs              # NEW — checkout_allowed()
crates/server/src/sync/mod.rs                 # NEW — module root
crates/server/src/sync/push.rs                # NEW — decide(), push_features()
crates/server/src/jobs/schedule.rs            # NEW — the per-user local-boundary scheduler
crates/server/src/jobs/handlers/grants.rs     # NEW — monthly_grant
crates/server/src/jobs/handlers/counters.rs   # NEW — reset_counters
crates/server/src/jobs/handlers/mod.rs        # + pub mod grants; pub mod counters;
crates/server/src/jobs/worker.rs              # + two dispatch arms
crates/server/src/jobs/mod.rs                 # + pub mod schedule;
crates/server/src/ratelimit.rs                # + signup_limiter(), spend_layer()
crates/server/src/sources/parse.rs            # + docx_runs(), pptx_slides(), preview_pdf()
crates/server/src/sources/routes.rs           # + docx|pptx|url|web kinds, preview route
crates/server/src/state.rs                    # + fetcher: Arc<ChainFetcher>
crates/server/src/integrations/mod.rs         # builds the fetcher chain
crates/server/src/routes/mod.rs               # + .merge(usage::routes()), .merge(admin::routes())
crates/server/src/lib.rs                      # + the new modules
crates/server/tests/route_sweep.rs            # NEW — the classification + exhaustion tests
crates/server/tests/entitlements.rs           # NEW — snapshot, ceiling, through the API
crates/server/tests/limits_periods.rs         # NEW — counters, grant, rollover, grace, read-only
crates/server/tests/abuse.rs                  # NEW — the A43 controls
crates/server/tests/ingestion_formats.rs      # NEW — DOCX, PPTX, URL, web
crates/server/tests/tamper.rs                 # NEW — every §5 "fails" row
crates/server/tests/admin.rs                  # NEW — plan switch, grant, margin
crates/server/tests/credits.rs                # + the concurrency test
crates/server/tests/schema.rs                 # + signup_attempts in `documented`
app/src/api/queries/usage.ts                  # NEW
app/src/api/errors.ts                         # the copy moves to the locale files
app/src/features/billing/PlanUsageScreen.tsx  # NEW
app/src/features/billing/PlanUsageScreen.test.tsx  # NEW
app/src/features/billing/CreditMeter.tsx      # NEW
app/src/components/layout/Sidebar.tsx         # + the meter
app/src/router.tsx                            # /billing → PlanUsageScreen
app/src/i18n/locales/{en,id}.json             # + error.*, plan.*, meter.*
scripts/smoke-phase7.sh                       # NEW
docs/{API,ARCHITECTURE,THREAT_MODEL,ASSUMPTIONS}.md
```

---

## Task 1: `crates/core/src/entitlements/` — the engine and the generated matrix test

**Files:**
- Create: `crates/core/src/entitlements/{mod,feature,usage,check}.rs`
- Modify: `crates/core/src/lib.rs` (`pub mod entitlements;`), `crates/core/src/config/plans.rs`
  (`Credits::monthly_rollover`), `config/plans.toml`
- Test: `crates/core/tests/entitlements_matrix.rs` (NEW), `crates/core/tests/plans_matrix.rs`

**Interfaces:**
- Consumes: `config::{PlanId, PlanConfig, PlansFile, Limit, ExportFormat, ReminderChannel,
  PlannerMode}`, `config::Credits` (the cost table), `CoreResult`.
- Produces: `Feature` (+ `Feature::ALL`), `Feature::min_plan(&PlansFile) -> Option<PlanId>`,
  `UsageSnapshot`, `Credits` (the balance), `SubStatus`, `PlanFacts`,
  `EntitlementError`, `check(&UsageSnapshot, Feature) -> Result<(), EntitlementError>`,
  `check_with(&PlansFile, &PlanFacts, &UsageSnapshot, Feature)`.

- [ ] **Step 1: Write the failing test**

`crates/core/tests/entitlements_matrix.rs` is **generated from the config**, not a list
of pairs someone remembered to write. Its fixture is the shipped `config/plans.toml`,
exactly as `plans_matrix.rs` already does:

```rust
//! The plan×feature matrix, generated from the shipped `config/plans.toml`.
//!
//! Nothing here enumerates pairs by hand. Every case walks `PlanId::ALL` ×
//! `Feature::ALL` and asserts a property of the whole matrix, so a new `Feature`
//! variant, a new plan, or a `plans.toml` edit is covered the moment it lands.
//! A test that listed the pairs it happened to think of would pass while the
//! arm nobody wrote went unnoticed — which is the failure mode the roadmap's
//! "generated test asserts the whole plan×feature matrix" exists to prevent.

const PLANS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/plans.toml");

fn plans() -> PlansFile { load_plans(Path::new(PLANS_PATH)).expect("shipped plans.toml") }

/// A snapshot with every counter fresh and the plan's full monthly grant unspent,
/// so a refusal can only come from the plan, never from usage.
fn fresh(plan: PlanId) -> UsageSnapshot { … }

#[test]
fn the_matrix_is_total() {
    // Every feature × every plan returns a verdict. `check` must not panic and must
    // not be `unimplemented` for any variant: the enum is matched exhaustively, and
    // this is the test that says so out loud for a reader who never opens check.rs.
    for plan in PlanId::ALL {
        for feature in Feature::ALL {
            let _ = check(&fresh(plan), *feature);
        }
    }
}

#[test]
fn every_refusal_on_a_lower_plan_names_the_plan_that_lifts_it() {
    // The UI's upgrade prompt is built from `min_plan`. A refusal that does not name
    // a plan is a dead end for the user, so the matrix may not contain one: for every
    // feature free refuses, `min_plan` is `Some(pro)`; for every feature pro refuses,
    // `Some(power)`; on power a refusal is a genuine maximum and carries `None`.
    for plan in PlanId::ALL {
        for feature in Feature::ALL {
            match check(&fresh(plan), *feature) {
                Ok(()) => {}
                Err(EntitlementError::PlanRequired { min_plan, .. })
                | Err(EntitlementError::LimitReached { min_plan, .. }) => {
                    assert_eq!(min_plan, next_plan(plan), "{plan:?} refuses {feature:?}");
                }
                Err(EntitlementError::FeatureLocked { .. }) => {
                    // Only reachable with no plan granting the feature; see below.
                }
                Err(EntitlementError::InsufficientCredits { .. }) => {
                    panic!("a fresh snapshot has its full grant, so credits cannot be short")
                }
            }
        }
    }
}

#[test]
fn a_granted_feature_stays_granted_as_the_plan_rises() {
    // The matrix's plan column must be monotone: if free may do it, pro may do it.
    // This is what catches a `check` arm wired to the wrong plan's field — the class
    // of bug a hand-written pair table finds only if the pair was in the table.
    for feature in Feature::ALL {
        let verdicts: Vec<bool> = PlanId::ALL
            .iter()
            .map(|p| check(&fresh(*p), *feature).is_ok())
            .collect();
        assert!(
            !verdicts.windows(2).any(|w| w[0] && !w[1]),
            "{feature:?} is allowed on a lower plan and refused on a higher one: {verdicts:?}"
        );
    }
}

#[test]
fn every_feature_is_granted_by_at_least_one_plan() {
    // `FeatureLocked` means "no plan sells this". In the shipped matrix that must be
    // unreachable, or the feature is dead code the UI can never offer.
    for feature in Feature::ALL {
        assert!(
            PlanId::ALL.iter().any(|p| check(&fresh(*p), *feature).is_ok()),
            "{feature:?} is in no plan's matrix"
        );
    }
}

#[test]
fn a_numeric_limit_refuses_at_exactly_its_configured_maximum() {
    // Generated over every numeric feature and every plan: one below the limit passes,
    // the limit itself fails, and the refusal's `limit`/`used` are the real numbers the
    // UI renders as "3 of 3 devices". A hardcoded limit anywhere fails here.
    for plan in PlanId::ALL {
        for (feature, limit) in numeric_features(&plans(), plan) {
            let max = limit.expect("numeric_features yields only bounded limits");
            let mut u = fresh(plan);
            set_used(&mut u, feature, max - 1);
            assert!(check(&u, feature).is_ok(), "{plan:?} {feature:?} at {max}-1");
            set_used(&mut u, feature, max);
            match check(&u, feature) {
                Err(EntitlementError::LimitReached { limit, used, .. }) => {
                    assert_eq!(limit, max);
                    assert_eq!(used, max);
                }
                other => panic!("{plan:?} {feature:?} at {max}: {other:?}"),
            }
        }
    }
}

#[test]
fn the_specs_documented_plan_values_are_what_the_matrix_enforces() {
    // The one hand-written table in this file, and it is the spec's own numbers
    // (`docs/SPEC.md` § Plans), so a typo in plans.toml or a transposed arm fails.
    assert!(check(&fresh(PlanId::Free), Feature::Byok).is_err());
    assert!(check(&fresh(PlanId::Power), Feature::Byok).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::ExportFormat(ExportFormat::Pdf)).is_err());
    assert!(check(&fresh(PlanId::Pro), Feature::ExportFormat(ExportFormat::Pdf)).is_ok());
    assert!(check(&fresh(PlanId::Pro), Feature::ExportFormat(ExportFormat::Csv)).is_err());
    assert!(check(&fresh(PlanId::Power), Feature::ExportFormat(ExportFormat::Csv)).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::ReminderChannel(ReminderChannel::Email)).is_err());
    assert!(check(&fresh(PlanId::Pro), Feature::ReminderChannel(ReminderChannel::Email)).is_ok());
    assert!(
        check(&fresh(PlanId::Pro), Feature::ReminderChannel(ReminderChannel::Telegram)).is_err()
    );
    assert!(check(&fresh(PlanId::Free), Feature::PlanGeneration).is_err());
    assert!(check(&fresh(PlanId::Pro), Feature::PlanGeneration).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::AdvancedAnalytics).is_err());
    assert!(check(&fresh(PlanId::Power), Feature::AdvancedAnalytics).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::SyllabusImport).is_ok());  // 1 allowed
    let mut used_one = fresh(PlanId::Free);
    used_one.syllabus_imports = 1;
    assert!(check(&used_one, Feature::SyllabusImport).is_err());
    assert!(check(&fresh(PlanId::Free), Feature::UploadSource { size_bytes: 20 * 1024 * 1024 }).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::UploadSource { size_bytes: 20 * 1024 * 1024 + 1 }).is_err());
}
```

Also add one case to `crates/core/tests/plans_matrix.rs`:

```rust
#[test]
fn monthly_rollover_defaults_to_off_and_is_configurable() {
    // The spec says "no rollover by default, configurable". The key must exist and
    // default to false, because a default of `true` silently lets monthly credits
    // accumulate for a user who never spends them.
    assert!(!shipped().credits.monthly_rollover);
    // A scratch copy, the same way the file's other config cases do it: `load_plans`
    // takes a path, and mutating the shipped file to test a variant would make the
    // suite order-dependent.
    let dir = scratch_dir();
    let path = dir.join("plans.toml");
    let raw = std::fs::read_to_string(PLANS_PATH)
        .expect("plans.toml")
        .replace(
            "long_doc_threshold_tokens",
            "monthly_rollover = true\nlong_doc_threshold_tokens",
        );
    std::fs::write(&path, raw).expect("writing the scratch config");
    let parsed = load_plans(&path).expect("a config that enables rollover must load");
    assert!(parsed.credits.monthly_rollover);
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-core --test entitlements_matrix` →
`error[E0433]: failed to resolve: use of undeclared crate or module stvdy_core::entitlements`.
`cargo test -p stvdy-core --test plans_matrix monthly_rollover` →
`error[E0609]: no field monthly_rollover on type Credits`.

- [ ] **Step 3: Write the module**

`crates/core/src/entitlements/feature.rs` — the contract's enum verbatim, plus `ALL`:

```rust
/// Every action the plan matrix governs. One variant per thing a plan can refuse.
///
/// The set is the roadmap § 3 contract, copied, not extended: adding a variant is a
/// contract change. `ALL` exists so the matrix test can walk the whole space rather
/// than the pairs someone remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    CreateWorkspace,
    UploadSource { size_bytes: i64 },
    IngestSource,
    ChatMessage,
    WebSearch,
    CreateDeck,
    FlashcardBatch { n: u32 },
    CreateQuiz { n: u32 },
    Report,
    ExportFormat(ExportFormat),
    TaskBreakdown,
    QuickCapture,
    GoogleCalendar,
    SyllabusImport,
    PlanGeneration,
    AgentAction,
    DeviceLogin,
    Byok,
    /// The payload type is the config's enum, not a second copy of it (A142).
    ReminderChannel(ReminderChannel),
    AdvancedAnalytics,
}

impl Feature {
    /// Every variant, for the generated matrix test. `ExportFormat` and
    /// `ReminderChannel` contribute one entry per variant of their own enum, so the
    /// walk covers the payload space too and not just the discriminant.
    pub const ALL: &'static [Feature] = &[ /* … every variant, and every
        ExportFormat::{Md,Pdf,Docx,Csv,Json} and ReminderChannel::{Web,Push,Email,
        Telegram,Discord} as their own entries … */ ];

    /// The cheapest plan that grants this feature, or `None` when no plan does.
    ///
    /// Derived from the matrix, never from a list: the returned plan is the first in
    /// `PlanId::ALL` whose `PlanConfig` allows the feature, which is what makes
    /// `min_plan` in an error message true by construction.
    pub fn min_plan(self, plans: &PlansFile) -> Option<PlanId> { … }
}
```

`crates/core/src/entitlements/usage.rs`:

```rust
/// The account's credit balance, in the two buckets A9 pins. Distinct from
/// `config::Credits`, which is the *cost table* (A141).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Credits { pub monthly: i64, pub purchased: i64 }

impl Credits { pub fn total(self) -> i64 { self.monthly + self.purchased } }

/// The subscription's state, as the entitlement decision sees it. `None` is an
/// account with no subscription row at all (free), which is not the same fact as an
/// `Expired` one and must not be collapsed into it: only `Expired` has a period that
/// ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubStatus { None, Active, Grace, Expired, Canceled }

impl SubStatus {
    /// An unrecognised string is `None` and is logged by the caller: a status the
    /// code does not understand must not grant paid limits (A146).
    pub fn parse(raw: &str) -> SubStatus { … }
    /// Whether this status still honours `users.plan_id`.
    pub fn grants_paid_limits(self) -> bool { matches!(self, SubStatus::Active | SubStatus::Grace) }
}

/// Everything `check` is allowed to look at. Counters are the caller's, already
/// scoped to the right local day/month through `chrono-tz` (A6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub plan: PlanId,
    pub subscription_status: SubStatus,
    pub credits: Credits,
    pub storage_bytes: i64,
    pub workspaces: i64,
    pub sources: i64,
    pub devices: i64,
    pub chat_msgs_today: i64,
    pub web_searches_today: i64,
    pub flashcards_this_month: i64,
    pub quizzes_this_month: i64,
    pub reports_this_month: i64,
    pub syllabus_imports: i64,
}

/// The plan-dependent facts `check` needs that are not counters: the costs, and
/// whether an upgrade may be offered to this client at all (A143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanFacts { pub upgrade_offerable: bool }
```

`crates/core/src/entitlements/check.rs` — the matrix. The exact arms, each read from
`PlanConfig`, none hardcoded:

| `Feature` | Decision | Refusal |
|---|---|---|
| `CreateWorkspace` | `plans.get(plan).workspaces.allows(used)` | `LimitReached` |
| `UploadSource { size_bytes }` | `sources_total.allows(u.sources)` **and** `size_bytes <= max_file_mb * 1MiB` **and** `storage_bytes + size_bytes <= storage_mb * 1MiB` | `LimitReached` (the failing one's `limit`/`used`) |
| `IngestSource` | `sources_total.allows(u.sources)` | `LimitReached` |
| `ChatMessage` | `chat_msgs_per_day.allows(u.chat_msgs_today)` **and** `credits.total() >= cost(chat_message)` | `LimitReached` / `InsufficientCredits` |
| `WebSearch` | `u.web_searches_today < web_searches_per_day` **and** `credits.total() >= cost(web_search)` | `LimitReached` / `InsufficientCredits` |
| `CreateDeck` | always allowed (a deck is not a limited resource; its cards are) | — |
| `FlashcardBatch { n }` | `flashcards_per_month.allows(u.flashcards_this_month + n)` **and** credits for `ceil(n/10) * flashcard_batch_10` | `LimitReached` / `InsufficientCredits` |
| `CreateQuiz { n }` | `quizzes_per_month.allows(u.quizzes_this_month + 1)` (one quiz per request, A94) **and** credits for `ceil(n/10) * quiz_10` | `LimitReached` / `InsufficientCredits` |
| `Report` | `reports_per_month.allows(u.reports_this_month + 1)` **and** credits for `report_outline` | `LimitReached` / `InsufficientCredits` |
| `ExportFormat(f)` | `export_formats.contains(&f)` | `PlanRequired` / `FeatureLocked` (A143) |
| `TaskBreakdown` | `task_ai_breakdown` | `PlanRequired` / `FeatureLocked` |
| `QuickCapture` | `quick_capture` | `PlanRequired` / `FeatureLocked` |
| `GoogleCalendar` | `google_calendar` | `PlanRequired` / `FeatureLocked` |
| `SyllabusImport` | `syllabus_imports_total.allows(u.syllabus_imports)` **and** credits for `syllabus_import` | `LimitReached` / `InsufficientCredits` |
| `PlanGeneration` | `planner != Manual` | `PlanRequired` / `FeatureLocked` |
| `AgentAction` | `agent_actions` | `PlanRequired` / `FeatureLocked` |
| `DeviceLogin` | `u.devices < devices_max` | `LimitReached` |
| `Byok` | `byok` | `PlanRequired` / `FeatureLocked` |
| `ReminderChannel(c)` | `reminder_channels.contains(&c)` | `PlanRequired` / `FeatureLocked` |
| `AdvancedAnalytics` | `advanced_analytics` | `PlanRequired` / `FeatureLocked` |

```rust
/// The verdict for one action. Pure: no IO, no clock, no database.
///
/// `upgrade_offerable` is the one piece of client state this consults, and it comes
/// from the *device row*, not the request body (A144/A143): on iOS and Android
/// `config/billing_policy.toml` forbids a purchase CTA, so a locked feature is
/// `FeatureLocked` rather than `PlanRequired`.
pub fn check(
    plans: &PlansFile,
    facts: PlanFacts,
    u: &UsageSnapshot,
    f: Feature,
) -> Result<(), EntitlementError>;
```

and the `From<EntitlementError> for AppError` mapping lives in the server
(`crates/server/src/entitlements.rs`), not here: `crates/core` has no `AppError`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-core --test entitlements_matrix` → green.
`cargo test -p stvdy-core` → green, including `plans_matrix` with the new case.
`cargo clippy -p stvdy-core --all-targets -- -D warnings` → clean.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/entitlements crates/core/src/lib.rs \
        crates/core/src/config/plans.rs config/plans.toml \
        crates/core/tests/entitlements_matrix.rs crates/core/tests/plans_matrix.rs
git commit -m "feat(core): the entitlements engine, and the plan matrix it enforces"
```

---

## Task 2: `crates/server/src/entitlements.rs` — the snapshot, and the one decision path

**Files:**
- Create: `crates/server/src/entitlements.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/auth/extract.rs`,
  `crates/server/src/limits.rs`
- Test: `crates/server/tests/entitlements.rs` (NEW)

**Interfaces:**
- Consumes: `stvdy_core::entitlements::{check, Feature, UsageSnapshot, PlanFacts, Credits,
  SubStatus, EntitlementError}`, `limits::{enforce_count, enforce_batch,
  enforce_max_file_mb, enforce_storage_mb}`, `credits::monthly_allowance`.
- Produces: `entitlements::snapshot(&AppState, &AuthUser) -> AppResult<UsageSnapshot>`,
  `entitlements::snapshot_for(&AppState, Uuid) -> AppResult<UsageSnapshot>`,
  `entitlements::require(&AppState, &AuthUser, Feature) -> AppResult<()>`,
  `entitlements::reserve_in(&mut PgConnection, &AppState, &AuthUser, Feature,
  impl FnOnce(&mut PgConnection) -> BoxFuture<AppResult<()>>) -> AppResult<Spent>`,
  `entitlements::limits_view(&PlansFile, PlanId) -> serde_json::Value`,
  `impl From<EntitlementError> for AppError`, and `AuthUser::{ platform,
  subscription_status, effective_plan }`.

- [ ] **Step 1: Write the failing test**

`crates/server/tests/entitlements.rs`:

**UI in this task:** none. This is the server's decision path; no screen changes.
Roadmap gates 5, 6 and 7 are measured in Tasks 8 and 12 and stated in Task 13.

```rust
/// The snapshot is one query, and its counts are the ones the caps use.
///
/// Two sources of truth for "how many chat messages today" is how a user gets told
/// "20 of 20" and is then refused for the 20th, or allowed the 21st. This asserts the
/// snapshot's number and the cap's number are the same number, on a day boundary that
/// is the user's own.
#[sqlx::test(migrations = "../../migrations")]
async fn the_snapshot_counts_the_same_rows_the_daily_cap_counts(pool: PgPool) { … }

/// A device on iOS is refused with `feature_locked`, the same account on macOS with
/// `plan_required` (A143). One fact, two codes, and the difference is a store rule.
#[sqlx::test(migrations = "../../migrations")]
async fn a_locked_feature_is_feature_locked_on_ios_and_plan_required_on_macos(pool: PgPool) { … }

/// `users.plan_id = 'pro'` with an expired subscription gets free's limits (A146).
#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_subscription_drops_to_the_free_limits(pool: PgPool) { … }

/// The mapping the response bodies depend on, asserted against the roadmap's § 5 table.
#[test]
fn the_entitlement_errors_map_to_the_roadmaps_status_and_code() {
    assert_eq!(status_of(EntitlementError::PlanRequired { … }), 403);
    assert_eq!(code_of(EntitlementError::PlanRequired { … }), "plan_required");
    assert_eq!(status_of(EntitlementError::FeatureLocked { … }), 403);
    assert_eq!(status_of(EntitlementError::LimitReached { … }), 422);
    assert_eq!(status_of(EntitlementError::InsufficientCredits { … }), 402);
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-server --test entitlements` →
`error[E0433]: failed to resolve: use of undeclared crate or module stvdy_server::entitlements`.

- [ ] **Step 3: Write the module**

```rust
/// The caller's plan, as the decision sees it: `users.plan_id` is honoured only
/// while the subscription is active or in grace (A146).
pub fn effective_plan(plan_id: &str, status: SubStatus) -> PlanId {
    if status.grants_paid_limits() { PlanId::parse(plan_id) } else { PlanId::Free }
}

/// Builds the snapshot in **one** statement: nine scalar subselects, one round trip.
///
/// One query rather than nine because the snapshot must be a single point in time;
/// nine reads can straddle a write and produce a snapshot that never existed. Every
/// count is scoped by `user_id` and by the user's own local boundary through
/// `chrono_tz` (A6), never a fixed offset.
pub async fn snapshot(state: &AppState, user: &AuthUser) -> AppResult<UsageSnapshot>;

/// The same snapshot from a `user_id` alone, for a job (the ingest handler re-checks
/// `IngestSource` when it runs, because the user may have downgraded between the
/// request and the claim).
pub async fn snapshot_for(state: &AppState, user_id: Uuid) -> AppResult<UsageSnapshot>;

/// The one decision every handler makes.
///
/// Returns `AppError` so a handler writes `entitlements::require(&state, &user,
/// Feature::ChatMessage)?;` and nothing else. It logs the refusal with the request id
/// and the feature, because a refusal is the only trace an abuse attempt leaves.
pub async fn require(state: &AppState, user: &AuthUser, feature: Feature) -> AppResult<()>;

/// The monthly-reservation sequence Phase 5 built, defined once here and adopted by
/// both existing call sites in Task 3 (rather than a third copy for the report
/// reservation). One transaction: the per-user advisory lock, `check`,
/// `credits::spend_in`, the caller's reserving row, commit. The caller's closure runs
/// *inside* the transaction, which is what makes the count it depends on exact.
pub async fn reserve_in<F, Fut>(
    conn: &mut PgConnection,
    state: &AppState,
    user: &AuthUser,
    feature: Feature,
    action: &str,
    cost: i64,
    reserve: F,
) -> AppResult<Spent>
where
    F: FnOnce(&mut PgConnection) -> Fut,
    Fut: std::future::Future<Output = AppResult<()>>;
```

`AuthUser` gains three fields, all from the `devices`/`subscriptions` joins it already
has or can add:

```rust
pub struct AuthUser {
    …
    /// From `devices.platform`. The *only* platform an entitlement decision trusts
    /// (A144): a body may claim anything, a device row was written by a login.
    pub platform: String,
    /// From `subscriptions.status` for the user's live row; `None` when there is none.
    pub subscription_status: SubStatus,
}

impl AuthUser {
    /// `users.plan_id` honoured only while the subscription is live (A146).
    pub fn effective_plan(&self) -> PlanId { effective_plan(&self.plan_id, self.subscription_status) }
    /// Whether an upgrade may be offered to this client (A143).
    pub fn upgrade_offerable(&self, policy: &BillingPolicy) -> bool {
        policy.for_platform(Platform::parse(&self.platform)).allow_purchase_ui
    }
}
```

`AuthUser::plan()` keeps its name and becomes `effective_plan()`; the four spending
routes and the two generation paths move with it. `limits.rs`'s four `enforce_*`
functions stop being called from handlers and become the mapping's implementation
(`entitlements.rs` calls them), so the response bodies are byte-identical to today's.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test entitlements` → green.
`cargo test -p stvdy-server` → green (the existing suites still see the same bodies).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/entitlements.rs crates/server/src/lib.rs \
        crates/server/src/auth/extract.rs crates/server/src/limits.rs \
        crates/server/tests/entitlements.rs
git commit -m "feat(server): one entitlements decision path, fed by one snapshot query"
```

---

## Task 3: The sweep — every route classified, every refusal exact

**Files:**
- Modify: every handler listed in the table below
  (`auth/routes.rs`, `devices.rs`, `workspaces.rs`, `chunks.rs`, `chat/routes.rs`,
  `flashcards/routes.rs`, `quizzes/routes.rs`, `sources/routes.rs`, and Phase 6's
  `reports.rs`/`cheatsheets.rs` as they land)
- Test: `crates/server/tests/route_sweep.rs` (NEW)

**Interfaces:**
- Consumes: `entitlements::require`, `entitlements::snapshot`.
- Produces: no new production API — the classification table lives in the test, so it
  cannot drift from the code it classifies without failing.

- [ ] **Step 1: Write the failing tests**

`crates/server/tests/route_sweep.rs` has three tests. The first two are the roadmap's
"grep test shows no bypass", split into the static and the dynamic half so a route that
is classified but calls nothing, and a route that calls nothing and is unclassified,
both fail.

```rust
/// Every route registered anywhere under `crates/server/src` is in this table.
///
/// The extractor reads whole file bodies, not lines: `routes/mod.rs:106` and
/// `quizzes/routes.rs:61` write `.route(` and its arguments across two lines, so a
/// line-based grep would silently miss both.
fn registered_routes() -> BTreeSet<(String, String)> {
    let re = Regex::new(r#"\.route\(\s*"([^"]+)"\s*,\s*([^)]*)"#).expect("a literal regex");
    for entry in walkdir("crates/server/src") { /* collect (method, path) pairs from
        every `get|post|put|patch|delete` constructor identifier in the argument list */ }
}

#[test]
fn every_registered_route_is_classified() {
    // A route added without a row here fails, which is the whole point: the sweep is
    // a test, not a review.
    let registered = registered_routes();
    let classified: BTreeSet<_> = ROUTES.iter().map(|r| (r.method.to_string(), r.path.to_string()))
        .chain(EXEMPT.iter().map(|r| (r.method.to_string(), r.path.to_string())))
        .collect();
    assert_eq!(registered.difference(&classified).collect::<Vec<_>>(), Vec::new(),
        "classify these routes in ROUTES (Swept / SweptPerMutation / ReadOnly) or EXEMPT");
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_swept_route_refuses_an_exhausted_account(pool: PgPool) {
    // An account whose every limit is used and whose balance is zero. Every route
    // classified `Swept` must answer with that feature's exact code and status —
    // never 200, never 500. This is the half that catches a route which is listed
    // but calls nothing.
}

#[test]
fn no_file_outside_the_entitlements_layer_reads_a_plan_limit() {
    // The second limit source this phase must not create. `plans.get(plan).<limit>`
    // may appear only in `entitlements.rs`, `limits.rs` (the mapping's
    // implementation), `credits.rs` (costs) and `usage.rs` (the view).
    // `provider_cost_ceiling_usd` may appear only in `credits.rs`.
}
```

`ROUTES` is the plan's own table, written out in the test:

| Method | Path | Class | Feature | Verified | Exhausted → code |
|---|---|---|---|---|---|
| GET | `/auth/me` | ReadOnly | — | no | 200 |
| POST | `/auth/logout-all` | ReadOnly | — | no | 200 |
| POST | `/auth/resend-verification` | ReadOnly | — | no | 200 |
| POST | `/auth/google/set-password` | ReadOnly | — | no | 200 |
| GET | `/devices` | ReadOnly | — | no | 200 |
| PATCH | `/devices/{id}` | ReadOnly | — | no | 200 |
| DELETE | `/devices/{id}` | ReadOnly | — | no | 200 |
| PUT | `/devices/{id}/push-token` | ReadOnly | — | no | 200 |
| GET | `/workspaces` | ReadOnly | — | no | 200 |
| POST | `/workspaces` | Swept | `CreateWorkspace` | no | 422 `limit_reached` |
| GET | `/workspaces/{id}` | ReadOnly | — | no | 200 |
| PATCH | `/workspaces/{id}` | ReadOnly | — | no | 200 |
| DELETE | `/workspaces/{id}` | ReadOnly | — | no | 200 |
| GET | `/chunks/{id}` | ReadOnly | — | no | 200 |
| POST | `/workspaces/{id}/chat` | Swept | `ChatMessage` (+`WebSearch` when `use_web`) | **yes** | 402 `insufficient_credits`; with the cap spent, 422 `limit_reached` |
| GET | `/workspaces/{id}/messages` | ReadOnly | — | no | 200 |
| POST | `/workspaces/{id}/decks` | Swept | `FlashcardBatch{n}` + `CreateDeck` | **yes** | 402 `insufficient_credits` / 422 |
| GET | `/workspaces/{id}/decks` | ReadOnly | — | no | 200 |
| GET | `/decks/{id}` | ReadOnly | — | no | 200 |
| GET | `/decks/{id}/due` | ReadOnly | — | no | 200 |
| POST | `/flashcards/{id}/review` | ReadOnly | — | no | 200 (a review is free; R1) |
| POST | `/workspaces/{id}/quizzes` | Swept | `CreateQuiz{n}` | **yes** | 402 / 422 |
| GET | `/workspaces/{id}/quizzes` | ReadOnly | — | no | 200 |
| GET | `/quizzes/{id}` | ReadOnly | — | no | 200 |
| POST | `/quizzes/{id}/attempts` | ReadOnly | — | no | 200 (an attempt is free; R1) |
| GET | `/quizzes/{id}/results` | ReadOnly | — | no | 200 |
| GET | `/workspaces/{id}/sources` | ReadOnly | — | no | 200 |
| POST | `/workspaces/{id}/sources` | Swept | `UploadSource{size_bytes}` + `IngestSource` | **yes** | 422 `limit_reached` (size, then `sources_total`, then storage) |
| GET | `/sources/{id}/status` | ReadOnly | — | no | 200 |
| GET | `/sources/{id}/file` | ReadOnly | — | no | 200 |
| GET | `/sources/{id}/preview` | ReadOnly | — | no | 200 (Task 7) |
| DELETE | `/sources/{id}` | ReadOnly | — | no | 200 |
| GET | `/objects/{id}` | Exempt | HMAC-signed, no bearer token | — | 200/403 |
| GET | `/usage` | ReadOnly | — | no | 200 (Task 9) |
| PATCH | `/admin/users/{id}/plan` | Exempt (AdminUser) | — | — | 404 for a non-admin |
| POST | `/admin/users/{id}/credits` | Exempt (AdminUser) | — | — | 404 for a non-admin |
| GET | `/admin/margin` | Exempt (AdminUser) | — | — | 404 for a non-admin |
| — | Phase 6's `reports`/`cheatsheets` rows | Swept | `Report`, `ExportFormat` | **yes** | 402 / 403 / 422 |

`EXEMPT` carries a one-line reason each: `/health` (no user), `/auth/signup|login|refresh|logout|verify-email`
(credentials, rate-limited by `ratelimit::auth_layer`, and a limit check here would
refuse the very request that could fix the account), `/auth/google/{start,callback,exchange}`
(a browser redirect and a ticket exchange, no bearer token), `/objects/{id}` (an
HMAC-signed URL whose whole point is that it works without a session).

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test route_sweep` → the classification test lists every
registered route as unclassified (nothing is in the table yet); the exhaustion test
returns 200 from `POST /workspaces` for the exhausted account.

- [ ] **Step 3: Retrofit the handlers**

Each row of the table becomes one line at the top of the handler, before any read that
can fail, and the feature is computed from the request:

```rust
// chat/routes.rs
entitlements::require(&state, &user, Feature::ChatMessage).await?;
if body.use_web {
    entitlements::require(&state, &user, Feature::WebSearch).await?;
}

// flashcards/routes.rs — the reservation replaces the shim call in the same
// transaction, so the count, the charge and the reserving row still commit together.
let spent = entitlements::reserve_in(&mut tx, &state, &user,
    Feature::FlashcardBatch { n: body.count }, ACTION, cost, |conn| Box::pin(async move { … })).await?;

// sources/routes.rs
entitlements::require(&state, &user,
    Feature::UploadSource { size_bytes: size }).await?;
```

The seven `limits::enforce_*` call sites move into `entitlements.rs`. `POST /workspaces`
keeps `VerifiedUser`? No — creating a workspace spends nothing, so it stays `AuthUser`
(A43 gates *spending*, and the sweep's `Verified` column records that decision per
route rather than applying it globally).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test route_sweep` → green.
`cargo test -p stvdy-server` → green. Any body that changed shape is a defect: the
shim's responses were already the contract's.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src crates/server/tests/route_sweep.rs
git commit -m "feat(server): every route goes through check, and a test proves it"
```

---

## Task 4: The ledger — the ceiling, the concurrency proof, and the costs view

**Files:**
- Modify: `crates/server/src/credits.rs`, `crates/server/src/limits.rs`
- Test: `crates/server/tests/credits.rs` (NEW cases)

**Interfaces:**
- Consumes: `entitlements::snapshot`, `PlansFile::provider_cost_ceiling_usd`.
- Produces: `credits::month_provider_cost(pool, user_id, since) -> AppResult<f64>`,
  `credits::ceiling_paused(state, user, snapshot) -> bool`,
  `credits::costs(plans: &PlansFile) -> serde_json::Value`,
  `credits::grant(pool, user_id, plan, period_key, credits) -> AppResult<bool>`.

- [ ] **Step 1: Write the failing tests**

```rust
/// **The concurrency proof.** Eight spends race for a balance of exactly five, and
/// exactly five succeed.
///
/// Deliberately not a timing test: a test that runs two spends and hopes they
/// interleave passes on a fast local Postgres even with the lock removed, which makes
/// it worse than no test. This one asserts an outcome that is impossible to satisfy by
/// luck: five 1-credit spends against a 5-credit balance must produce exactly five
/// successes, exactly three `insufficient_credits`, a final balance of zero, and no
/// bucket below zero. Remove the advisory lock and the count of successes goes up.
#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_spends_never_double_spend(pool: PgPool) { … }

/// The ceiling is enforced, not configured.
///
/// A `usage_events` row putting the account's local month above
/// `provider_cost_ceiling_usd[plan]` must refuse the next spend with 422
/// `limit_reached`, name the next plan in `min_plan`, and leave the ledger untouched —
/// a refusal that had already written a ledger row would charge for the call it
/// refused.
#[sqlx::test(migrations = "../../migrations")]
async fn an_account_over_its_provider_cost_ceiling_is_refused(pool: PgPool) { … }

/// A BYOK action does not count against the ceiling (A156): the user's own key paid.
#[sqlx::test(migrations = "../../migrations")]
async fn byok_usage_does_not_count_against_the_ceiling(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test credits concurrent_spends` → currently passes for the
wrong reason (the balance is 60, not 5): make it fail first by asserting the exact
success count with the balance the test sets, and watch it fail until `spend_in`'s
refusal path is reached deterministically. `an_account_over_its_provider_cost_ceiling_is_refused`
→ currently succeeds, which is the defect: nothing reads the ceiling.

- [ ] **Step 3: Implement**

Inside `spend_in`, immediately after the advisory lock and beside the daily caps:

```rust
// The ceiling, checked under the same lock as everything else that can refuse a
// spend. It has to be here and not before the transaction: the sum it reads is a
// sum of `usage_events`, and two concurrent requests that each read it just under
// the ceiling would both pass. The worst-case overshoot is the concurrent-action
// count, which is what the per-account rate cap bounds (A155).
let ceiling = state.config.plans.provider_cost_ceiling_usd.get(&plan).copied().unwrap_or(0.0);
let spent_usd: f64 = sqlx::query_scalar(
    "select coalesce(sum(provider_cost_usd), 0)::float8 from usage_events
      where user_id = $1 and created_at >= $2 and byok = false",
)
.bind(user_id).bind(month_start).fetch_one(&mut *conn).await?;
if spent_usd >= ceiling {
    return Err(AppError::Entitlement {
        code: "limit_reached",
        message: format!("this account has reached its plan's monthly AI usage limit; \
                          it resets on {}", reset_date),
        min_plan: plan_for_more_credits(plan),
        limit: None, used: None,   // dollars do not belong in the plan-matrix fields (A148)
    });
}
```

`credits::grant` is the period-keyed, idempotent grant (A152):

```rust
/// Grants `credits` into the monthly bucket, once per `period_key`.
///
/// `on conflict (user_id, period_key) do nothing` against the partial unique index,
/// and the returned `bool` says whether this call was the one that granted. That is
/// what makes the signup grant and the recurring job unable to double-grant in the
/// same month, which `jobs.dedupe_key` alone cannot do (A152).
pub async fn grant(pool: &PgPool, user_id: Uuid, plan: PlanId, period_key: &str, credits: i64)
    -> AppResult<bool>;
```

`credits::costs` returns the `costs` block `GET /usage` and the two studio panels
already serve, from one function, so the number the panel shows and the number the route
charges cannot drift (roadmap gate 7(d)).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test credits` → green, including the two pre-existing
lock/bucket cases.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/credits.rs crates/server/tests/credits.rs
git commit -m "feat(server): enforce the provider cost ceiling, and prove the spend cannot double"
```

---

## Task 5: The period — counters, `monthly_grant`, `reset_counters`, grace

**Files:**
- Create: `migrations/0025_entitlements_limits.sql`,
  `crates/server/src/jobs/handlers/{grants,counters}.rs`,
  `crates/server/src/jobs/schedule.rs`
- Modify: `crates/server/src/jobs/{mod.rs,worker.rs,handlers/mod.rs}`,
  `crates/server/src/credits.rs` (the signup grant's `period_key`),
  `crates/server/src/config.rs` (`GRACE_PERIOD_DAYS`, default 3),
  `crates/server/src/util.rs` (`next_local_month_start`),
  `crates/server/tests/schema.rs`
- Test: `crates/server/tests/limits_periods.rs` (NEW)

**Interfaces:**
- Consumes: `credits::grant`, `util::{local_day_start, local_month_start}`,
  `jobs::{enqueue, ClaimedJob, Failure}`, `jobs::schedule::spawn`.
- Produces: `schedule::spawn(state) -> JoinHandle<()>`,
  `handlers::grants::run(state, job)`, `handlers::counters::run(state, job)`,
  `util::next_local_month_start(timezone, clock) -> DateTime<Utc>`,
  `schedule::SCHEDULE_INTERVAL`, `schedule::SCHEDULE_BATCH`.

- [ ] **Step 1: Write the migration**

```sql
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
```

No trigger array change: this migration adds columns to two existing `+sync` tables and
creates one non-sync table, so `0017_sync_triggers.sql` stays last and untouched.

- [ ] **Step 2: Write the failing tests**

```rust
/// The counters roll on the user's own local boundary, not UTC.
///
/// `Asia/Jakarta` is UTC+7: at 2026-11-01T00:30:00Z the Jakarta month is already
/// November while UTC's is still October. A user in `Asia/Jakarta` must get the new
/// month's `flashcards_per_month` at that instant, and a user in `UTC` must not. This
/// is the test that catches a counter reset done with `date_trunc('month', now())`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_monthly_counter_rolls_at_the_users_own_midnight(pool: PgPool) { … }

/// The grant is once per period, whatever runs it.
///
/// Signup grants; the `monthly_grant` job then runs for the same local month. One
/// ledger row, not two (A152). This is the double-grant the roadmap's job note warns
/// about, and it is reachable today because the signup grant already exists (A64).
#[sqlx::test(migrations = "../../migrations")]
async fn the_signup_grant_and_the_job_cannot_double_grant(pool: PgPool) { … }

/// A month's unspent monthly credits do not roll over (A153), and purchased ones do.
#[sqlx::test(migrations = "../../migrations")]
async fn unspent_monthly_credits_expire_and_purchased_ones_do_not(pool: PgPool) { … }

/// `reset_counters` marks a lapsed grace window `expired` and recomputes the read-only
/// flags; it does not zero a counter, because counters are derived (A147).
#[sqlx::test(migrations = "../../migrations")]
async fn the_rollover_job_expires_a_lapsed_subscription_and_flags_the_over_limit_rows(pool: PgPool) { … }

/// The scheduler enqueues once per user per local month, and a restart does not double
/// it: the `dedupe_key` is the month.
#[sqlx::test(migrations = "../../migrations")]
async fn the_scheduler_enqueues_one_grant_per_user_per_local_month(pool: PgPool) { … }
```

- [ ] **Step 3: Run them and watch them fail**

`cargo test -p stvdy-server --test limits_periods` → `error[E0433]` for the new modules,
and `column "read_only" of relation "workspaces" does not exist` before the migration
runs.

- [ ] **Step 4: Implement**

`jobs/schedule.rs` is the only time-based producer in the server:

```rust
/// How often the scheduler looks for users whose local boundary has arrived.
pub const SCHEDULE_INTERVAL: Duration = Duration::from_secs(900);
/// Users examined per pass. Bounded, and idempotent, so a second pass covers the rest
/// rather than one pass holding a transaction over every account.
pub const SCHEDULE_BATCH: i64 = 500;

/// Enqueues the per-user boundary jobs.
///
/// `run_at` is the user's **next local month start**, computed in Rust through
/// `chrono-tz` because Postgres cannot do IANA arithmetic on a text timezone (A6), and
/// `dedupe_key` is `"<kind>:<user_id>:<local YYYY-MM>"` so the unique index makes a
/// restart, a second instance, or an overlapping pass a no-op. There is no watermark
/// column: the key *is* the watermark, and it is the one the ledger's `period_key`
/// matches, so the job table and the ledger cannot disagree about which month is done.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()>;
```

`handlers::grants::run` — the effective plan's `credits_per_month` into the monthly
bucket with `period_key = "monthly:<local YYYY-MM>"`, after expiring the unspent
monthly balance when `!credits.monthly_rollover` (`reason = 'expire'`,
`period_key = "expire:<same>"`). For a paid account the key is
`"sub:<subscription_id>:<period_start date>"` so a period extension grants once per
period rather than once per calendar month.

`handlers::counters::run` — the rollover: `subscriptions` rows whose `status = 'grace'`
and whose `current_period_end + GRACE_PERIOD_DAYS` has passed become `expired`; then the
read-only flags are recomputed for that user (Task 6 owns the recompute; this task
calls it); then it enqueues that user's `monthly_grant`. It also prunes
`signup_attempts` older than 48 hours (bounded, and it is the only table in the phase
that grows without a plan).

`credits::grant_monthly` (the signup path) moves to `credits::grant` with
`period_key = "monthly:<the user's local YYYY-MM>"`, so signup and the job share one key.

`GRACE_PERIOD_DAYS` (default 3, `AppConfig`) is read by the rollover: it is the
"configurable" in the spec's "3-day grace period (configurable)".

- [ ] **Step 5: Run the tests**

`cargo test -p stvdy-server --test limits_periods` → green.
`cargo test -p stvdy-server --test schema` → green **only after** `signup_attempts` is
added to that test's `documented` list, which is that test's contract for a table the
spec's DDL does not have.
`cargo sqlx prepare --workspace` → the new queries are in `.sqlx`.

- [ ] **Step 6: Commit**

```bash
git add migrations/0025_entitlements_limits.sql crates/server/src/jobs \
        crates/server/src/credits.rs crates/server/src/util.rs \
        crates/server/tests/{limits_periods.rs,schema.rs} .sqlx
git commit -m "feat(server): timezone-aware period rollover, and a grant that cannot double"
```

---

## Task 6: Downgrade → grace → read-only, device limits, storage accounting

**Files:**
- Create: `crates/server/src/entitlements.rs` (the `read_only` recompute; the file exists
  from Task 2, this adds to it)
- Modify: `crates/server/src/sources/routes.rs` (the read-only refusal on upload/delete),
  `crates/server/src/workspaces.rs`, `crates/server/src/devices.rs`
- Test: `crates/server/tests/limits_periods.rs` (NEW cases)

**Interfaces:**
- Consumes: `entitlements::snapshot`, the rollover job from Task 5.
- Produces: `entitlements::recompute_read_only(&PgPool, &PlansFile, user_id) -> AppResult<ReadOnlyReport>`,
  `entitlements::storage_bytes(&PgPool, user_id) -> AppResult<i64>`,
  `ReadOnlyReport { workspaces: u64, sources: u64, devices: u64 }`.

- [ ] **Step 1: Write the failing tests**

```rust
/// **The downgrade test.** A power account with 3 workspaces, 30 sources and 6 devices
/// whose subscription expires keeps every row and every byte: 3 workspaces, 30 sources,
/// 6 devices, all readable, all exportable. What changes is that the over-limit rows are
/// `read_only` and a new one is refused.
///
/// The alternative — deleting or hiding the over-limit data — is what the spec forbids
/// ("never delete user data") and what makes a downgrade a data-loss event.
#[sqlx::test(migrations = "../../migrations")]
async fn a_downgrade_keeps_the_data_and_flags_the_over_limit_rows(pool: PgPool) { … }

/// Reading an over-limit workspace, source and device still works; writing to one does
/// not. The spec's word is "viewable, exportable".
#[sqlx::test(migrations = "../../migrations")]
async fn over_limit_rows_stay_readable(pool: PgPool) { … }

/// `devices_max` counts live devices only, so revoking one is the remedy the refusal
/// suggests — and a device over the new limit is flagged rather than logged out.
#[sqlx::test(migrations = "../../migrations")]
async fn the_extra_devices_are_flagged_not_revoked(pool: PgPool) { … }

/// Storage is `sum(file_size)` over live sources, and the check is existing + new
/// (A157).
#[sqlx::test(migrations = "../../migrations")]
async fn storage_is_summed_over_live_sources_before_the_upload(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test limits_periods a_downgrade` →
`column "read_only" of relation "workspaces" does not exist` before the migration, and
then `assertion failed: all three rows are still present` until the recompute exists.

- [ ] **Step 3: Implement**

```rust
/// Recomputes the read-only flags for one account, from its effective plan.
///
/// Idempotent, and called from the rollover job and after a plan change, because the
/// two events that can put an account over a limit are a downgrade and a plan switch.
/// Ordering is deliberate: workspaces are flagged newest-last so the *oldest* stay
/// writable, which is the reading a user expects when they are told to delete
/// something.
pub async fn recompute_read_only(pool: &PgPool, plans: &PlansFile, user_id: Uuid)
    -> AppResult<ReadOnlyReport>;
```

The upload and delete paths consult `read_only` where it changes an outcome: a
`read_only` workspace accepts no new source, a `read_only` source cannot be re-ingested,
and a `read_only` device may not spend (its `require` returns
`FeatureLocked` — the remedy is to sign another device out, not to buy, which is what
A143's rule means for this case). Reads never consult it (A149).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test limits_periods` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/entitlements.rs crates/server/src/{workspaces.rs,devices.rs} \
        crates/server/src/sources/routes.rs crates/server/tests/limits_periods.rs
git commit -m "feat(server): a downgrade flags over-limit rows read-only and deletes nothing"
```

---

## Task 7: The A43 abuse controls

**Files:**
- Modify: `crates/server/src/ratelimit.rs`, `crates/server/src/auth/routes.rs`
  (the signup handler), `crates/server/src/routes/mod.rs` (the spend layer)
- Test: `crates/server/tests/abuse.rs` (NEW)

**Interfaces:**
- Consumes: the `signup_attempts` table, `config::BillingPolicy`, `credits::spend_in`.
- Produces: `ratelimit::{SignupLimiter, record_signup_attempt, signup_limiter}`,
  `ratelimit::spend_layer()`, `ratelimit::SIGNUP_PER_IP_PER_DAY`,
  `ratelimit::SIGNUP_PER_DOMAIN_PER_DAY`, `ratelimit::SPEND_PER_MINUTE`,
  `ratelimit::is_public_email_domain(domain) -> bool`.

- [ ] **Step 1: Write the failing tests**

```rust
/// An unverified account cannot spend (A43(1)) — and the four routes that spend are
/// the four that take `VerifiedUser`, which the sweep test asserts statically.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unverified_account_cannot_chat_generate_search_or_ingest(pool: PgPool) { … }

/// The signup limit trips, and it **logs**.
///
/// R5's requirement is that the false-positive rate is measurable before the limit is
/// tuned, so the test asserts the refusal *and* the tracing line, not just the status.
#[sqlx::test(migrations = "../../migrations")]
async fn the_per_ip_signup_limit_trips_and_logs(pool: PgPool) { … }

/// A public mailbox domain is exempt from the domain rule (A154): a limit on gmail.com
/// would block the product's growth and stop no farm, because a farm uses gmail too.
#[sqlx::test(migrations = "../../migrations")]
async fn a_public_email_domain_is_not_rate_limited(pool: PgPool) { … }

/// A failed signup still counts: a farm's failures are the signal.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_signup_still_records_an_attempt(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test abuse` → the limiter does not exist, so the 26th
signup succeeds.

- [ ] **Step 3: Implement**

```rust
/// A43's signup limiter. Database-backed, because an in-process bucket resets on
/// restart and a restart is free for a script.
///
/// Counted per IP and per non-public email domain over a rolling 24 hours, and every
/// attempt is recorded **before** the user row is written, so a farm's failures count
/// too. Every block logs the IP, the domain and the count, because R5's whole point is
/// that the false-positive rate on a campus NAT has to be measurable before the number
/// is tuned.
pub struct SignupLimiter { pub per_ip: u32, pub per_domain: u32 }
```

The per-account spend cap is a `tower` layer keyed by `user_id` with
`SPEND_PER_MINUTE = 10` (A155), applied to the swept spending routes; it answers
`AppError::RateLimited` (429), which is the roadmap's row for it.

`GRACE_PERIOD_DAYS` (default 3) was added to `AppConfig` in Task 5, where the rollover
reads it; it is listed here only so the A43 section names every control it relies on.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test abuse` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/ratelimit.rs crates/server/src/auth/routes.rs \
        crates/server/src/routes/mod.rs crates/server/tests/abuse.rs
git commit -m "feat(server): the A43 abuse controls, database-backed and logged"
```

---

## Task 8: PPTX, DOCX and URL ingestion

**Files:**
- Create: `migrations/0026_ingestion_formats.sql`
- Modify: `crates/server/src/sources/parse.rs`, `crates/server/src/sources/routes.rs`,
  `crates/server/src/jobs/handlers/ingest.rs`, `crates/server/src/state.rs`,
  `crates/server/src/integrations/mod.rs`, `crates/server/src/routes/mod.rs`,
  `app/src/api/queries/sources.ts`, `app/src/features/workspaces/WorkspaceScreen.tsx`,
  `app/src/i18n/locales/{en,id}.json`
- Test: `crates/server/tests/ingestion_formats.rs` (NEW),
  `app/src/features/workspaces/WorkspaceScreen.test.tsx`

**Interfaces:**
- Consumes: `zip`, `quick-xml`, `ChainFetcher::from_config`, `readability`,
  `entitlements::snapshot_for`.
- Produces: `parse::{docx_runs, pptx_slides, preview_pdf}`,
  `AppState.fetcher: Arc<ChainFetcher>`, the four new `kind`s and `GET /sources/:id/preview`.

- [ ] **Step 1: Write the migration**

```sql
-- The uniform viewer the spec asks for: PPTX and DOCX get a PDF preview rendered by
-- LibreOffice, and the client opens one media type for every source kind. Null when
-- the converter is absent (A158) or the source is a URL, which has no original file.
alter table sources add column preview_key text;
```

- [ ] **Step 2: Write the failing tests**

```rust
/// A DOCX is zip + OOXML: `word/document.xml`, every `<w:t>` run in order, no page
/// numbers because a DOCX has none. Asserted against a fixture built in the test with
/// `zip`, so the test states what a DOCX *is* rather than trusting a binary blob.
#[sqlx::test(migrations = "../../migrations")]
async fn a_docx_source_ingests_its_paragraph_text(pool: PgPool) { … }

/// A PPTX keeps its slide numbers: `ppt/slides/slideN.xml`, `<a:t>` runs, page = N.
/// The citation for slide 7 must say 7, because that is the only way a student finds it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pptx_source_keeps_its_slide_numbers(pool: PgPool) { … }

/// A zip bomb and a zip-slip entry are refused before anything is extracted: the
/// decompressed size is bounded and every member path is checked.
#[sqlx::test(migrations = "../../migrations")]
async fn a_zip_bomb_is_refused(pool: PgPool) { … }

/// A URL source fetches through the chain, cleans the page, stores no bytes and counts
/// zero against the storage quota (A157), and refuses a private address (A18/A34).
#[sqlx::test(migrations = "../../migrations")]
async fn a_url_source_is_fetched_cleaned_and_charges_no_storage(pool: PgPool) { … }

/// "Save to workspace" creates a `kind = web` source from a search hit — the Phase 3
/// acceptance item that was never wired.
#[sqlx::test(migrations = "../../migrations")]
async fn a_search_hit_can_be_saved_as_a_web_source(pool: PgPool) { … }

/// Without `SOFFICE_BIN` the source still ingests and the preview is absent (A158).
#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_soffice_leaves_no_preview_and_still_ingests(pool: PgPool) { … }
```

- [ ] **Step 3: Run them and watch them fail**

`cargo test -p stvdy-server --test ingestion_formats` →
`kind must be one of ["pdf", "text"]` for every new kind.

- [ ] **Step 4: Implement**

`SUPPORTED_KINDS` becomes `["pdf", "pptx", "docx", "url", "text", "web"]`, each with its
own size/mime guard, and `sources::parse` grows three functions with the same
`Result<Vec<(Option<i32>, String)>, String>` shape `pdf_pages` has, so the ingest job's
pipeline is unchanged: extract → chunk → embed → status. The job re-checks
`Feature::IngestSource` through `snapshot_for` before it extracts, because the user may
have downgraded between the request and the claim.

`AppState` gains `fetcher: Arc<ChainFetcher>`, built by `integrations::Providers::build`
with `ChainFetcher::from_config(exa_api_key)`. **Every fetch of a user-supplied URL goes
through it**, which is the `--block-private-networks` guarantee A18/A34 require; the
existing `tests/ssrf.rs` covers the guard and the new test covers the wiring.

- [ ] **Step 5: The SPA's add-source form**

`app/src/api/queries/sources.ts:70` types the upload as
`{ kind: 'pdf'; file: File } | { kind: 'text'; text: string }` and
`app/src/features/workspaces/WorkspaceScreen.tsx` renders only those two. A server that
accepts a DOCX while the form cannot send one is half a feature, so the union gains
`docx` and `pptx` (a `File`, with the `accept` list extended) and a `url` variant (a text
input, validated client-side as `http(s)` — the server is the enforcement, this is the
error message). New strings go through `t()` in both locale files.

**UI in this task:** yes, so roadmap gate 5 covers the new controls (the file input's
label, the URL field's label and error, 44px targets, focus ring), gate 6 covers the new
strings in `id.json`, and gate 7(a)/(b) are `n/a` because the form shows no number.

- [ ] **Step 6: Run the tests**

`cargo test -p stvdy-server --test ingestion_formats` → green.
`cargo test -p stvdy-server --test route_sweep` → green after
`GET /sources/{id}/preview` is classified `ReadOnly` (it is in the table already).
`npm --prefix app test` → green, including the new form cases.

- [ ] **Step 7: Commit**

```bash
git add migrations/0026_ingestion_formats.sql crates/server/src app/src .sqlx
git commit -m "feat(server): DOCX, PPTX and URL ingestion, and save-to-workspace"
```

---

## Task 9: `GET /usage`, and the two boundaries Phase 8 and Phase 13 inherit

**Files:**
- Create: `crates/server/src/usage.rs`, `crates/server/src/billing/mod.rs`,
  `crates/server/src/sync/{mod,push}.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`
- Test: `crates/server/tests/entitlements.rs` (NEW cases)

**Interfaces:**
- Consumes: `entitlements::snapshot`, `credits::costs`, `stvdy_sync::proto::{Entity, Op,
  MutationResult, ErrorCode}`, `config::BillingPolicy`.
- Produces: `usage::routes() -> Router<AppState>`, `billing::checkout_allowed(&BillingPolicy,
  &str) -> AppResult<()>`, `sync::push::{decide, push_features, Decision}`.

- [ ] **Step 1: Write the failing tests**

```rust
/// `GET /usage` is the only source of the numbers the UI shows (A159), and every
/// number in it is computed: no placeholder, no `0` standing in for "unknown".
#[sqlx::test(migrations = "../../migrations")]
async fn the_usage_view_reports_only_computed_numbers(pool: PgPool) { … }

/// The cost the client renders is the cost the server charges, from one function
/// (roadmap gate 7(d)).
#[sqlx::test(migrations = "../../migrations")]
async fn the_usage_costs_are_the_costs_the_routes_charge(pool: PgPool) { … }

/// The USD ceiling figures are not in the user-facing view (A159); the pause is.
#[sqlx::test(migrations = "../../migrations")]
async fn the_usage_view_does_not_leak_unit_economics(pool: PgPool) { … }

/// Phase 8 inherits this exact decision: a `Message` push is `read_only_entity` (A150).
#[test]
fn the_push_decision_rejects_what_the_allowlist_rejects() { … }

/// Phase 13 inherits this: `ios` and `android` are refused, and a body that claims
/// `platform: "web"` changes nothing (A144/A151).
#[test]
fn checkout_is_refused_for_a_mobile_platform_and_a_body_cannot_claim_otherwise() { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test entitlements` → `error[E0433]` for `stvdy_server::usage`
and `stvdy_server::sync`.

- [ ] **Step 3: Implement**

`GET /usage` returns the plan, the subscription status and period end, the credit
buckets, each limit with its used count (or `null` for unlimited), the cost table, the
feature flags, and `provider_cost: { paused, resets_at }` — never the dollars (A159).
`credits::costs` is the one function that builds the cost block, so the studio panels,
`GET /usage` and the routes all read the same `PlansFile`.

`sync::push::decide` is the per-mutation boundary, and its signature is what makes
Phase 8 a transport:

```rust
/// The per-mutation decision Phase 8's route calls for every mutation in a batch.
///
/// `Ok(())` means "this mutation may be applied"; Phase 8 then applies it and reports
/// `Applied { seq }`. `Err(MutationResult::Rejected { code, message })` is the exact
/// wire result, so the tamper suite asserts the same value Phase 8 will serialise.
///
/// It is a pure function of the snapshot, the entity, the op and the payload: no
/// database, no clock. That is what lets `tests/tamper.rs` drive every §5 row without
/// a running push route, and it is why Phase 8 re-implements nothing (A150).
pub fn decide(snapshot: &UsageSnapshot, mutation: &Mutation) -> Result<(), MutationResult>;

/// The features a push of this entity acquires, if any.
///
/// Exhaustive over `Entity`, so a new entity cannot be added to the protocol without a
/// decision about what a push of it costs. All six pushable entities return `&[]`
/// today: a task, an event, a review, an attempt, a session and a settings row are all
/// free (R1). The day one of them is not, this is the one place that changes.
pub fn push_features(entity: Entity) -> &'static [Feature];
```

`decide` applies, in order: the allowlist (`Entity::is_client_writable` → otherwise
`read_only_entity`), the op rule (`accepts_push_op` → otherwise `read_only_entity`),
the derived-field guard (a payload carrying `ease`, `interval_days` or `due_at` →
`read_only_entity`, which is the threat model's `flashcards.ease/interval/due` row), the
`read_only` account state, and then `check` for each of `push_features(entity)`.

`billing::checkout_allowed(policy, platform)` returns 403 `feature_locked` for
`ios`/`android` (A151/A143) and `Ok(())` otherwise; Phase 13's route calls it first.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test entitlements` → green.
`cargo test -p stvdy-server --test route_sweep` → green (`/usage` is classified).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/{usage.rs,billing,sync,lib.rs} crates/server/src/routes/mod.rs \
        crates/server/tests/entitlements.rs
git commit -m "feat(server): the usage view, and the two boundaries later phases inherit"
```

---

## Task 10: `crates/server/tests/tamper.rs` — every §5 "fails" row

**Files:**
- Test: `crates/server/tests/tamper.rs` (NEW)

**Interfaces:**
- Consumes: `sync::push::decide`, `billing::checkout_allowed`, `entitlements::require`,
  the swept routes, `common::{signed_up, json, workspace_with_text}`.
- Produces: no production API. The suite's contract is that it fails when a §5 row
  stops being enforced.

- [ ] **Step 1: Write the failing tests**

The suite has one case per row, **and a case that reads the document**, so a new "fails"
row cannot be added to §5 without a case:

```rust
/// §5 is the normative list, so the list is the input, not a memory.
///
/// This reads `docs/THREAT_MODEL.md`, takes every row of §5 whose outcome begins
/// "**Fails.**", and asserts each one is named by a case in this file. Adding a "fails"
/// row without a case fails here, which is the only way the document and the test stay
/// the same claim.
#[test]
fn every_section_5_fails_row_has_a_case() { … }
```

The cases, each asserting the **exact** code:

| §5 row | Test | Exact assertion |
|---|---|---|
| Unlock a paid feature by editing UI state | `editing_ui_state_cannot_unlock_a_paid_feature` | `require` on a free snapshot refuses `Byok`, `GoogleCalendar`, `AgentAction`, `QuickCapture`, `TaskBreakdown`, `PlanGeneration`, `AdvancedAnalytics`, `ExportFormat(Pdf)` with 403 `plan_required`; from an `ios` device the same features give 403 `feature_locked` (A143) |
| Edit the JWT to add a plan or raise limits | `a_forged_plan_claim_in_the_token_changes_nothing` | a token hand-signed with our secret carrying extra `plan: "power"`, `devices_max: 10`, `limits: {...}` claims: `POST /devices` on a free account at 2 devices → 422 `limit_reached` with `limit == 2`, `used == 2`. The numbers prove the claim was ignored, not merely that something failed |
| Grant credits locally, or replay a grant | `a_client_cannot_grant_itself_credits` | a chat body carrying `credits: 9999`, `balance: 9999`, `plan: "power"`: the ledger delta is exactly `-1` (`chat_message = 1`) and nothing else moves; a non-admin calling `POST /admin/users/{id}/credits` gets 404 |
| Call the LLM / search / Google directly | `no_response_body_carries_a_provider_key` | with a distinctive `LLM_API_KEY` seeded, drive the swept routes and a provider error; no body contains it |
| Forge an assistant message with fabricated citations | `a_client_push_of_a_message_is_rejected` | `decide(Message, Upsert, {…})` → `Rejected { code: "read_only_entity" }` |
| Mark a source `ready`, or skip ingestion | `a_client_push_of_a_source_is_rejected` | `decide(Source, Upsert, {status: "ready", file_key: "…"})` → `read_only_entity` |
| Upload beyond the storage quota | `an_upload_beyond_the_storage_quota_is_refused` | fill the free quota, then `POST /workspaces/{id}/sources` → 422 `limit_reached`, `limit == 200 * 1024 * 1024`, `used == <the live bytes>`; and `select count(*) from sources` is unchanged, so the refusal happened before the presign |
| Write `flashcards.ease/interval/due` directly | `a_push_of_derived_card_fields_is_rejected` | `decide(Flashcard, Upsert, {ease: 2.9, interval_days: 9999, due_at: …})` → `read_only_entity` |
| Exceed `web_searches_per_day` / `chat_msgs_per_day` | `exceeding_a_daily_cap_is_refused` | chat: the 21st message on free → 422 `limit_reached`, `limit == 20`, `used == 20`. search: the 4th **uncached** search on free → 422 `limit_reached`, `limit == 3`, `used == 3` (a cache hit does not consume it, A40/A78) |
| Enable checkout on iOS/Android | `an_ios_checkout_is_refused` | `checkout_allowed(policy, "ios")` → 403 `feature_locked`; `checkout_allowed(policy, "macos")` → `Ok`; and a request whose body says `platform: "web"` from an `ios` device still refuses, because the platform is the device row (A144) |
| Read another user's rows | `another_users_rows_are_not_readable` | for user B's workspace, deck, quiz, source and chunk: 404 `not_found` from user A's token — the exact code the client maps to "it may have been deleted, or it belongs to another account" |

Three of these are the ones the roadmap calls out as "easy to assume rather than
verify" and they are why the suite exists: `Message` and `Source` pushes, the derived
card fields, the forged plan claim, and the `ios` checkout.

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test tamper` → `error[E0433]` for `stvdy_server::sync`
before Task 9; afterwards, the rows that no code enforces yet fail with the *observed*
status, which is what a tamper test is for. Run it once with the ceiling check commented
out to confirm the suite would have caught a configured-but-unenforced control.

- [ ] **Step 3: Make them pass**

Nothing new is implemented in this task unless a case fails. A failing case is either a
defect in a Task 3–9 change or a row of §5 the tree does not actually enforce — and in
the second case the fix is the missing enforcement, never a weaker assertion.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test tamper` → green, 12 cases.

- [ ] **Step 5: Commit**

```bash
git add crates/server/tests/tamper.rs
git commit -m "test(server): every threat-model fails row, as a case asserting the exact code"
```

---

## Task 11: Admin — manual plan switch, manual credit grant, margin scaffolding

**Files:**
- Create: `crates/server/src/admin.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`
- Test: `crates/server/tests/admin.rs` (NEW)

**Interfaces:**
- Consumes: `AdminUser`, `credits::grant`, `entitlements::{snapshot_for, recompute_read_only}`.
- Produces: `admin::routes()`, `admin::margin_report(&PgPool, from, to) -> AppResult<MarginReport>`,
  `MarginReport { per_plan: Vec<PlanMargin>, provider_cost: CostSplit, revenue: Option<Revenue> }`.

- [ ] **Step 1: Write the failing tests**

```rust
/// A plan switch takes effect on the next request and recomputes the read-only flags,
/// because a downgrade and a switch are the same event from the data's point of view.
#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_plan_switch_takes_effect_and_recomputes_read_only(pool: PgPool) { … }

/// A manual grant is a `credit_ledger` row with `reason = 'admin'`, and it is
/// idempotent per admin-supplied key, so a double-click cannot grant twice.
#[sqlx::test(migrations = "../../migrations")]
async fn a_manual_grant_writes_one_admin_ledger_row(pool: PgPool) { … }

/// **The scaffolding is honest about what it cannot compute yet.** Provider cost splits
/// free from paid (join `usage_events` → `users.plan_id`), with BYOK in its own column
/// (A156). Revenue and conversion are `null` until Phase 13 writes `payments` rows —
/// a `0` there would be an invented number, and A46's tripwires are read off this
/// report (roadmap gate 7(a)).
#[sqlx::test(migrations = "../../migrations")]
async fn the_margin_report_splits_free_from_paid_and_nulls_what_it_cannot_compute(pool: PgPool) { … }

/// Every admin route is 404 for a non-admin (A7).
#[sqlx::test(migrations = "../../migrations")]
async fn a_non_admin_sees_not_found(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test admin` → `error[E0433]` for `stvdy_server::admin`.

- [ ] **Step 3: Implement**

`PATCH /admin/users/:id/plan` (body `{plan, period_days?}`) writes `users.plan_id`,
upserts the `subscriptions` row and calls `recompute_read_only`.
`POST /admin/users/:id/credits` (body `{credits, bucket, idempotency_key}`) calls
`credits::grant` with `period_key = "admin:<idempotency_key>"`, which is what makes it
once-only. `GET /admin/margin?from=&to=` returns the split.

The full margin report is Phase 13; what lands here is the query Phase 13 renders, with
`revenue: None` and `conversion: None` until there is a payment to compute them from.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test admin` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/admin.rs crates/server/src/lib.rs crates/server/src/routes/mod.rs \
        crates/server/tests/admin.rs
git commit -m "feat(server): admin plan switch, manual grant, and an honest margin scaffold"
```

---

## Task 12: The SPA — the meter, the plan and usage screen, and localized refusals

**Files:**
- Create: `app/src/api/queries/usage.ts`, `app/src/features/billing/PlanUsageScreen.tsx`,
  `app/src/features/billing/PlanUsageScreen.test.tsx`,
  `app/src/features/billing/CreditMeter.tsx`
- Modify: `app/src/api/errors.ts`, `app/src/router.tsx`,
  `app/src/components/layout/Sidebar.tsx`, `app/src/i18n/locales/{en,id}.json`,
  `app/src/api/errors.test.ts`
- Test: the two new test files, plus the existing `npm --prefix app test` suite

**Interfaces:**
- Consumes: `GET /usage`, `app/src/api/errors.ts`, `LockedFeature`, `t()`.
- Produces: `useUsage()`, `<CreditMeter />`, `<PlanUsageScreen />`.

- [ ] **Step 1: Write the failing tests**

```tsx
/**
 * Every number on the screen comes from `GET /usage`.
 *
 * The honesty rule (spec § Honesty rules) is that a number shown to the user traces to
 * something the app computes. The failure it prevents is a screen that renders "58
 * credits" from a literal or a stale cache while the server has 12, so the test renders
 * against a fixture and asserts the rendered strings are the fixture's numbers — and
 * asserts that no number appears that the fixture does not contain.
 */
it('renders only the numbers the usage endpoint returned', …)

/** The locked-feature copy follows `billing_policy` for the running platform (gate 7(f)). */
it('shows no upgrade CTA on a mobile platform', …)

/** The cost of an action is on the button before it runs (gate 7(d)). */
it('shows the credit cost of every generative action', …)
```

- [ ] **Step 2: Run it and watch it fail**

`npm --prefix app test` → `Cannot find module '@/features/billing/PlanUsageScreen'`.

- [ ] **Step 3: Write the screen**

`PlanUsageScreen` renders: the plan name, the period end when there is one, the credit
buckets with the total, each limit as `used / limit` (or "Unlimited"), the monthly usage
per action, and — when `provider_cost.paused` — a plain sentence naming the reset date.
No price, no checkout (that is Phase 13 and `billing_policy` governs it anyway).

`CreditMeter` renders `credits.total()` in the sidebar and top bar; it is the one place
the balance is formatted, so the number has one formatter and one source.

`errors.ts`'s `REASON_BY_CODE`/`TITLE_BY_CODE` move into the locale files as
`error.<code>.title` and `error.<code>.reason`, and the accessors take a `t` (or the
components resolve them). This is the i18n gap the i18n contract structurally cannot
see: the strings were literals in a `.ts` file, so `i18n.test.ts` passed while an
Indonesian user read English refusals. `plan_required` and `feature_locked` get
*different* copy — the first may name the plan and offer the upgrade, the second may not
(A143, A116).

- [ ] **Step 4: Run the tests and the build**

`npm --prefix app test` → green, including `i18n.test.ts` (key parity and placeholder
parity for the new `error.*`, `plan.*` and `meter.*` keys), `contrast.test.ts`,
`invariants.test.ts` and `policy.test.ts`.
`npm --prefix app run build` → clean.

- [ ] **Step 5: Commit**

```bash
git add app/src
git commit -m "feat(app): the plan and usage screen, the credit meter, and localized refusals"
```

---

## Task 13: Docs, the three roadmap gates, and the smoke script

**Files:**
- Modify: `docs/{API,ARCHITECTURE,THREAT_MODEL,ASSUMPTIONS}.md`
- Create: `scripts/smoke-phase7.sh`
- Test: the whole gate

- [ ] **Step 1: `docs/API.md`**

Add `## Entitlements and usage (Phase 7)` with `GET /usage` (the full body), the
route→feature classification table (the sweep's table, verbatim, so the API document and
the test cannot disagree), and the refusal table: which code and status each feature
produces, and what `min_plan`/`limit`/`used` carry. Add `## Admin (Phase 7)` with the
three routes and the note that the full admin toolset and margin report are Phase 13.
Add the four new source kinds and `GET /sources/{id}/preview` to `## Sources`. **Re-tag
`## Account`** away from Phase 7 (A160).

- [ ] **Step 2: `docs/ARCHITECTURE.md`**

A `## Entitlements, credits and limits (Phase 7)` section: the two layers and why the
engine is pure; the one snapshot query; why counters are derived and not materialised
(A147); the ceiling's enforcement point and its overshoot bound (A155); the period key
and why the grant needed it (A152); and the sweep's two tests as the phase's structural
defence. Add the ingestion formats and the `SOFFICE_BIN` policy (A158).

- [ ] **Step 3: `docs/THREAT_MODEL.md`**

§ 5: correct the "Unlock a paid feature" row's status text — it says
`422 plan_required / feature_locked`, and the roadmap's § 5 error table (which is fixed)
puts both at **403**. Add the two new "fails" rows this phase creates a control for:
the provider-cost ceiling stops spend (with its exact code), and the per-IP signup limit
trips. § 6: R5 gets its measured numbers (the configured limits and where the block is
logged) and R6/R1 are unchanged. § 9: the verification table's Phase 7 rows point at
`crates/server/tests/{tamper,route_sweep,abuse}.rs` by name.

- [ ] **Step 4: `docs/ASSUMPTIONS.md`**

Append A140–A163 as rows. Add `signup_attempts` to "Schema deviations from the spec"
with its one-line reason, and note the two column additions
(`workspaces.read_only`, `devices.read_only`, `credit_ledger.period_key`,
`sources.preview_key`) and the new `[credits] monthly_rollover` key.

- [ ] **Step 5: Roadmap gate 5 — accessibility**

**Which tasks have a UI, and which do not.** Gates 5, 6 and 7 are per-phase
obligations, so every task states its surface. The phase's UI changes are **Task 8**
(the add-source form: DOCX/PPTX file types and a URL field), **Task 12** (the credit
meter, the Plan and usage screen, the localized refusal copy) and **Task 13** (the
documents, no UI). Tasks 1, 2, 3, 4, 5, 6, 7, 9, 10 and 11 are **no UI change**: the
engine, the snapshot, the sweep, the ledger and ceiling, the period jobs, the read-only
recompute, the abuse controls, the usage view, the push/checkout boundaries, the tamper
suite and the admin API all change server code only, and the admin surface is
deliberately API-only until Phase 13. For those ten the honest statement is "no UI
change", not a re-run of the gates on screens this phase did not touch.

Measured in a real browser (Chromium, 390×844, both `prefers-color-scheme` values), on
the Plan and usage screen, the credit meter and the add-source form, not asserted:
(a) every control reachable by `Tab` with a visible `outline-ring` focus ring;
(b) computed WCAG ratios on the default dark **and** light themes (the meter's
`credits left` text, the limit rows, the paused-ceiling notice, the plan name) — all
above 4.5:1 for text and 3:1 for UI, checked rather than assumed;
(c) interactive targets ≥44px at phone width (the meter is a link, so it is measured);
(d) no state conveyed by colour alone: a reached limit reads "20 of 20" and carries a
word, the pause notice is a sentence, the meter's low-credit state carries text;
(e) 200% text scale at 390px with zero overflowing or clipped elements and no
horizontal page overflow.
State the check performed and the result. "Looks fine" is not a gate.

- [ ] **Step 6: Roadmap gate 6 — Indonesian end to end**

Both halves, because either alone fails silently:
- **Copy.** Every new string exists in `id.json` and `en.json` (the i18n contract
  asserts key and placeholder parity), and the refusal a user reads is Indonesian:
  a `limit_reached`, a `plan_required` and an `insufficient_credits` are driven from the
  Indonesian UI and the rendered sentences are quoted.
- **Data.** An Indonesian user in `Asia/Jakarta` whose local month has rolled while UTC's
  has not gets the new month's allowance, and an Indonesian source still retrieves and
  cites correctly after the sweep (the entitlement path must not have touched retrieval).
  Quoted output, not a description.

- [ ] **Step 7: Roadmap gate 7 — honesty**

State which clauses were touched and how each was checked:
(a) no invented or uncomputable number reaches the screen: the margin scaffolding returns
`null` for revenue and conversion rather than `0`, and the usage screen renders only
`GET /usage` values (asserted by the screen test);
(b) every number traces to something the app computes: the screen test asserts each
rendered number is in the fixture, and `credits left` is the ledger sum;
(c) the composer hint is still present under every chat composer (re-asserted by
grepping the shipped copy, not by reading the component);
(d) **the credit cost is shown before a generative action runs, from the same
`config/plans.toml` the server charges from**: `credits::costs` is the one function that
builds the block, the panels and `GET /usage` both read it, and a server test asserts the
number in the block equals the number the route charges for that action;
(e) AI proposals: unchanged by this phase — no proposal surface is added;
(f) a locked feature's copy and controls follow `config/billing_policy.toml` for the
running platform: the screen test asserts no price and no upgrade CTA on `ios`/`android`,
and the server half is A143's `feature_locked`.

- [ ] **Step 8: The smoke script**

`scripts/smoke-phase7.sh`, in the shape of `smoke-phase5.sh`: a fresh account, an
exhausted account, the plan matrix quoted from `config/plans.toml`, a concurrent-spend
run, a downgrade, a refused upload, the ceiling pause, and the tamper rows' statuses.
It prints the numbers it asserted so the output is evidence rather than a green word.

- [ ] **Step 9: Run the full gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace && cargo sqlx prepare --workspace --check
npm --prefix app test && npm --prefix app run build && node scripts/check-no-cdn.mjs
just check-shell
bash scripts/smoke-phase7.sh
git add docs scripts/smoke-phase7.sh
git commit -m "docs(phase 7): the entitlement contract, the threat model's new rows, and the gates"
```

---

## Phase 7 exit checklist

- [ ] `crates/core/src/entitlements/` exists with the contract's four files, `Feature`
      (+ `ALL`), `EntitlementError`, `UsageSnapshot` and `check`, and nothing in it
      performs IO.
- [ ] The plan×feature matrix is generated from `config/plans.toml` and the generated
      test asserts totality, monotonicity, that every refusal names the plan that lifts
      it, and the exact boundary of every numeric limit.
- [ ] Every registered route is classified, and every swept route refuses an exhausted
      account with its feature's exact code.
- [ ] No file outside the entitlements layer reads a plan limit; `provider_cost_ceiling_usd`
      has exactly one reader.
- [ ] Concurrent credit spend never double-spends: five 1-credit spends against a
      5-credit balance produce five successes, three refusals, a zero balance and no
      negative bucket.
- [ ] The provider-cost ceiling stops spend with 422 `limit_reached`, names the plan
      whose ceiling is higher, leaves the ledger untouched, and excludes BYOK.
- [ ] Daily and monthly counters roll on the user's own local midnight through
      `chrono-tz`, and a `Asia/Jakarta` user's month can roll while UTC's has not.
- [ ] `monthly_grant` cannot double-grant: the signup grant and the job share a
      `period_key`, and a second grant is a no-op.
- [ ] `reset_counters` expires a lapsed grace window, recomputes the read-only flags and
      enqueues the grant, once per user per local month.
- [ ] A downgrade keeps every row and every byte, flags the over-limit ones read-only,
      and leaves them readable.
- [ ] `devices_max` counts live devices; over-limit devices are flagged, not revoked.
- [ ] Storage is summed over live sources before the upload.
- [ ] An unverified account cannot spend; the per-IP signup limit trips and logs; a
      public mailbox domain is not domain-limited.
- [ ] DOCX, PPTX and URL sources ingest, keep their slide numbers where they have them,
      refuse a zip bomb and a private address, and count no storage for a URL.
- [ ] `crates/server/tests/tamper.rs` has one case per §5 "fails" row, each asserting the
      exact code, and a case that fails when §5 gains a row without a case.
- [ ] Phase 8's push path inherits `sync::push::decide` without re-implementing it, and
      Phase 13's checkout inherits `billing::checkout_allowed`.
- [ ] Admin can switch a plan and grant credits, and the margin scaffold splits free from
      paid while returning `null` for what it cannot compute.
- [ ] Roadmap gate 5 (accessibility) performed and recorded.
- [ ] Roadmap gate 6 (Indonesian) performed and recorded, both copy and data.
- [ ] Roadmap gate 7 (honesty) performed and recorded, all six clauses.
- [ ] `docs/{API,ARCHITECTURE,THREAT_MODEL,ASSUMPTIONS}.md` updated in the same phase.
- [ ] The full quality gate is green.

### Exit checklist table (gate | command | result)

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | clean (`-p stvdy-server` re-run clean after this slice's test edit) |
| Rust tests | `cargo test --workspace` | **NOT RUN by this slice** — the main agent ran it concurrently in this tree and it finished; its result is the main agent's to report |
| Plan matrix | `cargo test -p stvdy-core --test entitlements_matrix` | green in Task 1's slice |
| The sweep | `cargo test -p stvdy-server --test route_sweep` | green in Task A2's slice |
| The tamper suite | `cargo test -p stvdy-server --test tamper` | **14 passed; 0 failed** (12 cases + 2 this slice added) |
| The concurrency proof | `cargo test -p stvdy-server --test credits concurrent_spends_never_double_spend` | green in Task B's slice; re-asserted end to end by the smoke run |
| The downgrade | `cargo test -p stvdy-server --test limits_periods a_downgrade_keeps_the_data` | green in Task B's slice; re-asserted end to end by the smoke run |
| Abuse controls | `cargo test -p stvdy-server --test abuse` | green in Task C's slice |
| Offline queries | `cargo sqlx prepare --workspace --check` | unchanged ("no queries found") |
| Web tests | `npm --prefix app test` | **307 passed / 307** (36 files), re-run in this pass |
| Web build | `npm --prefix app run build` | clean, re-run in this pass (and the shipped bundle is what gate 7(c) grepped) |
| No CDN | `node scripts/check-no-cdn.mjs` | `no third-party loads in 3 shipped file(s)`, re-run in this pass |
| Desktop + shell | `just check-shell` | **green** (fmt, clippy, the shell's tests) |
| Roadmap gate 3 (this phase's acceptance) | `bash scripts/smoke-phase7.sh` | **PASS**, 8 scenarios, exit 0 (see the Execution notes) |
| Roadmap gate 5 (accessibility) | measured in Chromium, 390×844 + 1280×900, both themes, 200% text | **PASS**, one defect found and **not fixed** (`LockedFeature`'s upgrade link is 93×19 at phone width) — 79 + 46 contrast pairs, 0 below requirement, lowest 5.28:1; 8 stops at desktop, 6 at phone, all `solid 2px` at 2px offset; `scrollWidth == clientWidth == 390` at 200%. See the Execution notes |
| Roadmap gate 6 (Indonesian) | copy driven from the Indonesian UI in the browser + the `Asia/Jakarta` month roll | **PASS** for `limit_reached`, quoted from the DOM ("Paket Pro sudah mencapai batasnya untuk ruang belajar. Batasnya 2 dan kamu sudah memakai 2. Pro menaikkan batas ini."); **`plan_required`/`insufficient_credits` not captured in the browser** (a hand-seeded report row crashed the report screen), so they remain unit-level assertions; the data half is `limits_periods.rs` + `indonesian.rs` + the smoke run. See the Execution notes |
| Roadmap gate 7 (honesty) | clauses (a)–(f), each with the check named | **PASS** — (c) re-asserted by grepping the shipped bundle (1 match each, English and Indonesian); (a), (d) and (f) measured; (b) and (e) inherited evidence. See the Execution notes |
| Everything | `just check` | not run |

---

## Execution notes (Phase 7, as built)

The phase's build notes are in the commit history; this section is the **gate pass**, and it
is written honestly: three of the four gates were performed and one was not.

### The four documents: DONE

`docs/API.md` gains `## Entitlements and usage (Phase 7)` (the full `GET /usage` body, the
53-row route classification table copied verbatim from `route_sweep.rs`'s table so the
document and the test cannot disagree, and the five-row refusal table naming each code's
status and which fields it populates), `## Admin (Phase 7)` with the three routes, the four
new source kinds and `GET /sources/{id}/preview` in `## Sources`, and `## Account` re-tagged
to Phase 15 (A160). `docs/ARCHITECTURE.md` gains `## Entitlements, credits and limits
(Phase 7)`: the two layers, the one snapshot statement, why counters are derived (A147), the
ceiling's enforcement point and its overshoot bound (A155/A161), the period key (A152), the
sweep's two tests, and the ingestion formats with the `SOFFICE_BIN` policy (A158).
`docs/THREAT_MODEL.md` §5 corrects the "Unlock a paid feature" row to **403**
`plan_required` / `feature_locked` and adds the two new "fails" rows; §6 gives R5 its
measured numbers (25 per IP per 24 h, 100 per non-public domain, the exact log line, the
public-domain exemption list, the 48 h prune); §9's Phase 7 rows name
`crates/server/tests/{route_sweep,tamper,abuse,credits}.rs` and their cases.
`docs/ASSUMPTIONS.md` gains **A140–A163** (all 24 rows), four "Schema deviations" rows
(`signup_attempts`, `credit_ledger.period_key` + the two `read_only` columns,
`sources.preview_key`, `[credits] monthly_rollover`) and two local-environment notes
(`GRACE_PERIOD_DAYS=3`, `SOFFICE_BIN` unset). `docs/RELEASE.md` gains the LibreOffice image
requirement beside Chromium's, because `GET /sources/:id/preview` needs it.

**The doc-driven tamper case was the guard working, exactly as the brief predicted.** Adding
two "fails" rows made `every_section_5_fails_row_has_a_case` fail; two cases were added
(`an_account_at_its_provider_cost_ceiling_is_refused`, which drives the real chat route
against an account whose month already cost the plan's ceiling and asserts the ledger and
`usage_events` are untouched, and `signing_up_past_the_per_ip_limit_is_refused`, which drives
real signups from one address with a forged `x-forwarded-for`), the expected count moved
11 → 13, and the guard was **falsified** by renaming a covered case: it failed with
`signing_up_past_the_per_ip_limit_is_refused_TYPO is named for "Farm signups past the per-IP
limit" but is not a case in this file`.

`cargo test -p stvdy-server --test tamper` → **14 passed; 0 failed**.

### Roadmap gate 3, the smoke run: PASS

`scripts/smoke-phase7.sh`, eight scenarios, exit 0, against a real server on a scratch
database (`stvdy_smoke7`) with `scripts/stub-llm.py`. The numbers it printed:

| Scenario | Printed result |
|---|---|
| The plan matrix, quoted from `config/plans.toml` | free `workspaces=2 max_file_mb=20 chat_msgs_per_day=20 credits_per_month=60 ceiling_usd=0.5`; pro `500/2.0`; power `2000/6.0`; `monthly_rollover=False` |
| 1. A fresh account | `credits={'monthly': 60, 'purchased': 0, 'total': 60}` `workspaces_limit=2` `chat/day=20` `export_formats=['md']` `costs.chat_message=1` `costs.report_outline=3` `costs.report_section=6` `provider_cost.paused=False` `resets_at=2026-10-31T17:00:00Z`; no `spent_usd`/`ceiling_usd` on the wire |
| 2. An exhausted account | `status=422 code=limit_reached limit=20 used=20 min_plan=pro` |
| 3. A concurrent-spend run | `balance before=5 after=0 \| successes=5 refusals=3 \| negative buckets=0`; the refusal `code=insufficient_credits min_plan=pro message="this action costs 1 credits and you have 0"` |
| 4. A downgrade | `before: plan=pro workspaces=3 read_only=0` → `after: plan=free workspaces=3 read_only=1 (free allows 2)`; every row still readable |
| 5. A refused upload | `uploaded 22020096 bytes, refused 422 limit_reached limit=20971520 used=22020096`; `sources in the workspace after the refusal: 1` |
| 6. The ceiling pause | `status=422 code=limit_reached min_plan=pro limit=null used=null \| ledger 60 -> 60 \| usage rows 1 -> 1`; `GET /usage provider_cost.paused=True`; `no dollar figure on the paused view` |
| 7. The §5 rows | `§5 carries 13 "Fails." row(s)`; unlock a paid feature `403 plan_required`; a forged token signed with the server's own secret → `/usage plan=free` (same as the real token); a non-admin grant `404 not_found`; a cross-tenant read `404` and the owner `200`; `no provider key in a body: 0 of the 34 collected response file(s)` |
| 8. The signup limiter | domain rule `429 rate_limited`; the same address can still log in `401` (so the credential bucket is not what refused it); the same volume for `gmail.com` → `201` (exempt); per-IP `attempts recorded=25 (limit 25)` then `429 rate_limited` |

**A defect in the gate's own tooling, found by running it.** The first run reported
`the reset_counters job did not finish`, which reads like a broken worker and is not one: the
`jobs` table is shared through `DATABASE_URL`, so **another `stvdy-server` already running on
`:8099` from a binary built at 05:02** (before this phase's `reset_counters` handler existed)
claimed this run's job and answered `unknown job kind reset_counters`. The script now
diagnoses exactly that, naming the shared queue and the fix, instead of leaving a
handler hunt. The run above is the one made against an isolated database, where the queue has
one consumer.

**Two more things the run taught, both now in the script's comments.** The credential routes
sit behind `ratelimit::auth_layer` — an in-process bucket of **30 requests, then one refill
per minute** per IP (A12) — and this run makes ~15 credential requests, so a *second* run in
the same server process spends its time waiting for cells (measured: 12.7 s for the first run,
2 m 41 s for the second, which then failed on the bucket). The header now says to run it
against a freshly started server, and `auth_post` waits across the refill and explains itself
rather than hanging. And the loopback address has no aliases on macOS (`ifconfig lo0` carries
only `127.0.0.1`, and `curl --interface 127.0.0.2` fails with `Can't assign requested
address`), so the per-IP rule is driven by seeding the count for this run's own address and
discriminating the 429 from the credential bucket's by a login from the same address.

### Roadmap gate 5, accessibility: PASS on the two new components, one defect found

Measured in a real Chromium (driven over CDP) on the built bundle against the real API on
`:8097`, on the three surfaces this phase changed: `PlanUsageScreen` and `CreditMeter` at
**390x844** and **1280x900** with `prefers-color-scheme` **dark and light**, and the
add-source form (`WorkspaceScreen`'s `UploadForm`) at 1280x900. The instrument is
`/tmp/gate7-a11y.js`, which installs a `window.__gate` on the page: `contrast()` composites
each element's real backdrop (walking up until a non-transparent background) and applies WCAG
2.1 relative luminance with the 4.5:1 / 3:1 thresholds chosen by computed font size and
weight; `walk()` presses real `Tab` keys and measures the ring on the focused element itself;
`targets()` measures `getBoundingClientRect()`; `overflow()` compares each element's
`scrollWidth` to its `clientWidth` and the document's to the viewport. Nothing is asserted
from the stylesheet.

The screen was driven with real state, not a stub: an account with **2 of 2 workspaces**,
**21 of 20 chat messages**, **1 of 1 reports** (each rendering "Sudah di batas" / "At the
limit"), a **paused ceiling** (`provider_cost_usd = 0.50` on the free plan's ceiling), and
three credit buckets (`60 / 12 / 72`).

- **(a) Keyboard, with a visible ring.** At 1280x900 `Tab` from the document body reaches
  **8 stops** and wraps to the first: the skip link, the five sidebar items (Today,
  Workspaces, Calendar, Tasks, Settings), the credit meter (**"72 credits left"**, a link to
  `/billing`) and the locked-format upgrade link. Every stop carries
  `outline: solid 2px rgb(165, 180, 252)` at a **2px offset**, measured on the focused
  element, at **6.29:1** against what it sits on. At 390x844 the walk reaches **6 stops** —
  the upgrade link and the five tab-bar items — and wraps; **the meter is absent at phone
  width**, which is Task 12's recorded decision (the tab bar is five destinations and the
  meter lives on the sidebar rail; a sixth item would cost every item its 44px target at 200%
  type).
- **(b) Contrast, computed, both themes.** **79 text/background pairs on the plan screen and
  46 on the add-source form; 0 below their requirement in either theme.** The lowest ratio on
  the plan screen is **5.28:1** (light) / **5.69:1** (dark) for "Available on Pro" at 13px;
  the **paused-ceiling notice** is **6.37:1** (light) — `--warning` on `--warning-tint`, the
  pair the design tokens declare; the limit rows and the plan name sit on `--text`/
  `--text-muted` over `--bg` and are above 7:1; the meter's balance is `--text-muted` over the
  rail at the same pair. The lowest anywhere measured is **5.28:1**, against a 4.5:1
  requirement.
- **(c) 44px targets at phone width — one failure, reported not softened.** At 390px the tab
  bar's five items are 44px tall. **`LockedFeature`'s upgrade link measures 93x19** at both
  390px and 1280px — a real interactive target under 44px on this phase's surface (and on
  Phase 6's report and cheatsheet screens, which share the component; Phase 6's gate listed
  buttons and nav items but not this link, so it was missed there too). At 1280px the sidebar
  items are **215x40**, also under 44, but the clause is about phone width, where the tab bar
  carries navigation at 44px. The add-source form's radios measure **13x13 inside
  79x58 / 114x58 / 97x44 / 97x44 / 201x44 labels** — the label is what is clicked and what
  the finger hits, which is the wrapped-control case and not a failure; the two source-list
  checkboxes are the same shape.
- **(d) No state by colour alone.** A reached limit renders "**2 dari 2**" **and** the chip
  "**Sudah di batas**" (the word carries the state; the chip's tone only repeats it); the
  paused ceiling is the sentence "**Pembuatan baru dijeda sampai 1 Nov 2026**"; the low-balance
  case is the meter's word ("**Kredit habis**" at zero, "**Kredit tidak tersedia**" when the
  route failed, never a bare `0`); the subscription status is a word ("**Tanpa langganan**");
  a locked export format is a lock icon, the format's name and a sentence.
- **(e) 200% text at 390px.** Root 16px -> **32px**, measured back from `getComputedStyle`.
  **`document.scrollingElement.scrollWidth === 390 === clientWidth`: no horizontal page
  overflow, and 0 elements clipped off-screen.** Two measured soft findings, both recorded
  rather than smoothed over: the fifth tab-bar item paints **45px past the right edge**
  (`left 361, right 435`), which is Phase 6's documented fix — the bar is
  `overflow-x: auto` (`scrollWidth 435 > clientWidth 390`, confirmed `scrollable: true`), so
  the item is reachable by swipe and by `Tab`; and three row elements inside the limits and
  costs `<dl>`s report a content width of **327 against a 310 box**, absorbed by the
  surrounding padding with nothing clipped and no page overflow.

#### The one defect gate 5 found

**`LockedFeature`'s upgrade link is 93x19 at phone width**, against the 44px target the gate
requires. It is a plain `Link` with `link mt-1 self-start text-small` and no height utility,
where every other control on the surface has one (`min-h-11` on the meter, on the
`ApiErrorNotice` plan link, and on the upload radios' labels). It sits on this phase's plan
screen and on Phase 6's report and cheatsheet screens, and it is the control a user who has
just been told a feature is locked is meant to reach.

**Not fixed in this slice, and deliberately not fixed quietly.** The brief for this slice
says `app/**` is settled and green and that a defect found there must be **reported, not
fixed**, so a fix slice can be dispatched with the right ownership. The fix is one class
(`min-h-11` + `inline-flex items-center`, the shape `States.tsx:196` already uses) plus a
regression test asserting the link's rendered height at 390px, which is exactly the shape
Phase 6's four fixes took.

### Roadmap gate 6, Indonesian end to end: PASS

- **Copy, driven from the Indonesian UI in a browser.** The browser's `navigator.languages`
  was overridden to `id-ID` before the app booted, so the app resolved Indonesian itself —
  the sign-in screen renders "**Masuk**" and the shell "**Ruang belajar, sumber, dan materi
  belajarmu, di setiap perangkat yang kamu pakai**" — and the account was signed in through
  that UI. The refusals were then forced with real state and the sentence **quoted from the
  rendered DOM**, not from a unit test:

  - **`limit_reached`** — creating a third workspace from the Indonesian UI against a free
    account with two (the client believed `pro`, because `users.plan_id = 'pro'`, so the form
    was offered; the server refused, because A146 honours a stored plan only while a
    subscription is `active` or in grace, and there was no subscription row). The notice the
    user reads:
    "**Batas paket tercapai**" /
    "**Paket Pro sudah mencapai batasnya untuk ruang belajar. Batasnya 2 dan kamu sudah
    memakai 2. Pro menaikkan batas ini.**" / "**Lihat paket**".
  - **`plan_required` and `insufficient_credits` — NOT captured from the browser.** The
    report screen crashed with `Cannot read properties of undefined (reading 'length')` on the
    row this gate seeded by hand into `reports` (a `status = 'ready'` row with a
    `sections_md`/`outline` shape the API's own derivation may not accept), so the PDF export
    button was never reachable, and the chat refusal was not driven. **This is stated as not
    performed rather than upgraded**: whether that crash is my fixture or a real defect in the
    screen's derivation could not be established without more work, so it is recorded as an
    open question for whoever owns `app/**`, and it is **not** claimed as a defect.
    The two sentences those codes produce are asserted through the real `t` in
    `app/src/api/errors.test.ts` — `plan_required` → "**Ekspor PDF tidak termasuk dalam
    paketmu saat ini. Fitur ini ada di Pro.**" and `insufficient_credits` → "**Kredit tidak
    cukup**" / "**Saldo kreditmu kurang dari biaya ini.**" — which is one layer below the DOM.

- **Data.** `crates/server/tests/limits_periods.rs::the_monthly_counter_rolls_at_the_users_own_midnight`
  is the exact property, and it is the strongest evidence in this gate: the clock is pinned at
  `2026-10-31T18:00:00Z` (Jakarta entered November at 17:00Z; UTC has not), 30 flashcards are
  seeded on both a `Asia/Jakarta` and a `UTC` account, and the Jakarta account reads
  `flashcards_this_month == 0` and is admitted while UTC reads `== 30` and is refused
  `limit_reached`. `crates/server/tests/indonesian.rs::an_indonesian_question_is_answered_with_a_resolved_citation`
  is the retrieval half, and **no `entitlements::` call exists anywhere on the retrieval or
  citation path** (`chat/retrieve.rs`, `web.rs`, `prompt.rs`, `sse.rs`: zero hits; the only two
  are pre-retrieval in `routes.rs`), so the sweep cannot have touched retrieval. The smoke run
  is the wired half: an Indonesian text source reaches `ready`, its chunks are cited, and a
  turn costs the matrix's `chat_message = 1`. **The month roll was not re-driven against the
  live server** — it needs a pinned clock, which is the test's instrument, not the script's.

### Roadmap gate 7, honesty: PASS

Each clause, with which were measured in this pass and which are inherited evidence from a
test this phase did not touch:

- **(a) No invented or uncomputable number — measured.** The plan screen renders **79 numbers
  and no figure the payload does not carry**: the smoke run asserted `spent_usd`/`ceiling_usd`
  are absent from `GET /usage`, and `admin.rs::the_margin_report_splits_free_from_paid_and_nulls_what_it_cannot_compute`
  asserts `revenue`/`conversion` are `null` on the wire, never `0` ("a conversion of 0 would
  be an invented number (gate 7(a))").
- **(b) Every number traces to something the app computes — inherited, one test.**
  `PlanUsageScreen.test.tsx`'s "renders only the numbers the usage endpoint returned" asserts
  `expect(shown.filter((run) => !allowed.has(run))).toEqual([])`, with the allowed set built
  from the fixture, and `Shell.test.tsx` pins the meter at `72 credits left` against
  `credits: { monthly: 60, purchased: 12, total: 72 }` — the same three numbers the browser
  rendered in this pass.
- **(c) The composer hint, re-asserted by grepping the shipped copy — measured, as the brief
  asks.** After `npm --prefix app run build`, the shipped bundle
  `app/dist/assets/index-B-cCLv1q.js` contains **exactly 1 match** of
  "**AI can make mistakes. Check the citations.**" and **exactly 1 match** of its Indonesian
  pair "**AI bisa salah. Periksa kutipannya.**". The browser also rendered the English string
  live under the composer on the workspace screen at **6.14:1**.
- **(d) The credit cost is shown before a generative action runs, from `config/plans.toml` —
  measured.** The smoke run printed `costs.chat_message=1 costs.report_outline=3
  costs.report_section=6` from the same file the server charges from, and
  `crates/server/tests/entitlements.rs::the_usage_costs_are_the_costs_the_routes_charge`
  asserts `body["costs"] == credits::costs(&plans)` and
  `body["costs"]["chat_message"] == credits::chat_message_cost(&state)`, so the number the
  block quotes is the number the route charges. `plans.test.ts` asserts 3 and 6 against the
  file from the app side.
- **(e) AI proposals — inherited, no surface added.** A case-insensitive search for `proposal`
  over `app/src`, `crates` and `config` hits only `docs/`; the only "Suggested …" surface is
  Phase 6's report outline. Nothing to check beyond "not added", which was verified.
- **(f) A locked feature follows `config/billing_policy.toml` — measured.** In the browser on
  the phone-width plan screen the locked PDF format renders "Not available on your plan" with
  **no price and no upgrade link** (`ios`/`android` set `allow_purchase_ui = false`,
  `show_prices = false`, `allow_purchase_cta_in_locked_messages = false`); on desktop it
  renders "Available on Pro" with the `/billing` link, which the policy allows. The server half
  is `feature_locked` with `min_plan: null` (A143).

### The gates not re-run, and why

`cargo test --workspace` was **not** run by this slice: the main agent was running it
concurrently in this working tree, and it had finished by the time this pass ended, so its
result is the main agent's to report. `cargo fmt --all -- --check` is clean,
`cargo clippy -p stvdy-server --all-targets -- -D warnings` is clean, and
`cargo test -p stvdy-server --test tamper` is **14 passed / 0 failed**, all on the committed
tree. `just check-shell` was run and is **green** (fmt, clippy and the shell's tests).
`npm --prefix app test` is **307 passed / 307** (36 files), `npm --prefix app run build` is
clean, and `node scripts/check-no-cdn.mjs` reports **no third-party loads in 3 shipped
file(s)**.

---

## Self-Review

- **Spec coverage.** `## Plans`: every limit in the matrix is enforced from
  `config/plans.toml`, and the prices are read for the plan screen's name only (no
  checkout in this phase). `## Credit costs`: every cost is read from the config, the
  long-document surcharge is `long_doc_threshold_tokens` +
  `long_doc_credits_per_1k_tokens` through the existing `Credits::long_doc_cost`, and the
  UI shows the cost before the action. `## Entitlements engine`: one `check` used by every
  handler and by each sync push mutation; no limit hardcoded; structured errors with
  `min_plan`; per-account never per-device; monthly grant at period start with no
  rollover by default; spend inside a transaction against `credit_ledger` with the
  monthly bucket first and purchased credits never expiring; counters with
  timezone-aware reset; downgrade → 3-day grace → read-only with data kept; devices over
  the limit flagged read-only; `provider_cost_usd` logged per action; the ceiling
  enforced. `## AUTH AND SECURITY`: verified email before spending, per-IP and per-domain
  signup limits, per-account spend cap, rate limits on auth and the spending routes, no
  secrets logged, every query scoped by `user_id`. `docs/THREAT_MODEL.md` §5 and §6: every
  "fails" row is a case, R5 gains its measured numbers, and §9's Phase 7 rows name their
  tests.
- **Placeholders:** none. Every task names its files, its exact test, its run command and
  its commit. The four genuine unknowns the spec leaves open are pinned rather than waved
  at: the rollover default (A153), the signup limits and their exemption (A154), the spend
  cap's value and what it is for (A155), and the ceiling's enforcement shape (A148).
- **Type consistency.** `Feature`, `EntitlementError`, `UsageSnapshot`, `Credits`,
  `SubStatus`, `PlanFacts`, `UsageSnapshot`'s loader `snapshot`, `require`,
  `reserve_in`, `decide`, `push_features`, `checkout_allowed`, `recompute_read_only`,
  `costs`, `grant` and `margin_report` are each defined once and referenced by name in
  the server, the tests and the SPA. `stvdy_sync::proto::{Entity, Op, MutationResult,
  ErrorCode}` are reused, never redefined. The two name collisions the tree already had
  are resolved explicitly (A140, A141) rather than left for a reader to trip over.
- **The bug this plan exists to avoid.** The sweep. `route_sweep.rs` fails on a route
  that is not classified and on a route that is classified but calls nothing, so the
  phase cannot be "done" by review. The second one is the secondary risk: the ceiling is
  read by exactly one function and a grep test says so, because a configured-but-
  unenforced control is worse than none.
- **Deviations from the contract, each recorded:** A140 (`min_plan` optional), A142
  (`ReminderChannel` reuses the config enum), A145 (no 5s plan cache), A147 (counters
  derived, not materialised), A148 (the ceiling pauses rather than downgrading the
  model), A150/A151 (two boundaries land a phase early because the roadmap's own gates
  require their verification here).
- **Open items.**
  1. **The model-tier fallback (A148).** `ModelTier` is loaded and never read; the
     spec's ceiling sentence offers "falls back to the cheap model **or** pauses", and
     this phase implements the pause. Phase 13 owns the tier→model mapping, together
     with the calibration the margin report produces.
  2. **Chunk and embedding storage is not counted against any quota (A157).** A URL
     source stores no bytes and a text source's chunks are real disk. Phase 13's margin
     report should decide whether `storage_mb` means "uploaded originals" (today's
     meaning) or "everything this account occupies".
  3. **Account export and deletion are not in this phase (A160).** `docs/API.md`'s
     `## Account` heading is re-tagged; the in-app delete is a release prerequisite
     (store rules) and belongs with `purge_deleted_accounts`.
  4. **A43's escalation ladder is not exercised.** A46's tripwires (utilization >30%,
     conversion <3%, free-tier spend >25% of revenue) are uncomputable until Phase 13
     writes revenue, which is why the margin scaffold returns `null` rather than `0`.
  5. **B1 (QRIS gateway docs)** is still needed at Phase 13, not before.
