# Phase 9. Tasks, events, Today, timer, settings. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the roadmap
> disagree, the roadmap wins and this file is the bug.

**Goal:** a student can write down what they have to do — a task with subtasks, priority, a
due date and an estimate, an event, a settings row — and it works with the network down.
They can type one line ("selesaikan tugas kripto hari Jumat") and get a draft they confirm
or edit before anything is saved, or ask for a breakdown of a task and accept, edit or
dismiss the proposed subtasks. The home screen answers "what is due, what is on today, and
what should I study now", and a focus timer logs real minutes to `study_sessions` — across
a restart, once.

**Architecture:** this is the first phase that is *a client of* the sync engine rather than
a producer of synced data. Phase 8 lands the protocol, the outbox, the SQLite cache, the
apply path and the convergence tests; this phase writes the four entities the student
edits, the validators that keep the server's own fields out of a client payload, and the
screens. Two consequences shape every task below:

- **Every write goes through one path.** The SPA never calls REST for a synced entity: it
  calls `platform.sync.write(entity, id, payload)` (Phase 8), which is the outbox on native
  and the REST route on web. The REST routes therefore exist for the web build and for the
  first sync of an offline row, and they are not a second write path — they are the same
  table triggers, and the roadmap already says so ("both funnel through the same table
  triggers and therefore produce the same `change_log` entries and the same LWW outcome").
- **The Today view is a pure function of rows the device already has.** That is the only
  reason the roadmap's exit gate for this phase ("the Today view and focus timer work
  offline") is achievable at all, and it is why there is no `GET /today` route: a server
  route cannot work offline by definition. The day boundaries and the streak live in
  `app/src/features/today/day.ts`, mirroring `app/src/features/study/due.ts`, so the rules
  that fail *silently* for an Indonesian user (a UTC day boundary, a streak off by one)
  are unit-tested instead of asserted.

**Tech Stack:** Rust (core + server, no new crates), Postgres/sqlx, React + Vite +
TypeScript + Tailwind, TanStack Query, Tauri 2 (one new command pair for local state).

**Exit gate (observable, from the roadmap):** offline task + event + session writes sync
after reconnect; the Today view and focus timer work offline.

**Three roadmap gates are not optional here:**
- Gate 5, accessibility: this phase adds four screens and a form. The check performed and
  its measured result are stated in Task 14, not claimed.
- Gate 6, Indonesian end to end: an Indonesian capture text resolves to a due date in
  `Asia/Jakarta` (not UTC), an Indonesian task title produces Indonesian subtasks
  (`{{output_language}}`), and a `23:30+07:00` event lands in the correct local day. All
  three fail silently while English tests stay green.
- Gate 7, honesty: the two generative actions show their credit cost before they run, a
  breakdown is labelled "Suggested …" and saves nothing until confirmed, and every number
  on the Today screen traces to a row.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes.
  Prompts live in versioned modules, never inline. `tracing` structured logs with
  request ids; log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Server is the only source of truth.** Clients never call LLM, search or Google
  directly. All heavy work is server-side.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings. Everything else queues nothing. Three of the six are this
  phase's, and they are the three the exit gate measures.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
  Local-date math only through `chrono-tz` on the server and `Intl` with an explicit
  `timeZone` on the client. Never the machine zone.
- **Entitlements:** no limit is hardcoded; every limit is read from `config/plans.toml`
  through `crates/server/src/entitlements.rs`. Errors serialize to
  `{ code, message, min_plan?, limit?, used? }`.
- **Quality gate:** `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `npm --prefix app test`, `npm --prefix app run build`,
  `node scripts/check-no-cdn.mjs`, `cargo sqlx prepare --workspace --check`.
- **UI language English**, but Indonesian documents, queries and *typed task text* must
  work end to end.

### Values this phase charges and refuses (copied from `config/plans.toml`)

| Action | Cost | Read from |
|---|---|---|
| quick capture | `2` | `[credits] task_breakdown` (A181 — the matrix has no `quick_capture` key) |
| task breakdown | `2` | `[credits] task_breakdown` |
| task / event / session / settings write | `0` | — |

| Feature | free | pro | power |
|---|---|---|---|
| `quick_capture` | false → 403 `plan_required` | true | true |
| `task_ai_breakdown` | false → 403 `plan_required` | true | true |
| `reminder_channels` | `[web, push]` | `[web, push, email]` | `+telegram, discord` |

---

## What already exists (checked in this tree, not assumed)

**The schema is complete for tasks, events, sessions and settings. No new table, and
`0017_sync_triggers.sql` is not touched.**

- `migrations/0012_tasks_goals_sessions_settings_actions.sql`:
  - `tasks (id, user_id, workspace_id, parent_id references tasks on delete cascade,
    title, notes, due_at, estimate_min, priority default 2, status default 'todo',
    created_at, updated_at, deleted_at)` with `index on (user_id, status, due_at)` and
    `index on (parent_id)`. **No `position` column** — see A187.
  - `study_sessions (id primary key, user_id, workspace_id, task_id references tasks on
    delete set null, started_at, duration_min, created_at, updated_at, deleted_at)` with
    `index on (user_id, started_at)`. Client-generated id, append-only.
  - `user_settings (user_id primary key, study_start_hour default 8, study_end_hour
    default 22, max_study_min_per_day default 240, block_len_min default 50, break_min
    default 10, reminder_channels jsonb default '["web"]', updated_at)`. **No `language`,
    no `theme`, no quiet hours** — the spec's DDL has the first two and `0012` dropped
    them; the spec's settings screen requires the third. See A193.
- `migrations/0013_calendar_events.sql`: `events (id, user_id, workspace_id, kind, title,
  starts_at, ends_at, external_id, etag, connection_id references calendar_connections,
  source default 'app', task_id references tasks on delete set null, created_at,
  updated_at, deleted_at)`, `index on (user_id, starts_at)`, `index on (task_id)`, and
  `unique index on (connection_id, external_id) where external_id is not null`.
- `migrations/0017_sync_triggers.sql` already carries `tasks`, `goals`, `events`,
  `study_sessions` and `pending_actions` in `id_tables`, and gives `user_settings` its
  `touch_updated_at` + `log_change_by_user` pair with no DELETE trigger. `crates/server/tests/schema.rs`
  asserts exactly those tables carry both triggers. **This phase adds no `+sync` table and
  changes no trigger array**, so that file stays the last one that attaches a trigger, and
  `schema.rs` needs no new `documented` row. The precedent is `0021`–`0023`, which each say
  so in their own header comment.
- `crates/sync/src/proto/entities.rs`: `Entity::{Task, Event, Goal, StudySession,
  UserSettings, PendingAction}` already exist with `table_name`, `dependency_rank`,
  `is_client_writable`, `is_append_only` and `accepts_push_op`. **Nothing to add here.**
- `crates/sync/src/proto/push.rs`: `Mutation { mutation_id, entity, op, id, payload,
  client_ts }`. `client_ts` already exists on the wire, which is what makes the session
  bound in A195 clock-skew-safe without adding a field.
- `crates/sync/src/lib.rs` is `pub mod proto;` and nothing else. **There is no
  `crates/sync/src/client/`** — the engine, the outbox, the apply path and the transport
  are Phase 8's, and this plan does not touch them.

**Entitlements are ready; the actions are not.**

- `crates/core/src/entitlements/feature.rs`: `Feature::TaskBreakdown` and
  `Feature::QuickCapture` exist, and `min_plan` is derived from the matrix.
  `crates/core/src/entitlements/check.rs:96-97` groups them with the boolean grants and
  `:156-157` reads `cfg.task_ai_breakdown` / `cfg.quick_capture`.
- `crates/server/src/entitlements.rs`: `require`, `require_for`,
  `reserve_in(conn, state, user, Option<Feature>, action, cost, reserve_closure)` — the
  one-transaction reservation a generative route needs, and `limits_view(plans, plan)`,
  which already exposes `reminder_channels`, `quick_capture` and `task_ai_breakdown` so the
  SPA can read what a plan grants without a plan-field read in a handler.
- `crates/core/src/config/plans.rs:123` `CreditAction` has `TaskBreakdown` and **no
  `QuickCapture`**; `config/plans.toml [credits]` has `task_breakdown = 2` and **no
  `quick_capture` key**. See A181.

**The prompts exist and are not wired.**

- `prompts/quick_capture.v1.md` uses `{{now}}`, `{{timezone}}`, `{{capture_text}}` and
  documents `{"title","notes","due_at","priority","estimate_min"}`. It has **no
  `<user_context_data>` block**, and `capture_text` is already in `UNTRUSTED_VARIABLES`.
- `prompts/task_breakdown.v1.md` uses `{{task_title}}`, `{{due_at}}`, `{{notes}}`,
  `{{output_language}}` and documents
  `{"subtasks":[{"title","estimate_min"}],"total_estimate_min"}`. It has **no
  `<user_context_data>` block**; `task_title` and `notes` are already in
  `UNTRUSTED_VARIABLES`.
- `crates/core/src/prompts/registry.rs:100-108`: `ModuleId` has exactly five variants and
  `ALL: [ModuleId; 5]`. `SOURCE_LANGUAGE` (`:57`) is "the same language as the source
  material (do not translate)". **Neither task module is registered**, so no
  `usage_events.prompt_version` row can reference them — which is what makes A182 legal.
- `crates/core/src/prompts/mod.rs`: `UNTRUSTED_VARIABLES` (`:65`) already carries
  `task_title`, `notes`, `capture_text`; `compose_task` (`:389`); `values_of` (`:275`)
  fills `now` and `timezone` from `ChatVars`, so a module declaring them gets them
  without the caller passing anything; `escape_if_untrusted` (`:537`).
  `prompts/json_contract.v1.md` uses `{{output_language}}`, and it is rendered with the
  *module's* table — so every module must declare `output_language`, whether or not its
  own file uses it.
- `crates/core/tests/prompts.rs` has two sweeps that iterate `ModuleId::ALL`:
  `every_module_declares_exactly_the_variables_its_file_uses` (in `mod task_composition`)
  and `every_untrusted_value_is_delimited_as_data_in_every_module` (`:~1130`), whose own
  doc comment says "Phase 9 adds `goal_title`/`task_title`/`notes`, Phase 11
  `capture_text`". **Registering the two modules without editing the two files fails the
  second sweep**, which is the sweep doing its job.

**The server has no tasks, events, sessions or settings module.**

- `crates/server/src/` contains no `tasks.rs`, `events.rs`, `study.rs`, `settings.rs` or
  `dashboard.rs`, and `crates/server/src/routes/mod.rs` merges only `devices`,
  `workspaces`, `chunks`, `chat`, `flashcards`, `quizzes`, `reports`, `cheatsheets` and
  `sources`. The roadmap's tree names `tasks.rs`, `events.rs`, `study.rs` and
  `settings.rs`; this phase creates exactly those four.
- `crates/server/tests/route_sweep.rs` is a classification table of 53 rows with
  `Class::{Swept, ReadOnly, Exempt}`, a `Row { method, path, class, reason, feature,
  refusal }`, `every_registered_route_is_classified` (fails on an unclassified route **and**
  on a stale row), and `every_swept_route_refuses_an_exhausted_account`, whose
  `Fixture::call` is a per-route `match` — **a new swept route needs a driver arm, not just
  a row**. Its third test forbids a plan-limit field read outside the entitlements layer,
  and `LIMITS` (`:1078-1101`) includes `quick_capture`, `task_ai_breakdown` and
  `reminder_channels`, so a handler that reads `plans.get(..).reminder_channels` fails the
  sweep.
- `crates/server/src/util.rs` has `local_day_start(timezone, clock)` and
  `local_month_start(timezone, clock)` — the `chrono-tz` helpers, with the "never a fixed
  offset" rule stated in their doc comments.

**The SPA has the shells and none of the behaviour.**

- `app/src/features/today/TodayScreen.tsx` (1891 bytes) is a stub: four `Section`s
  (`today.studyBlocks`, `today.dueCards`, `nav.tasks`, `today.exams`) each rendering an
  `EmptyState`, with a comment saying "There is no data layer yet" and **no test file**.
- `app/src/router.tsx`: `/tasks` and `/settings` are `NotBuiltRoute`s. `routes.tasks.what`
  already reads "with quick capture and an assistant that proposes breakdowns you confirm
  before they save" and `routes.settings.what` "timezone and study hours, daily limits,
  reminder channels, connected calendars, and your signed-in devices".
- `app/src/features/study/due.ts` + `due.test.ts` — the pattern for pure client logic
  ("a second implementation in TypeScript would be a second schedule").
- `app/src/lib/time.ts`: `DEFAULT_TIMEZONE`, `localDateKey(instant, timeZone)` (en-CA
  `YYYY-MM-DD`), `formatInZone`, `zoneOffsetLabel`.
- `app/src/features/billing/plans.ts`: `parsePlans` reads **only** `report_outline` and
  `report_section` from `[credits]`; `REPORT_CREDITS` and `PLANS` are exported. The cost a
  screen shows comes from here, never a literal.
- `app/src/platform/types.ts`: `Platform` is `kind | runtime | secureStore | notifications
  | files | deepLinks | openExternal | haptic | checkForUpdate`. **No local state and no
  sync** — Phase 8 adds `sync`, this phase adds `localState`.
- `app/src/i18n/locales/en.json` already has the keys this phase's honesty rules need:
  `honesty.suggested` ("Suggested {{what}}"), `honesty.nothingAddedYet` ("Nothing is added
  until you confirm."), `honesty.composerHint`, `states.offlineReason` ("This needs a
  connection. Your tasks, events, and reviews still work."), `lock.availableOn`,
  `lock.upgrade`.
- `docs/API.md:778-815` already reserves the sections this phase fills:
  `## Tasks (Phase 9)`, `## Events (Phase 9)`, `## Settings (Phase 9)`, and
  `## Study and dashboard (Phase 12)` which currently holds `POST /sessions`.

**Docs that must move in the same commits:** `docs/API.md` (the three placeholder
sections), `docs/SYNC.md` (the pushable entities' payload rules change — see A189, A191,
A192, A195), `docs/ARCHITECTURE.md` (the offline write path as exercised, the timer's local
state, the one asymmetry between native and web reads), `docs/ASSUMPTIONS.md`
(A181–A200) and `docs/THREAT_MODEL.md` § 6 R1 (an addendum: the plausibility bound R1 names
now exists, and why it arrived in this phase).

---

## Inherited from Phase 8 (a hard dependency — not re-planned here)

Phase 8 lands first. This plan consumes, and never re-implements, the following. Each name
is Phase 8's; if a name here disagrees with what Phase 8 ships, Phase 8's name wins and this
file is the bug.

| Inherited | What this phase does with it |
|---|---|
| `crates/sync/src/client/{engine,outbox,apply,conflict,transport}.rs` — the client engine, the outbox, the `PRAGMA defer_foreign_keys=ON` apply, and a `Transport` trait a test can make fail | Task 11 drives it with a transport that fails, then succeeds. Nothing here re-implements it. |
| `crates/server/src/sync/{mod,pull,push,stream,apply,log}.rs`; `push.rs::decide` from Phase 7's A150 | Task 10 adds one validator call per entity to `decide`. It does not move the decision, and it adds no new rejection mechanism. |
| The advisory-lock-first rule in the push transaction | Task 10 relies on it: a task plus its subtasks is the roadmap's own named deadlock case, and it is already closed. |
| `Platform.sync` (`SyncApi`: `status()`, `read(entity, filter?)`, `write(entity, id, payload)`, `tombstone(entity, id)`, `syncNow()`, `reset()`, `onStatus(handler)`) and its Tauri commands | Every SPA write and read of a synced entity goes through it. The web adapter is REST-backed; the native adapter is the cache + outbox. |
| The `sync_status` / `SyncStatus` surface (`synced | syncing | offline | error`) and the last-synced timestamp | The Today screen's sync line and the settings screen's "Reset local data and resync" call `platform.sync.reset()`. |
| The `local_state`-style storage the cache is built on | Task 12 adds a two-command `local_state_get` / `local_state_set` pair over the same SQLite file, for the focus timer's running state (A197). Phase 8's `src-tauri/src/cache/schema.rs` gains one table in that task. |
| Phase 8's `route_sweep.rs` rows for `/sync/pull`, `/sync/push`, `/sync/stream` | This phase adds only its own fourteen rows. |

---

## Decisions this plan pins (so parallel work cannot diverge)

Each is a place the spec, the roadmap and the tree leave a choice. They are appended to
`docs/ASSUMPTIONS.md` as rows **A181–A200** in the same commits as the code. Phase 8 is
drafting A161–A180 (confirmed with that plan's author), so this block starts at A181.

1. **A181 — quick capture is priced as `task_breakdown` (2), not with a new key.**
   `config/plans.toml [credits]` has no `quick_capture` entry and
   `CreditAction` has no `QuickCapture` variant. The roadmap's own rule is that where the
   matrix has no key, the closest stated action's price applies rather than one invented in
   code, and A120 is the precedent (a cheatsheet is charged as `report_section`). The SPA
   reads the same number from `config/plans.toml` through `plans.ts`.
2. **A182 — `prompts/quick_capture.v1.md` and `prompts/task_breakdown.v1.md` are edited in
   place, not superseded.** The edit is the `<user_context_data>` block that
   `every_untrusted_value_is_delimited_as_data_in_every_module` requires, and nothing else.
   Rule 1 of `prompts/README.md` ("never edit a shipped version") exists so
   `usage_events.prompt_version` stays attributable; a version no `ModuleId` can produce has
   never been logged, so there is nothing to attribute. The check that this is true is
   `git log -S'QuickCaptureV1'` being empty at the time of the change, and the two modules
   being absent from `ModuleId::ALL` in the parent commit.
3. **A183 — `{{output_language}}` is `VarKind::Caller` in both new modules, supplied from
   `user_settings.language`.** `Optional(SOURCE_LANGUAGE)` would default to "the same
   language as the source material", and neither module has sources: the default would be a
   silent English breakdown of an Indonesian task. `prompts/json_contract.v1.md` uses
   `{{output_language}}` and is rendered with the module's table, so both modules must
   declare it even though `quick_capture.v1.md`'s own text does not use it.
4. **A184 — a quick-capture draft is never persisted.** `POST /tasks/quick-capture` writes
   one `credit_ledger` row and one `usage_events` row and returns a proposal. The only
   writer of a task row is `POST /tasks` (or a push). Consequence, stated rather than
   hidden: a reload loses an unaccepted draft. That is what "confirm" means, and it is the
   mechanical defence against "a draft that saves without confirmation".
5. **A185 — a `due_at` the parser cannot read becomes `null` with a warning, not a
   failure.** The reply carries `warnings: ["due_at: could not read \"friday-ish\""]` and the
   draft still returns. Dropping the date silently is worse than a visible warning, and
   failing the request after the credits were spent is worse than both.
6. **A186 — a breakdown proposal is never persisted either, and accepting it is N ordinary
   task creates.** `POST /tasks/:id/breakdown` returns
   `{subtasks: [{title, estimate_min}], total_estimate_min}` and saves nothing. Accepting
   sends one `POST /tasks` per kept subtask with `parent_id` and `position` (A187); editing
   and dismissing are client-side acts on the proposal. `total_estimate_min` is the model's
   number and is shown only inside the "Suggested …" region; the `estimate_min` that gets
   stored is whatever the student left after editing.
7. **A187 — `tasks.position int not null default 0` orders subtasks, and the order key is
   `(position, created_at, id)`.** Phase 5 shipped `quiz_questions` ordered by
   `created_at` and every question of one generation shared a timestamp to the microsecond,
   so the order fell through to a random uuid and the quiz came back in a different order
   than the model wrote it (A100). A breakdown has exactly that shape. `position` is a
   normal task field: the client sets it, push carries it, and LWW treats it like any other.
8. **A188 — deleting a parent tombstones its children, and the local apply must too.**
   Postgres's `on delete cascade` fires each child's DELETE trigger, so `log_change` writes
   the child tombstones and the server side needs no code. The *local* side does: a device
   that deletes a parent offline must tombstone the children in the same transaction, or it
   shows orphaned subtasks until the next pull. Tested in Task 11, not assumed.
9. **A189 — `events.source`, `external_id`, `etag` and `connection_id` are server-owned.**
   A push or REST write that sets any of them (other than `source = 'app'` with the other
   three null) is rejected: `read_only_entity` through push, 400 `validation` through REST.
   This is the same class as `flashcards.ease/interval/due` and it matters more, because
   Phase 10 fills those columns from Google and ICS: a client that could write them could
   forge a "Google" event, hijack the `(connection_id, external_id)` unique index, or
   overwrite a synced external row that the next incremental sync would then not match.
10. **A190 — a deadline is an `event` with `kind = 'deadline'` and `ends_at = starts_at`.**
    `events.ends_at` is `not null` and there is no all-day flag in the spec's DDL or in the
    table. Date-only serialisation is Phase 10's ICS feed's problem, not this phase's, and
    inventing an `all_day` column now would give Phase 10 two ways to say the same thing.
11. **A191 — the `user_settings` push `id` is the authenticated user's id.** The table's
    primary key *is* `user_id` and it has no `id` column, so the contract's `Mutation.id`
    has nothing else to be. A mutation whose `id` differs from the caller is rejected
    `validation`, and a payload carrying `user_id` is rejected rather than ignored: the
    field is not the client's to state, and silently dropping it would make a forged
    payload look accepted.
12. **A192 — `PUT /settings` is a partial write that writes the whole row, and a reminder
    channel the plan does not grant is refused per channel with 403 `plan_required`.**
    Partial, because the screen saves per section and a full replace would let a stale tab
    clobber a channel set changed on another device. Whole-row, because the roadmap gives
    `user_settings` record-level LWW and no field-level merge; pretending otherwise would
    need a per-field timestamp the table does not have. The channel check goes through
    `entitlements::require(Feature::ReminderChannel(c))` — never a
    `plans.get(..).reminder_channels` read, which the route sweep forbids and which would be
    a second source of truth for what a plan grants.
13. **A193 — quiet hours, `language` and `theme` join `user_settings`; the quiet window may
    wrap midnight.** `quiet_start_hour` / `quiet_end_hour` are nullable ints (a pair: both
    or neither) and the test is "inside the window", not `start < end`, because `22 → 7` is
    the normal case. `language` (`en | id`) and `theme` (`system | light | dark`) match the
    spec's DDL, which `0012` dropped; they belong on the synced settings row so a choice
    made on the phone is the choice the laptop uses.
14. **A194 — the timezone stays on `users`, and `PUT /settings` writes it there.**
    `users.timezone` predates any settings row, the reminder and planner math reads it for
    an account that may have no settings row yet, and a second copy on `user_settings` is
    exactly the drift the "local-date math only through `chrono-tz`" rule would not catch.
    `GET /settings` therefore returns it beside the row rather than inside it.
15. **A195 — `study_sessions` gets a plausibility bound, and the comparison is against the
    *client's* own report.** `1 <= duration_min <= 1440`; `started_at <= server_now + 24 h`
    (the same bound a review and an attempt already use); and
    `started_at + duration_min <= client_ts + 2 min`, where `client_ts` is the field the
    mutation contract already carries, reused by `POST /sessions`'s body. Comparing against
    the server's clock instead would reject a legitimate offline session from a device whose
    clock runs fast — rejecting valid data is its own defect. `docs/THREAT_MODEL.md` § 6 R1
    already names this exact fix ("a session cannot exceed elapsed wall time"); the addendum
    records that the bound now exists, because the Today screen and the streak display these
    numbers as facts.
16. **A196 — the Today view is computed on the client and there is no `GET /today` route.**
    The roadmap's gate is "the Today view works offline", and a server route cannot. The day
    buckets, the streak and the due counts are pure functions in
    `app/src/features/today/day.ts`, unit-tested against a fixed clock, mirroring
    `app/src/features/study/due.ts`.
17. **A197 — the focus timer's running state is device-local, and the session id is minted
    at *start*.** It is not a synced row (an unfinished session is not a fact) and not a
    task. `Platform.localState` holds `{id, started_at, task_id?, workspace_id?}`; the id
    survives the restart with the timer, so the flush after a restart is the *same*
    append-only id and `on conflict (id) do nothing` makes a double flush a no-op. That is
    the mechanical defence against "a timer that logs a session twice on restart".
18. **A198 — a timer found running after `TIMER_STALE_AFTER_MIN = 240` minutes is not
    auto-resumed.** The screen offers Save (the elapsed duration, clamped to 1440, editable)
    or Discard (which writes nothing). Auto-resuming an overnight timer would log a
    plausible-looking false session into the streak and the Today counts, which is a number
    the honesty gate says must trace to something real.
19. **A199 — the Study hub, reminders and the dashboard are not in this phase.** The
    roadmap's Phase 9 line names the Today view, the timer and settings; the Study hub's two
    cross-workspace list routes (`decks` with due badges, `quizzes` with scores) are new
    surface for a screen the phase is not gated on, reminders need the job engine and the
    push channels, and the dashboard is Phase 12's own line. Recorded here rather than left
    implicit, so the spec's § Today paragraph is accounted for item by item in the
    Self-Review.
20. **A200 — `PUT /settings` is `ReadOnly` in the route sweep, and the channel refusal is
    asserted in the settings suite.** A `Swept` row's refusal is driven by
    `Fixture::call`, which has no per-row body, so a swept row for `PUT /settings` could only
    assert what an *empty* body does — which is refuse nothing. The honest classification is
    `ReadOnly` with a reason that names the per-channel check, plus a real test in
    `crates/server/tests/settings.rs` that a free account's `telegram` is 403
    `plan_required`.

---

## File structure (new this phase)

```
migrations/0028_tasks_events_settings.sql   # tasks.position; user_settings quiet hours,
                                            # language, theme
prompts/quick_capture.v1.md                 # EDIT — the <user_context_data> block (A182)
prompts/task_breakdown.v1.md                # EDIT — same
crates/core/src/prompts/registry.rs         # + QuickCaptureV1, TaskBreakdownV1
crates/core/src/generation/capture.rs       # NEW — the draft contract
crates/core/src/generation/breakdown.rs     # NEW — the proposal contract
crates/core/src/generation/mod.rs           # + the two modules
crates/core/src/tasks.rs                    # NEW — order, tree, due window (pure)
crates/core/src/study.rs                    # NEW — the session bound (pure)
crates/core/src/lib.rs                      # + pub mod tasks; pub mod study;
crates/core/tests/prompts.rs                # + the two modules' contract cases
crates/core/tests/capture.rs                # NEW
crates/core/tests/breakdown.rs              # NEW
crates/core/tests/tasks.rs                  # NEW
crates/core/tests/study.rs                  # NEW
crates/server/src/tasks.rs                  # NEW — CRUD + quick-capture + breakdown
crates/server/src/events.rs                 # NEW — CRUD, server-owned fields
crates/server/src/study.rs                  # NEW — POST /sessions, GET /sessions,
                                            #       GET /activity
crates/server/src/settings.rs               # NEW — GET/PUT /settings
crates/server/src/sync/push.rs              # + the per-entity validator calls (Phase 8's)
crates/server/src/routes/mod.rs             # + four merges
crates/server/src/lib.rs                    # + the four modules
crates/server/tests/tasks.rs                # NEW
crates/server/tests/events.rs               # NEW
crates/server/tests/study.rs                # NEW
crates/server/tests/settings.rs             # NEW
crates/server/tests/offline_tasks.rs        # NEW — the phase's exit gate
crates/server/tests/route_sweep.rs          # + 14 rows and 2 drivers
crates/server/tests/tamper.rs               # + 3 push rows (A189, A191, A195)
app/src/platform/types.ts                   # + LocalStateApi
app/src/platform/{web,desktop,mobile}.ts    # + localState
src-tauri/src/cache/schema.rs               # + the local_state table
src-tauri/src/commands.rs                   # + local_state_get / local_state_set
app/src/api/queries/tasks.ts                # NEW
app/src/api/queries/events.ts               # NEW
app/src/api/queries/settings.ts             # NEW
app/src/features/billing/plans.ts           # + task_breakdown, and the export
app/src/features/today/day.ts               # NEW — the pure rules (A196)
app/src/features/today/day.test.ts          # NEW
app/src/features/today/useFocusTimer.ts     # NEW
app/src/features/today/useFocusTimer.test.ts# NEW
app/src/features/today/FocusTimer.tsx       # NEW
app/src/features/today/QuickCapture.tsx     # NEW
app/src/features/today/TodayScreen.tsx      # real
app/src/features/today/TodayScreen.test.tsx # NEW
app/src/features/tasks/TasksScreen.tsx      # NEW
app/src/features/tasks/BreakdownPanel.tsx   # NEW
app/src/features/tasks/TasksScreen.test.tsx # NEW
app/src/features/settings/SettingsScreen.tsx# NEW
app/src/features/settings/SettingsScreen.test.tsx # NEW
app/src/router.tsx                          # /tasks and /settings become real
app/src/i18n/locales/{en,id}.json           # the new keys, in both files
scripts/smoke-phase9.sh                     # NEW
docs/{API,SYNC,ARCHITECTURE,ASSUMPTIONS,THREAT_MODEL}.md
```

**Migration numbering.** `migrations/` holds `0001`–`0023` today; Phase 7's plan claims
`0025`–`0026` and Phase 8 claims `0027` and above. This phase therefore takes **`0028`**, and
the rule at execution time is to re-read `migrations/` before creating the file — a number
taken by a phase that landed in between is a rename, not a rewrite.

---

## Task 1: The two prompt modules — the data blocks, the registry, and the contract cases

**Files:**
- Modify: `prompts/quick_capture.v1.md`, `prompts/task_breakdown.v1.md`
- Modify: `crates/core/src/prompts/registry.rs`
- Test: `crates/core/tests/prompts.rs`

**Interfaces:**
- Consumes: `ModuleId`, `VarKind`, `TASK_BASE_SECTIONS`, `compose_task`, `UNTRUSTED_VARIABLES`.
- Produces: `ModuleId::QuickCaptureV1` and `ModuleId::TaskBreakdownV1`, reachable from
  `ModuleId::ALL` and from `compose_task`.

- [ ] **Step 1: Write the failing test**

The two sweeps already iterate `ModuleId::ALL`, so registering the variants without the
declarations fails them. Add the cases only these two modules can fail:

```rust
/// Both task modules have no sources, so `output_language` cannot default to
/// "the same language as the source material": a default would silently produce an
/// English breakdown of an Indonesian task. The caller states the language, from
/// `user_settings.language`.
#[test]
fn the_task_modules_require_a_language_rather_than_defaulting_to_the_source_language() {
    for module in [ModuleId::QuickCaptureV1, ModuleId::TaskBreakdownV1] {
        assert_eq!(
            variable_kind(module_variables(module), "output_language"),
            Some(VarKind::Caller),
            "{}: there are no sources to detect a language from",
            module.version()
        );
    }

    // `now` and `timezone` are filled from `ChatVars` by the composer, so a module
    // that uses them declares them without the caller passing anything. They must
    // still be declared, or the render leaves an unfilled `{{now}}` behind.
    for name in ["now", "timezone"] {
        assert_eq!(
            variable_kind(module_variables(ModuleId::QuickCaptureV1), name),
            Some(VarKind::Caller),
            "quick_capture.v1 uses {{{{{name}}}}}"
        );
    }
}

/// The documented schema in each module must parse into the Rust struct the server
/// deserialises, or the prompt and the contract have drifted apart.
#[test]
fn the_task_modules_document_the_schemas_their_contracts_parse() {
    let capture = /* the JSON block from prompts/quick_capture.v1.md */;
    let draft: stvdy_core::generation::capture::DraftFile =
        serde_json::from_str(capture).expect("the module's own example must parse");
    assert_eq!(draft.priority, 2);

    let breakdown = /* the JSON block from prompts/task_breakdown.v1.md */;
    let file: stvdy_core::generation::breakdown::BreakdownFile =
        serde_json::from_str(breakdown).expect("the module's own example must parse");
    assert_eq!(file.subtasks.len(), 1);
    assert_eq!(file.total_estimate_min, 180);
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-core --test prompts task_modules` →
`error[E0599]: no variant named QuickCaptureV1`.

- [ ] **Step 3: Edit the two prompt files (A182)**

Wrap every user-typed value in the block the composer's rule requires. In
`prompts/quick_capture.v1.md`:

```markdown
TASK: Turn the user's text into a task draft.

Current date/time: {{now}}
Timezone: {{timezone}}

<user_context_data>
User text: "{{capture_text}}"
</user_context_data>
```

and the same shape in `prompts/task_breakdown.v1.md` around `{{task_title}}`, `{{due_at}}`
and `{{notes}}` — one block, with `{{due_at}}` inside it too, because a date the student
typed ("besok") arrives as free text through the capture path. **Nothing else in either file
changes**: the rules, the schema line and the wording are the file's contract.

- [ ] **Step 4: Add the two variants**

In `crates/core/src/prompts/registry.rs`, following the existing shape exactly:

```rust
/// The quick-capture module. No sources: the input is one line the student typed.
pub const QUICK_CAPTURE_V1: &str = include_str!("../../../../prompts/quick_capture.v1.md");
/// The task-breakdown module. No sources: the input is a task the student wrote.
pub const TASK_BREAKDOWN_V1: &str = include_str!("../../../../prompts/task_breakdown.v1.md");

pub enum ModuleId {
    FlashcardsV1,
    QuizV1,
    ReportOutlineV1,
    ReportSectionV1,
    CheatsheetV1,
    QuickCaptureV1,
    TaskBreakdownV1,
}

impl ModuleId {
    pub const ALL: [ModuleId; 7] = [ /* …the five, then the two above… */ ];

    // version(): "quick_capture.v1", "task_breakdown.v1"
    // text():    the two new `include_str!`s
    // base_sections(): TASK_BASE_SECTIONS for both, like every strict-JSON module

    pub fn variables(self) -> &'static [(&'static str, VarKind)] {
        match self {
            // …the existing five arms unchanged…
            //
            // `now` and `timezone` are `Caller` and filled by `values_of(ChatVars)`,
            // so the caller passes nothing for them and they still cannot be left
            // unfilled. `output_language` is `Caller` rather than
            // `Optional(SOURCE_LANGUAGE)` because this module has no sources: see A183.
            ModuleId::QuickCaptureV1 => &[
                ("now", VarKind::Caller),
                ("timezone", VarKind::Caller),
                ("capture_text", VarKind::Caller),
                ("output_language", VarKind::Caller),
            ],
            ModuleId::TaskBreakdownV1 => &[
                ("task_title", VarKind::Caller),
                ("due_at", VarKind::Caller),
                ("notes", VarKind::Caller),
                ("output_language", VarKind::Caller),
            ],
        }
    }
}
```

`{{due_at}}` and `{{notes}}` are `Caller` and not `Optional`: `task_breakdown.v1.md` renders
`Due: {{due_at}}` and `Notes: {{notes}}` as lines the model reads, and the caller has the
values (a task with no due date has the string the caller chooses — Task 6 passes `"none"`,
which is a statement the prompt can read, not an absence).

- [ ] **Step 5: Run the tests**

`cargo test -p stvdy-core --test prompts` — the whole file, because the two sweeps are the
assertion that matters and a targeted run would not run them.

- [ ] **Step 6: Commit**

```
feat(core): the quick-capture and task-breakdown prompt modules
```

---

## Task 2: `crates/core/src/generation/capture.rs` — the draft contract

**Files:**
- Create: `crates/core/src/generation/capture.rs`
- Modify: `crates/core/src/generation/mod.rs`
- Test: `crates/core/tests/capture.rs`

**Interfaces:**
- Consumes: `GenerationError`, `strip_code_fence`, `chrono_tz::Tz`.
- Produces:

```rust
/// The model's reply to `quick_capture.v1`, before any interpretation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftFile {
    pub title: String,
    #[serde(default)] pub notes: Option<String>,
    #[serde(default)] pub due_at: Option<String>,
    #[serde(default = "default_priority")] pub priority: i32,
    #[serde(default)] pub estimate_min: Option<i32>,
}

/// The draft the route returns: a proposal, never a row (A184).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaptureDraft {
    pub title: String,
    pub notes: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub estimate_min: Option<i32>,
    /// One line per field the reply could not supply as asked (A185).
    pub warnings: Vec<String>,
}

/// `tz` is the student's zone, used only to *read* the reply's offset and to
/// interpret a date the model left naive — never to invent one.
pub fn parse_draft(raw: &str, tz: Tz) -> Result<CaptureDraft, GenerationError>;
```

- [ ] **Step 1: Write the failing test**

```rust
/// The reply's offset is kept, not re-derived. A model told the timezone resolves
/// "Jumat" to `2026-10-16T23:59:00+07:00`; reading that as UTC moves the deadline
/// seven hours and, at 23:59, a whole day.
#[test]
fn a_due_date_keeps_the_offset_the_model_resolved_it_in() {
    let raw = r#"{"title":"Selesaikan tugas kripto","notes":null,
                  "due_at":"2026-10-16T23:59:00+07:00","priority":1,"estimate_min":90}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(
        draft.due_at,
        Some("2026-10-16T16:59:00Z".parse::<DateTime<Utc>>().unwrap())
    );
    assert_eq!(draft.priority, 1);
    assert!(draft.warnings.is_empty());
}

/// An unreadable date is a warning, not a lost field and not a failed request (A185):
/// the credits are already spent by the time this runs.
#[test]
fn an_unreadable_due_date_becomes_a_warning_and_keeps_the_draft() {
    let raw = r#"{"title":"Baca bab 4","notes":null,"due_at":"friday-ish",
                  "priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("the draft still comes back");
    assert_eq!(draft.due_at, None);
    assert_eq!(draft.warnings.len(), 1);
    assert!(draft.warnings[0].contains("friday-ish"), "{:?}", draft.warnings);
}

/// A naive datetime is read in the student's zone, which is the one thing the model
/// was told and the one thing it most often drops.
#[test]
fn a_naive_due_date_is_read_in_the_students_zone() {
    let raw = r#"{"title":"Kuis","notes":null,"due_at":"2026-10-16T23:59:00",
                  "priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(
        draft.due_at,
        Some("2026-10-16T16:59:00Z".parse::<DateTime<Utc>>().unwrap())
    );
    assert_eq!(draft.warnings.len(), 1, "the assumption is stated, not silent");
}

/// An out-of-range priority or estimate is clamped and warned about. A priority of 0
/// or an estimate of 9000 minutes would reach the schema and fail the insert.
#[test]
fn out_of_range_numbers_are_clamped_with_a_warning() {
    let raw = r#"{"title":"Tugas","notes":null,"due_at":null,
                  "priority":7,"estimate_min":9000}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.priority, 2);
    assert_eq!(draft.estimate_min, Some(1440));
    assert_eq!(draft.warnings.len(), 2);
}

/// An empty title is a schema failure and therefore retryable, because a task with no
/// title is not a task and the second attempt costs one call rather than a broken row.
#[test]
fn an_empty_title_is_a_schema_failure() {
    let raw = r#"{"title":"   ","notes":null,"due_at":null,"priority":2,"estimate_min":null}"#;
    assert!(matches!(
        parse_draft(raw, jakarta()),
        Err(GenerationError::Schema(_))
    ));
    assert!(is_retryable(&parse_draft(raw, jakarta()).unwrap_err()));
}

/// A fenced reply still parses: a provider adds the fence despite the module saying not to.
#[test]
fn a_fenced_reply_still_parses() { /* ```json … ``` around the first case's body */ }
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-core --test capture` → `error[E0433]: failed to resolve: use of
undeclared crate or module 'capture'`.

- [ ] **Step 3: Write the module**

Parse with `strip_code_fence` → `serde_json::from_str::<DraftFile>` (a `Json` error, which
`is_retryable` already retries) → normalise:

- `title`: trim; empty ⇒ `GenerationError::Schema("the draft has no title")`.
- `due_at`: `DateTime::parse_from_rfc3339` first; on failure
  `NaiveDateTime::parse_from_str` with `%Y-%m-%dT%H:%M:%S` then `%Y-%m-%dT%H:%M` and
  `%Y-%m-%d` (midnight local); a naive value is converted with `tz.from_local_datetime(..).earliest()`
  and warned about; anything else is `None` + a warning naming the raw string.
- `priority`: clamp to `1..=3`, warn when clamped.
- `estimate_min`: clamp to `1..=1440`, warn when clamped; `Some(0)` and negatives become
  `None` + a warning (an estimate of zero minutes is not an estimate).
- `notes`: trim; an empty string becomes `None`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-core --test capture`

- [ ] **Step 5: Commit**

```
feat(core): the quick-capture draft contract, and what a bad date does
```

---

## Task 3: `crates/core/src/generation/breakdown.rs` — the proposal contract

**Files:**
- Create: `crates/core/src/generation/breakdown.rs`
- Modify: `crates/core/src/generation/mod.rs`
- Test: `crates/core/tests/breakdown.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakdownFile {
    pub subtasks: Vec<ProposedSubtask>,
    pub total_estimate_min: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedSubtask {
    pub title: String,
    pub estimate_min: i64,
}

/// The proposal, in the order the model wrote it. `position` is assigned here so the
/// order cannot fall through to a uuid when a batch is inserted (A187).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    pub subtasks: Vec<ProposedSubtask>,
    pub total_estimate_min: i64,
    pub warnings: Vec<String>,
}

pub const MIN_SUBTASKS: usize = 3;
pub const MAX_SUBTASKS: usize = 8;

pub fn parse_proposal(raw: &str) -> Result<Proposal, GenerationError>;
```

- [ ] **Step 1: Write the failing test**

```rust
/// The prompt asks for 3-8 subtasks in the 15-90 minute range, and the contract is what
/// makes that a contract. Fewer than three restates the parent; more than eight is a
/// project plan. A violation is a schema failure, so the one retry the server already
/// performs is what fixes it.
#[test]
fn a_proposal_outside_three_to_eight_subtasks_is_a_schema_failure() {
    assert!(matches!(parse_proposal(&file_of(2)), Err(GenerationError::Schema(_))));
    assert!(matches!(parse_proposal(&file_of(9)), Err(GenerationError::Schema(_))));
    assert!(parse_proposal(&file_of(3)).is_ok());
    assert!(parse_proposal(&file_of(8)).is_ok());
}

/// The declared total must equal the sum, and the sum is what the caller uses. A total
/// the model wrote that disagrees with its own subtasks is a number that cannot be
/// shown, so the reply is rejected and retried rather than displayed with the sum
/// silently substituted.
#[test]
fn a_total_that_disagrees_with_its_own_subtasks_is_a_schema_failure() {
    let mut raw = file_of(4);
    // …rewrite total_estimate_min to sum + 30…
    assert!(matches!(parse_proposal(&raw), Err(GenerationError::Schema(_))));
}

/// An estimate outside 15-90 minutes is a warning, not a failure: it is a judgement the
/// student can fix in the proposal list, and rejecting the whole reply over one number
/// costs a second model call for something a single keystroke fixes.
#[test]
fn an_out_of_range_estimate_is_warned_about_and_kept() {
    let proposal = parse_proposal(&/* 4 subtasks, one at 240 minutes */).expect("parses");
    assert_eq!(proposal.subtasks[2].estimate_min, 240);
    assert_eq!(proposal.warnings.len(), 1);
    assert!(proposal.warnings[0].contains("240"));
}

/// A subtask title that restates the parent task is the one thing the prompt explicitly
/// forbids and the one thing a model does anyway. It is dropped with a warning rather
/// than failing the reply, and dropping it cannot take the count below the minimum
/// because the minimum is checked after the drop.
#[test]
fn a_subtask_that_restates_its_parent_is_dropped_with_a_warning() {
    let proposal = parse_proposal(&/* 4 subtasks, the first titled "Selesaikan tugas kripto"
                                      for the parent "Selesaikan tugas kripto" */).unwrap();
    assert_eq!(proposal.subtasks.len(), 3);
    assert_eq!(proposal.warnings.len(), 1);
}

/// Duplicate titles are kept. Two subtasks can legitimately share a title ("Review"),
/// and dropping one would silently shorten the student's list.
#[test]
fn duplicate_subtask_titles_are_kept() { /* … */ }

/// The order is the model's. `position` is 0..n-1 in that order, which is what stops a
/// batch insert from coming back shuffled (A187, Phase 5's A100).
#[test]
fn the_proposal_order_is_the_models_order() {
    let proposal = parse_proposal(&file_of(5)).unwrap();
    let titles: Vec<&str> = proposal.subtasks.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Baca bab 1", "Baca bab 2", "Baca bab 3", "Baca bab 4", "Baca bab 5"]);
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-core --test breakdown`

- [ ] **Step 3: Write the module**

`parse_proposal(raw)` = `strip_code_fence` → `serde_json` → validate: non-empty title after
trim (else `Schema`); count in `3..=8` **after** dropping restatements of the parent — which
means the signature needs the parent title:

```rust
pub fn parse_proposal(raw: &str, parent_title: &str) -> Result<Proposal, GenerationError>;
```

(the test above calls it with the parent title; the earlier snippets elide it for length).
Total equality is compared after the drops, against the sum of what remains; a mismatch is
`Schema`. Estimates outside `15..=90` warn and are kept. Titles are compared to the parent
with a normalised form (trim, case-fold, collapse internal whitespace) — the same
normalisation Phase 5's MCQ grading uses.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-core --test breakdown`

- [ ] **Step 5: Commit**

```
feat(core): the task-breakdown proposal contract
```

---

## Task 4: `crates/core/src/tasks.rs` and `crates/core/src/study.rs` — the pure rules

**Files:**
- Create: `crates/core/src/tasks.rs`, `crates/core/src/study.rs`
- Modify: `crates/core/src/lib.rs`
- Test: `crates/core/tests/tasks.rs`, `crates/core/tests/study.rs`

**Interfaces:**
- Consumes: `chrono`, `chrono_tz::Tz`, `uuid::Uuid`.
- Produces:

```rust
// crates/core/src/tasks.rs
pub const MIN_PRIORITY: i32 = 1;
pub const MAX_PRIORITY: i32 = 3;
pub const MAX_ESTIMATE_MIN: i32 = 1440;
pub const MAX_TITLE: usize = 500;
pub const MAX_NOTES: usize = 20_000;

/// A task as the rules see it — no db, no serde, no clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub title: String,
    pub due_at: Option<DateTime<Utc>>,
    pub estimate_min: Option<i32>,
    pub priority: i32,
    pub position: i32,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus { Todo, Doing, Done }

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("a title is required")]
    TitleEmpty,
    #[error("the title is longer than {MAX_TITLE} characters")]
    TitleTooLong,
    #[error("priority must be between 1 and 3")]
    Priority,
    #[error("the estimate must be between 1 and 1440 minutes")]
    Estimate,
    #[error("a subtask cannot be its own parent")]
    SelfParent,
}

/// The one order the Tasks screen and the Today screen both use. Deterministic: the id
/// breaks every tie, because two tasks can share a due date to the second and a batch
/// insert shares `created_at` to the microsecond (A187).
pub fn order_tasks(rows: &mut [TaskRow]);

/// Live subtasks of `parent`, in `order_tasks` order, with their own children flattened
/// one level deep. One level is what the UI shows and what the spec asks for.
pub fn subtasks_of(rows: &[TaskRow], parent: Uuid) -> Vec<&TaskRow>;

/// The tasks due on the student's local calendar day `date_key` (`YYYY-MM-DD`), plus the
/// overdue ones. `due_at` is compared through `tz`, never through a fixed offset.
pub fn due_on(rows: &[TaskRow], tz: Tz, date_key: &str) -> Vec<&TaskRow>;

// crates/core/src/study.rs
pub const MIN_SESSION_MIN: i32 = 1;
pub const MAX_SESSION_MIN: i32 = 1440;
pub const SESSION_CLOCK_GRACE_MIN: i64 = 2;
pub const MAX_FUTURE_HOURS: i64 = 24;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("a session must be between 1 and 1440 minutes")]
    Duration,
    #[error("the session starts more than 24 hours in the future")]
    StartsInFuture,
    #[error("the session is longer than the time between its start and the device's clock")]
    BeyondElapsed,
}

/// A195: the bound, with the comparison against the *client's* own report.
pub fn check_session(
    started_at: DateTime<Utc>,
    duration_min: i32,
    client_ts: DateTime<Utc>,
    server_now: DateTime<Utc>,
) -> Result<(), SessionError>;
```

- [ ] **Step 1: Write the failing test**

`crates/core/tests/tasks.rs`:

```rust
/// A batch insert shares `created_at` to the microsecond, so ordering by time alone is a
/// random order. Position first, then time, then id (A187).
#[test]
fn tasks_with_the_same_position_and_timestamp_still_order_deterministically() {
    let mut rows = vec![row_at(0, "2026-10-11T00:00:00Z", 3), row_at(0, "2026-10-11T00:00:00Z", 1)];
    order_tasks(&mut rows);
    assert_eq!(rows[0].id, Uuid::from_u128(1));
}

/// Due first, then priority, then position: the student's next action is at the top, and
/// a task with no due date sorts after every dated one rather than to the top.
#[test]
fn the_order_is_due_then_priority_then_position_and_undated_tasks_come_last() { /* … */ }

/// `due_on` is a local-calendar question. An event or task at 2026-10-16T23:30+07:00 is on
/// the 16th in Jakarta and the 16th in UTC — but one at 2026-10-16T07:00+07:00 is the 16th
/// locally and the 15th at 23:00 UTC, which is the case a naive implementation gets wrong.
#[test]
fn due_on_uses_the_students_local_day_not_utc() {
    let task = task_due("2026-10-16T07:00:00+07:00");
    assert_eq!(due_on(&[task.clone()], jakarta(), "2026-10-16").len(), 1);
    assert_eq!(due_on(&[task], jakarta(), "2026-10-15").len(), 0);
}

/// A task due yesterday is still due today, and it is not "today's task" by date alone.
#[test]
fn overdue_tasks_are_included_and_marked() { /* `due_on` returns them; the caller labels them */ }

/// A deleted parent takes its subtree out of every list, and a deleted child never shows.
#[test]
fn a_deleted_row_is_absent_from_the_order_and_the_tree() { /* … */ }

/// Subtasks are one level: a grandchild is not returned as a child of the root.
#[test]
fn subtasks_of_returns_one_level() { /* … */ }
```

`crates/core/tests/study.rs`:

```rust
/// The normal case: five minutes of focus, reported when it ended.
#[test]
fn a_session_within_its_elapsed_time_is_accepted() {
    assert!(check_session(at("2026-10-11T09:00:00Z"), 5, at("2026-10-11T09:05:00Z"),
                          at("2026-10-11T09:06:00Z")).is_ok());
}

/// The timer's own failure mode: a device that was offline for a day reports a session
/// that ended before it pushed. The comparison is against `client_ts`, so the push time
/// is irrelevant.
#[test]
fn a_session_pushed_a_day_later_is_still_accepted() {
    assert!(check_session(at("2026-10-10T09:00:00Z"), 50, at("2026-10-10T09:50:00Z"),
                          at("2026-10-11T09:00:00Z")).is_ok());
}

/// A device whose clock runs two hours fast reports a start in the future and an end
/// "before" the server's now. Rejecting it would reject valid data, so the bound reads
/// the client's own clock (A195).
#[test]
fn a_device_whose_clock_runs_fast_does_not_lose_its_session() {
    assert!(check_session(at("2026-10-11T11:00:00Z"), 30, at("2026-10-11T11:30:00Z"),
                          at("2026-10-11T09:30:00Z")).is_ok());
}

/// The fabrication this bound exists for: 600 minutes claimed from a five-minute report.
#[test]
fn a_duration_longer_than_the_client_reported_elapsed_time_is_refused() {
    assert!(matches!(
        check_session(at("2026-10-11T09:00:00Z"), 600, at("2026-10-11T09:05:00Z"),
                      at("2026-10-11T09:06:00Z")),
        Err(SessionError::BeyondElapsed)
    ));
}

/// The boundaries: 1 and 1440 accepted, 0 and 1441 refused; 24 hours into the future
/// accepted, 25 refused — the same bound a review and an attempt already use.
#[test]
fn the_bounds_are_exact() { /* … */ }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-core --test tasks` and `cargo test -p stvdy-core --test study`

- [ ] **Step 3: Write both modules**

`tasks.rs` is pure data in, pure data out — no `serde_json`, no sqlx, no clock. The
`order_tasks` comparator is written once and both the server query's `ORDER BY` and the
client's sort must match it; the plan's Task 6 repeats it in SQL with a comment naming this
function, because the server sorts in the database and this sorts a vector.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-core --test tasks --test study`

- [ ] **Step 5: Commit**

```
feat(core): the task order, the due window, and the session bound
```

---

## Task 5: `migrations/0028_tasks_events_settings.sql`

**Files:**
- Create: `migrations/0028_tasks_events_settings.sql`

**Interfaces:**
- Produces: `tasks.position`; `user_settings.{quiet_start_hour, quiet_end_hour, language,
  theme}`.
- Does **not** produce: any `+sync` table, any trigger, any change to
  `0017_sync_triggers.sql` or `crates/server/tests/schema.rs`.

- [ ] **Step 1: Read `migrations/` and confirm the number**

`ls migrations/` — the highest on disk today is `0023`. Phase 7's plan claims `0025`–`0026`
and Phase 8 claims `0027`+. If `0028` is taken, take the next free number and update this
file's references to it.

- [ ] **Step 2: Write the migration**

```sql
-- Phase 9's three settings the table never had, and the column that stops a batch of
-- subtasks from coming back shuffled.
--
-- This migration adds no `+sync` table and attaches no trigger: `tasks` and
-- `user_settings` are already in `0017_sync_triggers.sql`'s arrays (and `user_settings`
-- in its own `log_change_by_user` pair), so that file stays the last migration that
-- attaches a trigger and `crates/server/tests/schema.rs` needs no new row. The precedent
-- is 0021-0023, which each say the same thing in their header.

-- A breakdown inserts 3-8 subtasks in one transaction, and a batch insert shares
-- `created_at` to the microsecond, so `order by created_at` is a random order (A187).
-- `position` is an ordinary client-writable field: the client numbers them 0..n-1 and
-- push carries it like any other.
alter table tasks
  add column position int not null default 0;

-- Quiet hours are a pair or neither: both null means "no quiet hours", and a window may
-- wrap midnight (`22 -> 7`), so the reader tests membership and never `start < end`.
alter table user_settings
  add column quiet_start_hour int,
  add column quiet_end_hour int,
  add constraint user_settings_quiet_hours_paired
    check ((quiet_start_hour is null) = (quiet_end_hour is null)),
  add constraint user_settings_quiet_hours_range
    check (quiet_start_hour is null
           or (quiet_start_hour between 0 and 23 and quiet_end_hour between 0 and 23)),
  -- The spec's DDL has these two on `user_settings`; 0012 dropped them. They are here
  -- rather than on `users` because a language chosen on the phone is the language the
  -- laptop should use, which is what the synced row is for.
  add column language text not null default 'en',
  add column theme text not null default 'system',
  add constraint user_settings_language check (language in ('en', 'id')),
  add constraint user_settings_theme check (theme in ('system', 'light', 'dark'));

-- The study-hours and block-length columns already have defaults and no bounds; a value
-- out of range reaches the planner in Phase 11 as a negative window. The bounds are added
-- here, where the columns are, rather than as a validation the caller must remember.
alter table user_settings
  add constraint user_settings_hours
    check (study_start_hour between 0 and 23
           and study_end_hour between 1 and 24
           and study_start_hour < study_end_hour),
  add constraint user_settings_limits
    check (max_study_min_per_day between 15 and 1440
           and block_len_min between 5 and 180
           and break_min between 0 and 60);
```

- [ ] **Step 3: Apply it and check the two assertions**

`sqlx migrate run` then `cargo test -p stvdy-server --test schema` — that suite asserts
exactly the `0017` table list carries both triggers, which is the assertion that would fail
if this migration had added one.

- [ ] **Step 4: Commit**

```
feat(db): subtask order, quiet hours, and the settings the spec's DDL had
```

---

## Task 6: `crates/server/src/tasks.rs` — CRUD, quick capture, break-down

**Files:**
- Create: `crates/server/src/tasks.rs`
- Modify: `crates/server/src/routes/mod.rs` (`+ .merge(tasks::routes())`),
  `crates/server/src/lib.rs`
- Test: `crates/server/tests/tasks.rs`

**Interfaces:**
- Consumes: `entitlements::{require, reserve_in}`, `credits::spend_in`, `AppState`,
  `AuthUser`, `stvdy_core::tasks::*`, `generation::capture`, `generation::breakdown`,
  `prompts::compose_task`, `prompts::registry::ModuleId`.
- Produces:

```rust
pub fn routes() -> Router<AppState>;

/// The payload validators, shared by these routes and by `sync::push::decide` (Task 10).
/// One implementation, two callers: a second one would be a second answer to "may this
/// row exist".
pub fn validate_task(payload: &serde_json::Value, is_create: bool) -> Result<NewTask, Reject>;
pub struct Reject { pub code: &'static str, pub message: String }
```

Routes (paths exactly as `docs/API.md:778` reserves them):

| Route | Notes |
|---|---|
| `GET /tasks` | `?status=&workspace_id=&parent_id=&due_before=&due_after=&limit=` (default 200, max 500). Tombstones excluded, `user_id` scoped. `ORDER BY` matches `stvdy_core::tasks::order_tasks`, with a comment naming it. |
| `POST /tasks` | Body may carry `id` (A8). 201 `{task}`. 409 when that id exists. |
| `PATCH /tasks/:id` | Partial; 200 `{task}`; 404 for another user's or a deleted row. |
| `DELETE /tasks/:id` | 204. Tombstones the task **and its live descendants** in one transaction (A188). |
| `POST /tasks/quick-capture` | Body `{text}`. `require(Feature::QuickCapture)` then one `reserve_in` transaction. 200 `{draft, credits_spent, credits_remaining, prompt_version}`. Writes **no task** (A184). |
| `POST /tasks/:id/breakdown` | Body `{}`. `require(Feature::TaskBreakdown)` then `reserve_in`. 200 `{proposal, credits_spent, credits_remaining, prompt_version}`. Writes **no task** (A186). |

- [ ] **Step 1: Write the failing test**

```rust
/// A free account has `quick_capture = false` in the matrix, so the refusal is the engine's
/// and not a literal in this file.
#[sqlx::test(migrations = "../../migrations")]
async fn quick_capture_is_refused_on_a_plan_that_does_not_grant_it(pool: PgPool) {
    // 403 { code: "plan_required", min_plan: "pro", message: non-empty }
}

/// The credits are spent and the draft comes back, and the task count is unchanged. This
/// is the assertion the "a draft that saves without confirmation" risk is closed by: a
/// capture that inserted a row would pass every other test in this file.
#[sqlx::test(migrations = "../../migrations")]
async fn a_capture_returns_a_draft_and_writes_no_task(pool: PgPool) {
    // pro account, stub LLM returns the module's own example
    // 200, draft.title non-empty, credits_spent == 2
    // `select count(*) from tasks where user_id = $1` == 0
    // `select count(*) from usage_events where action = 'quick_capture'` == 1
}

/// The prompt the model received carries the student's zone and the current instant, and
/// the captured text inside a `<user_context_data>` block.
#[sqlx::test(migrations = "../../migrations")]
async fn the_capture_prompt_carries_the_zone_the_clock_and_a_data_block(pool: PgPool) {
    // read ScriptedLlm::requests()[0], assert it contains "Asia/Jakarta",
    // a line "<user_context_data>", and no unfilled "{{"
}

/// A breakdown writes no subtasks: accepting is a separate act (A186).
#[sqlx::test(migrations = "../../migrations")]
async fn a_breakdown_returns_a_proposal_and_writes_no_subtasks(pool: PgPool) {
    // one task exists; breakdown; assert 200, proposal.subtasks.len() >= 3,
    // and `select count(*) from tasks where parent_id = $1` == 0
}

/// Accepting is N ordinary creates, and each one is a real row with the parent and its
/// position, in the order proposed (A187).
#[sqlx::test(migrations = "../../migrations")]
async fn accepting_a_proposal_creates_ordered_subtasks(pool: PgPool) {
    // POST /tasks x3 with parent_id and position 0,1,2
    // GET /tasks?parent_id=… returns them in 0,1,2 order with distinct positions
}

/// A task whose title is 500 characters is accepted; 501 is 400 validation.
#[sqlx::test(migrations = "../../migrations")]
async fn the_title_bound_is_exact(pool: PgPool) { /* … */ }

/// Deleting a parent removes its live children from every read, in one transaction.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_parent_tombstones_its_children(pool: PgPool) {
    // parent + 2 children; DELETE parent; assert both children have deleted_at not null
    // and that `change_log` carries a delete row for each child (A188)
}

/// Another account's task is a 404, never a 403: the existence of a row is not the
/// caller's business.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_task_is_not_found(pool: PgPool) { /* … */ }

/// The prompt's settings come from the account, not from the request: a caller cannot
/// make the model answer in a language they did not choose, and `{{output_language}}` is
/// never left unfilled.
#[sqlx::test(migrations = "../../migrations")]
async fn the_breakdown_language_comes_from_the_settings_row(pool: PgPool) {
    // set user_settings.language = 'id'; assert the prompt contains "id"
}
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test tasks` → the module does not exist.

- [ ] **Step 3: Write the module**

The two generative routes follow the shape `crates/server/src/cheatsheets/routes.rs` already
uses: `require` first (a counted refusal must be the one the caller sees), then
`reserve_in` with `feature: None` (the credits are the whole gate for a feature the matrix
prices but does not count), inside which the model is called **after** the transaction
commits — the provider call must not hold the advisory lock. Concretely: reserve and spend
in one transaction, commit, then call the model, then `record_usage` with the tokens and
provider cost against the returned `usage_event_id`, then attach the prompt version. If the
model fails, `credits::refund_in` the reservation exactly as the generators do, so a
provider failure is not a charge for nothing.

`POST /tasks/quick-capture` composes `ModuleId::QuickCaptureV1` with
`base: ChatVars { now: the clock, timezone: users.timezone, … }` and module vars
`{"capture_text": text, "output_language": settings.language}`. `now` and `timezone` are
declared `Caller` but filled from `values_of(ChatVars)`, so they are not passed twice.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test tasks`

- [ ] **Step 5: Commit**

```
feat(server): tasks, quick capture, and a breakdown that saves nothing
```

---

## Task 7: `crates/server/src/events.rs` — CRUD and the server-owned fields

**Files:**
- Create: `crates/server/src/events.rs`
- Modify: `crates/server/src/routes/mod.rs`, `crates/server/src/lib.rs`
- Test: `crates/server/tests/events.rs`

**Interfaces:**
- Produces:

```rust
pub fn routes() -> Router<AppState>;
pub fn validate_event(payload: &serde_json::Value, is_create: bool) -> Result<NewEvent, Reject>;
```

Routes:

| Route | Notes |
|---|---|
| `GET /events?from=&to=` | `from` required; `to` defaults to `from + 7 days`. `ORDER BY starts_at asc, id asc`. Overlap, not containment: an event that started yesterday and ends tomorrow is in today's window. |
| `POST /events` | `kind` in `class | exam | study_block | deadline | external`; `ends_at >= starts_at`; `source` accepted only as `"app"`. |
| `PATCH /events/:id` | Partial. A `source != 'app'` row is **not editable at all**: 403 `forbidden` with a message naming the calendar, because Phase 10's Google/ICS rows are read-only in the UI and a 404 would read as "deleted". |
| `DELETE /events/:id` | 204 tombstone; same rule for a non-`app` row. |

- [ ] **Step 1: Write the failing test**

```rust
/// The columns Phase 10 fills are not the client's to write (A189). Each one is its own
/// case, because a validator that checked only `source` would pass a payload that sets
/// `external_id` and silently break the next incremental sync's `(connection_id,
/// external_id)` match.
#[sqlx::test(migrations = "../../migrations")]
async fn a_client_cannot_set_the_columns_the_calendar_sync_owns(pool: PgPool) {
    for body in [
        json!({ "source": "google", … }),
        json!({ "external_id": "abc123", … }),
        json!({ "etag": "W/\"1\"", … }),
        json!({ "connection_id": Uuid::now_v7(), … }),
    ] {
        // 400 { code: "validation" }
    }
    // and `source: "app"` with the other three absent is 201
}

/// A Google-sourced event is readable and not writable, and the refusal says why.
#[sqlx::test(migrations = "../../migrations")]
async fn an_externally_sourced_event_cannot_be_edited_or_deleted(pool: PgPool) {
    // insert with source='google' directly; PATCH → 403 forbidden; DELETE → 403
    // GET /events still returns it
}

/// `ends_at >= starts_at`; a deadline with equal ends is valid (A190).
#[sqlx::test(migrations = "../../migrations")]
async fn a_deadline_is_an_event_with_equal_ends(pool: PgPool) {
    // POST { kind: "deadline", starts_at: X, ends_at: X } → 201
    // POST { kind: "deadline", starts_at: X, ends_at: X - 1min } → 400 validation
}

/// The window is an overlap, not a containment: the class that started yesterday and ends
/// tomorrow is on today's timeline, and a query that required containment would drop it
/// from the Today view with nothing on the page to say why.
#[sqlx::test(migrations = "../../migrations")]
async fn the_window_is_an_overlap(pool: PgPool) { /* … */ }

/// A kind outside the five is refused rather than stored as a string the UI cannot render.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_kind_is_refused(pool: PgPool) { /* … */ }

/// A task link survives, and deleting the task nulls the event's link rather than
/// deleting the event (`on delete set null`).
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_linked_task_keeps_the_event(pool: PgPool) { /* … */ }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test events`

- [ ] **Step 3: Write the module**

`validate_event` is the single place the field rules live; the REST handler and Task 10's
push decision both call it. The `PATCH`/`DELETE` guard for a non-`app` row is a read of the
row's `source` inside the same transaction as the update, not a separate check-then-write,
so a Phase 10 sync landing between them cannot be overwritten.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test events`

- [ ] **Step 5: Commit**

```
feat(server): events, and the four columns a client may not write
```

---

## Task 8: `crates/server/src/study.rs` — the sessions, and the reads the Today view needs

**Files:**
- Create: `crates/server/src/study.rs`
- Modify: `crates/server/src/routes/mod.rs`, `crates/server/src/lib.rs`
- Test: `crates/server/tests/study.rs`

**Interfaces:**
- Consumes: `stvdy_core::study::check_session`, `util::local_day_start`.
- Produces:

```rust
pub fn routes() -> Router<AppState>;
pub fn validate_session(payload: &serde_json::Value) -> Result<NewSession, Reject>;
```

Routes:

| Route | Notes |
|---|---|
| `POST /sessions` | Body `{id, started_at, duration_min, task_id?, workspace_id?, client_ts}` — the mutation's own shape, so one validator serves both callers (A195). Append-only: `on conflict (id) do nothing`, response `{applied: bool}` (the quiz-attempt precedent). |
| `GET /sessions?from=&to=` | The Today timeline and "time studied today". `ORDER BY started_at desc, id desc`. |
| `GET /activity?from=&to=` | `{reviews: [ts], attempts: [ts], sessions: [ts]}` — the three instants a streak is made of, so the *client* owns the streak rule and there is one implementation of it (A196). The web build has no local cache, which is why this read exists as a route at all. |

- [ ] **Step 1: Write the failing test**

```rust
/// The normal path: a session is recorded once and reports `applied`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_is_recorded_once_and_reports_applied(pool: PgPool) {
    // POST with a client id → 201 {applied: true}; replay the same id → {applied: false};
    // `select count(*) from study_sessions where id = $1` == 1
}

/// The fabrication A195 exists for, and the reason the bound is not a client check.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_longer_than_the_client_reported_elapsed_time_is_refused(pool: PgPool) {
    // started_at 09:00, duration 600, client_ts 09:05 → 400 validation
}

/// A legitimate offline session arrives hours later and is accepted, because the bound
/// reads `client_ts` and not the server's clock.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_pushed_hours_later_is_accepted(pool: PgPool) { /* … */ }

/// `activity` reports the three instants and nothing else, in one round trip.
#[sqlx::test(migrations = "../../migrations")]
async fn activity_reports_reviews_attempts_and_sessions(pool: PgPool) {
    // seed one review, one attempt, one session; assert three arrays of one, and that a
    // row outside the window is absent
}

/// Another account's session is not in the window, and a session for another account's
/// task is a 404 rather than a foreign key violation surfacing as a 500.
#[sqlx::test(migrations = "../../migrations")]
async fn sessions_are_scoped_and_a_foreign_task_is_not_found(pool: PgPool) { /* … */ }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test study`

- [ ] **Step 3: Write the module**

`validate_session` parses the payload into `NewSession` and runs `check_session` with
`server_now = state.clock.now()`. The insert is
`insert … on conflict (id) do nothing returning id` and `applied = row.is_some()`.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test study`

- [ ] **Step 5: Commit**

```
feat(server): study sessions, the bound on their length, and the activity read
```

---

## Task 9: `crates/server/src/settings.rs` — settings, quiet hours, and the plan's channels

**Files:**
- Create: `crates/server/src/settings.rs`
- Modify: `crates/server/src/routes/mod.rs`, `crates/server/src/lib.rs`
- Test: `crates/server/tests/settings.rs`

**Interfaces:**
- Consumes: `entitlements::require`, `Feature::ReminderChannel`, `stvdy_core::config::ReminderChannel`.
- Produces:

```rust
pub fn routes() -> Router<AppState>;

/// The settings row, created on first read with the column defaults, so a read never has
/// to answer "no settings yet" and a write never has to decide whether to insert.
pub async fn load_or_create(pool: &PgPool, user_id: Uuid) -> AppResult<SettingsRow>;
```

Routes:

| Route | Notes |
|---|---|
| `GET /settings` | `{ settings: { study_start_hour, study_end_hour, max_study_min_per_day, block_len_min, break_min, reminder_channels, quiet_start_hour, quiet_end_hour, language, theme, updated_at }, timezone }` (A194). |
| `PUT /settings` | Partial; only supplied fields change; the whole row is written, so `user_settings` stays record-level LWW (A192). Each supplied channel goes through `require(Feature::ReminderChannel(c))` **before** the write. `timezone` is written to `users`. |

`PUT /settings/api-key` and `DELETE /settings/api-key` stay in `docs/API.md` tagged
Phase 13 (BYOK is Power's, and the key store is `user_api_keys`, which this phase does not
touch).

- [ ] **Step 1: Write the failing test**

```rust
/// A free plan's channels are `[web, push]`, so `telegram` is 403 `plan_required` with
/// `min_plan: "power"` — the only plan that grants it. The check is the engine's; a
/// `plans.get(..).reminder_channels` read here would fail the route sweep's third test
/// and be a second source of truth (A192).
#[sqlx::test(migrations = "../../migrations")]
async fn a_channel_the_plan_does_not_grant_is_refused_with_the_plan_that_does(pool: PgPool) {
    // free account: PUT { reminder_channels: ["web", "telegram"] } → 403 plan_required,
    // min_plan == "power"; and the row is unchanged (the refusal precedes the write)
}

/// The granted set saves and comes back, in the order the client sent.
#[sqlx::test(migrations = "../../migrations")]
async fn a_granted_channel_set_saves(pool: PgPool) { /* pro: [web, push, email] → 200 */ }

/// Quiet hours wrap midnight, so `22 -> 7` is valid and the *membership* test is what a
/// consumer uses. A naive `start < end` check rejects the normal case (A193).
#[sqlx::test(migrations = "../../migrations")]
async fn quiet_hours_may_wrap_midnight(pool: PgPool) {
    // PUT { quiet_start_hour: 22, quiet_end_hour: 7 } → 200
    // PUT { quiet_start_hour: 22 } alone → 400 validation (the pair is both or neither)
    // PUT { quiet_start_hour: 24 } → 400 validation
}

/// A partial write leaves everything it did not mention alone, and a second write with
/// one field does not reset the rest — the failure a full-replace PUT would cause when a
/// stale tab saves one section.
#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_write_does_not_reset_the_rest(pool: PgPool) { /* … */ }

/// The timezone is on `users` and this route is the only writer (A194), so it round-trips
/// and an unknown zone is refused rather than stored as a string `chrono-tz` will silently
/// read as UTC.
#[sqlx::test(migrations = "../../migrations")]
async fn the_timezone_round_trips_and_an_unknown_zone_is_refused(pool: PgPool) {
    // PUT { timezone: "Asia/Jakarta" } → 200, GET returns it
    // PUT { timezone: "Mars/Olympus" } → 400 validation
}

/// The out-of-range values the migration's constraints would reject reach the caller as a
/// 400 with a code, never as a 500 from a constraint violation.
#[sqlx::test(migrations = "../../migrations")]
async fn out_of_range_study_hours_are_a_validation_error(pool: PgPool) {
    // study_start_hour 23 with study_end_hour 7 → 400 validation, not 500
}

/// A read creates the row rather than answering "nothing", so the screen has one shape.
#[sqlx::test(migrations = "../../migrations")]
async fn a_first_read_returns_the_defaults(pool: PgPool) { /* … */ }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test settings`

- [ ] **Step 3: Write the module**

Validate before writing, so the migration's `check` constraints are a backstop rather than
the error path. `language` and `theme` are validated against their `in (…)` sets in Rust
for the same reason.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test settings`

- [ ] **Step 5: Commit**

```
feat(server): settings, quiet hours, and a channel the plan grants
```

---

## Task 10: The push path — the validators, the sweep rows, and the tamper rows

**Files:**
- Modify: `crates/server/src/sync/push.rs` (Phase 8's file: one call per entity)
- Modify: `crates/server/tests/route_sweep.rs` (14 rows, 2 drivers, 1 fixture row)
- Modify: `crates/server/tests/tamper.rs` (3 rows)
- Test: the two suites above

**Interfaces:**
- Consumes: `tasks::validate_task`, `events::validate_event`, `study::validate_session`, the
  `user_settings` rules.
- Produces: `decide` rejects a payload the entity's validator rejects, with the validator's
  code.

- [ ] **Step 1: Write the failing tests**

In `crates/server/tests/route_sweep.rs`, add the rows. `read_only_rows()` gains twelve:

```rust
ro("GET", "/tasks", "reads the caller's own tasks"),
ro("POST", "/tasks", "creating a task spends nothing"),
ro("PATCH", "/tasks/{id}", "editing a task spends nothing"),
ro("DELETE", "/tasks/{id}", "deleting a task spends nothing"),
ro("GET", "/events", "reads the caller's own events"),
ro("POST", "/events", "creating an event spends nothing"),
ro("PATCH", "/events/{id}", "editing an event spends nothing"),
ro("DELETE", "/events/{id}", "deleting an event spends nothing"),
ro("GET", "/sessions", "reads the caller's own sessions"),
ro("POST", "/sessions", "a session is free (R1), like a review"),
ro("GET", "/activity", "reads the caller's own activity"),
ro("GET", "/settings", "reads the caller's own settings"),
ro(
    "PUT",
    "/settings",
    "a channel the plan does not grant is refused per channel by the engine, which \
     crates/server/tests/settings.rs asserts with a body (A200)",
),
```

and `swept_rows()` gains two:

```rust
swept("POST", "/tasks/quick-capture", "QuickCapture", 403, "plan_required"),
swept("POST", "/tasks/{id}/breakdown", "TaskBreakdown", 403, "plan_required"),
```

`Fixture` gains a `task_id` (one `insert into tasks (id, user_id, title)` in `build`), and
`Fixture::call` gains the two driver arms:

```rust
("POST", "/tasks/quick-capture") => {
    json(app, "POST", "/tasks/quick-capture", token,
         serde_json::json!({ "text": "finish crypto assignment friday" })).await
}
("POST", "/tasks/{id}/breakdown") => {
    json(app, "POST", &format!("/tasks/{task}/breakdown"), token,
         serde_json::json!({})).await
}
```

In `crates/server/tests/tamper.rs`, three rows that only the push path can prove:

```rust
/// A forged external event through push, which is the path a patched client would use
/// because it skips the REST validator's `403` (A189).
async fn a_pushed_event_with_a_google_source_is_rejected(/* … */) {
    // Rejected { code: "read_only_entity" }
}

/// The settings row's primary key is the user's id, so a mutation whose `id` is someone
/// else's is refused rather than applied to the caller (A191).
async fn a_pushed_settings_row_for_another_user_id_is_rejected(/* … */) {
    // Rejected { code: "validation" }
}

/// The session bound holds on the push path, which is the path an offline timer uses
/// (A195).
async fn a_pushed_session_beyond_its_elapsed_time_is_rejected(/* … */) {
    // Rejected { code: "validation" }
}
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test route_sweep` → "classify these routes in ROUTES …".
`cargo test -p stvdy-server --test tamper` → the three new cases fail on the rejection code.

- [ ] **Step 3: Add the calls to `decide`**

In `crates/server/src/sync/push.rs`, the per-entity arm calls the entity's validator before
applying, and maps a `Reject` to `MutationResult::Rejected { code, message }`:

```rust
// The validators live with the entities (tasks.rs, events.rs, study.rs, settings.rs) and
// this is their second caller: the REST routes are the first. One implementation, so a
// rule cannot hold on one path and not the other.
Entity::Task => crate::tasks::validate_task(payload, op == Op::Upsert)?,
Entity::Event => crate::events::validate_event(payload, op == Op::Upsert)?,
Entity::StudySession => crate::study::validate_session(payload)?,
Entity::UserSettings => crate::settings::validate_settings_push(user_id, id, payload)?,
```

The `study_sessions` validator needs the mutation's `client_ts`, which `decide` already has
in hand — pass it rather than reading a clock (A195).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test route_sweep --test tamper`

- [ ] **Step 5: Commit**

```
feat(server): the push path validates what the REST path validates
```

---

## Task 11: The offline gate — `crates/server/tests/offline_tasks.rs`

**Files:**
- Create: `crates/server/tests/offline_tasks.rs`
- Test: itself

**Interfaces:**
- Consumes: Phase 8's `stvdy_sync::client::{engine, transport}` and `Transport`.
- Produces: the phase's exit gate, as a test.

This task is written **before** the SPA work on purpose. The roadmap's gate is about the
engine, and a UI test that passes because the screen happens to be online is exactly the
failure the gate exists to catch.

- [ ] **Step 1: Write the test**

```rust
/// The phase's exit gate: a task, an event and a session written with the network down
/// sync after reconnect, and the Today view's data is all there.
///
/// Three assertions make this a gate rather than a demonstration, and each closes a way
/// the test could pass without the feature working:
///
///  1. **The server has nothing before the reconnect.** Read the database directly. A
///     "offline" write that silently went through REST would otherwise pass the rest of
///     the test unchanged.
///  2. **The ids match after the reconnect.** The client mints them (A8/A197), so equality
///     is what proves the row the device shows is the row the server holds, not a
///     same-shaped row the server created.
///  3. **A second `sync_now` changes nothing.** Replaying the outbox is the normal case
///     after a lost response, and a second row would double-count a session.
#[sqlx::test(migrations = "../../migrations")]
async fn a_task_an_event_and_a_session_written_offline_sync_after_reconnect(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "offline@example.test").await;

    let mut a = Engine::new(/* device A over a transport that is offline */);
    let mut b = Engine::new(/* device B, online */);
    a.bootstrap().await;  // first sync while online, so A has a cursor and a user
    b.bootstrap().await;

    a.go_offline();
    let task_id = Uuid::now_v7();
    a.write(Entity::Task, task_id, json!({
        "title": "Selesaikan tugas kripto", "due_at": "2026-10-16T16:59:00Z",
        "priority": 1, "position": 0
    })).await;
    let event_id = Uuid::now_v7();
    a.write(Entity::Event, event_id, json!({
        "kind": "exam", "title": "UAS Jaringan", "starts_at": …, "ends_at": …, "source": "app"
    })).await;
    let session_id = Uuid::now_v7();
    a.write(Entity::StudySession, session_id, json!({
        "started_at": …, "duration_min": 50, "client_ts": …
    })).await;

    // 1. The UI's promise, before any network: all three are readable on the device.
    assert_eq!(a.read(Entity::Task, &filter_for(task_id)).await.len(), 1);
    assert_eq!(a.read(Entity::Event, &filter_for(event_id)).await.len(), 1);
    assert_eq!(a.read(Entity::StudySession, &filter_for(session_id)).await.len(), 1);

    // 2. And the server has nothing: the write really was local.
    for (table, id) in [("tasks", task_id), ("events", event_id), ("study_sessions", session_id)] {
        let count: i64 = sqlx::query_scalar(&format!("select count(*) from {table} where id = $1"))
            .bind(id).fetch_one(&harness.state.pool).await.unwrap();
        assert_eq!(count, 0, "{table} must not have the row before the reconnect");
    }

    a.go_online();
    a.sync_now().await;
    a.sync_now().await;  // 3. the replay

    for (table, id) in [("tasks", task_id), ("events", event_id), ("study_sessions", session_id)] {
        let count: i64 = sqlx::query_scalar(&format!("select count(*) from {table} where id = $1"))
            .bind(id).fetch_one(&harness.state.pool).await.unwrap();
        assert_eq!(count, 1, "{table} must hold exactly the row the device minted");
    }

    // The other device sees all three, which is the product's whole premise.
    b.sync_now().await;
    assert!(b.read(Entity::Task, &filter_for(task_id)).await.len() == 1);
    assert!(b.read(Entity::Event, &filter_for(event_id)).await.len() == 1);
    assert!(b.read(Entity::StudySession, &filter_for(session_id)).await.len() == 1);

    // A delta pull carries the three rows and nothing else: no cross-tenant leak.
    let changes = b.changes_since(cursor_before).await;
    assert_eq!(changes.len(), 3, "{changes:#?}");
}

/// Deleting a parent offline removes its children locally too, or the device shows
/// orphaned subtasks until the next pull (A188).
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_parent_offline_tombstones_its_children_locally(pool: PgPool) {
    // parent + 2 subtasks created offline, synced, then the parent deleted offline:
    // assert `a.read(Entity::Task, &filter_for(child))` is empty on the device
}

/// The timer's own failure mode: the same session id flushed twice writes one row (A197).
#[sqlx::test(migrations = "../../migrations")]
async fn flushing_the_same_session_id_twice_writes_one_row(pool: PgPool) {
    // a.write(StudySession, id, …) twice while offline, then sync:
    // `select count(*) from study_sessions where id = $1` == 1
}

/// An online write through REST and an offline write of the same row produce the same
/// change_log entry and the same LWW outcome, which is what makes the two paths one path.
#[sqlx::test(migrations = "../../migrations")]
async fn the_rest_path_and_the_push_path_agree_on_the_log(pool: PgPool) {
    // POST /tasks on device B; assert a's pull carries exactly one upsert for that id,
    // and that the entity name is "tasks" in both cases
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-server --test offline_tasks` — the engine API from Phase 8 is present
and the routes are not, so the first failure is the REST route the test's second half uses.

- [ ] **Step 3: Make it pass**

No production code is expected from this task: it is the assertion that Tasks 5–10 and Phase
8 together do what the roadmap's gate says. Anything it finds is a defect in one of those,
fixed there, with the fix named in the execution notes.

- [ ] **Step 4: Run it**

`cargo test -p stvdy-server --test offline_tasks`

- [ ] **Step 5: Commit**

```
test(server): the offline task, event and session sync after reconnect
```

---

## Task 12: The SPA data layer, `Platform.localState`, and the focus timer

**Files:**
- Modify: `app/src/platform/types.ts`, `app/src/platform/{web,desktop,mobile}.ts`
- Modify: `src-tauri/src/cache/schema.rs`, `src-tauri/src/commands.rs`
- Create: `app/src/api/queries/{tasks,events,settings}.ts`
- Modify: `app/src/features/billing/plans.ts`
- Create: `app/src/features/today/useFocusTimer.ts`, `app/src/features/today/FocusTimer.tsx`
- Test: `app/src/features/today/useFocusTimer.test.ts`, `app/src/features/billing/plans.test.ts`

**Interfaces:**
- Produces:

```ts
// app/src/platform/types.ts — the capability the timer needs, beside Phase 8's `sync`.
export interface LocalStateApi {
  /** Reads a JSON value by key, or null. Never throws for a missing key. */
  get<T>(key: string): Promise<T | null>
  set(key: string, value: unknown): Promise<void>
  delete(key: string): Promise<void>
}

export interface Platform {
  // …existing members…
  localState: LocalStateApi
}
```

```ts
// app/src/features/today/useFocusTimer.ts
export interface RunningTimer {
  /** Minted at start and persisted with the timer, so a restart resumes the same
   *  append-only session id and a double flush is a no-op (A197). */
  id: string
  startedAt: string
  taskId: string | null
  workspaceId: string | null
}
export type TimerState =
  | { kind: 'idle' }
  | { kind: 'running'; timer: RunningTimer; elapsedMin: number }
  | { kind: 'stale'; timer: RunningTimer; elapsedMin: number }

export const TIMER_STALE_AFTER_MIN = 240
export function useFocusTimer(): {
  state: TimerState
  start(taskId?: string | null): Promise<void>
  stop(): Promise<void>
  discard(): Promise<void>
  saveStale(durationMin: number): Promise<void>
}
```

- [ ] **Step 1: Write the failing tests**

```ts
/// The session id is minted at start and persisted, so a restart resumes the same id.
/// This is the assertion the "logs a session twice on restart" risk is closed by: a
/// fresh id on every resume would pass every other test in this file.
it('resumes the same session id after a remount', async () => {
  // start; unmount; re-mount with the same local state; stop
  // the written payload's id === the id the first mount started with
})

/// Stopping writes exactly one session, with the elapsed minutes and the task link.
it('stopping writes one session with the elapsed minutes', async () => {
  // fake timers advance 12 minutes; stop; assert one write with duration_min === 12
})

/// A second flush of the same timer writes nothing: the id is already in the outbox.
it('a double stop writes once', async () => { /* … */ })

/// A timer found running after four hours is not auto-resumed: the screen offers Save or
/// Discard, because auto-resuming an overnight timer logs a false session (A198).
it('a timer older than four hours is stale rather than resumed', async () => { /* … */ })

/// Discard writes nothing at all.
it('discarding a stale timer writes no session', async () => { /* … */ })

/// The logged duration is the elapsed time, never the configured block length: a timer
/// stopped after three minutes must not record fifty (gate 7(b)).
it('the duration is the elapsed time and not the block length', async () => { /* … */ })
```

In `app/src/features/billing/plans.test.ts`:

```ts
/// Both generative task actions are priced from the matrix, and the number the screen
/// shows before the action runs comes from the file rather than a literal (gate 7(d)).
it('reads task_breakdown from the credits table', () => {
  expect(TASK_CREDITS.breakdown).toBe(2)
})
it('returns null rather than a guess when the key is absent', () => { /* … */ })
```

- [ ] **Step 2: Run them and watch them fail**

`npm --prefix app test -- useFocusTimer plans` → `useFocusTimer` does not exist.

- [ ] **Step 3: Write the platform capability and the hook**

- `web.ts`: `localState` over `localStorage` with a `stvdy.` prefix; `get` returns `null` on
  a missing key and on unparseable JSON (and logs the parse failure), `delete` removes it.
  This is not a lie about offline support: the web build has no outbox, and the timer's
  running state is the only thing here.
- `desktop.ts` / `mobile.ts`: `invoke('local_state_get' | 'local_state_set')`.
- `src-tauri/src/commands.rs`: `local_state_get(key) -> Option<String>` and
  `local_state_set(key, value)`, over a new `local_state(key text primary key, value text
  not null, updated_at timestamptz)` table in Phase 8's `cache/schema.rs` — one table, added
  in this task because the cache file is Phase 8's and this is the only new client-side
  table the product has.
- `useFocusTimer`: on mount, read `stvdy.focus_timer`; if absent, `idle`; if present and
  `elapsed <= TIMER_STALE_AFTER_MIN`, `running`; else `stale`. `start` mints
  `crypto.randomUUID()` and persists; `stop` computes the elapsed minutes with the device
  clock (the honest source for a timer), writes one session through
  `platform.sync.write('study_session', id, payload)` with `client_ts: new Date().toISOString()`
  (the field A195's bound reads), deletes the local state, and returns to `idle`. The
  elapsed display ticks from a `setInterval` that is derived from `startedAt`, not
  accumulated, so a throttled background tab does not drift.
- `plans.ts`: extend the parser with `task_breakdown` and export `TASK_CREDITS`.

- [ ] **Step 4: Run the tests**

`npm --prefix app test`

- [ ] **Step 5: Commit**

```
feat(app): the local-state capability, the focus timer, and the cost of a breakdown
```

---

## Task 13: The Today view, quick capture, break-down, tasks, and settings

**Files:**
- Create: `app/src/features/today/day.ts`, `day.test.ts`, `QuickCapture.tsx`,
  `TodayScreen.test.tsx`, `app/src/features/tasks/TasksScreen.tsx`,
  `BreakdownPanel.tsx`, `TasksScreen.test.tsx`,
  `app/src/features/settings/SettingsScreen.tsx`, `SettingsScreen.test.tsx`
- Modify: `app/src/features/today/TodayScreen.tsx`, `app/src/router.tsx`,
  `app/src/i18n/locales/{en,id}.json`

**Interfaces:**
- Produces:

```ts
// app/src/features/today/day.ts — the pure rules (A196). No React, no fetch, no Date.now().
export interface Activity {
  reviews: string[]      // ISO instants
  attempts: string[]
  sessions: string[]
}
export interface TodayInput {
  now: Date
  timeZone: string       // the user's IANA zone
  tasks: TaskView[]
  events: EventView[]
  dueCards: number
  activity: Activity
}

export interface TodayBuckets {
  /** Tasks and events whose local day is `now`'s, in `orderTasks` order. */
  today: Array<TaskView | EventView>
  /** Tasks due before the local day started, and not done. */
  overdue: TaskView[]
  /** Deadlines and exams in the next seven local days. */
  thisWeek: EventView[]
  /** Minutes logged in study_sessions today, in the user's zone. */
  minutesToday: number
  /** Consecutive local days, ending today or yesterday, with a review, an attempt or a
   *  session. A day counts if it has any of the three (the spec's rule). */
  streakDays: number
  /** The three steps the spec's new-user variant names, each with its own count. */
  setup: { workspaces: number; sources: number; decks: number }
}
export function todayBuckets(input: TodayInput): TodayBuckets
```

- [ ] **Step 1: Write the failing tests**

```ts
/// The one that fails silently for an Indonesian user: a task due at 07:00+07:00 is on
/// today's list in Jakarta and on yesterday's in UTC.
it('places a task in the local day, not the UTC day', () => {
  const buckets = todayBuckets({ now: at('2026-10-16T01:00:00Z'), timeZone: 'Asia/Jakarta', … })
  expect(buckets.today).toContainEqual(expect.objectContaining({ id: taskAt('2026-10-16T07:00:00+07:00').id }))
})

/// A task due at 23:30+07:00 belongs to the 16th locally and the 16th in UTC — the case
/// a naive implementation also gets right, which is why both are asserted.
it('keeps a late-evening task on the same local day', () => { /* … */ })

/// The streak counts a day with a session and nothing else, and stops at the first gap.
it('counts a day with any one of a review, an attempt or a session', () => { /* … */ })
it('stops the streak at the first missing day', () => { /* … */ })
it('counts today even when it is not over yet', () => { /* … */ })

/// A task due yesterday is overdue and is not also in today's list: showing it twice is
/// the double-count a "due today" filter written as a range produces.
it('does not list an overdue task as today', () => { /* … */ })

/// Minutes today come from the sessions, in the local day, and nothing is estimated.
it('sums only the minutes actually logged today', () => { /* … */ })

/// With no data the buckets are empty and the counts are zero: no invented numbers.
it('reports zeros rather than placeholders', () => { /* … */ })
```

`TodayScreen.test.tsx`:

```tsx
/// The roadmap's offline half: with `platform.sync.read` served from a local store and the
/// network transport failing, the screen still shows today's task, today's event and the
/// due count, and the sync line reads "Offline" with the reason the locale already has.
it('renders today from the local store while offline', async () => { /* … */ })

/// The honesty rule: the number in "N due" is the number of due cards, not a phrase.
it('shows the due count it was given', async () => { /* … */ })

/// Quick capture shows the cost before it runs and shows a draft as a draft: nothing is
/// saved until the student confirms (gate 7(d) and 7(e)).
it('shows the credit cost before the capture runs and saves nothing until confirmed', async () => {
  // assert the button's label contains "2"
  // run it; assert the draft card is labelled with honesty.suggested
  // assert `platform.sync.write` was not called
  // press Confirm; assert exactly one write with the draft's fields
})

/// A free account sees the locked state rather than a button that will 403.
it('locks quick capture on a plan that does not grant it', async () => {
  // assert LockedFeature with honesty/lock keys, and no capture field
})
```

`TasksScreen.test.tsx`:

```tsx
/// The proposal is labelled and nothing is written until Confirm; Dismiss writes nothing.
it('labels the proposal as suggested and writes only on confirm', async () => { /* … */ })
it('dismissing the proposal writes nothing', async () => { /* … */ })
/// Editing an estimate before confirming writes the edited number, not the model's.
it('writes the edited estimate', async () => { /* … */ })
/// A subtask list is one level, with the parent's progress as a count of done children.
it('shows subtasks under their parent', async () => { /* … */ })
```

`SettingsScreen.test.tsx`:

```tsx
/// Quiet hours save and wrap midnight, and the form says which window it means.
it('saves a quiet window that wraps midnight', async () => { /* … */ })
/// A channel the plan does not grant is shown locked with the plan that has it, and
/// choosing it is impossible rather than a 403 after the fact.
it('locks a channel the plan does not grant and names the plan that does', async () => { /* … */ })
/// "Reset local data and resync" is offered and calls platform.sync.reset().
it('offers the local reset', async () => { /* … */ })
/// The sections this phase does not own are named, not omitted.
it('names the sections other phases own', async () => { /* … */ })
```

- [ ] **Step 2: Run them and watch them fail**

`npm --prefix app test -- today tasks settings`

- [ ] **Step 3: Write the screens**

- `TodayScreen`: replaces the four empty states with the real ones — "Due now" (the due-card
  count and a one-tap review), today's timeline (events and tasks, `formatInZone`), overdue,
  deadlines this week, quick capture, the focus timer, "Continue where you left off", the
  streak, and the new-user checklist when all three counts are zero. Reads go through
  `platform.sync.read`; the sync line uses `states.synced` / `states.syncing` /
  `states.offline` + `states.offlineReason`.
- `QuickCapture.tsx`: a one-line input, the cost from `TASK_CREDITS.breakdown`, the
  `honesty.composerHint` line under it (the field is free text that reaches a model), the
  draft card labelled `honesty.suggested` with `honesty.nothingAddedYet`, and Confirm /
  Edit / Dismiss. Confirm writes one task through `platform.sync.write`. A draft with
  warnings shows them as text beside the field they concern.
- `TasksScreen` + `BreakdownPanel.tsx`: the list with subtasks one level deep, the create
  form, and the break-down panel labelled `honesty.suggested` with the per-subtask estimate
  editable and the total recomputed locally as the sum of what is left. Accept writes N
  tasks with `parent_id` and `position` 0..n-1.
- `SettingsScreen.tsx`: Study preferences (timezone, study hours, daily limit, block and
  break length), Notifications (channels with the plan-locked ones, quiet hours),
  Language, Appearance, Devices and sync (the reset, plus a link to the Devices screen),
  and a named list of the sections other phases own — Calendar connections (Phase 10),
  Data export and deletion (Phase 7/15), BYOK (Phase 13) — each saying which phase brings
  it, rather than an absent control.
- `router.tsx`: `/tasks` → `TasksScreen`, `/settings` → `SettingsScreen`.
- `en.json` / `id.json`: every new key in **both** files. The i18n contract test compares
  key sets and `{{placeholder}}` sets, so a missing Indonesian key or a dropped `{{count}}`
  fails it.

- [ ] **Step 4: Run the tests and the build**

`npm --prefix app test && npm --prefix app run build && node scripts/check-no-cdn.mjs`

- [ ] **Step 5: Commit**

```
feat(app): Today, quick capture, break-down, tasks and settings
```

---

## Task 14: Docs, the three gates, and the smoke script

**Files:**
- Modify: `docs/API.md`, `docs/SYNC.md`, `docs/ARCHITECTURE.md`,
  `docs/ASSUMPTIONS.md`, `docs/THREAT_MODEL.md`
- Create: `scripts/smoke-phase9.sh`

- [ ] **Step 1: `docs/API.md`**

Replace the three placeholder sections with real ones and re-tag the fourth:

- `## Tasks (Phase 9)` — the six routes, the `TaskView` shape, the refusal codes, and the
  two sentences that are the contract: quick capture and break-down return proposals and
  write no task; accepting a proposal is N creates.
- `## Events (Phase 9)` — the four routes, the overlap rule for `from`/`to`, the
  `source = 'app'` rule and the four server-owned columns (A189), and the read-only rule
  for an externally sourced row.
- `## Sessions and activity (Phase 9)` — `POST /sessions`, `GET /sessions`,
  `GET /activity`, the bound and the exact body fields including `client_ts` (A195), and
  the `applied` flag.
- `## Settings (Phase 9)` — `GET`/`PUT /settings`, the partial-write rule, the per-channel
  refusal, quiet hours wrapping midnight, and the timezone living on `users`.
- `## Study and dashboard (Phase 12)` becomes `## Dashboard (Phase 12)` with `GET
  /dashboard`, since `POST /sessions` has moved to this phase.

- [ ] **Step 2: `docs/SYNC.md`**

Add `### Entity payload validation` under Push, with the four rules a pushable entity now
carries: the four server-owned `events` columns (A189), the `user_settings` id rule and its
per-channel plan check (A191/A192), the `study_sessions` bound and why it reads `client_ts`
(A195), and the parent-delete cascade producing one tombstone per child (A188). Note that
these are the *same* functions the REST routes call, so a rule cannot hold on one path only.

- [ ] **Step 3: `docs/ARCHITECTURE.md`**

Add `## Phase 9: offline writes and the focus timer` — what the device holds (the four
entities in the cache, the timer's local state), why the Today view has no route, the one
asymmetry between the builds (the web build has no cache, so `GET /activity` is its
read for the streak), and the timer's single-log design (the id minted at start).

- [ ] **Step 4: `docs/ASSUMPTIONS.md`**

Append A181–A200 as rows, and one line in the "Schema deviations" table for
`tasks.position`, the `user_settings` columns and the five new `check` constraints.

- [ ] **Step 5: `docs/THREAT_MODEL.md`**

Append to § 6 R1: the bound now exists (`check_session`), it was added in Phase 9 because
the Today screen and the streak display these numbers as facts, and R1 remains accepted —
the bound narrows what a fabricated row can look like without making the rows trusted.

- [ ] **Step 6: Roadmap gate 5 — accessibility, on four screens**

Measured in a real browser, both `prefers-color-scheme` values, at 390x844 and 1365x900,
on the Today screen, the quick-capture field and its draft card, the break-down panel, and
the settings form. State the check performed and the result for each of the five:

- **(a) keyboard only** — `Tab` reaches every control in order with a visible focus ring; a
  task is created, a capture draft confirmed, a proposal accepted and a setting saved
  without a mouse. `Escape` closes the draft card and returns focus to the input.
- **(b) contrast** — computed WCAG ratios for the body text, the meta line, the buttons,
  the locked-channel row and the timer's digits, dark **and** light, against 4.5 (3.0 for
  large text). The timer's digits are large text; the state word beside them is not.
- **(c) 44px targets** — the capture field, the start/stop button, the checkbox rows in the
  proposal list and the channel rows, at phone width. Measured, not assumed.
- **(d) no state by colour alone** — the timer says "Running" / "Paused" / "Stopped" in
  words beside the clock, the sync line says "Offline" in words, an overdue task carries the
  word, and a locked channel says which plan has it.
- **(e) 200% text scale at 390px** — no clipped or overflowing element, no horizontal page
  overflow. Phase 5's `fieldset` finding (`min-inline-size: min-content`) is the precedent:
  a checkbox group is exactly the shape that refuses to shrink, and the proposal list is a
  checkbox group.

- [ ] **Step 7: Roadmap gate 6 — Indonesian end to end**

- **Server.** `an_indonesian_capture_resolves_a_relative_date_in_jakarta`: the capture text
  "selesaikan tugas kripto hari Jumat" with `now` = Thursday 2026-10-15T09:00+07:00 produces
  a draft whose `due_at` is Friday 23:59 **+07:00**, and the prompt carried
  `Asia/Jakarta` and the text inside a `<user_context_data>` block.
- **Server.** `an_indonesian_task_breaks_down_into_indonesian_subtasks`: with
  `user_settings.language = 'id'`, the composed prompt contains the language and the
  returned subtasks are the Indonesian ones the stub returned.
- **UI.** Driven in the browser against the real server: an Indonesian capture produces the
  draft card in Indonesian, and a task due at `2026-10-16T07:00+07:00` appears on the 16th
  in the Today view while a UTC implementation would put it on the 15th.
- The silent failures this catches, named: a date resolved in UTC (a Friday deadline that
  lands on Thursday), an unfilled `{{output_language}}` (English subtasks), and a local-day
  bucket computed with the machine zone.

- [ ] **Step 8: Roadmap gate 7 — honesty**

State each, with where it was checked:

- **(a)** No number on the four screens is invented: the due count, the minutes today, the
  streak and the per-day counts are all computed from rows; the breakdown's estimates are
  inside the "Suggested …" region and are editable; the timer shows measured elapsed time.
- **(b)** Each number traces: "N due" = due cards, "N min today" = the sum of today's
  `study_sessions.duration_min`, "N day streak" = consecutive local days with a row, the
  setup checklist's counts = live workspaces/sources/decks.
- **(c)** `honesty.composerHint` is present under the quick-capture field, because it is a
  free-text box whose content reaches a model.
- **(d)** Both generative actions show their cost before they run, read from
  `config/plans.toml` through `TASK_CREDITS` (never a literal).
- **(e)** A proposal is labelled `honesty.suggested` with `honesty.nothingAddedYet`, and the
  two tests in Task 13 assert that nothing is written until Confirm.

- [ ] **Step 9: The smoke script**

`scripts/smoke-phase9.sh` — signup, set `language = id`, quick-capture an Indonesian
sentence, assert the draft's `due_at` is a Jakarta evening and that the task count is
unchanged, create the task, break it down, accept two subtasks, log a session with a
plausible `client_ts` and assert a replay is `applied: false`, put a too-long session and
assert the 400, and read `/activity`. Follows `scripts/smoke-phase5.sh`'s shape: it reads
the one key it needs from `.env` (not `source .env`, which Phase 5's notes record as a
hazard), and ends with `=== PASS`.

- [ ] **Step 10: Run the full gate**

| Gate | Command |
|---|---|
| format | `cargo fmt --check` |
| lints | `cargo clippy --workspace --all-targets -- -D warnings` |
| tests | `cargo test --workspace` |
| SPA tests | `npm --prefix app test` |
| web build | `npm --prefix app run build` |
| no CDN | `node scripts/check-no-cdn.mjs` |
| sqlx | `cargo sqlx prepare --workspace --check` |
| smoke | `bash scripts/smoke-phase9.sh` |

- [ ] **Step 11: Commit**

```
docs(phase 9): the task, event, session and settings contract, and the three gates
```

---

## Phase 9 exit checklist

| # | Gate | Command | Result |
|---|---|---|---|
| 1 | Offline task + event + session writes sync after reconnect | `cargo test -p stvdy-server --test offline_tasks` | |
| 2 | The server had nothing before the reconnect | the same test, assertion 2 | |
| 3 | A replayed flush writes one row | `cargo test -p stvdy-server --test offline_tasks flushing` | |
| 4 | The Today view and the timer work offline | `npm --prefix app test -- TodayScreen useFocusTimer` | |
| 5 | Quick capture and break-down cost 2 and save nothing | `cargo test -p stvdy-server --test tasks` | |
| 6 | A free plan gets 403 `plan_required` on both | `cargo test -p stvdy-server --test route_sweep` | |
| 7 | A session beyond its elapsed time is refused on both paths | `cargo test -p stvdy-server --test study tamper` | |
| 8 | A client cannot write the calendar-owned event columns | `cargo test -p stvdy-server --test events tamper` | |
| 9 | Every new route is classified | `cargo test -p stvdy-server --test route_sweep` | |
| 10 | The local day and the streak are unit-tested | `npm --prefix app test -- day` | |
| 11 | `0017` is still the last trigger migration | `cargo test -p stvdy-server --test schema` | |
| 12 | Roadmap gate 5 (accessibility) measured and recorded | Task 14 Step 6 | |
| 13 | Roadmap gate 6 (Indonesian) measured and recorded | Task 14 Step 7 | |
| 14 | Roadmap gate 7 (honesty) stated and recorded | Task 14 Step 8 | |
| 15 | `docs/{API,SYNC,ARCHITECTURE,ASSUMPTIONS,THREAT_MODEL}.md` updated in the same commits | — | |
| 16 | The full quality gate is green | Task 14 Step 10 | |

---

## Execution notes

To be filled in as the phase is built, in the shape Phase 5 and Phase 6 use: what the plan
did not anticipate, each with the file that proved it; the gate measurements' actual
numbers; the verification table with real counts; and a `NOT VERIFIED HERE` line naming
what the phase could not exercise (the native iOS and Android timer and offline paths, which
have no simulator run in this phase).

---

## Self-Review

**Spec coverage, item by item.** `docs/SPEC.md` § Tasks: "CRUD with subtasks, priority, due
date, estimate, workspace link" → Task 6; "quick-capture endpoint accepting free text …
parsed by the LLM into a task draft … relative dates resolved in the user's timezone" →
Tasks 1, 2, 6; "'Break down': LLM proposes subtasks with time estimates … the user
accepts/edits/dismisses before saving" → Tasks 1, 3, 6, 13. § Today, Study hub, focus timer,
reminders, dashboard: the Today view's six parts and the new-user variant → Task 13; "the
streak … a day counts if the user reviews at least one card, answers a quiz, or logs a
study session (in the user's timezone)" → Tasks 8 and 13; "Pomodoro-style focus timer
logging to `study_sessions` … Timer state survives app restarts" → Task 12; reminders →
Phase 12 (A199); the Study hub → A199; the dashboard → Phase 12. § Screens item 13,
Settings: Study preferences, Notifications (channels and quiet hours), Language,
Appearance, Devices and sync (the local reset) → Task 9 and Task 13; Calendar connections
(Phase 10), Data (Phase 7/15), BYOK (Phase 13) → named on the screen and left to their
phases. § What is cached locally on native clients: unchanged, and this phase is the first
consumer of it.

**Placeholders:** none. The choices the spec does not make are pinned rather than waved at:
the price of a quick capture (A181), whether the two prompt files may be edited (A182),
whether a draft or a proposal is persisted (A184/A186), how subtasks are ordered (A187),
which event columns are the server's (A189), where the settings live (A193/A194), what a
session's length is bounded by (A195), and what is deliberately not here (A199).

**Type consistency:** `CaptureDraft`, `Proposal`, `ProposedSubtask`, `TaskRow`,
`TaskStatus`, `SessionError`, `NewTask`, `NewEvent`, `NewSession`, `SettingsRow`, `Reject`,
`RunningTimer`, `TimerState`, `Activity`, `TodayBuckets`, `LocalStateApi` are each defined
once and referenced by name from the server and the SPA. `GenerationError`, `is_retryable`,
`strip_code_fence`, `EntitlementError`, `Mutation`, `MutationResult`, `Feature::QuickCapture`,
`Feature::TaskBreakdown`, `CreditAction::TaskBreakdown`, `Platform.sync` and `SyncStatus`
are reused, never redefined.

**The phase's real risk, and its mechanical defence.** The risk is that **the offline path
is where a "works online" feature silently is not.** Three things in this phase look
finished on a laptop with Wi-Fi and are not: a write that goes to REST instead of the
outbox, a Today view that reads a server route, and a timer that logs a session twice after
a restart. Each gets a defence that fails loudly rather than a claim:

- The gate test asserts **the server has nothing before the reconnect** by reading the
  database directly, so a write that secretly went through REST cannot pass.
- The Today view has no route to call (A196), so there is nothing to fall back to; its
  rules are pure functions with their own tests, and the screen test runs with a failing
  transport.
- The session id is minted at start and persisted with the timer (A197), and
  `on conflict (id) do nothing` makes a replay a no-op; the test flushes twice and asserts
  one row.
- The two "looks saved" generative actions are closed the same way: the routes write no
  task (A184/A186), and the tests assert the count is unchanged after a capture, a
  breakdown and a dismissal.

**What this plan does not do, deliberately:** it does not touch `crates/sync/src/client/`
or the push transport (Phase 8's), it does not add a `+sync` table or a trigger (so
`0017_sync_triggers.sql` stays last and `schema.rs` is unchanged), it does not add a
`GET /today` route, and it does not plan the Study hub, reminders or the dashboard (A199).

**Open items:**
- B1 (QRIS gateway docs) is still needed at Phase 13, not before.
- If Phase 8's web adapter ever grows a generic per-entity read, `GET /activity` becomes
  redundant; it is the web build's only stand-in for a cache this phase otherwise does not
  need.
- The estimate in a breakdown is the model's, and the plan deliberately does not correct it:
  it is labelled as a suggestion and is editable, which is the honest treatment rather than
  a computed one.
