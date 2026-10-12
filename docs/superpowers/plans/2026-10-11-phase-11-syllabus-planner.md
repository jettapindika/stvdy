# Phase 11. Syllabus + planner. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`
> (**§ 6 Deterministic planner**, which is normative and written out in full, § 3
> Entitlements, § 7 Prompts, § 8 Job kinds) and in `docs/SPEC.md` (§ Syllabus import,
> § Planner, § Honesty rules, § Plans, § Credit costs). This plan implements them; it
> never redefines them. Where this plan and the roadmap disagree, the roadmap wins and
> this file is the bug. Every place this plan *does* deviate from a contract is listed
> in "Decisions this plan pins" with a number and a one-line justification.

**Goal:** a student uploads a syllabus, reads what the model extracted **with the exact
source line it came from**, fixes or drops the items they disagree with, and confirms —
and only then do events, tasks and goals exist. Then the student asks for a plan and gets
a proposal that provably fits their week: it never overlaps a class, never exceeds the
daily maximum, never leaves the study hours, and always leaves the buffer before the
exam. The proposal is *suggested*, previewed, draggable, and accepted; replanning keeps
the blocks the student already committed to.

**Architecture:** two layers, the same split the entitlements engine uses.

- `crates/core/src/planner/` is **pure**: `PlannerInput` in, `PlanProposal` out, no
  database, no clock, no IO, no network. `now` is a field of the input, not a call. That
  is what makes the whole algorithm unit-testable against a fixed instant, and it is what
  contract § 6 means by "same input ⇒ same output".
- `crates/server/src/planner/` is the only thing that *builds* a `PlannerInput` — it
  reads the goal, the busy calendar, the due cards and the quiz weaknesses — and the only
  thing that turns a `PlanProposal` into rows. The effort estimate is the one model call,
  and it estimates **minutes only**: `prompts/planner_effort.v1.md` says in its own text
  that the scheduler is deterministic code that runs after it, and the module returns
  nothing but numbers.

The same split for the syllabus: `crates/core/src/generation/syllabus.rs` parses the
model's JSON and maps it to review items (pure, `deny_unknown_fields`, bounded); the
server uploads, extracts through `ingest_source`, and writes on confirm.

**Tech Stack:** Rust (core + server), Postgres/sqlx, `chrono-tz`, React + Vite +
TypeScript + Tailwind, TanStack Query. **No new crate dependency.** `chrono-tz` is
already used for the counters and the local-day helpers, `uuid` for ids, `serde_json` for
the JSONB columns. A dependency added for arithmetic the contract already spells out
would be a second convention for the same thing.

**Exit gate (observable, from the roadmap):** syllabus → review screen →
events/tasks/goals; and the proposal respects busy time, the daily maximum, study hours,
and the buffer.

---

## This phase's real risk, and the defence

> **The planner is the most-tested pure function in the product and the easiest thing in
> the product to make non-deterministic.** Every one of the three ways to do it is a small,
> plausible line: a `HashMap` whose iteration order reaches the output, a `Utc::now()` read
> inside the algorithm, an unstable sort over a partial order. None of them fails a test
> that only checks "the blocks look right on Tuesday", and all of them are invisible in
> review — the code reads fine. And the failure is not cosmetic: the same goal planned
> twice gives a different week, so a student who re-opens the planner sees their study
> blocks move on their own, and a replan cannot be diffed against what it replaced.
>
> **The defence is three tests, not a review.** (1)
> `crates/core/tests/planner.rs::the_same_input_produces_byte_identical_output` builds the
> same logical input **through two different construction paths** — one feeding the
> accuracy map from a `HashMap`, one from a `BTreeMap` — and asserts the two
> `serde_json::to_vec` results are byte-for-byte equal. A `HashMap` whose order leaks into
> the output cannot pass that, because the two paths insert in different orders. (2)
> `crates/core/tests/planner_purity.rs` reads every file under
> `crates/core/src/planner/` with `include_str!` and fails on `Utc::now`, `Local::now`,
> `SystemTime`, `HashMap`, `HashSet`, `sort_unstable` or `partial_cmp` — the module uses
> `BTreeMap`/`BTreeSet` and total orders only, and the test says so mechanically rather
> than in a comment. (3) `replan_keeps_pinned_blocks_exactly` pins the property that makes
> replanning trustworthy at all.
>
> **The secondary risk, with its own defence: the review screen is where a wrong
> extraction gets written without the student noticing.** The model returns a `confidence`
> per item, and the honesty rules forbid showing a confidence score — so a screen that
> rendered it as a percentage would be both a copy violation and a false assurance. The
> decision (A261) is that the number is used to **order** the list and to flag "needs a
> check", and the screen shows the **evidence quote** instead: the exact source line the
> item was read from. The mechanical defence is
> `crates/server/tests/syllabus.rs::nothing_is_written_until_the_student_confirms`, which
> asserts that after an upload the `events`, `tasks` and `goals` tables are **empty** for
> that workspace, and that after a confirm only the kept indices produced rows.
>
> **The third risk: the DST day is where local-date arithmetic silently skips or
> duplicates a block.** `chrono_tz`'s `from_local_datetime` returns a `LocalResult` with
> three shapes, and `Single(..)` is the one everybody writes. Two tests use
> `Europe/London` with a study window that crosses the transition — 2026-03-29 (the 01:00
> hour does not exist) and 2026-10-25 (the 01:00 hour happens twice) — and assert the
> capacity in minutes for that day, which is 23 hours' and 25 hours' worth of window
> respectively. `Asia/Jakarta` has no DST, so a test that used it would prove nothing;
> gate 6 uses Jakarta for the *end-to-end* flow, and these two use London for the
> arithmetic.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes. Prompts
  live in versioned modules, never inline. `tracing` structured logs with request ids;
  log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Server is the only source of truth.** Clients never call LLM, search or Google APIs
  directly. Entitlements are decided on the server, per handler and per sync mutation
  (A42). No limit or cost is hardcoded: the matrix lives in `config/plans.toml` and is
  loaded once at startup.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
  Local-date math only through `chrono-tz` (A6), never a fixed offset. The planner is
  **DST-safe**: a non-existent local time is skipped, an ambiguous one takes the earlier
  offset.
- **The planner is pure.** No database, no clock, no IO, no network in
  `crates/core/src/planner/`. `now` is a field of `PlannerInput`.
- **Plan limits and credit costs are copied, never re-derived** (from
  `config/plans.toml`, which is the authority). The values this phase's tests assert are
  in the table below.
- **Errors serialize to** `{ code, message, min_plan?, limit?, used? }` — every field
  always present, `null` when not applicable. Status/code pairs are the roadmap's § 5
  table: `plan_required` 403, `limit_reached` 422, `insufficient_credits` 402,
  `validation` 400.
- **Quality gate:** `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `npm --prefix app test`, `npm --prefix app run build`,
  `node scripts/check-no-cdn.mjs`, `cargo sqlx prepare --workspace --check`, the
  `src-tauri` trio (`just check-shell`), and `cargo sqlx prepare --workspace` re-run after
  any query changes.
- **UI language English**, but Indonesian documents and queries must work end to end, and
  every new string this phase adds goes through `t()` with both `en.json` and `id.json`
  entries.
- **Honesty rules are a copy contract.** No invented or uncomputable number reaches the
  screen. A generative action shows its credit cost before it runs. An AI proposal is
  labelled "Suggested …" and nothing is written until the student confirms.

### Values this phase charges and refuses (copied from `config/plans.toml`)

Credit costs — `[credits]`: `syllabus_import = 8`, `plan_generation = 2`. No other action
this phase adds costs anything: confirming a syllabus, accepting a plan and replanning
spend nothing beyond the generation already charged.

| Plan | `syllabus_imports_total` | `planner` | `Feature::SyllabusImport` | `Feature::PlanGeneration` |
|---|---|---|---|---|
| free | `1` | `manual` | granted while the count is under 1, then 422 `limit_reached` | **refused**: 403 `plan_required`, `min_plan: "pro"` |
| pro | `unlimited` | `auto` | granted | granted; `balancer_enabled = false`, `spread_k = 0` |
| power | `unlimited` | `auto+balancer+weakness_blocks` | granted | granted; `balancer_enabled = true`, `spread_k = <[planner].spread_k>` |

`crates/core/src/entitlements/check.rs:88` reads `cfg.syllabus_imports_total` for
`SyllabusImport`, and `:159` reads `Feature::PlanGeneration => cfg.planner !=
PlannerMode::Manual`. That one line is why the free plan's refusal is a `plan_required`
with a real `min_plan`, and why A257 needs no second rule.

---

## What already exists (checked in this tree, not assumed)

**The prompts exist and are not wired. Both are missing the data block the composer
requires.**

- `prompts/syllabus.v1.md` uses `{{timezone}}`, `{{semester_context}}` and
  `{{output_language}}`, and documents
  `{"course":{…},"meetings":[…],"deadlines":[…],"exams":[…],"notes":[…]}` with an
  `evidence` string and a `confidence` per item. It has **no `<user_context_data>`
  block**, and `semester_context` is **not** in `UNTRUSTED_VARIABLES` — a student types
  "Semester 1 2025/2026" or, equally, "ignore all previous instructions".
- `prompts/planner_effort.v1.md` uses `{{goal_title}}`, `{{target_date}}`,
  `{{topics_json}}` and `{{output_language}}`, and documents
  `{"topics":[{"topic","estimate_min","reason"}],"total_estimate_min"}`. `goal_title` is
  already in `UNTRUSTED_VARIABLES` (`crates/core/src/prompts/mod.rs:68`) but the module has
  **no `<user_context_data>` block**, so the moment it is registered the untrusted sweep
  fails.
- `crates/core/src/prompts/registry.rs:100-108`: `ModuleId` has exactly five variants and
  `ALL: [ModuleId; 5]`; the `include_str!` constants are at `:12-49`; `SOURCE_LANGUAGE`
  (`:57`) is "the same language as the source material (do not translate)". **Neither
  module is registered**, so no `usage_events.prompt_version` row can reference them yet.
- `crates/core/tests/prompts.rs` has two sweeps that iterate `ModuleId::ALL`
  (`every_module_declares_exactly_the_variables_its_file_uses`, and
  `every_untrusted_value_is_delimited_as_data_in_every_module` at `:1128`), whose own doc
  comment says "Phase 9 adds `goal_title`/`task_title`/`notes`, Phase 11 `capture_text`".
  **Registering the two modules without editing the two files fails the second sweep**,
  which is the sweep doing its job.

**The schema for goals, events and tasks is complete. The planner needs one column that
does not exist.**

- `migrations/0012_tasks_goals_sessions_settings_actions.sql`: `goals (id, user_id,
  workspace_id, title, target_date timestamptz not null, source_ids uuid[], created_at,
  updated_at, deleted_at)` with `index on (user_id, target_date)`. **No `topics` column**
  — the topics come from `source_ids` plus the effort estimate, and are stored on the
  proposal (A252). **`created_at` is nullable-by-default but always set** (`default
  now()`), which is what makes the tie-break in step 5 of the algorithm total.
  `tasks (…, due_at, estimate_min, priority, status, …)` and `study_sessions` are there
  too, and `user_settings` carries `study_start_hour default 8`, `study_end_hour default
  22`, `max_study_min_per_day default 240`, `block_len_min default 50`, `break_min default
  10` — **the three settings the planner needs are already stored** (A256).
- `migrations/0013_calendar_events.sql`: `events (id, user_id, workspace_id, kind, title,
  starts_at, ends_at, external_id, etag, connection_id, source default 'app', task_id,
  created_at, updated_at, deleted_at)`, `index on (user_id, starts_at)`, `index on
  (task_id)`, `unique index on (connection_id, external_id) where external_id is not
  null`. `kind` is free text documented as `class | exam | study_block | deadline |
  external` — **the planner's `busy` filter and its output kind both already exist**.
  **There is no `goal_id`** — see A263.
- `migrations/0017_sync_triggers.sql:14-18`: `id_tables` already carries `workspaces,
  sources, messages, decks, flashcards, flashcard_reviews, quizzes, quiz_questions,
  quiz_attempts, reports, cheatsheets, tasks, goals, events, study_sessions,
  pending_actions`, and `user_settings` gets its own pair. `crates/server/tests/schema.rs`
  asserts exactly those tables carry both triggers. **This phase adds no `+sync` table and
  changes no trigger array**, so that file stays the last one that attaches a trigger and
  needs no edit — which is precisely why `syllabus_drafts` and `plan_proposals` are
  deliberately **not** `+sync` (A251, A252). The precedent is `0021`–`0023`, each of which
  says so in its own header comment.
- `migrations/0007_workspaces_sources_chunks.sql:16`: `sources.kind` is documented as
  `pdf | pptx | docx | url | text | web | syllabus` — **`syllabus` is already a legal
  kind**, so the upload writes a source row and no enum changes.
- `crates/server/tests/schema.rs:283-296`: the `documented` list is `[report_revisions,
  cheatsheets, email_verification_tokens, login_tickets, search_cache, signup_attempts]`.
  Anything not in the spec's DDL must be a row there, so this phase adds two.

**Entitlements are ready; the two actions are not wired to anything.**

- `crates/core/src/entitlements/feature.rs`: `Feature::SyllabusImport` and
  `Feature::PlanGeneration` exist, and `Feature::ALL` walks them. `min_plan` is derived
  from the matrix, so the free plan's refusal names `pro` by construction.
- `crates/server/src/entitlements.rs`: `require` (`:147`), `require_for` (`:173`),
  `reserve_in(conn, state, user, Option<Feature>, action, cost, reserve_closure)` (`:203`)
  — the one-transaction reservation a generative route needs, which takes the per-user
  advisory lock, checks the device-read-only flag **before** the feature branch, then
  spends and calls the closure with the `usage_event_id` — and `refusal(plan, error)`
  (`:690`).
- `crates/server/src/credits.rs:540`: `RefundTarget { Deck, Quiz, Report, Cheatsheet }`
  with one SQL arm per target and a boot sweep in `main.rs`. A charge that cannot be
  followed by a row is "charged and not delivered", so this phase adds two arms (A131's
  rule, applied to the two new chargeable things).
- `crates/server/src/jobs/worker.rs:85-105` `dispatch` knows ten kinds and returns
  `Failure::permanent("unknown job kind {other}")` for anything else. `plan_goal` is in
  the roadmap's § 8 list and **not** in the match.
- `crates/server/src/jobs/mod.rs:65` `enqueue(pool, queue, kind, payload, dedupe_key)`.
  `ClaimedJob { id, kind, payload, attempts }` with `user_id()` at `:163`.

**The server has no goals, syllabus or planner module.**

- `crates/server/src/` contains no `goals.rs`, `syllabus.rs` or `planner/`.
  `routes/mod.rs` merges `devices, workspaces, chunks, chat, flashcards, quizzes,
  reports, cheatsheets, sources` — this phase adds three merges. The roadmap's tree names
  `goals.rs`, `syllabus.rs` and `planner/{mod,routes,apply}.rs`; this phase creates
  exactly those five files.
- `crates/server/tests/route_sweep.rs` is a classification table of 53 rows (39 today plus
  Phase 9's fourteen) with `Class::{Swept, ReadOnly, Exempt}`, a
  `Row { method, path, class, reason, feature, refusal }`, `every_registered_route_is_classified`
  (fails on an unclassified route **and** on a stale row), and
  `every_swept_route_refuses_an_exhausted_account`, whose `Fixture::call` is a per-route
  `match` — **a new swept route needs a driver arm, not just a row**. This phase adds
  thirteen rows and three driver arms.
- `crates/server/src/util.rs` has `local_day_start(timezone, clock)` (`:14`),
  `local_month_start` (`:36`), `next_local_month_start` (`:58`) and `local_month_key`
  (`:84`) — the `chrono-tz` helpers, with the "never a fixed offset" rule in their doc
  comments.
- `crates/server/src/jobs/handlers/ingest.rs:113` matches `kind` on `"text"` and `"pdf"`
  only; `other => Failure::permanent(format!("unsupported source kind {other}"))`. And
  `crates/server/src/sources/routes.rs:31` `SUPPORTED_KINDS` is `["pdf", "text"]` — so the
  generic upload route refuses `syllabus`, which is why this phase's own route writes the
  row (A250).
- `crates/server/src/integrations/clock.rs` is `SystemClock`; `crates/core/src/traits/testing.rs:30`
  is `FixedClock::at(ts)`. Both already exist, which is what makes the planner's `now`
  injectable end to end.

**The SPA has the shells and none of the behaviour.**

- `app/src/router.tsx`: `/calendar` (Phase 10) and `/tasks`, `/settings` (Phase 9) are
  `NotBuiltRoute`s; **there is no `/planner` route at all**, though `nav.planner` exists
  in both locales. `/syllabus/*` does not exist either.
- `app/src/i18n/locales/en.json:602` already has a `planner` object with exactly two keys,
  `rebalance` ("Rebalance my week") and `rebalanceReason`; `id.json:593` mirrors both.
  `honesty.suggested` ("Suggested {{what}}"), `honesty.nothingAddedYet`, `lock.availableOn`
  and `lock.upgrade` exist for the review screen and the Power-gated balancer.
- `app/src/features/` has `auth, billing, chat, quiz, reports, studio, study, styleguide,
  today, workspaces`. **No `planner/`, no `syllabus/`, no `calendar/`.** This phase creates
  the first two.
- `app/src/features/today/TodayScreen.tsx` renders a `today.exams` section whose
  `emptyReason` already reads "Add an exam as a goal and it becomes the anchor the planner
  schedules against" — the screen Phase 11 finally feeds.
- `app/src/lib/time.ts`: `DEFAULT_TIMEZONE`, `localDateKey(instant, timeZone)` (en-CA
  `YYYY-MM-DD`), `formatInZone`, `zoneOffsetLabel`. The preview grid renders through these
  and never through `new Date().toLocaleString()`.
- `app/src/features/billing/plans.ts` `parsePlans` reads **only** `report_outline` and
  `report_section` from `[credits]`. The cost a screen shows comes from there, never a
  literal — so showing `syllabus_import`/`plan_generation` costs means extending it.

**Docs that must move in the same commits:** `docs/API.md:786-793` (the reserved
`## Goals and planner (Phase 11)` and `## Syllabus (Phase 11)` sections become real),
`docs/ARCHITECTURE.md` (a new `## Planner (Phase 11)` section after `## Quiz (Phase 5)`),
`docs/ASSUMPTIONS.md` (A250–A269, plus two schema-deviation rows), `docs/SYNC.md` (the
`events.goal_id` payload note).

---

## Inherited from Phases 9 and 10 (hard dependencies — not re-planned here)

Phases 9 and 10 land first. This plan consumes, and never re-implements, the following.
Each name is theirs; if a name here disagrees with what they ship, their name wins and
this file is the bug.

| Inherited | What this phase does with it |
|---|---|
| `crates/server/src/tasks.rs` and `crates/server/src/events.rs` — the sync-native CRUD, the server-owned fields, `GET /events?from=&to=` | The syllabus confirm writes `tasks` and `events` through the same table shapes; the planner reads `events` for `busy` and `tasks` for the due-date demand. Neither file is edited. |
| `events.kind` values (`class | exam | study_block | deadline | external`) and `events.source default 'app'` | The planner's output is `kind='study_block'`, `source='app'`. The `busy` filter is `kind <> 'study_block'`. No new kind is invented. |
| `crates/server/src/settings.rs` and `user_settings`' `study_start_hour`, `study_end_hour`, `max_study_min_per_day`, `block_len_min`, `break_min` | `StudySettings` and half of `PlannerConfig` are read from the user's own settings row (A256). No second settings surface. |
| `crates/server/src/study.rs` — `study_sessions`, and the reads the Today view needs | Replan treats a goal's already-started `study_block` events as elapsed time (A265). Nothing here re-reads sessions. |
| Phase 10's `crates/core/src/calendar/expand.rs` — the pure, bounded recurrence expansion (`all(limit)` + check `limited`, per the roadmap's risk register) — and its **expand-on-write** rule: every occurrence is a row in `events`, and `events` gains no `rrule` column | A confirmed class meeting carries an RFC 5545 `RRULE`; the confirm path expands it with `calendar::expand` and writes one `events` row per occurrence, exactly as Phase 10's ICS import does. Phase 10's plan already states why: storing the series and expanding on read "would need a second recurrence engine in the planner, in the SPA's week grid and in the ICS feed". This plan adds no second engine and does not re-open that decision. |
| Phase 10's `GET /cal/:token.ics` and the calendar UI at `/calendar` | The planner screen links to it and does not replace it. This phase adds `/planner`, not `/calendar`. |
| Phase 9's `route_sweep.rs` rows, and Phase 10's | This phase adds only its own thirteen. |
| `Platform.sync` and the `src-tauri` SQLite cache (Phase 8) | The one synced-shape change this phase makes is `events.goal_id` (A263); the cache's `schema.rs` gains that column in the same task. |

---

## Decisions this plan pins (so parallel work cannot diverge)

Each is a place the spec, the roadmap and the tree leave a choice. They are appended to
`docs/ASSUMPTIONS.md` as rows **A250–A269** in the same commits as the code. Phase 6 wrote
A119–A131 (reserved to A139), Phase 7 holds A140–A163, Phase 8 holds A164–A189, Phase 9
holds A190–A229 and Phase 10 holds A230–A249, so this block starts at A250.

1. **A250 — The syllabus extraction rides `ingest_source`; no new job kind is added.**
   The roadmap § 8 list is fixed and has no syllabus kind, so `sources.kind='syllabus'`
   gets a parse arm (pdf or text, by `mime`) and then one extra step: compose
   `syllabus.v1.md`, call the model, write `syllabus_drafts`. A parse failure is
   `Failure::permanent`; a model or transport failure is `Failure::transient` and the
   extraction is **idempotent** — it `update`s the draft row keyed by the unique
   `source_id`, so a retry overwrites rather than appending. The alternative, a
   synchronous route like `create_outline`, was rejected: the outline is one short call
   over selected chunks, and a syllabus is a whole document, which the spec requires be
   async with visible progress.

2. **A251 — `syllabus_drafts` is a new table and deliberately NOT `+sync`.** A draft is
   server-owned: the model produced it, the student confirms it, and it has no client
   identity. There is no `Entity` variant for it, and adding one would mean attaching a
   trigger from `0017_sync_triggers.sql`, which must stay the last trigger-attaching
   migration. It therefore gets a `documented` row in `crates/server/tests/schema.rs` and
   a row in `docs/ASSUMPTIONS.md` § Schema deviations — the `search_cache` precedent.

3. **A252 — `plan_proposals` is a new table and deliberately NOT `+sync`.** The roadmap's
   conflict table names "planner output" among the server-owned entities that are
   "read-only for clients except via their dedicated endpoints", which is exactly what
   `GET /plans/:id` is. The accepted blocks reach every device the way the spec says they
   do — as `events` — and the proposal itself never syncs. Same `documented` row and
   deviations row as A251.

4. **A253 — Planning is a job, not a route.** The effort estimate is a model call, so
   `POST /goals/:id/plan` reserves the 2 credits, creates the `plan_proposals` row in
   status `planning`, enqueues `plan_goal` and answers **202** with `{ "proposal_id": … }`;
   `GET /plans/:id` is how the client polls. The contrast with `create_outline` is
   deliberate and is the same distinction A250 draws: one short call may run in a route,
   a whole-document call may not.

5. **A254 — `semester_context` joins `UNTRUSTED_VARIABLES` and both modules gain a
   `<user_context_data>` block.** `semester_context` is free text a student types, so it
   is data, not a setting — the same argument `prompts/README.md` makes for `{{style}}`
   and `{{length}}`. `planner_effort.v1.md` wraps `{{goal_title}}`, which is already
   listed. The prompt **filenames** (`syllabus.v1.md`, `planner_effort.v1.md`) differ from
   the spec's (`syllabus_extract.v1.md`, `effort_estimate.v1.md`); that is already
   recorded as A112 and this plan does not re-open it, because renaming changes
   `usage_events.prompt_version` for every future turn.

6. **A255 — The planner's non-user tunables live in `config/plans.toml` under a new
   `[planner]` table.** `deadline_buffer_min = 120`, `revisit_days = 3`,
   `spread_k = 1.5`, `max_horizon_days = 120`. They are loaded into a new
   `PlannerTunables` struct on `PlansFile` (`crates/core/src/config/plans.rs`), beside
   `provider_cost_ceiling_usd`, and a missing `[planner]` table is a `validate()` error
   rather than a default — the same reason the credit table has no defaults.
   `max_horizon_days` is a bound, not a preference: a goal dated five years out would
   otherwise make the day loop unbounded, which the roadmap's own risk register calls "a
   real server hang".

7. **A256 — `block_len_min`, `break_min` and `max_study_min_per_day` come from
   `user_settings`, not from the plan.** They are the student's own choices and are
   already stored (`0012`); duplicating them in `plans.toml` would create two answers to
   one question. `PlannerConfig` is composed by the server from `user_settings` +
   `[planner]` + the plan's `PlannerMode`, and the pure planner never reads config.

8. **A257 — `PlannerMode` maps to `PlannerConfig` in exactly one place.** `Manual` never
   reaches the planner, because `Feature::PlanGeneration` is already `planner != manual`
   and the route refuses with 403 `plan_required`/`min_plan: pro`. `Auto` ⇒
   `balancer_enabled = false, spread_k = 0.0`. `AutoBalancerWeaknessBlocks` ⇒
   `balancer_enabled = true, spread_k = <[planner].spread_k>`. This is what makes "balancer
   and weak-topic blocks on Power" structural rather than a copy decision a screen could
   get wrong, and it is why a free account cannot get a balanced plan by calling the route
   directly.

9. **A258 — Topic weight is `1 + spread_k × (1 − accuracy)`, and a topic with no quiz
   attempts weighs 1.** `accuracy` is `Option<f64>`: the goal's topic accuracy over the
   last 14 days, `None` when there are no attempts. `None` weighs as if accurate, because
   the app must not invent a weakness out of missing data — the same honesty rule that
   makes the prep-progress card say "no quizzes yet" instead of 0%.

10. **A259 — `overload_pct` is a documented constant, not a `PlannerConfig` field.**
    `balance::OVERLOAD_PCT: f64 = 0.9`, with its own doc comment. Contract § 6 fixes
    `PlannerConfig`'s six fields, and adding a seventh would be a contract change made by a
    phase plan.

11. **A260 — `due_review_min = due_cards × DUE_CARD_MIN`, with `DUE_CARD_MIN: i64 = 1`.**
    A flashcard is a sub-minute item and a minute is the smallest unit every other number
    in the planner uses, so one minute per card is the honest rounding. It is a documented
    constant with a test, and the workload meter's "planned minutes" therefore traces to
    `due_cards × 1`, which is a number the app computes.

12. **A261 — The extraction's `confidence` is never rendered as a number.** The review
    screen orders its list by confidence ascending (lowest first, so the items most likely
    to be wrong are at the top) and marks an item below `REVIEW_FLAG_CONFIDENCE = 0.6` as
    needing a check. What the screen shows is the **evidence quote** — the exact source
    line — because the spec's honesty rules forbid showing confidence scores, and because
    a quoted line is what actually lets a student verify the item.

13. **A262 — `PlanWarning` has exactly two variants, and the enum is closed.**
    `Overloaded { days: Vec<NaiveDate> }` (the contract's own variant, from step 6) and
    `PastDeadline { goal_id: Uuid }` (a goal whose `target_date` is at or before `now`;
    it is reported rather than dropped, and it does not stop the other goals). A third
    variant is a contract change.

14. **A263 — `events` gains `goal_id uuid references goals(id) on delete set null`.**
    A planned block must be attributable to the goal it was planned for: replan needs it,
    the workload meter groups by it, and the timeline shows it. It is the same argument
    `0013` already made for `task_id` — a link without a FK drifts — and the same
    consequence follows: a schema-deviation row, a `src-tauri` cache column in the same
    task, and a `docs/SYNC.md` payload note. The `+sync` triggers are unaffected, because
    `log_change()` logs the whole row.

15. **A264 — `POST /plans/accept` re-validates the client's adjusted blocks and refuses
    the whole batch or none of it.** The client may drag a block; the server still checks
    the merged result against busy time, the study window and `max_study_min_per_day`, and
    answers 400 `validation` naming the offending block rather than writing part of a
    plan. Drag-adjust must not be able to produce the overlap the planner's own invariants
    forbid. Accept spends nothing: the 2 credits were charged at `POST /goals/:id/plan`.

16. **A265 — Replan splits the goal's existing blocks into `pinned` and `busy`.** A block
    that starts after `now` is `pinned` and is kept exactly. A block that has already
    started is added to `busy`, because contract step 2 subtracts `busy ∪ pinned` and
    does **not** subtract `kind='study_block'` rows by default — without this, replan
    would cheerfully schedule new material over a session the student is sitting in.

17. **A266 — `PlanProposal` serializes in a fixed order.** Blocks by `(starts_at,
    goal_id, topic)`, `day_load` by date ascending, `warnings` by variant order then by
    payload. Determinism is asserted on `serde_json::to_vec`, byte for byte, which is why
    the order is a decision rather than an implementation detail.

18. **A267 — The horizon ends at `max(target_date).local_date() + 1 day`**, so the
    deadline day is inside it and a block can be placed on the day of the exam if the
    student's window allows. The horizon is additionally clipped to
    `[today, today + max_horizon_days]` (A255).

19. **A268 — The balancer moves the latest movable block to the least-loaded day, bounded
    to 100 iterations.** Movable means: not pinned, and not a review-reserved block (the
    review pass of step 4 outranks the balancer). The target is the least-loaded day in
    the horizon that has a free slot of the block's length. The loop is
    `while any day > OVERLOAD_PCT × max_study_min_per_day && a movable block exists &&
    iterations < MAX_ITERATIONS`, and the count is returned so a test can assert the bound
    was respected rather than merely not exceeded by luck.

20. **A269 — The confirm mapping is fixed and lives in `generation::syllabus`.** A kept
    meeting → one `events` row per expanded occurrence (`kind='class'`, `source='app'`,
    expanded with Phase 10's `crates/core/src/calendar/expand.rs` and its expand-on-write
    rule, capped at `MAX_OCCURRENCES = 60`). A kept deadline → one `tasks` row (`due_at`
    set, `priority` 1, `estimate_min` null). A kept exam → one `goals` row (`target_date`
    set, `source_ids` the workspace sources the student selected for it). Review-reserved
    blocks are emitted as `BlockKind::Review` so the accept path writes them with a title
    that says "Review", and the balancer can tell them apart (A268).

---

## File structure (new this phase)

```
migrations/0030_syllabus_drafts.sql                 # new table; not +sync (A251)
migrations/0031_plan_proposals.sql                  # new table + events.goal_id (A252/A263)
crates/core/src/generation/syllabus.rs              # the extraction contract + review items
crates/core/src/generation/effort.rs                # the effort contract + reconcile
crates/core/src/planner/mod.rs                      # PlannerInput, PlanProposal, propose()
crates/core/src/planner/model.rs                    # the value types contract § 6 names
crates/core/src/planner/slots.rs                    # horizon, windows, busy, chunking (steps 1-2)
crates/core/src/planner/allocate.rs                 # demand, reviews, allocation (steps 3-6)
crates/core/src/planner/balance.rs                  # the Power balancer (step 7)
crates/core/src/planner/replan.rs                   # pinned blocks
crates/core/src/planner/progress.rs                 # prep progress (pure, no composite score)
crates/core/tests/generation_syllabus.rs
crates/core/tests/generation_effort.rs
crates/core/tests/planner.rs                        # the contract § 6 test list
crates/core/tests/planner_purity.rs                 # the no-clock/no-HashMap sweep
crates/core/tests/planner_progress.rs
crates/server/src/goals.rs                          # goals CRUD + prep progress
crates/server/src/syllabus.rs                       # upload, draft, confirm
crates/server/src/planner/mod.rs                    # input building, config mapping
crates/server/src/planner/routes.rs                 # the four routes
crates/server/src/planner/apply.rs                  # validate + write the accepted blocks
crates/server/src/jobs/handlers/plan.rs             # the plan_goal job
crates/server/tests/syllabus.rs
crates/server/tests/planner.rs
crates/server/tests/planner_jakarta.rs              # gate 6
app/src/api/queries/goals.ts
app/src/api/queries/planner.ts
app/src/api/queries/syllabus.ts
app/src/features/planner/PlannerScreen.tsx
app/src/features/planner/ProposalPreview.tsx
app/src/features/planner/preview.ts                 # pure: the drag-adjust rules
app/src/features/planner/preview.test.ts
app/src/features/planner/WorkloadMeter.tsx
app/src/features/planner/PrepProgress.tsx
app/src/features/syllabus/SyllabusReviewScreen.tsx
app/src/features/syllabus/review.ts                 # pure: the review list's ordering
app/src/features/syllabus/review.test.ts
scripts/smoke-phase11.sh
```

**Modified:** `prompts/syllabus.v1.md`, `prompts/planner_effort.v1.md`,
`crates/core/src/prompts/registry.rs`, `crates/core/src/prompts/mod.rs`,
`crates/core/src/generation/mod.rs`, `crates/core/src/lib.rs`,
`crates/core/src/config/plans.rs`, `config/plans.toml`, `crates/server/src/lib.rs`,
`crates/server/src/routes/mod.rs`, `crates/server/src/credits.rs`,
`crates/server/src/jobs/worker.rs`, `crates/server/src/jobs/handlers/mod.rs`,
`crates/server/src/jobs/handlers/ingest.rs`, `crates/server/src/main.rs`,
`crates/server/tests/schema.rs`, `crates/server/tests/route_sweep.rs`,
`src-tauri/src/cache/schema.rs`, `app/src/router.tsx`, `app/src/i18n/locales/en.json`,
`app/src/i18n/locales/id.json`, `app/src/features/billing/plans.ts`, `docs/API.md`,
`docs/ARCHITECTURE.md`, `docs/ASSUMPTIONS.md`, `docs/SYNC.md`.

**Migration numbers.** `0025`/`0026` are Phase 7's, `0027` is Phase 8's, `0028` is Phase
9's and `0029` (`0029_calendar.sql`) is Phase 10's, per those plans; the directory's
highest file today is `0025_entitlements_limits.sql` because those phases have not run
yet. This plan takes the next two free numbers, **0030** and **0031**. **Re-read
`migrations/` at implementation time** — a number claimed by a plan that lands first is a
rename and nothing else, and the rule is "next free", not "this file's guess".

---

## Task 1: The two prompt modules — the data blocks, the registry, and the contract cases

**Files:**
- Modify: `prompts/syllabus.v1.md`, `prompts/planner_effort.v1.md`
- Modify: `crates/core/src/prompts/registry.rs`
- Modify: `crates/core/src/prompts/mod.rs` (the `UNTRUSTED_VARIABLES` list only)

**The failing test first.**

- [ ] **Step 1: Register the two modules with the data blocks missing, and watch the sweep
      catch it.** Add the `include_str!` constants, the two `ModuleId` variants, their
      `version()`/`text()`/`variables()`/`base_sections()` arms and the `ALL` length, and
      add `"semester_context"` to `UNTRUSTED_VARIABLES`. Change nothing in the two `.md`
      files yet.

`cargo test -p stvdy-core --test prompts` — fails in
`every_untrusted_value_is_delimited_as_data_in_every_module` with a marker escaping its
`<user_context_data>` region for `planner_effort.v1` (`goal_title`) and `syllabus.v1`
(`semester_context`). That failure is the sweep doing its job, and it is the reason this
task exists.

- [ ] **Step 2: `crates/core/src/prompts/registry.rs`.**

```rust
pub const SYLLABUS_V1: &str = include_str!("../../../../prompts/syllabus.v1.md");
pub const PLANNER_EFFORT_V1: &str = include_str!("../../../../prompts/planner_effort.v1.md");

/// The default for `{{semester_context}}`. A syllabus often carries no semester label, and
/// the module's date-resolution rule needs something to read rather than an empty string.
const NO_SEMESTER_CONTEXT: &str = "no semester label was provided with the document";
```

```rust
pub enum ModuleId {
    FlashcardsV1,
    QuizV1,
    ReportOutlineV1,
    ReportSectionV1,
    CheatsheetV1,
    SyllabusV1,
    PlannerEffortV1,
}

impl ModuleId {
    pub const ALL: [ModuleId; 7] = [
        ModuleId::FlashcardsV1,
        ModuleId::QuizV1,
        ModuleId::ReportOutlineV1,
        ModuleId::ReportSectionV1,
        ModuleId::CheatsheetV1,
        ModuleId::SyllabusV1,
        ModuleId::PlannerEffortV1,
    ];

    pub fn version(self) -> &'static str { /* …, "syllabus.v1", "planner_effort.v1" */ }
    pub fn text(self) -> &'static str { /* …, SYLLABUS_V1, PLANNER_EFFORT_V1 */ }
    pub fn variables(self) -> &'static [(&'static str, VarKind)] {
        match self {
            // …
            // `timezone` is `Caller`, not `Optional`: the module's date-resolution rule is
            // meaningless without it, and a default would silently resolve "12 Nov" in the
            // wrong zone — which is the one mistake a syllabus import cannot survive.
            ModuleId::SyllabusV1 => &[
                ("timezone", VarKind::Caller),
                (
                    "semester_context",
                    VarKind::Optional(NO_SEMESTER_CONTEXT),
                ),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
            // `topics_json` is machine JSON the server built, not user text, so it is
            // `Caller` and not in `UNTRUSTED_VARIABLES`. `goal_title` is the student's own
            // words and is.
            ModuleId::PlannerEffortV1 => &[
                ("goal_title", VarKind::Caller),
                ("target_date", VarKind::Caller),
                ("topics_json", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
        }
    }
    pub fn base_sections(self) -> &'static [&'static str] { /* all seven: TASK_BASE_SECTIONS */ }
}
```

- [ ] **Step 3: `crates/core/src/prompts/mod.rs`.** Append to `UNTRUSTED_VARIABLES`, with
      the comment the list's style uses:

```rust
    // `semester_context` is typed into the syllabus form ("Semester 1 2025/2026"), and the
    // semester label is the value the module resolves a year-less date against — so it is
    // the one field an injected sentence could use to move every date in the import.
    "semester_context",
```

- [ ] **Step 4: `prompts/syllabus.v1.md`.** Wrap the semester context in the block the base
      prompt uses, so the value is labelled as data:

```markdown
<user_context_data>
semester_context: "{{semester_context}}"
</user_context_data>
```

- [ ] **Step 5: `prompts/planner_effort.v1.md`.** Same, for the goal title:

```markdown
<user_context_data>
goal_title: "{{goal_title}}"
</user_context_data>
```

- [ ] **Step 6: Run the contract sweep.**
      `cargo test -p stvdy-core --test prompts` — green. Then
      `cargo test -p stvdy-core` to confirm nothing else read `ModuleId::ALL`'s length.

- [ ] **Step 7: Commit.** `feat(core): register the syllabus and effort prompt modules`

---

## Task 2: The two strict-JSON contracts and the review mapping

**Files:**
- Create: `crates/core/src/generation/syllabus.rs`, `crates/core/src/generation/effort.rs`
- Modify: `crates/core/src/generation/mod.rs` (`pub mod syllabus; pub mod effort;`)
- Create: `crates/core/tests/generation_syllabus.rs`, `crates/core/tests/generation_effort.rs`

**The failing test first.**

- [ ] **Step 1: Write the two test files.** The assertions that matter:

```rust
// crates/core/tests/generation_syllabus.rs
#[test]
fn an_item_without_evidence_is_refused() {
    // The module's rule is "evidence: a short exact snippet copied from the source, under
    // 15 words". An empty quote is not a weaker citation, it is no citation: the review
    // screen's whole job is to show the student the line, so an item that cannot show one
    // must fail the parse and be retried rather than reach the screen.
    let raw = r#"{"course":{},"meetings":[{"title":"Lecture","rrule":"FREQ=WEEKLY;COUNT=3",
        "start_time":"08:00","end_time":"09:40","location":null,"evidence":"","confidence":0.9}],
        "deadlines":[],"exams":[],"notes":[]}"#;
    assert!(matches!(parse_syllabus(raw), Err(GenerationError::Schema(_))));

    // 20 words is also refused: the bound is what stops a model quoting a paragraph and
    // calling it evidence.
}

#[test]
fn an_item_carries_the_line_it_was_read_from() {
    // review_items() keeps the quote and the confidence, and orders lowest-confidence first.
}
```

```rust
// crates/core/tests/generation_effort.rs
#[test]
fn a_reply_that_drops_a_topic_is_refused() {
    // The module says "Return exactly one entry per input topic, using the same topic
    // names." reconcile() enforces it, and the caller retries once.
    let raw = r#"{"topics":[{"topic":"Graphs","estimate_min":120,"reason":"few sources"}],
                 "total_estimate_min":120}"#;
    assert!(matches!(reconcile(&parse_effort(raw).unwrap(), &["Graphs".into(), "Trees".into()]),
                     Err(GenerationError::Schema(_))));
}
```

`cargo test -p stvdy-core --test generation_syllabus --test generation_effort` — fails to
compile (`parse_syllabus` does not exist).

- [ ] **Step 2: `crates/core/src/generation/syllabus.rs`.** The structs mirror the module's
      schema exactly, with `deny_unknown_fields` (a drifted prompt must fail loudly) and
      bounds that a model ignoring the module cannot exceed:

```rust
pub const MAX_MEETINGS: usize = 40;
pub const MAX_DEADLINES: usize = 60;
pub const MAX_EXAMS: usize = 20;
pub const MAX_NOTES: usize = 20;
/// The module says "under 15 words"; this is what makes it enforceable.
pub const MAX_EVIDENCE_WORDS: usize = 15;
/// Below this, the review screen marks the item as needing a check (A261). The number
/// never reaches the screen.
pub const REVIEW_FLAG_CONFIDENCE: f64 = 0.6;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyllabusFile {
    pub course: CourseInfo,
    pub meetings: Vec<Meeting>,
    pub deadlines: Vec<Deadline>,
    pub exams: Vec<Exam>,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourseInfo { pub name: Option<String>, pub code: Option<String>, pub instructor: Option<String> }
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meeting { pub title: String, pub rrule: String, pub start_time: String, pub end_time: String,
                    pub location: Option<String>, pub evidence: String, pub confidence: f64 }
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deadline { pub title: String, pub due: String, pub weight_percent: Option<f64>,
                     pub evidence: String, pub confidence: f64 }
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exam { pub title: String, pub starts_at: Option<String>, pub ends_at: Option<String>,
                  pub topics: Vec<String>, pub evidence: String, pub confidence: f64 }

/// One row of the review screen, with the source line it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewItem {
    pub kind: ReviewKind,
    /// Stable across a re-parse of the same reply, so the confirm body can name it by
    /// index: `kind` then position within the reply, in the module's own field order.
    pub index: usize,
    pub title: String,
    pub evidence: String,
    pub confidence: f64,
    pub needs_check: bool,
    pub when: Option<DateTime<Utc>>,
    pub rrule: Option<String>,
    pub start_time: Option<(u32, u32)>,
    pub end_time: Option<(u32, u32)>,
    pub topics: Vec<String>,
    pub weight_percent: Option<f64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind { Meeting, Deadline, Exam }

pub fn parse_syllabus(raw: &str) -> Result<SyllabusFile, GenerationError>;
/// Ordered by confidence ascending, then by `(kind, index)` so the order is total.
pub fn review_items(file: &SyllabusFile, tz: Tz) -> Result<Vec<ReviewItem>, GenerationError>;
```

`review_items` refuses (`GenerationError::Schema`) when: any `evidence` is empty or longer
than `MAX_EVIDENCE_WORDS`; any `confidence` is outside `0.0..=1.0`; a count exceeds its
`MAX_*`; `start_time`/`end_time` are not `HH:MM`; `due`/`starts_at`/`ends_at` do not parse
as RFC 3339 **in `tz`**; or a `rrule` does not parse with `rrule`'s bounded iterator. It
never drops an item silently — a dropped item is a deadline the student never sees.

- [ ] **Step 3: `crates/core/src/generation/effort.rs`.**

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortFile { pub topics: Vec<EstimatedTopic>, pub total_estimate_min: i64 }
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EstimatedTopic { pub topic: String, pub estimate_min: i64, pub reason: String }

pub const MAX_ESTIMATE_MIN: i64 = 6000;

pub fn parse_effort(raw: &str) -> Result<EffortFile, GenerationError>;
/// Returns `(topic, estimate_min)` in the order the caller asked for, or `Schema` when a
/// topic is missing, an extra one appears, a name is duplicated, the total does not match
/// the sum, or any estimate is outside `1..=MAX_ESTIMATE_MIN`.
pub fn reconcile(estimate: &EffortFile, requested: &[String]) -> Result<Vec<(String, i64)>, GenerationError>;
```

- [ ] **Step 4: Run.** `cargo test -p stvdy-core --test generation_syllabus --test generation_effort`

- [ ] **Step 5: Commit.** `feat(core): the syllabus and effort JSON contracts`

---

## Task 3: `migrations/0030_syllabus_drafts.sql`

**Files:**
- Create: `migrations/0030_syllabus_drafts.sql`
- Modify: `crates/server/tests/schema.rs` (the `documented` list)
- Modify: `docs/ASSUMPTIONS.md` (one row in § Schema deviations)

**The failing test first.**

- [ ] **Step 1: Write the migration, run the schema test, watch it fail.**
      `cargo test -p stvdy-server --test schema` — fails with
      `tables exist that are neither in the spec nor a documented deviation:
      ["syllabus_drafts"]`.

- [ ] **Step 2: The migration.**

```sql
-- Syllabus extraction drafts.
--
-- Server-owned, and deliberately NOT a `+sync` table (A251): the model produced
-- the extraction, the student confirms it, and only the confirmed items become
-- events, tasks and goals. There is no `Entity` variant for a draft, and adding
-- one would mean attaching a trigger from `0017_sync_triggers.sql`, which must
-- stay the last migration that attaches one. The precedent is `0020_search_cache`
-- and `0023_report_generation`, neither of which touches the trigger array.
--
-- The unique index on `source_id` is what makes the extraction idempotent: a
-- retried `ingest_source` updates the same draft rather than appending a second
-- one, and `GET /syllabus/:id/draft` has exactly one row to read.
create table syllabus_drafts (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  source_id uuid not null references sources on delete cascade,
  status text not null default 'extracting',  -- extracting | ready | failed | confirmed
  extraction jsonb,
  prompt_version text,
  error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create unique index on syllabus_drafts (source_id);
create index on syllabus_drafts (user_id, status);
```

- [ ] **Step 3: `crates/server/tests/schema.rs`.** Append `"syllabus_drafts"` to
      `documented`, with the one-line comment the list's style uses, naming A251.

- [ ] **Step 4: `docs/ASSUMPTIONS.md` § Schema deviations.** One row: new table
      `syllabus_drafts`, why it is not `+sync`, and why it is keyed uniquely on
      `source_id`.

- [ ] **Step 5: Run.** `cargo test -p stvdy-server --test schema` — green. Then
      `cargo sqlx prepare --workspace` (the queries land in Task 4, so this is a no-op
      here; run it again after Task 4).

- [ ] **Step 6: Commit.** `feat(server): the syllabus draft table, and the deviation row it needs`

---

## Task 4: `crates/server/src/syllabus.rs` — upload, draft, and the extraction step

**Files:**
- Create: `crates/server/src/syllabus.rs`
- Create: `crates/server/tests/syllabus.rs` (the first three tests)
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`
- Modify: `crates/server/src/jobs/handlers/ingest.rs` (the `syllabus` arm)
- Modify: `crates/server/tests/route_sweep.rs` (the three rows)

**The failing test first.**

- [ ] **Step 1: Write the first three tests in `crates/server/tests/syllabus.rs`.**

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn uploading_a_syllabus_charges_eight_credits_and_returns_a_draft(pool: PgPool) {
    // 202 { "draft_id": … }; exactly one credit_ledger row of -8 with action
    // "syllabus_import"; a sources row with kind = 'syllabus' and status = 'queued';
    // a syllabus_drafts row in status 'extracting' with a null extraction.
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_import_on_the_free_plan_is_refused_with_limit_reached(pool: PgPool) {
    // free allows syllabus_imports_total = 1. The second upload is 422 limit_reached with
    // limit 1 and used 1, and the ledger is untouched — the refusal happens before the
    // charge, which is what `reserve_in`'s closure ordering buys.
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_draft_is_not_found(pool: PgPool) {
    // GET /syllabus/{id}/draft for a draft owned by someone else is 404, never 403:
    // a 403 would confirm the row exists.
}
```

`cargo test -p stvdy-server --test syllabus` — fails to compile (no `syllabus::routes`).

- [ ] **Step 2: `crates/server/src/syllabus.rs`.**

```rust
pub fn routes(body_limit_bytes: usize) -> Router<AppState>;
// .route("/workspaces/{id}/syllabus", post(create))
// .route("/syllabus/{id}/draft", get(draft))
// .route("/syllabus/{id}/confirm", post(confirm))

pub async fn create(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    multipart: Multipart,
) -> AppResult<(StatusCode, Json<serde_json::Value>)>;
```

`create` follows `sources::routes::create`'s shape (`:238`) for the multipart read, the
`HARD_MAX_BYTES` guard and `object_key`, and `reports::routes::create_outline`'s shape
(`:126`) for the charge:

1. workspace ownership through `workspaces::owned_name`;
2. `reserve_in(&mut *tx, &state, &user, Some(Feature::SyllabusImport), "syllabus_import",
   state.config.plans.credits.syllabus_import, |conn, usage_event_id| …)` — the closure
   inserts the `sources` row, the `syllabus_drafts` row and the object, so a failure to
   write any of them rolls the charge back with it;
3. `jobs::enqueue(&state.pool, &state.queue, "ingest_source", json!({"source_id": …,
   "user_id": …}), None)`;
4. 202 `{ "draft_id": … }`.

`draft` returns the stored extraction plus the review list, or 409 `conflict` while the
status is `extracting`, and 422 `conflict`-free failure detail when it is `failed`.

- [ ] **Step 3: The `ingest_source` arm.** In
      `crates/server/src/jobs/handlers/ingest.rs`, add a `"syllabus"` arm to the `kind`
      match that parses exactly like `"pdf"` (or `"text"`, by `mime`), and after the
      chunks are written, one extra step:

```rust
/// The extraction step, for `kind = 'syllabus'` only (A250).
///
/// It rides `ingest_source` because the roadmap's job-kind list is fixed and has no
/// syllabus kind, and it runs here rather than in the route because a whole document is
/// not a route-sized call — the contrast is `create_outline`, which is one short call and
/// does run in its route.
///
/// Idempotent: the draft is keyed uniquely on `source_id`, so a retry overwrites the
/// extraction rather than appending a second draft. A parse failure is permanent; a model
/// or transport failure is transient.
async fn extract_syllabus(state: &AppState, source_id: Uuid, user_id: Uuid, text: &str)
    -> Result<(), Failure>;
```

It reads the user's `timezone` from `user_settings`/`users`, composes `ModuleId::SyllabusV1`
through `prompts::compose_task` with `timezone`, `semester_context: None` and
`output_language`, calls the provider with `temperature` low, parses with
`parse_syllabus`, maps with `review_items`, and writes `extraction`, `prompt_version` and
`status = 'ready'`. On failure it writes `status = 'failed'` and `error`, sets the source
to `failed` **only** for a permanent failure, and returns the `Failure`.

- [ ] **Step 4: `crates/server/tests/route_sweep.rs`.** Three rows and one driver arm:

| Method | Path | Class | Why / feature | Refusal |
|---|---|---|---|---|
| POST | `/workspaces/{id}/syllabus` | Swept | `SyllabusImport` | 422 `limit_reached` |
| GET | `/syllabus/{id}/draft` | ReadOnly | reads the caller's own draft | — |
| POST | `/syllabus/{id}/confirm` | ReadOnly | the import was already counted and charged; confirming writes rows and spends nothing | — |

- [ ] **Step 5: Run.** `cargo test -p stvdy-server --test syllabus --test route_sweep` and
      `cargo sqlx prepare --workspace`.

- [ ] **Step 6: Commit.** `feat(server): syllabus upload, the draft, and the extraction step`

---

## Task 5: The confirm path — nothing is written until the student confirms

**Files:**
- Modify: `crates/server/src/syllabus.rs` (the `confirm` handler)
- Modify: `crates/server/tests/syllabus.rs` (the exit gate's first half)

**The failing test first.**

- [ ] **Step 1: Write the two tests that are the review screen's whole contract.**

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn nothing_is_written_until_the_student_confirms(pool: PgPool) {
    // After an upload and a completed extraction: `events`, `tasks` and `goals` are EMPTY
    // for this workspace. This is the assertion that makes the review screen a screen
    // rather than a formality.
}

#[sqlx::test(migrations = "../../migrations")]
async fn confirming_writes_only_the_kept_items(pool: PgPool) {
    // A draft with 3 meetings, 2 deadlines and 1 exam; the body keeps indices 0 and 3 and
    // drops the rest. Then: 1 goal, 1 task, and `MAX_OCCURRENCES`-bounded class events for
    // meeting 0 only. The dropped indices exist in no table.
}
```

`cargo test -p stvdy-server --test syllabus` — fails (no `confirm`).

- [ ] **Step 2: The handler.**

```rust
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmBody { pub items: Vec<ConfirmItem> }

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmItem {
    /// The `ReviewItem::index` the student saw. An index that is not in the draft is a 400,
    /// never a silently ignored row.
    pub index: usize,
    pub keep: bool,
    /// The student's edits. `None` keeps what the model extracted; `Some` replaces it.
    pub title: Option<String>,
    pub when: Option<String>,
    pub rrule: Option<String>,
    pub topics: Option<Vec<String>>,
    pub weight_percent: Option<f64>,
}

pub async fn confirm(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(draft_id): Path<Uuid>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<serde_json::Value>>;
```

One transaction. The draft must be `ready` and the caller's; the status becomes
`confirmed`; the mapping is A269, and the expansion of a meeting's `rrule` calls **Phase
10's** `crates/core/src/calendar/expand.rs` — pure, bounded, expand-on-write — so a
meeting becomes one `events` row per occurrence and there is still exactly one recurrence
engine in the product. A draft that is already `confirmed` answers 409 `conflict` rather
than writing twice.

- [ ] **Step 3: Run.** `cargo test -p stvdy-server --test syllabus`

- [ ] **Step 4: Commit.** `feat(server): the syllabus review's confirm, and only confirmed items`

---

## Task 6: `crates/core/src/planner/{mod,model,slots}.rs` — horizon, windows, chunking

**Files:**
- Create: `crates/core/src/planner/mod.rs`, `model.rs`, `slots.rs`
- Create: `crates/core/tests/planner.rs`, `crates/core/tests/planner_purity.rs`
- Modify: `crates/core/src/lib.rs` (`pub mod planner;`)

**The failing test first.**

- [ ] **Step 1: Write the purity sweep and the first five contract tests.**

```rust
// crates/core/tests/planner_purity.rs
/// The planner is a pure function, and "pure" is a property of the source, not of a
/// comment. A clock read or a hash-ordered container is a one-line change that no
/// behavioural test has to notice, and the failure is a study week that reshuffles itself
/// between two identical requests.
#[test]
fn the_planner_module_has_no_clock_and_no_hash_ordered_container() {
    for (name, source) in [
        ("mod.rs", include_str!("../src/planner/mod.rs")),
        ("model.rs", include_str!("../src/planner/model.rs")),
        ("slots.rs", include_str!("../src/planner/slots.rs")),
        ("allocate.rs", include_str!("../src/planner/allocate.rs")),
        ("balance.rs", include_str!("../src/planner/balance.rs")),
        ("replan.rs", include_str!("../src/planner/replan.rs")),
    ] {
        for forbidden in ["Utc::now", "Local::now", "SystemTime", "HashMap", "HashSet",
                          "sort_unstable", "partial_cmp"] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }
}
```

```rust
// crates/core/tests/planner.rs
#[test]
fn blocks_never_overlap_busy_time() { /* … */ }

#[test]
fn study_hours_are_respected() {
    // Every block's local start >= start_hour and local end <= end_hour, checked by
    // converting back to the input's `tz` — not by comparing UTC hours, which would pass in
    // Jakarta and be wrong everywhere else.
}

#[test]
fn study_hours_survive_a_dst_spring_forward_day() {
    // Europe/London, 2026-03-29, window 00:00–06:00. 01:00–02:00 local does not exist, so
    // the day's capacity is 300 minutes, not 360. Asserts the exact capacity AND that no
    // block starts at a local time in the missing hour.
}

#[test]
fn study_hours_take_the_earlier_offset_on_an_ambiguous_day() {
    // Europe/London, 2026-10-25, window 00:00–06:00. 01:00–02:00 local happens twice, so
    // the day's capacity is 420 minutes and the block that starts at 01:30 is the BST one.
}

#[test]
fn max_study_min_per_day_is_respected() { /* … */ }

#[test]
fn block_and_break_lengths_are_exact() {
    // Every block is exactly `block_len_min` long except the last in a window, which may be
    // shorter, and consecutive blocks in one window are separated by exactly `break_min`.
}
```

`cargo test -p stvdy-core --test planner --test planner_purity` — fails to compile.

- [ ] **Step 2: `crates/core/src/planner/model.rs`.** The contract § 6 types, verbatim in
      field order and name, plus the four it names without writing out:

```rust
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct PlannerInput {
    pub now: DateTime<Utc>,
    pub tz: Tz,
    pub settings: StudySettings,
    pub goals: Vec<GoalInput>,
    pub due_review_min: i64,
    pub busy: Vec<Interval>,
    pub pinned: Vec<Interval>,
    pub config: PlannerConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StudySettings { pub start_hour: u32, pub end_hour: u32, pub max_study_min_per_day: i64 }

#[derive(Debug, Clone, PartialEq)]
pub struct GoalInput {
    pub id: Uuid,
    pub title: String,
    pub target_date: DateTime<Utc>,
    /// The step-5 tie-break. Not a clock reading: it is the row's own `created_at`.
    pub created_at: DateTime<Utc>,
    pub topics: Vec<Topic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub source_id: Option<Uuid>,
    pub name: String,
    pub remaining_min: i64,
    /// `None` when the topic has no quiz attempts in the last 14 days (A258).
    pub accuracy: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval { pub starts_at: DateTime<Utc>, pub ends_at: DateTime<Utc> }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannerConfig {
    pub block_len_min: i64,
    pub break_min: i64,
    pub deadline_buffer_min: i64,
    pub revisit_days: i64,
    pub spread_k: f64,
    pub balancer_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlanProposal {
    pub blocks: Vec<ProposedBlock>,
    pub day_load: Vec<DayLoad>,
    pub deficit_min: i64,
    pub warnings: Vec<PlanWarning>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ProposedBlock {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub goal_id: Uuid,
    pub topic: String,
    pub kind: BlockKind,
    /// True for a block that came in through `PlannerInput::pinned` (A265).
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum BlockKind { NewMaterial, Review }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DayLoad { pub date: NaiveDate, pub planned_min: i64, pub capacity_min: i64 }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum PlanWarning {
    Overloaded { days: Vec<NaiveDate> },
    PastDeadline { goal_id: Uuid },
}
```

`PlanProposal` derives `Serialize` but not `Deserialize`: the proposal is produced by the
server and read by a client, and the accept path validates the **blocks the client sends**
against the invariants rather than trusting a round-tripped `PlanProposal` (A264).

- [ ] **Step 3: `crates/core/src/planner/slots.rs`.** Steps 1 and 2 of the algorithm.

```rust
/// Step 1. `None` when there is no goal to plan for, or every goal is in the past.
pub(crate) fn horizon(input: &PlannerInput) -> Option<(NaiveDate, NaiveDate)>;

/// `LocalResult` handling in exactly one place, because it is the rule that is wrong
/// everywhere it is written twice.
///
/// `None` (a non-existent local time on a spring-forward day) is skipped: there is no such
/// instant. `Ambiguous(a, _)` (an autumn day where the hour happens twice) takes `a`, the
/// earlier offset, so the day's blocks are placed in the first pass of the hour and the
/// second pass is not double-counted.
pub(crate) fn local_at(day: NaiveDate, hour: u32, minute: u32, tz: Tz) -> Option<DateTime<Utc>>;

/// Step 2. Merges overlapping and adjacent intervals first, then subtracts, then clips to
/// `max_study_min_per_day`, then chunks. Returns the day's capacity in minutes alongside.
pub(crate) fn free_slots(input: &PlannerInput, day: NaiveDate) -> (Vec<Interval>, i64);

pub(crate) fn merge(intervals: &[Interval]) -> Vec<Interval>;
pub(crate) fn chunk(window: Interval, block_len_min: i64, break_min: i64) -> Vec<Interval>;
/// `now` rounded up to the next block boundary, measured from the day's window start.
pub(crate) fn ceil_to_block(input: &PlannerInput) -> DateTime<Utc>;
```

`ceil_to_block` exists because the first day is a partial day: without it, a request made
at 14:07 would emit a block starting at 14:07, which is not a block length and not a
boundary a student can reason about.

- [ ] **Step 4: `crates/core/src/planner/mod.rs`.** The public surface and the step order,
      with `propose` wired to `slots` only for now:

```rust
//! The deterministic planner. Contract § 6 of the master roadmap, implemented.
//!
//! Pure: no database, no clock, no IO, no network. `now` is a field of `PlannerInput`.
//! Same input ⇒ byte-identical output (A266), which `tests/planner.rs` asserts rather than
//! assumes.

mod allocate;
mod balance;
mod model;
mod replan;
mod slots;

pub use model::*;

/// The one entry point. The steps run in the contract's fixed order and no other.
pub fn propose(input: &PlannerInput) -> PlanProposal {
    // 1-2 slots, 3-6 allocate, 7 balance — added by the next two tasks.
}
```

- [ ] **Step 5: Run.** `cargo test -p stvdy-core --test planner --test planner_purity`

- [ ] **Step 6: Commit.** `feat(core): the planner's horizon, study windows and block chunking`

---

## Task 7: `crates/core/src/planner/allocate.rs` — demand, the review pass, the spread cap

**Files:**
- Create: `crates/core/src/planner/allocate.rs`
- Modify: `crates/core/src/planner/mod.rs` (wire steps 3–6)
- Modify: `crates/core/tests/planner.rs`

**The failing test first.**

- [ ] **Step 1: Add the seven tests.**

```rust
#[test]
fn weak_topics_weigh_more_than_strong_ones() {
    // Two topics, identical `remaining_min`, accuracies 0.4 and 0.95, spread_k 1.5. The
    // weak one gets strictly more minutes, and the ratio is within 1% of
    // (1 + 1.5*0.6) / (1 + 1.5*0.05). A test that only asserted "more" would pass on a
    // formula that was merely monotone and wrong by a factor.
}

#[test]
fn a_topic_with_no_attempts_weighs_the_same_as_a_perfect_one() {
    // A258: missing data is not evidence of weakness.
}

#[test]
fn the_spread_cap_prevents_front_loading() {
    // One topic, 600 minutes of remaining work, ten days available. No single day gets
    // more than `max(block_len_min, ceil(remaining_min / days_left))`, so the work is
    // spread rather than finished on day one.
}

#[test]
fn the_deadline_buffer_is_left_free_of_new_material() {
    // No `NewMaterial` block starts inside `[target_date - deadline_buffer_min,
    // target_date)`. Asserted on the block kind, not on a gap, so a review block placed
    // there does not make the test vacuous.
}

#[test]
fn the_review_pass_reserves_one_block_per_day_before_the_exam() {
    // One `BlockKind::Review` block on each of the last `revisit_days` local dates before
    // `target_date`, for each goal (A269 / contract step 4).
}

#[test]
fn deficit_is_reported_when_capacity_is_short() {
    // 20 hours of demand in a 3-hour week: a best-effort plan is still emitted, and
    // `deficit_min` equals demand minus capacity, with `Overloaded` naming the full days.
    // "Never silently drop demand" is the contract's own sentence.
}

#[test]
fn a_deadline_before_now_is_a_warning_not_a_panic() {
    // `PastDeadline { goal_id }`, no blocks for that goal, and the other goal is planned.
}
```

`cargo test -p stvdy-core --test planner` — fails on the new assertions.

- [ ] **Step 2: The module.**

```rust
/// Step 3. `D = Σ_topic remaining_min × weight(topic) + due_review_min`.
pub(crate) fn demand(input: &PlannerInput) -> i64;

/// `1 + spread_k × (1 − accuracy)`, and `1` when `accuracy` is `None` (A258).
pub(crate) fn weight(topic: &Topic, spread_k: f64) -> f64;

/// Step 4. Consumes the slots it reserves, and returns the review blocks.
pub(crate) fn reserve_reviews(input: &PlannerInput, slots: &mut Vec<Slot>) -> Vec<ProposedBlock>;

/// Steps 5 and 6. Goals by `target_date` asc then `created_at`; topics by weight desc then
/// name; each topic capped per day at `max(block_len_min, ceil(remaining_min / days_left))`.
pub(crate) fn allocate(
    input: &PlannerInput,
    slots: &mut Vec<Slot>,
) -> (Vec<ProposedBlock>, i64, Vec<PlanWarning>);
```

```rust
/// A free slot, and how much of it is still unclaimed. Not part of the public surface: a
/// caller that could see the leftover capacity could be tempted to schedule against it.
pub(crate) struct Slot { pub interval: Interval, pub remaining_min: i64 }
```

- [ ] **Step 3: Add the determinism test, which is the one this task's ordering makes
      non-obvious.** It builds the same logical input twice — once with the accuracy map
      assembled in a `HashMap` and read out in insertion order, once from a `BTreeMap` — and
      asserts `serde_json::to_vec(&propose(&a)) == serde_json::to_vec(&propose(&b))`,
      byte-for-byte.

```rust
#[test]
fn the_same_input_produces_byte_identical_output() { /* … */ }
```

- [ ] **Step 4: Run.** `cargo test -p stvdy-core --test planner`

- [ ] **Step 5: Commit.** `feat(core): the planner's demand, review pass and spread-capped allocation`

---

## Task 8: `crates/core/src/planner/balance.rs` — the Power balancer

**Files:**
- Create: `crates/core/src/planner/balance.rs`
- Modify: `crates/core/src/planner/mod.rs` (wire step 7, gated on `balancer_enabled`)
- Modify: `crates/core/tests/planner.rs`

**The failing test first.**

- [ ] **Step 1: Add the two tests.**

```rust
#[test]
fn the_balancer_terminates_and_moves_the_latest_movable_block() {
    // A day at 240/240 with a movable block, and a day at 30/240. Asserts the returned
    // iteration count is <= MAX_ITERATIONS, that the heavy day's load strictly fell, and
    // that the block that moved is the latest-starting movable one.
}

#[test]
fn the_balancer_never_moves_a_pinned_block() {
    // The same shape with the only movable block pinned: the returned count is 0 and the
    // proposal is byte-identical to the same input with `balancer_enabled = false`.
}
```

- [ ] **Step 2: The module.**

```rust
/// The share of `max_study_min_per_day` above which a day counts as overloaded.
///
/// A constant rather than a `PlannerConfig` field, because contract § 6 fixes that struct's
/// six fields and a seventh is a contract change (A259).
pub(crate) const OVERLOAD_PCT: f64 = 0.9;

/// The termination bound (A268). The loop cannot spin: it either moves a block, which
/// strictly decreases the overloaded day's load, or it stops.
pub(crate) const MAX_ITERATIONS: usize = 100;

/// Step 7. Returns the iterations actually used, so a test can assert the bound was
/// respected rather than merely not exceeded by luck.
pub(crate) fn rebalance(
    input: &PlannerInput,
    blocks: &mut Vec<ProposedBlock>,
    day_load: &mut [DayLoad],
) -> usize;
```

A block is movable iff `!block.pinned && block.kind == BlockKind::NewMaterial` (A268).
After a move, `day_load` is recomputed from `blocks` rather than patched, so the two can
never disagree.

- [ ] **Step 3: Wire step 7 in `propose`,** gated on `input.config.balancer_enabled`, which
      the server sets from `PlannerMode` (A257).

- [ ] **Step 4: Run.** `cargo test -p stvdy-core --test planner --test planner_purity`

- [ ] **Step 5: Commit.** `feat(core): the Power balancer, bounded to a hundred iterations`

---

## Task 9: `replan.rs` and `progress.rs`

**Files:**
- Create: `crates/core/src/planner/replan.rs`, `crates/core/src/planner/progress.rs`
- Modify: `crates/core/src/planner/mod.rs`
- Modify: `crates/core/tests/planner.rs`
- Create: `crates/core/tests/planner_progress.rs`

**The failing test first.**

- [ ] **Step 1: Add the tests.**

```rust
// crates/core/tests/planner.rs
#[test]
fn replan_keeps_pinned_blocks_exactly() {
    // Three pinned intervals and a goal that would otherwise want those slots. Every pinned
    // interval appears in the output as a block with `pinned: true` and the identical
    // `starts_at`/`ends_at`, and no new block overlaps one.
}
```

```rust
// crates/core/tests/planner_progress.rs
#[test]
fn prep_progress_reports_null_accuracy_when_there_are_no_attempts() {
    // The spec is explicit: "quiz accuracy (last 14 days, or 'no quizzes yet')". `None`,
    // never 0 — 0% is a claim the app cannot make.
}

#[test]
fn prep_progress_has_no_composite_score() {
    // `PrepProgress` has exactly the four fields the spec names, and no `readiness`,
    // `grade` or `score`. Asserted by destructuring the struct exhaustively, so adding a
    // fifth field fails to compile rather than passing silently.
}
```

- [ ] **Step 2: `replan.rs`.**

```rust
/// `propose`, with `PlannerInput::pinned` emitted first as blocks with `pinned: true` and
/// never moved. Everything else is identical: replan is not a second algorithm, it is the
/// same one with more of its input already decided (A265).
pub fn replan(input: &PlannerInput) -> PlanProposal;
```

- [ ] **Step 3: `progress.rs`.**

```rust
/// The goal's true components, and nothing else (spec § Honesty rules: no composite
/// "readiness" and no grade prediction).
#[derive(Debug, Clone, PartialEq)]
pub struct PrepProgress {
    /// The share of the goal's topics that have at least one review or quiz attempt.
    pub topic_coverage_pct: Option<i64>,
    /// Over the last 14 days, or `None` — never 0 when there are no attempts.
    pub quiz_accuracy_pct: Option<i64>,
    pub cards_due: i64,
    pub weak_topics: Vec<String>,
}

pub fn prep_progress(topics: &[Topic], attempts: &[TopicAttempts], cards_due: i64) -> PrepProgress;

/// What one topic's attempts looked like, as the server reads them.
#[derive(Debug, Clone, PartialEq)]
pub struct TopicAttempts { pub topic: String, pub attempts: i64, pub correct: i64 }
```

`weak_topics` is the topics whose accuracy is below `WEAK_TOPIC_ACCURACY = 0.6`, ordered
by accuracy ascending then by name — the same ordering rule A266 uses, for the same reason.

- [ ] **Step 4: Run.** `cargo test -p stvdy-core --test planner --test planner_progress`

- [ ] **Step 5: Commit.** `feat(core): replan with pinned blocks, and the prep-progress calculation`

---

## Task 10: `migrations/0031_plan_proposals.sql` and `events.goal_id`

**Files:**
- Create: `migrations/0031_plan_proposals.sql`
- Modify: `crates/server/tests/schema.rs`, `docs/ASSUMPTIONS.md`, `docs/SYNC.md`
- Modify: `src-tauri/src/cache/schema.rs` (the `events.goal_id` column)

**The failing test first.**

- [ ] **Step 1: Write the migration and run the schema test.**
      `cargo test -p stvdy-server --test schema` — fails on `plan_proposals`, then on
      nothing once the `documented` row is added.

- [ ] **Step 2: The migration.**

```sql
-- The planner's proposal.
--
-- Server-owned: the roadmap's conflict table lists "planner output" among the
-- entities that are "read-only for clients except via their dedicated endpoints",
-- which is what `GET /plans/:id` is. Not a `+sync` table, for the same reason as
-- `syllabus_drafts` (A252). The accepted blocks reach every device the way the
-- spec says planner results do — as `events`.
--
-- `proposal` holds the serialized `PlanProposal`; `deficit_min` and `warnings` are
-- lifted out of it because the list screen reads them without parsing the whole
-- thing, and because a query for "which proposals could not fit" is a query.
create table plan_proposals (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  goal_id uuid not null references goals on delete cascade,
  status text not null default 'planning',  -- planning | ready | failed | accepted | superseded
  proposal jsonb,
  deficit_min int,
  warnings jsonb,
  prompt_version text,
  error text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index on plan_proposals (user_id, goal_id, created_at desc);

-- A planned block belongs to the goal it was planned for (A263). The same argument
-- `0013` already made for `task_id`: a link without a FK drifts, and a replan or a
-- workload meter that reads a dangling id is a bug that only shows up later.
--
-- This does not touch `0017_sync_triggers.sql`: `log_change()` logs the whole row,
-- so a new column needs no trigger change and the trigger migration stays last.
alter table events add column goal_id uuid references goals(id) on delete set null;
create index on events (goal_id);
```

- [ ] **Step 3: The three consequences, in the same commit.** (a) `documented` +=
      `"plan_proposals"`; (b) an ASSUMPTIONS § Schema deviations row covering both the new
      table and the new column; (c) `src-tauri/src/cache/schema.rs` gains `goal_id text`
      on the `events` table, and `docs/SYNC.md` notes that the `Event` payload now carries
      it — the client cache must have the column or an applied event loses it.

- [ ] **Step 4: Run.** `cargo test -p stvdy-server --test schema` and
      `cargo test -p stvdy-sync` (the payload round-trip).

- [ ] **Step 5: Commit.** `feat(server): the plan proposal table, and the goal link a block needs`

---

## Task 11: `crates/server/src/goals.rs` and `crates/server/src/planner/`

**Files:**
- Create: `crates/server/src/goals.rs`, `crates/server/src/planner/{mod.rs,routes.rs,apply.rs}`
- Create: `crates/server/tests/planner.rs` (the first three tests)
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`
- Modify: `crates/server/tests/route_sweep.rs` (ten rows, two driver arms)
- Modify: `config/plans.toml`, `crates/core/src/config/plans.rs` (the `[planner]` table)

**The failing test first.**

- [ ] **Step 1: Write the first three tests.**

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn planning_a_goal_on_the_free_plan_is_refused_with_plan_required(pool: PgPool) {
    // 403 plan_required, min_plan "pro". This is A257's structural claim, asserted rather
    // than described: the free plan's `planner = "manual"` is the whole rule.
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_proposal_that_cannot_fit_reports_its_deficit(pool: PgPool) {
    // A goal with far more demand than the week holds: `deficit_min > 0`, `Overloaded` names
    // days, and a best-effort plan is still there.
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_proposal_is_not_found(pool: PgPool) { /* 404 */ }
```

- [ ] **Step 2: `config/plans.toml` and `crates/core/src/config/plans.rs`.** Add the
      `[planner]` table (A255) and a `PlannerTunables` struct on `PlansFile` with
      `validate()` requiring it, beside `provider_cost_ceiling_usd`. A missing table is a
      config error, not a silent default.

- [ ] **Step 3: `crates/server/src/goals.rs`.** `GET /goals`, `POST /goals`,
      `GET /goals/{id}`, `PATCH /goals/{id}`, `DELETE /goals/{id}`,
      `GET /goals/{id}/progress`. Goals are **not** plan-gated: a free account has goals and
      a manual planner. `progress` builds `PrepProgress` from the goal's `source_ids`, the
      quiz attempts over the last 14 days and the due cards, and returns it — with
      `quiz_accuracy_pct: null` when there are none.

- [ ] **Step 4: `crates/server/src/planner/mod.rs`.** The only place a `PlannerInput` is
      built and the only place `PlannerMode` becomes a `PlannerConfig` (A257):

```rust
pub fn config_from(settings: &StudySettingsRow, mode: PlannerMode, t: &PlannerTunables) -> PlannerConfig;
pub async fn input_for(state: &AppState, user_id: Uuid, goal_id: Uuid) -> AppResult<PlannerInput>;
pub fn busy_from(events: &[EventRow], tasks: &[TaskRow]) -> Vec<Interval>;
```

`busy_from` is `kind <> 'study_block'` events plus tasks with a `due_at`, merged, plus the
goal's already-started `study_block` events (A265). `due_review_min` is `due_cards ×
DUE_CARD_MIN` (A260), read from the same due computation the study screen uses.

- [ ] **Step 5: `crates/server/src/planner/routes.rs`.** Four routes, exact paths from the
      spec's API list:

```rust
pub fn routes() -> Router<AppState>;
// .route("/goals/{id}/plan", post(create_proposal))     // 202, reserves 2 credits
// .route("/plans/{id}", get(proposal))                  // the poll
// .route("/plans/accept", post(accept))                 // the write
// .route("/plan/replan", post(replan))                  // 202, reserves 2 credits
```

`POST /plans/accept` and `GET /plans/{id}` sit beside Phase 13's `GET /plans` (the billing
plan list); the paths are the spec's, they differ in method and depth, and the route sweep
carries both rows.

- [ ] **Step 6: `crates/server/src/planner/apply.rs`.**

```rust
/// Validates the client's blocks and writes them, or writes none of them (A264).
///
/// The client may drag; the server still checks. A block that overlaps `busy`, leaves the
/// study window, or pushes its day past `max_study_min_per_day` is a 400 naming the index —
/// not a silently dropped row, because a preview that accepted the drag and then wrote
/// something else is worse than a refusal.
pub async fn accept(
    state: &AppState,
    user: &VerifiedUser,
    proposal_id: Uuid,
    blocks: Vec<AcceptBlock>,
) -> AppResult<Vec<Uuid>>;
```

- [ ] **Step 7: `crates/server/tests/route_sweep.rs`.** Ten rows and two driver arms
      (`POST /goals/{id}/plan`, `POST /plan/replan`):

| Method | Path | Class | Why / feature | Refusal |
|---|---|---|---|---|
| GET | `/goals` | ReadOnly | reads the caller's own goals | — |
| POST | `/goals` | ReadOnly | creating a goal spends nothing; the planner is the gated part | — |
| GET | `/goals/{id}` | ReadOnly | reads the caller's own goal | — |
| PATCH | `/goals/{id}` | ReadOnly | editing a goal spends nothing | — |
| DELETE | `/goals/{id}` | ReadOnly | deleting frees the anchor, not a slot | — |
| GET | `/goals/{id}/progress` | ReadOnly | computed from the caller's own attempts | — |
| POST | `/goals/{id}/plan` | Swept | `PlanGeneration` | 403 `plan_required` |
| GET | `/plans/{id}` | ReadOnly | reads the caller's own proposal | — |
| POST | `/plans/accept` | ReadOnly | the 2 credits were charged at plan time | — |
| POST | `/plan/replan` | Swept | `PlanGeneration` | 403 `plan_required` |

- [ ] **Step 8: Run.** `cargo test -p stvdy-server --test planner --test route_sweep` and
      `cargo sqlx prepare --workspace`.

- [ ] **Step 9: Commit.** `feat(server): goals CRUD, the proposal preview, accept and replan`

---

## Task 12: The `plan_goal` job, the dispatch arm, and the refund arm

**Files:**
- Create: `crates/server/src/jobs/handlers/plan.rs`
- Modify: `crates/server/src/jobs/handlers/mod.rs`, `crates/server/src/jobs/worker.rs`,
  `crates/server/src/credits.rs`, `crates/server/src/main.rs`
- Modify: `crates/server/tests/planner.rs`, `crates/server/tests/syllabus.rs`

**The failing test first.**

- [ ] **Step 1: Add the two refund tests.**

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_plan_generation_refunds_the_two_credits(pool: PgPool) {
    // A provider that always errors: the proposal is `failed`, `error` is set, and the
    // ledger holds +2 with `RefundTarget::PlanProposal`. Then a boot sweep recovers nothing
    // a second time — the same assertion `reports.rs` makes for its own target.
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_extraction_refunds_the_eight_credits(pool: PgPool) {
    // Same shape, `RefundTarget::SyllabusDraft`.
}
```

- [ ] **Step 2: `crates/server/src/jobs/handlers/plan.rs`.**

```rust
/// The `plan_goal` job: the one model call, then the pure planner, then the row.
///
/// The call estimates minutes and nothing else (`planner_effort.v1.md` says so in its own
/// text, and `reconcile` refuses a reply that answers a scheduling question anyway). The
/// placement is `stvdy_core::planner::propose`, which has no clock and no IO.
pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure>;
```

It reads `now` **once** from `state.clock.now()` at the top and puts it in the
`PlannerInput`; nothing below reads a clock. A permanent failure (a malformed goal, an
effort reply that `reconcile` refuses twice) marks the proposal `failed`; a transient one
leaves it `planning` for the retry.

- [ ] **Step 3: The three wiring edits.** `handlers/mod.rs` gains `pub mod plan;`;
      `worker.rs::dispatch` gains `"plan_goal" => crate::jobs::handlers::plan::run(state,
      job).await,`; `credits.rs::RefundTarget` gains `PlanProposal` and `SyllabusDraft`
      with their `label()`, `list_failed_sql()` and `clear_sql()` arms; `main.rs`'s boot
      sweep grows by two.

- [ ] **Step 4: Run.** `cargo test -p stvdy-server --test planner --test syllabus` and
      `cargo sqlx prepare --workspace`.

- [ ] **Step 5: Commit.** `feat(server): the plan_goal job, and the refund arm a failed plan needs`

---

## Task 13: `crates/server/tests/planner.rs` — the exit gate

**Files:**
- Modify: `crates/server/tests/planner.rs`

**The failing test first.**

- [ ] **Step 1: The gate test.**

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn the_accepted_plan_respects_busy_time_study_hours_the_daily_max_and_the_buffer(
    pool: PgPool,
) {
    // Seed: settings 08:00–22:00, max 240/day, block 50, break 10; two `class` events on
    // Wednesday; one goal 21 days out with three topics and a weak one.
    // Flow: POST /goals/:id/plan → run the job → GET /plans/:id → drag one block →
    // POST /plans/accept.
    // Assert on the WRITTEN events: none overlaps a class event; every one is inside
    // 08:00–22:00 local; no day exceeds 240 minutes; no new-material block starts inside the
    // 120-minute buffer; every row has `goal_id` set and `source = 'app'`; the weak topic
    // got more minutes than the strong one.
}

#[sqlx::test(migrations = "../../migrations")]
async fn replan_keeps_the_pinned_blocks_and_leaves_the_started_one_alone(pool: PgPool) {
    // After the first accept: move one block to the past, replan, and assert the past block
    // is untouched, the future ones are exactly where they were, and the new proposal does
    // not overlap either (A265).
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_accepted_proposal_cannot_be_accepted_twice(pool: PgPool) {
    // 409 conflict, and the second call writes no second set of events. Idempotence by
    // refusal, which is the same rule the syllabus confirm uses.
}
```

- [ ] **Step 2: Run the gate and quote its output in the phase summary.**
      `cargo test -p stvdy-server --test planner -- --nocapture`

- [ ] **Step 3: Commit.** `test(server): the planner exit gate, and the sweep's new rows`

---

## Task 14: The SPA — the planner, the preview and the review screen

**Files:**
- Create: `app/src/api/queries/{goals,planner,syllabus}.ts`
- Create: `app/src/features/planner/{PlannerScreen.tsx,ProposalPreview.tsx,preview.ts,preview.test.ts,WorkloadMeter.tsx,PrepProgress.tsx}`
- Create: `app/src/features/syllabus/{SyllabusReviewScreen.tsx,review.ts,review.test.ts}`
- Modify: `app/src/router.tsx`, `app/src/i18n/locales/{en,id}.json`,
  `app/src/features/billing/plans.ts`

**The failing test first.**

- [ ] **Step 1: Write the two pure-logic test files.** The rule is the one Phase 9 set for
      the timer: the client's arithmetic is a pure module with its own tests, because a
      second implementation of the server's invariant in a component would be a second
      schedule.

```ts
// app/src/features/planner/preview.test.ts
it('refuses a drag that would overlap a class before the request is sent', () => {
  // The grid knows the busy intervals it was given, so the refusal is local and immediate.
  // The server checks again (A264) — this is the courtesy, not the control.
});

it('the workload meter counts minutes, not blocks', () => {
  // The spec's meter is "planned minutes today vs the user's daily limit".
});
```

```ts
// app/src/features/syllabus/review.test.ts
it('an unconfirmed item is never in the confirm payload', () => {
  // review.ts builds the body; an item the student dropped has no index in it.
});

it('the review list is ordered by confidence ascending and shows the evidence', () => {
  // A261: the number orders the list, the quote is what is rendered.
});
```

`npm --prefix app test` — fails (the modules do not exist).

- [ ] **Step 2: `PlannerScreen.tsx`.** A new `/planner` route (the `nav.planner` label
  already exists in both locales) with the spec's `Plan | Tasks` segmented control — Tasks
  is Phase 9's screen, linked rather than copied. It holds: the goals list with the
  `PrepProgress` card, the `WorkloadMeter`, the `ProposalPreview` behind the
  `honesty.suggested` label, and the Power-only "Rebalance my week" button behind
  `LockedFeature` with the existing `planner.rebalance` / `planner.rebalanceReason` copy.

- [ ] **Step 3: `ProposalPreview.tsx`.** The time grid the spec's screen 9 describes:
  07:00–22:00, blocks positioned by start and duration, busy time muted and read-only,
  drag to move, the current-time line from `formatInZone` and never `new Date()`. Accept
  and Dismiss are the two actions; Edit is the drag.

- [ ] **Step 4: `SyllabusReviewScreen.tsx`.** One row per `ReviewItem`, each showing the
  parsed value and **the evidence quote**, with a checkbox to keep and inline edits for
  title, time and topics. Items below `REVIEW_FLAG_CONFIDENCE` render first with the
  "needs a check" mark and **no number** (A261). `honesty.nothingAddedYet` is the header
  line: nothing is written until Confirm.

- [ ] **Step 5: `app/src/features/billing/plans.ts`.** `parsePlans` also reads
  `syllabus_import` and `plan_generation`, so the two screens show the credit cost before
  the action runs — the honesty rule, and the reason that file is the only place a cost
  comes from.

- [ ] **Step 6: Both locales.** Every new key in `en.json` and `id.json`, including the
  Indonesian for the planner's warnings and the review screen's marks.

- [ ] **Step 7: Run.** `npm --prefix app test && npm --prefix app run build`

- [ ] **Step 8: Commit.** `feat(app): the planner, the proposal preview and the syllabus review screen`

---

## Task 15: Docs, the three gates, and the smoke run

**Files:**
- Modify: `docs/API.md`, `docs/ARCHITECTURE.md`, `docs/ASSUMPTIONS.md`, `docs/SYNC.md`
- Create: `crates/server/tests/planner_jakarta.rs`, `scripts/smoke-phase11.sh`

**The failing test first.**

- [ ] **Step 1: Gate 6's test, before the docs.**

```rust
// crates/server/tests/planner_jakarta.rs
#[sqlx::test(migrations = "../../migrations")]
async fn the_planner_and_its_copy_are_correct_in_asia_jakarta(pool: PgPool) {
    // The user's tz is Asia/Jakarta (the default, and where the product is sold). Runs the
    // whole flow — upload, extract, confirm, plan, accept — and asserts:
    //   * every written event's local wall-clock time is inside 08:00–22:00 in Jakarta,
    //     read back through `chrono_tz`, not through an offset;
    //   * a `due` of "2026-11-12T23:59:00+07:00" survives the round trip unchanged, which is
    //     the assertion that catches a UTC-stored, offset-rendered date;
    //   * the proposal's warning copy renders in Indonesian with every placeholder filled
    //     (`t('planner.overloaded', { count: … })` has no `{{` left), and in English too.
    // Jakarta has no DST, which is exactly why the DST arithmetic is tested on Europe/London
    // in `tests/planner.rs` and this test is about the zone the product ships in.
}
```

`cargo test -p stvdy-server --test planner_jakarta` — fails until Task 14's locale keys
exist, which is why it is written here rather than with the server tests.

- [ ] **Step 2: `docs/API.md`.** Replace the two reserved sections
      (`## Goals and planner (Phase 11)` at `:786`, `## Syllabus (Phase 11)` at `:790`) with
      the real thing in the style the surrounding sections use: every route, its statuses,
      its refusal codes, and the two 202s and their poll.

- [ ] **Step 3: `docs/ARCHITECTURE.md`.** A `## Planner (Phase 11)` section after
      `## Quiz (Phase 5)`: the pure/impure split, the seven steps and the order they run
      in, why the extraction rides `ingest_source` (A250), why the proposal is not synced
      (A252) and why the accepted blocks are (as `events`), and the determinism defence
      with the three tests that carry it.

- [ ] **Step 4: `docs/ASSUMPTIONS.md`.** A250–A269 in the "Pinned decisions" table, plus the
      two schema-deviation rows from Tasks 3 and 10. Row numbers are the plan's; the table
      is the record.

- [ ] **Step 5: `docs/SYNC.md`.** The `Event` payload note from Task 10, in the section that
      lists what each synced entity carries.

- [ ] **Step 6: `scripts/smoke-phase11.sh`,** in the shape the earlier phases' smoke scripts
  use: migrate, boot, seed a user, upload a fixture syllabus, run the job, confirm, plan,
  accept, and print the written blocks.

- [ ] **Step 7: Gate 5 (accessibility), measured, on the two new screens.**
  (a) keyboard only: tab from the goals list to the preview, move a block with the arrow
  keys, reach Accept and the review screen's every control, each with a visible
  `outline-ring`; (b) contrast on both themes with the existing token test extended to the
  new components' class strings; (c) at 375px width every control is ≥44px; (d) "needs a
  check" is not colour alone — it carries a text mark; (e) 200% text scale with no clipping
  on the grid's labels. Record the check and the result; "looks fine" is not a gate.

- [ ] **Step 8: Gate 7 (honesty), measured.** State which rules this phase touched: the
  "Suggested plan" label on the proposal (`honesty.suggested`); no confidence number
  anywhere (A261, asserted by a grep test over the two feature directories); the credit
  cost shown before both generative actions (Task 14 Step 5); every number on the screens
  traces to `prep_progress` or `day_load`; and no composite readiness or grade anywhere
  (Task 9's exhaustive destructure).

- [ ] **Step 9: Run everything.**
      `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
      && cargo test --workspace && npm --prefix app test && npm --prefix app run build &&
      node scripts/check-no-cdn.mjs && cargo sqlx prepare --workspace --check`

- [ ] **Step 10: Commit.** `docs(phase 11): the planner and syllabus contract, and the gate results`

---

## Phase 11 exit checklist

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
| 9 | **Exit gate, part 1** — syllabus → review → events/tasks/goals | `cargo test -p stvdy-server --test syllabus` | |
| 10 | **Exit gate, part 2** — the proposal respects busy time, the daily max, study hours, the buffer | `cargo test -p stvdy-server --test planner -- --nocapture` | |
| 11 | Contract § 6's ten unit tests | `cargo test -p stvdy-core --test planner` | |
| 12 | The purity sweep | `cargo test -p stvdy-core --test planner_purity` | |
| 13 | Prompt contract | `cargo test -p stvdy-core --test prompts` | |
| 14 | Route sweep (13 new rows) | `cargo test -p stvdy-server --test route_sweep` | |
| 15 | Schema deviations | `cargo test -p stvdy-server --test schema` | |
| 16 | Gate 5 — accessibility | the five measured checks in Task 15 Step 7 | |
| 17 | Gate 6 — Indonesian, `Asia/Jakarta` | `cargo test -p stvdy-server --test planner_jakarta` | |
| 18 | Gate 7 — honesty | the five statements in Task 15 Step 8 | |
| 19 | Smoke | `scripts/smoke-phase11.sh` | |

## Execution notes

To be filled in as the phase is built, in the shape Phase 5 and Phase 6 use: what the plan
did not anticipate, each with the file that proved it; the gate measurements' actual
numbers; and anything a later phase inherits that this one could not know.

## Self-Review

**Spec coverage, item by item.** `docs/SPEC.md` § Syllabus import: "source kind=syllabus"
→ Task 4 (`sources.kind` is already a legal value, `0007:16`); "The LLM extracts structured
JSON … each with evidence and confidence" → Tasks 1–2; "Show a REVIEW screen where the user
edits/accepts items" → Tasks 5 and 14; "only then create events, tasks, and goals" →
Task 5, asserted by `nothing_is_written_until_the_student_confirms`. § Planner: "Input:
goals …, quiz weakness data, due flashcards, existing events (busy time), tasks with
estimates, user_settings" → Task 11's `input_for`/`busy_from` and A256; "work backward from
each goal date … weight weak topics higher … respecting max minutes/day, block length,
breaks, and never overlapping busy time" → Tasks 6–7; "spread evenly with
spaced-repetition-style revisits before the exam" → Task 7's spread cap and review pass;
"leave a buffer before deadlines" → Task 7's buffer test; "The LLM may only help estimate
effort … Slot placement is plain Rust" → Tasks 2 and 12, and the prompt says it in its own
text; "Output a SUGGESTED plan the user previews, tweaks, and accepts (then write events)"
→ Tasks 11 and 14; "Replan endpoint reschedules unfinished blocks" → Task 9's `replan`;
"Prep progress for a goal (`GET /goals/:id/progress`) … No composite score and no grade
prediction" → Task 9's `progress.rs` and Task 11; "Workload meter: planned minutes today vs
the user's daily limit" → Task 14's `WorkloadMeter`; "Workload balancer … Weak-topic auto
review blocks (Power)" → Task 8 and A257; § Plans' `planner = "manual" | "auto" |
"auto+balancer+weakness_blocks"` → A257 and the values table; § Credit costs'
`syllabus_import=8, plan_generation=2` → the values table and Tasks 4, 11; § unit tests'
"planner (no overlap with busy time, respects daily max and study hours, deadline buffer,
weak-topic weighting, timezone/DST-safe), prep-progress calculation" → Tasks 6–9; § Screens
9, 10 and 11 → Task 14. Roadmap contract § 6's ten named tests map one-to-one onto Tasks 6–9
and are listed by name in the exit checklist.

**Placeholders.** None. Every type, function, test name, route, path, migration and
command above is concrete. The two things a reader might expect and will not find are
deliberate and are recorded as decisions rather than left implicit: `syllabus_drafts` and
`plan_proposals` are not `+sync` (A251, A252), and no new job kind is added (A250).

**Type consistency.** `PlannerInput`, `PlanProposal`, `ProposedBlock`, `DayLoad`,
`PlanWarning`, `Interval`, `GoalInput`, `Topic`, `StudySettings` and `PlannerConfig` are
defined once, in `crates/core/src/planner/model.rs`, in the field order contract § 6 writes
them, and referenced by name everywhere else — including in this document's later tasks.
`ReviewItem`/`ReviewKind` are defined once, in `generation/syllabus.rs`. `BlockKind` is the
one addition to the contract's own vocabulary, and A269 says why. The contract's
`PlanProposal`/`PlanWarning` are the roadmap's names, not new ones.

**Two things this plan knowingly leaves to the phase that lands first.** Phase 10's
`crates/core/src/calendar/expand.rs` (the confirm path calls it and adds no second
recurrence engine) and Phase 10's migration number, which its plan has now claimed as
`0029_calendar.sql` — so this plan's two are **0030** and **0031**, and the file says to
re-read `migrations/` rather than trust that either is still free. Both are stated in the
file rather than discovered during execution.
