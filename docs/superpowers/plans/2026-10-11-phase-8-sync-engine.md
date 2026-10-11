# Phase 8. Sync engine. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** one account's writes reach every device. A native client keeps a SQLite
cache and an outbox, writes locally first, pushes through an idempotent mutation
replay, and pulls a per-user change log whose cursor can only ever advance over a
contiguous prefix. A replayed mutation applies nothing. A tombstone beats a
concurrent edit. A cursor older than retained history is told to resync from zero.
A revoked device is wiped. And a pull for one account can never contain another
account's `seq`.

**Architecture:** the server owns the clock. `change_log.seq` is a `bigserial`
assigned inside an AFTER trigger that first takes a per-user advisory lock, which is
what makes `seq` order equal commit order for one user — a property the cursor
depends on and the only reason the cursor can be a single integer. `POST /sync/push`
takes the same lock as the **first** statement of its transaction, before it touches
a row, so the lock order is uniformly advisory-then-rows and a two-mutation batch
from two devices cannot deadlock (`40P01`). The per-mutation decision is **not** this
phase's: Phase 7's Task 9 lands `crates/server/src/sync/push.rs::decide` (A150) — still
absent from this tree at the time of writing, so Task 1 opens by reading it — and this
phase adds only the transport around it: the lock, the idempotency log, the apply, the
results. The client engine lives in `crates/sync`, generic over a `LocalStore` and a
`Transport` trait, so the same code runs inside the Tauri shell over `rusqlite` and
inside the server's own convergence tests over an in-memory store: the engine is
tested against the real routes rather than against a mock of them.

**Tech Stack:** Rust (core + sync + server; `rusqlite` and `reqwest` only in
`src-tauri`), Postgres/sqlx, React + Vite + TypeScript + Tailwind, TanStack Query,
Tauri 2.

**Exit gate (observable, from the roadmap):** 3 simulated devices converge; a replayed
mutation is idempotent; tombstone vs edit; 410 → full resync; a revoked device is
wiped.

**The phase's named risk, and its mechanical defence.** The roadmap says this is "the
phase most likely to leak cross-tenant data". Every read on the sync path is therefore
scoped by `user_id` in the same statement that reads it — never by a filter applied
afterwards — and three tests make that structural rather than claimed:
`a_pull_never_contains_another_accounts_seq` (Task 3), `the_payload_loader_cannot_be_
pointed_at_another_users_row` (Task 3), and the second-account leg of the convergence
suite (Task 11). The second risk is that a replayed mutation must apply **nothing**:
`a_replayed_mutation_applies_nothing` (Task 4) asserts the row's `updated_at` and the
`change_log` count are both unchanged, not merely that the response looks the same.

**Roadmap gates 5/6/7 for this phase.** This phase changes the SPA (sync status, the
Devices screen), so all three apply; each task states its own status below and Task 12
records the measured result.

- **Gate 5 (accessibility):** the sync status control and the Devices screen are
  keyboard-reachable with a visible focus ring, pass contrast in both themes, keep
  ≥44px targets at phone width, carry no state in colour alone (the status is a word
  plus a glyph-free text label), and render at 200% text on 390px without clipping.
- **Gate 6 (Indonesian end to end):** the data half is real here — an Indonesian task
  title, notes and event title round-trip through push and pull byte-exact in both
  directions, and a tombstone of an Indonesian-titled row carries the right id. The
  copy half is the two locale files (`sync.*`, `devices.*`), which
  `app/src/i18n/i18n.test.ts` already enforces for key parity and placeholder parity.
- **Gate 7 (honesty):** the status is derived from stored facts only (`devices.
  last_sync_at`, the outbox length, the last attempt's outcome); "Last synced …" never
  renders a locally invented time; a rejected mutation is shown with the server's
  `code` and the row is marked "Not saved" rather than counted as synced; and the
  status never reads "Synced" while the outbox is non-empty (A187).

---

## Global Constraints (copied from the roadmap; every task includes these)

- **Server is the only source of truth.** Clients never call LLM, search, or Google
  APIs directly. All heavy work is server-side.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings. That list is exactly `Entity::is_client_writable`, and
  `/sync/push` rejects everything else with `read_only_entity`. Chat, generation, web
  search, uploads/ingestion, checkout and calendar queue nothing.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default
  `Asia/Jakarta`). Local-date math only through `chrono-tz`.
- **Account-wide entitlements, never per device.** Account sync works on ALL plans;
  only device count and storage differ.
- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes.
  `tracing` structured logs with request ids; **log latency for sync pull/push**.
- **Quality gate:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace`, `npm --prefix app test`, `npm --prefix app run
  build`, `node scripts/check-no-cdn.mjs`, `just check-shell` (which runs the shell's
  fmt/clippy/test and depends on `check-web`, because `generate_context!` needs
  `app/dist`), `cargo sqlx prepare --workspace --check`.
- **Migrations are `NNNN_slug.sql`, ordered, and `0017_sync_triggers.sql` stays the
  last trigger-attaching migration.** This phase adds **one** table that is not
  synced, so it needs no change to 0017's array — and a new `+sync` table would have to
  be inserted *above* 0017, which this phase does not do.
- **UI language English**, but Indonesian documents and queries must work end to end.

---

## What already exists (checked in this tree, not assumed)

### `crates/sync` — the protocol half is built, the client half is not

- `crates/sync/src/lib.rs:7` is `pub mod proto;` only. **`crates/sync/src/client/` does
  not exist** (verified: `ls crates/sync/src` → `lib.rs`, `proto/`).
- `crates/sync/src/proto/entities.rs` (163 lines) already has:
  - `Entity` with **17** variants — the roadmap's 16 plus `Cheatsheet`
    (`entities.rs:20`), which Phase 6 added.
  - `Entity::ALL` (`:30`), `table_name()` (`:50`) mapping to the Postgres table name
    because `change_log.entity` stores `tg_table_name`, `from_table_name()` (`:72`).
  - `dependency_rank()` (`:78`) returning the dense `1..=17` of the roadmap's list,
    with `Cheatsheet => 17` appended (`:87-89`) so the dense property the wire test
    asserts still holds.
  - `is_client_writable()` (`:112`) — exactly the six entities the roadmap's allowlist
    names — `is_append_only()` (`:126`) — `FlashcardReview | QuizAttempt |
    StudySession` — and `accepts_push_op()` (`:134`), which is where `user_settings`
    loses `Op::Delete` (`:142`).
  - `Op` (`:149`) and `Change { seq, entity, id, op, data: Option<Value> }` (`:156`).
- `crates/sync/src/proto/error.rs:23` already has `ErrorCode::ReadOnlyEntity`, and
  `as_str()` (`:43`) returns `"read_only_entity"` — the exact string the threat model's
  §5 rows assert.
- `crates/sync/src/proto/push.rs` already has `Mutation { mutation_id, entity, op, id,
  payload, client_ts }`, `PushRequest { device_id, mutations }`, `PushResponse
  { results }` and `MutationResult` with `#[serde(tag = "status", rename_all =
  "snake_case")]` → `{"status":"applied","seq":N}` / `{"status":"rejected","code":…,
  "message":…}` / `{"status":"conflict","winner":…}`. Note `Rejected.code` is a
  `String`, not an `ErrorCode`, so the server writes `ErrorCode::as_str()`.
- `crates/sync/src/proto/pull.rs` has `PullResponse { changes, next_cursor, has_more }`
  and nothing else.
- `crates/sync/tests/wire.rs` exists and asserts the bijective entity/table mapping, the
  dense rank, and the pushable set against the spec's offline-write scope.
- **MISSING:** `crates/sync/src/proto/cursor.rs` (the roadmap's crate tree lists it),
  the whole of `crates/sync/src/client/`, and any per-entity payload column list — so
  today nothing in the tree says *which* columns of a row travel to a client.
- `crates/sync/Cargo.toml` depends on `serde`, `serde_json`, `uuid`, `chrono`,
  `thiserror` only: **no `async-trait`, no `tokio`**. The client engine needs both
  (the roadmap fixes `async_trait` as the trait style for provider traits and the
  engine's `Transport` is async).

### The sync plumbing in SQL is complete and this phase must not disturb it

- `migrations/0005_change_log_mutation_log.sql`: `change_log (seq bigserial primary
  key, user_id uuid not null references users on delete cascade, entity text, entity_id
  uuid, op text, origin_device_id uuid, changed_at timestamptz default now())` with
  `index on change_log (user_id, seq)` (`:18`) and `index on change_log (changed_at)`
  (`:20`); `mutation_log (mutation_id uuid primary key, user_id uuid not null
  references users on delete cascade, device_id uuid references devices on delete set
  null, result jsonb not null, created_at timestamptz default now())` with `index on
  mutation_log (user_id, created_at)` (`:31`). **`mutation_id` is a global primary
  key**, which is why A172 below is a rule and not a preference.
- `migrations/0006_sync_functions.sql` (114 lines) is the single source of truth for the
  trigger bodies: `touch_updated_at` (`:44`), `log_change` (`:52`, reads only
  `user_id`/`id`/`deleted_at`, takes `pg_advisory_xact_lock(hashtext(uid::text))`
  **before** the insert at `:91`, and reads the device from
  `current_setting('app.device_id', true)` at `:95`), and `log_change_by_user` (`:104`,
  the `user_settings` case, same lock, always `upsert`).
- `migrations/0017_sync_triggers.sql` (40 lines) attaches both triggers to the 16-table
  array (`:14-19`) plus `user_settings` (`:34-40`), and its header says it must stay
  last. `crates/server/tests/schema.rs:87` asserts the trigger set equals
  `Entity::ALL`'s table names, so the wire list and the SQL list cannot drift.
- **No `pg_advisory_xact_lock` caller exists outside those two trigger bodies**, so the
  push route's first statement (the roadmap's lock-order rule) is genuinely new work.

### `crates/server/src/sync/` does not exist yet — Phase 7 Task 9 delivers `decide`

Verified at this snapshot: `ls crates/server/src/sync` → no such directory, and
`git log --all -- crates/server/src/sync` is empty. Phase 7 is in flight in this tree
(`crates/server/src/entitlements.rs` exists at 28,658 bytes, `limits.rs` at 5,195, and
`usage.rs`, `admin.rs`, `billing/`, `jobs/schedule.rs`, `tests/tamper.rs` are all
absent). Phase 7's plan, Task 9, commits to:

```rust
pub fn decide(snapshot: &UsageSnapshot, mutation: &Mutation) -> Result<(), MutationResult>;
pub fn push_features(entity: Entity) -> &'static [Feature];
```

and states that `decide` applies, in order: the allowlist (`Entity::is_client_writable`
→ `read_only_entity`), the op rule (`accepts_push_op` → `read_only_entity`), the
derived-field guard (a payload carrying `ease`, `interval_days` or `due_at` →
`read_only_entity`), the `read_only` account state, then `check` for each of
`push_features(entity)`. **Task 1 of this plan opens by reading the landed file and
pinning its real signature in a compile-time assertion**, because this phase's whole
push route is built on it and A150 says this phase re-implements nothing.

What *does* exist on the server side today, and is reused verbatim:

- `crates/server/src/error.rs:47` — `AppError::ResyncRequired`, whose `status()` is
  `StatusCode::GONE` and whose `code()` is `"resync_required"`. **The 410 body already
  exists; this phase adds no error variant.**
- `crates/server/src/entitlements.rs` — `snapshot` (`:72`), `snapshot_for` (`:80`),
  `snapshot_in` (`:109`), `require` (`:131`), `require_for` (`:141`), `reserve_in`
  (`:171`), `limits_view` (`:220`), `facts` (`:65`), `effective_plan` (`:52`).
- `crates/server/src/auth/extract.rs:20-37` — `AuthUser` carries `user_id`,
  `device_id`, `session_id`, `email`, `email_verified`, `plan_id`, `timezone`,
  `is_admin`, `student_verified`, `platform` and `subscription_status`. The push route
  needs `device_id` and `platform`, and both are already there.
- `crates/server/src/routes/mod.rs:105-128` — the authenticated router merges one
  `routes()` per feature module; `devices::routes()` is already registered (`:118`).
- `crates/server/src/ratelimit.rs` exists (3,391 bytes) with an auth layer; Phase 7 adds
  `signup_limiter()` and `spend_layer()`. This phase adds a sync layer (A185).
- `crates/server/src/jobs/mod.rs` — `Queue`, `enqueue(pool, queue, kind, payload,
  dedupe_key)`, `Failure::{transient, permanent}`, `MAX_ATTEMPTS`, `POLL_INTERVAL`;
  `jobs/worker.rs:86-96` dispatches by `kind`; `migrations/0015_jobs.sql:11` already
  lists `compact_change_log` as a kind in its comment, and `:16` names it as a
  singleton, so the job kind and the `dedupe_key` column exist and nothing implements
  the handler. Phase 7 adds `jobs/schedule.rs`; this phase adds one arm to it.

### The SPA and the shell: what is there, and what is not

- `app/src/platform/types.ts:3` already declares `export type SyncStatus = 'synced' |
  'syncing' | 'offline' | 'error'`. The spec's four states are "Synced / Syncing /
  Offline / Needs attention", so this union gains `'needs_attention'` (Task 10) — the
  name is reused, not reinvented.
- `app/src/platform/types.ts` `Platform` has `secureStore`, `notifications`, `files`,
  `deepLinks`, `openExternal`, `haptic`, `checkForUpdate`. **No `sync` member.**
- `app/src/components/layout/Shell.tsx` and `Sidebar.tsx` — the nav is exactly five
  labels (`nav.today`, `nav.workspaces`, `nav.calendar`, `nav.tasks`, `nav.settings`)
  and the phone `TabBar` shares `.nav-item-tab`. Devices is therefore reached from
  Settings, not as a sixth nav item (Task 10).
- `app/src/api/queries/` has `auth, chat, cheatsheets, flashcards, quizzes, reports,
  sources, workspaces` — **no `sync.ts`, no `devices.ts`**, although the device routes
  shipped in Phase 1.
- `app/src/api/sse.ts` is the streaming reader (fetch + `ReadableStream` +
  `eventsource-parser`), written for the POST chat stream; the sync stream is a GET and
  reuses the same reader (Task 10).
- `src-tauri/src/` is `main.rs`, `lib.rs`, `commands.rs`, `deeplink.rs`, `keychain.rs`.
  **No `cache/`, no `commands/` directory, no `rusqlite`, no HTTP client.**
  `src-tauri/Cargo.toml` does not depend on `stvdy-sync` — while the root
  `Cargo.toml:7` says the shell "path-depends on crates/sync". That comment is
  currently false; Task 9 makes it true.
- `src-tauri` is a separate workspace (`Cargo.toml:9` `exclude = ["src-tauri"]`) with
  its own `Cargo.lock`, and CI's `shell` job (`macos-15`) runs `cargo fmt/clippy/test
  --manifest-path src-tauri/Cargo.toml`. `justfile`'s `check-shell` already carries the
  comment "Phase 8's sync engine would accumulate unlinted and untested" without it.
- `devices` already has `last_seen_at` and `last_sync_at` (`0004_devices_auth_push.sql:9-10`).
  **Nothing in the tree writes `last_sync_at`** (`grep -rn last_sync_at crates` → the
  migration only), so the Devices screen's "Last synced" has a column and no writer.

### Documentation state

- `docs/SYNC.md` (136 lines) is already the normative protocol document: cursor model
  and the advisory-lock argument (`:3-29`), lock order (`:31-38`), pull (`:40-60`), push
  and idempotency (`:62-93`), realtime hint (`:95-104`), conflict rules (`:106-116`),
  offline scope (`:118-123`), local cache (`:125-128`), compaction and resync
  (`:130-136`). It documents the *contract*; the initial-pass anchor, the watermark
  table, the payload shapes, the outbox and the revoked wipe are this phase's additions
  and go into it (Task 12).
- `docs/API.md:207` is `## Sync (Phase 8)` with one line naming the three routes.
- `docs/ARCHITECTURE.md:841` `## Local cache` already fixes the design: SQLite plus an
  outbox owned by the Rust shell (`rusqlite`, `bundled`), never by JS; the web build has
  no local database; SQLCipher is a config flag, off by default.
- `docs/THREAT_MODEL.md:106-107` are the two `read_only_entity` rows (`Message` and
  `Source`) and `:112` is the `flashcards.ease/interval/due` row; `:139` is the
  cross-tenant row whose enforcement names "Phase 8 test"; §6 R2 is the accepted local-
  cache risk.
- `docs/ASSUMPTIONS.md` highest row **A131** (Phase 6 wrote A119–A131; its plan reserved
  to A139, and Phase 7's plan reserves A140–A163). **This phase pins A164 onward.**

---

## Decisions this plan pins (so parallel work cannot diverge)

Each is a place the spec, the roadmap and the tree leave a choice. They are appended to
`docs/ASSUMPTIONS.md` as rows **A164–A189** in the same commits as the code.

1. **A164 — The initial pass is snapshot-anchored.** `GET /sync/pull?cursor=0` returns
   the first page ordered by `(dependency_rank, seq)` and sets `next_cursor` to the
   user's `max(seq)` **at the moment the pass began** (the *anchor*). Later pages of the
   same pass send `cursor=<anchor>&offset=<n>`, and every row of the pass satisfies
   `seq <= anchor`. Without the anchor, offset paging over a rank-ordered stream can
   skip a row: a new workspace (rank 1) inserted while the client is deep in rank-3
   territory shifts every later row one position right, so the row at the old page
   boundary is never delivered and the client's subsequent cursor is already past it —
   permanent loss. The anchor makes the pass set immutable, and anything committed
   during the pass has `seq > anchor` and arrives on the next incremental pull. A client
   interrupted mid-pass may restart it from offset 0; the pass is idempotent (upserts).
2. **A165 — Staleness is `cursor > 0 && cursor < min_retained_seq`, where
   `min_retained_seq` is a per-user watermark row written by compaction.** Comparing
   against `min(seq)` instead would be wrong in a way that breaks new users: `seq` comes
   from a global sequence, so a brand-new account's first change can be seq 500, and a
   device at cursor 0 would be told to resync on its very first request. A cursor of 0
   is never stale, so a full resync is always available.
3. **A166 — The 410 uses the existing `AppError::ResyncRequired`.** No new error
   variant: `crates/server/src/error.rs:47` already maps to 410 `resync_required`, which
   is the contract's row. The client's reaction is `wipe() → cursor = 0 → full pull`,
   and it is not surfaced as an error to the user (the spec: "the client does a full
   resync").
4. **A167 — `PullResponse` gains no field.** The only wire addition is an optional
   `offset` **query parameter**, legal only when `cursor=0`. The DTO the contract spells
   stays byte-identical, and a client that never sends `offset` still works (it just
   re-requests offset 0, which is idempotent).
5. **A168 — The pull ships a per-entity column allowlist, and it excludes `user_id`,
   `sources.file_key` and `sources.preview_key`.** `user_id` is the caller's own and
   redundant on every row; `file_key`/`preview_key` are internal object-storage keys
   whose client-facing form is a signed URL (`GET /sources/:id/file`), so shipping them
   would put a storage layout in every device's SQLite file. `updated_at` and
   `deleted_at` are included, because the client needs them for tombstone handling and
   for the "Last synced" display. The list lives in `crates/sync` so both ends read one
   copy.
6. **A169 — An unrecognised `change_log.entity` is a 500 `internal`, not a skipped
   row.** It can only mean the trigger array and `Entity::ALL` have drifted, and a
   silent skip would be permanent loss on every device. `crates/server/tests/schema.rs:87`
   makes the drift impossible, so this is a backstop rather than a policy.
7. **A170 — One transaction per push batch, with a SAVEPOINT per mutation.**
   `select pg_advisory_xact_lock(hashtext($1::text))` is the first statement,
   `select set_config('app.device_id', $1, true)` the second, then one savepoint per
   mutation so an apply failure rolls back that mutation alone. The roadmap fixes the
   lock ordering; the savepoints are what makes "a rejected mutation does not abort the
   batch" true for an apply failure as well as for a decision refusal.
8. **A171 — The request's `device_id` must equal the authenticated device.** A mismatch
   is 403 `forbidden`. `origin_device_id` exists to attribute a change to the device
   that caused it, and a body free to claim any device id makes that attribution
   worthless.
9. **A172 — The idempotency key is global.** A `mutation_id` already recorded for a
   *different* user is `Rejected{code: "conflict"}`, never applied and never answered
   with that user's stored result. `mutation_log.mutation_id` is a primary key, so the
   collision is otherwise unrepresentable, and returning another account's stored result
   would be a cross-tenant read on the one route this phase is most worried about.
10. **A173 — Ownership is checked in the same statement that writes.** Every apply is
    `insert … / update … where id = $1 and user_id = $2`; a mutation whose id belongs to
    another user is `Rejected{code: "not_found"}` and logged with both ids. A separate
    `select` before the write is a check-then-act that a second device can interleave.
11. **A174 — Append-only entities are `on conflict (id) do nothing`, and a no-op replay
    reports the row's existing `seq`.** A retried review therefore does not read as a new
    change, and the client's cursor does not move for a write that changed nothing.
12. **A175 — The conflict table is data, in `crates/sync/src/client/conflict.rs`, and the
    server's apply reads it.** One copy of `append_only | lww | field_merge |
    tombstone_wins` per entity, so the client and the server cannot disagree about the
    rule. The *decision* stays the server's (A150); the client only reacts to
    `MutationResult`.
13. **A176 — `tasks` and `events` mutations carry a patch plus the base row**
    (`payload = {"patch": {…}, "base": {…}|null}`); every other pushable entity carries
    the full row. Disjointness cannot be computed from one side: the server has no
    per-field history, so it needs both the client's changed keys (`patch`) and the row
    the client edited from (`base`). `base: null` means create. The wire type is
    unchanged — `payload` is `Option<Value>` and its shape is ours.
14. **A177 — The merge rule for `tasks`/`events`.** The other writer's changed set is
    `{k : stored[k] != base[k]}`; the client's is `keys(patch)`. If they are disjoint the
    patch is applied to the stored row, `updated_at` is touched by the trigger, and the
    result is `Applied`. If they overlap, the whole incoming patch wins (record-level LWW
    on server receive time), because a per-field tie-break would need a per-field clock
    the schema does not have and inventing one would make two devices disagree about
    which write won.
15. **A178 — A tombstone wins, and the loser is told.** An edit against a tombstoned row
    is `MutationResult::Conflict { winner: <the stored row, including its deleted_at> }`,
    which is the contract's conflict shape and the exit gate's "tombstone vs edit" case.
    The one exception is the explicit restore the contract names: a patch (or row) whose
    `deleted_at` is present and `null` restores the row and logs an `upsert`.
16. **A179 — `user_settings` is keyed by `user_id`.** Its mutation's `id` is ignored on
    apply (the row is the caller's), and the response echoes the caller's user id as the
    entity id, matching what `log_change_by_user` writes into `change_log.entity_id`.
    `Op::Delete` never reaches the apply: `Entity::accepts_push_op` already refuses it.
17. **A180 — `devices.last_sync_at` is written by pull and by push, in the same
    transaction as the work, and it is the only source of "Last synced".** Nothing writes
    that column today; without this the Devices screen would have to invent a time from
    the client's clock, which gate 7(a) forbids.
18. **A181 — The sync stream is a per-connection poll of `max(seq)` every 2 seconds with
    a 15-second heartbeat comment, emitting `event: seq`, `data: {"seq":N}`.** No
    `LISTEN/NOTIFY` and no cross-instance bus, which is what makes it correct behind more
    than one server instance. It sends the current `max(seq)` immediately on connect, so
    a device that reconnects after a gap pulls at once instead of waiting for the next
    change. The event name is additive to the roadmap §4 list, which covers the chat
    stream; `docs/SYNC.md` and `docs/API.md` both record it.
19. **A182 — `crates/sync`'s client engine is generic over a `LocalStore` trait and a
    `Transport` trait; `rusqlite` and `reqwest` live in `src-tauri`.** The engine must
    compile into the server's test binaries (which must not link a C SQLite) and into the
    shell, and the roadmap's crate note says `crates/sync` is shared by both. The
    in-memory store and the router-backed transport used by the convergence suite are
    test implementations of those two traits, so the engine is exercised against the real
    routes rather than a mock.
20. **A183 — The web build has no local cache and no outbox.** Its `Platform.sync` is
    online-only: `status()` is a constant `synced`, writes go through the REST routes,
    and `reset()` is a no-op. `docs/ARCHITECTURE.md` § Local cache already states the web
    build has no local database, and the spec scopes the cache to "native clients".
21. **A184 — Compaction's retention is two windows.** Upsert rows older than
    `CHANGE_LOG_RETENTION_DAYS` (default **90**) are deleted; tombstone (`delete`) rows
    are kept for `CHANGE_LOG_TOMBSTONE_RETENTION_DAYS` (default **180**, i.e. at least
    the retention window), because a device that has been offline for three months must
    still be told that a row was removed rather than silently keep it. The job writes the
    per-user watermark in the same transaction and runs daily under
    `dedupe_key = "compact_change_log:<UTC date>"`.
22. **A185 — The sync routes are rate-limited per account at 600 requests/minute.** The
    spec requires sync to be rate-limited; the limit is deliberately far above a real
    device's rate (a pull per change plus a reconnect) so that a legitimate burst is
    never the thing that trips it, and a stream reconnect is not abuse.
23. **A186 — A revoked device is wiped, not merely refused.** Any authenticated sync
    route answers 401 for a revoked device (its sessions are revoked in the same
    transaction as the device, `devices.rs:167-183`, so this is the existing behaviour).
    The engine treats a 401 on its own device as `Revoked`, emits
    `stvdy://sync-revoked`, deletes the SQLite file **including the outbox**, clears the
    keychain token, and the SPA signs out with the spec's copy. The outbox matters: a
    pending offline write from a revoked device must not be replayed by the next account
    that signs in on that machine.
24. **A187 — Sync status is derived from stored facts only.** `synced` = the outbox is
    empty **and** the last pull and push succeeded **and** `last_synced_at` is known;
    `syncing` = a pass is in flight; `offline` = the transport failed with a network
    error; `needs_attention` = the last pass left a rejected mutation, shown with the
    server's `code`. "Last synced …" renders from `devices.last_sync_at`. No status ever
    reads "Synced" while the outbox is non-empty.
25. **A188 — An `upsert` change whose row is gone is emitted as `data: null`, and the
    client applies `data: null` as a delete regardless of `op`.** A row a device cannot
    see must never be created on that device. This is also what makes the payload
    loader's `user_id` scoping safe in the pathological case: a `change_log` row whose
    `entity_id` belongs to another account loads no payload at all, so the worst outcome
    is a spurious local delete, never a cross-tenant read.
26. **A189 — Devices is reached from the sync status control, which the shell renders in
    both form factors.** `/settings` is still a `NotBuiltRoute` (`app/src/router.tsx:92`)
    and Settings is Phase 9's (`docs/API.md` § Settings is tagged Phase 9), so there is
    no Settings screen to link from and inventing one here would be Phase 9 work under a
    Phase 8 heading. A sixth nav item is also not available: the sidebar and the phone
    tab bar are both built from the same five labels. The status control is therefore one
    component rendered by `Shell` (sidebar foot on desktop, a chip in the tab bar's row
    on phone), and both forms link to `/devices`, which carries the device list, the
    "Last synced" detail and "Reset local data and resync".

---

## File structure (new this phase)

```
migrations/0027_sync_watermarks.sql        # change_log_watermark (not synced)
crates/sync/Cargo.toml                     # + async-trait, tokio
crates/sync/src/lib.rs                     # + pub mod client;
crates/sync/src/proto/mod.rs               # + pub mod cursor;
crates/sync/src/proto/cursor.rs            # NEW — PullQuery, CursorError, is_stale
crates/sync/src/proto/entities.rs          # + payload_columns(), payload_shape()
crates/sync/src/client/mod.rs              # NEW (Task 5) — module root, re-exports
crates/sync/src/client/store.rs            # NEW — LocalStore, ApplyReport, StoreError
crates/sync/src/client/transport.rs        # NEW — Transport, TransportError
crates/sync/src/client/outbox.rs           # NEW — the queue: order, retry, backoff
crates/sync/src/client/apply.rs           # NEW — one-transaction apply, parent deferral
crates/sync/src/client/conflict.rs         # NEW — the shared conflict table
crates/sync/src/client/engine.rs           # NEW — Engine, sync_once, status
crates/sync/tests/protocol.rs              # NEW — cursor, allowlist, payload shapes
crates/sync/tests/engine.rs                # NEW — the engine over an in-memory store
crates/server/src/sync/mod.rs              # NEW — routes(), touch_last_sync()
crates/server/src/sync/pull.rs             # NEW — GET /sync/pull
crates/server/src/sync/push.rs             # NEW — POST /sync/push (around Phase 7's decide)
crates/server/src/sync/apply.rs            # NEW — apply_one, merge_patch, Outcome
crates/server/src/sync/log.rs              # NEW — replay/record, seq_for
crates/server/src/sync/stream.rs           # NEW — GET /sync/stream
crates/server/src/jobs/handlers/compact.rs # NEW — compact_change_log
crates/server/src/jobs/handlers/mod.rs     # + pub mod compact;
crates/server/src/jobs/worker.rs           # + the compact_change_log dispatch arm
crates/server/src/jobs/schedule.rs         # + the daily compaction arm (Phase 7 created it)
crates/server/src/config.rs                # + CHANGE_LOG_RETENTION_DAYS / _TOMBSTONE_…
crates/server/src/ratelimit.rs             # + sync_layer()
crates/server/src/routes/mod.rs            # + .merge(sync::routes())
crates/server/src/lib.rs                   # + pub mod sync;
crates/server/tests/schema.rs              # + change_log_watermark in both lists
crates/server/tests/route_sweep.rs         # + the three routes
crates/server/tests/sync_pull.rs           # NEW
crates/server/tests/sync_push.rs           # NEW
crates/server/tests/sync_stream.rs         # NEW
crates/server/tests/sync_compaction.rs     # NEW
crates/server/tests/sync_convergence.rs    # NEW — the exit gate
src-tauri/Cargo.toml                       # + stvdy-sync, rusqlite, reqwest
src-tauri/src/lib.rs                       # + pub mod cache; pub mod sync;
src-tauri/src/cache/mod.rs                 # NEW — the connection, the pragmas, wipe
src-tauri/src/cache/schema.rs              # NEW — the DDL, entity tables, the outbox
src-tauri/src/cache/queries.rs             # NEW — LocalStore over rusqlite
src-tauri/src/sync/mod.rs                  # NEW — the transport, the loop, the events
src-tauri/src/sync/transport.rs            # NEW — Transport over reqwest
src-tauri/src/sync/commands.rs             # NEW — the IPC surface
src-tauri/tests/cache.rs                   # NEW — the cache, the outbox, the wipe
app/src/platform/types.ts                  # + SyncApi, 'needs_attention'
app/src/platform/web.ts                    # + the online-only SyncApi
app/src/platform/desktop.ts                # + the IPC-backed SyncApi
app/src/platform/mobile.ts                 # + the IPC-backed SyncApi
app/src/api/queries/sync.ts                # NEW
app/src/api/queries/devices.ts             # NEW
app/src/api/sse.ts                         # + streamSync()
app/src/features/sync/SyncStatus.tsx       # NEW
app/src/features/sync/SyncStatus.test.tsx  # NEW
app/src/features/devices/DevicesScreen.tsx # NEW
app/src/features/devices/DevicesScreen.test.tsx  # NEW
app/src/components/layout/Shell.tsx      # renders the status control in both form factors
app/src/components/layout/TabBar.tsx     # + the status chip beside the five labels
app/src/components/layout/Sidebar.tsx    # + the status control at the foot
app/src/router.tsx                         # + /devices
app/src/i18n/locales/{en,id}.json          # + sync.*, devices.*
scripts/smoke-phase8.sh                    # NEW
docs/{SYNC,API,ARCHITECTURE,ASSUMPTIONS}.md
.env.example                               # + the two retention keys
```

---

## Task 1: `crates/sync` — the cursor, the payload allowlist, and the boundary Phase 7 left

**Files:**
- Create: `crates/sync/src/proto/cursor.rs`, `crates/sync/tests/protocol.rs`
- Modify: `crates/sync/src/proto/mod.rs`, `crates/sync/src/proto/entities.rs`,
  `crates/sync/Cargo.toml`
- Read first, do not modify: `crates/server/src/sync/push.rs` (Phase 7 Task 9)

**Interfaces:**
- Consumes: `Entity`, `Op`, `Change`, `PullResponse`, `ErrorCode`.
- Produces: `PullQuery { cursor, offset, limit }`, `PullQuery::parse(Option<i64>,
  Option<i64>, Option<i64>) -> Result<PullQuery, CursorError>`, `PullQuery::kind() ->
  CursorKind`, `is_stale(cursor: i64, min_retained_seq: i64) -> bool`, `DEFAULT_LIMIT`,
  `MAX_LIMIT`, `Entity::payload_columns() -> &'static [&'static str]`,
  `Entity::payload_shape() -> PayloadShape`.

- [ ] **Step 1: Read what Phase 7 landed, and assert its signature**

```bash
sed -n '1,60p' crates/server/src/sync/push.rs     # the module header and the imports
grep -n "pub fn decide\|pub fn push_features\|pub enum Decision" crates/server/src/sync/push.rs
```

If Phase 7 has not landed yet, stop and report: this phase's push route is a transport
around `decide` (A150) and re-implementing it here is the one thing the assignment
forbids. Record the file's real signature in the task's commit message so the next
reader does not have to re-derive it.

- [ ] **Step 2: Write the failing tests**

`crates/sync/tests/protocol.rs` (NEW):

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_sync::proto::cursor::{is_stale, CursorError, CursorKind, PullQuery, DEFAULT_LIMIT, MAX_LIMIT};
use stvdy_sync::proto::{Entity, PayloadShape};

/// The default and the cap are the contract's numbers, and a request outside them is
/// clamped rather than refused — except a limit of 0 or a negative, which is a caller
/// bug and not a smaller page.
#[test]
fn the_page_size_is_defaulted_and_capped() {
    assert_eq!(PullQuery::parse(None, None, None).unwrap().limit, DEFAULT_LIMIT);
    assert_eq!(PullQuery::parse(Some(0), None, Some(10_000)).unwrap().limit, MAX_LIMIT);
    assert_eq!(PullQuery::parse(Some(0), None, Some(1)).unwrap().limit, 1);
    assert!(matches!(
        PullQuery::parse(Some(0), None, Some(0)),
        Err(CursorError::LimitOutOfRange { .. })
    ));
    assert!(matches!(
        PullQuery::parse(Some(-1), None, None),
        Err(CursorError::NegativeCursor)
    ));
}

/// `offset` is meaningful only inside an initial pass. Accepting it alongside a
/// non-zero cursor would page a rank-ordered stream with a seq cursor and deliver the
/// wrong rows, which is worse than a 400.
#[test]
fn an_offset_is_only_legal_on_the_initial_pass() {
    assert!(matches!(
        PullQuery::parse(Some(0), Some(20), None).unwrap().kind(),
        CursorKind::Initial { offset: 20 }
    ));
    assert!(matches!(
        PullQuery::parse(Some(41), None, None).unwrap().kind(),
        CursorKind::Incremental { after: 41 }
    ));
    assert!(matches!(
        PullQuery::parse(Some(41), Some(20), None),
        Err(CursorError::OffsetWithCursor)
    ));
}

/// A new device must never be told to resync: `seq` comes from a global sequence, so an
/// account's first change can be seq 500 and a watermark comparison against `min(seq)`
/// would 410 the very first request (A165).
#[test]
fn a_cursor_of_zero_is_never_stale_and_a_cursor_below_the_watermark_is() {
    assert!(!is_stale(0, 500));
    assert!(!is_stale(0, 0));
    assert!(!is_stale(500, 500));
    assert!(is_stale(499, 500));
}

/// The payload allowlist is the leak surface: `user_id` is the caller's own and
/// redundant, and the two storage keys are internal (A168).
#[test]
fn the_payload_allowlist_excludes_the_identity_and_the_storage_keys() {
    for entity in Entity::ALL {
        let columns = entity.payload_columns();
        assert!(!columns.is_empty(), "{entity:?} ships no columns at all");
        assert!(
            !columns.contains(&"user_id"),
            "{entity:?} ships user_id; every row already belongs to the caller"
        );
        assert!(
            columns.contains(&"updated_at"),
            "{entity:?} must ship updated_at: the client's LWW display needs it"
        );
    }

    let source = Entity::Source.payload_columns();
    assert!(!source.contains(&"file_key"), "an object key is not a client field");
    assert!(!source.contains(&"preview_key"), "an object key is not a client field");
    assert!(source.contains(&"status"), "the client renders ingestion status");

    assert_eq!(Entity::Task.payload_shape(), PayloadShape::PatchWithBase);
    assert_eq!(Entity::Event.payload_shape(), PayloadShape::PatchWithBase);
    assert_eq!(Entity::FlashcardReview.payload_shape(), PayloadShape::Row);
    assert_eq!(Entity::UserSettings.payload_shape(), PayloadShape::Row);
}
```

- [ ] **Step 3: Run them and watch them fail**

`cargo test -p stvdy-sync --test protocol` → `error[E0432]: unresolved import
stvdy_sync::proto::cursor`.

- [ ] **Step 4: Implement**

`crates/sync/src/proto/cursor.rs`:

```rust
pub const DEFAULT_LIMIT: i64 = 200;
pub const MAX_LIMIT: i64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullQuery {
    pub cursor: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("cursor cannot be negative")]
    NegativeCursor,
    #[error("offset cannot be negative")]
    NegativeOffset,
    #[error("limit must be between 1 and {MAX_LIMIT}")]
    LimitOutOfRange { limit: i64 },
    #[error("offset is only meaningful when cursor is 0")]
    OffsetWithCursor,
}

/// What a cursor asks for. The two arms are different queries, not two spellings of
/// one: `Initial` is a snapshot pass over a rank-ordered stream (A164), `Incremental`
/// is a walk up the seq clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Initial { offset: i64 },
    Incremental { after: i64 },
}

impl PullQuery {
    pub fn parse(cursor: Option<i64>, offset: Option<i64>, limit: Option<i64>)
        -> Result<Self, CursorError>;
    pub fn kind(&self) -> CursorKind;
}

/// A cursor older than retained history must resync (A165). `min_retained_seq` is the
/// per-user watermark compaction writes; 0 means nothing has ever been compacted.
pub fn is_stale(cursor: i64, min_retained_seq: i64) -> bool {
    cursor > 0 && cursor < min_retained_seq
}
```

`entities.rs` gains `payload_columns()` (one arm per entity, matching the DDL actually
read in Step 5 of Task 3), `PayloadShape`, and `payload_shape()`; `proto/mod.rs` gains
`pub mod cursor;` and re-exports `cursor::{is_stale, CursorError, CursorKind, PullQuery,
DEFAULT_LIMIT, MAX_LIMIT}` and `entities::PayloadShape`. `Cargo.toml` gains
`async-trait.workspace = true` and `tokio.workspace = true` (the engine in Task 8 needs
both; adding them here keeps the manifest change in one commit).

- [ ] **Step 5: Run the tests and the wire suite**

`cargo test -p stvdy-sync` → green, including the existing `wire.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/sync/Cargo.toml crates/sync/src/proto crates/sync/tests/protocol.rs
git commit -m "feat(sync): the cursor rules, the payload allowlist, and the shapes the client reads"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 2: `migrations/0027_sync_watermarks.sql` — the table the 410 needs

**Files:**
- Create: `migrations/0027_sync_watermarks.sql`
- Modify: `crates/server/tests/schema.rs`, `docs/ASSUMPTIONS.md` (§ Schema deviations)

**Interfaces:**
- Produces: table `change_log_watermark (user_id uuid primary key references users on
  delete cascade, min_retained_seq bigint not null default 0, compacted_at timestamptz)`.

- [ ] **Step 1: Write the failing test**

In `crates/server/tests/schema.rs`, add `change_log_watermark` to `NON_SYNC_TABLES`
(`:34`, so `non_sync_tables_have_no_triggers` covers it) and **not** to `SYNC_TABLES`
(`:17`, so `the_trigger_set_is_exactly_the_sync_tables` keeps passing), and add the
table to the `documented` array (`:283`):

```rust
    // Phase 8. Compaction's per-user watermark: the seq below which this account's
    // history is gone, which is what a cursor is compared against for 410 (A165).
    // Not a `+sync` table: it is a server-side cursor fact with no client entity, and
    // a device that learned it would learn nothing it could act on.
    "change_log_watermark",
```

```rust
/// A table with no client entity must never be able to reach one (A165/A184): no
/// triggers, and a primary key that is the user id rather than a row id.
#[sqlx::test(migrations = "../../migrations")]
async fn the_watermark_is_server_side_only(pool: PgPool) {
    let triggers: i64 = sqlx::query_scalar(
        "select count(*) from pg_trigger t join pg_class c on c.oid = t.tgrelid
         where not t.tgisinternal and c.relname = 'change_log_watermark'",
    ).fetch_one(&pool).await.expect("count");
    assert_eq!(triggers, 0);

    let cols: Vec<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
         where table_name = 'change_log_watermark'",
    ).fetch_all(&pool).await.expect("columns");
    assert_eq!(
        cols.iter().cloned().collect::<std::collections::BTreeSet<_>>(),
        ["user_id", "min_retained_seq", "compacted_at"]
            .iter().map(|s| s.to_string()).collect()
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p stvdy-server --test schema the_watermark_is_server_side_only` →
`relation "change_log_watermark" does not exist`. `cargo test -p stvdy-server --test
schema tables_added_beyond_the_spec_are_deliberate` → passes only because the table
does not exist yet; the row added in Step 1 is what keeps it passing after Step 3.

- [ ] **Step 3: Write the migration**

```sql
-- Compaction's per-user watermark.
--
-- A cursor is compared against `min_retained_seq` to decide 410 resync_required
-- (docs/SYNC.md). It cannot be compared against `min(seq)`: seq is a bigserial, so a
-- brand-new account's first change can be seq 500 and a device at cursor 0 would be
-- told to resync on its first request (A165).
--
-- Not a `+sync` table, so it must not appear in 0017_sync_triggers.sql's array. 0017
-- stays the last trigger-attaching migration: this file creates no triggers.
create table change_log_watermark (
  user_id uuid primary key references users on delete cascade,
  min_retained_seq bigint not null default 0,
  compacted_at timestamptz
);
```

- [ ] **Step 4: Run the schema suite**

`cargo test -p stvdy-server --test schema` → green.

- [ ] **Step 5: Record the deviation**

Add to `docs/ASSUMPTIONS.md` § "Schema deviations from the spec": `change_log_watermark`
(A165/A184). It is one of the rows of A164–A189 and lands with this commit.

- [ ] **Step 6: Commit**

```bash
git add migrations/0027_sync_watermarks.sql crates/server/tests/schema.rs docs/ASSUMPTIONS.md
git commit -m "feat(server): a per-user compaction watermark, so a cursor can be told it is stale"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 3: `crates/server/src/sync/pull.rs` — `GET /sync/pull`

**Files:**
- Create: `crates/server/src/sync/mod.rs`, `crates/server/src/sync/pull.rs`,
  `crates/server/src/sync/log.rs`, `crates/server/tests/sync_pull.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`,
  `crates/server/tests/route_sweep.rs`, `crates/server/src/ratelimit.rs`

**Interfaces:**
- Consumes: `stvdy_sync::proto::{PullResponse, Change, Entity, Op}`, `PullQuery`,
  `is_stale`, `AppError::ResyncRequired`, `AuthUser`.
- Produces: `sync::routes() -> Router<AppState>`, `sync::pull::pull(State, AuthUser,
  Query<PullParams>) -> AppResult<Json<PullResponse>>`, `sync::pull::load_page(&mut
  PgConnection, Uuid, &PullQuery) -> AppResult<Page>`, `sync::pull::load_payload(&mut
  PgConnection, Uuid, Entity, &[Uuid]) -> AppResult<HashMap<Uuid, Value>>`,
  `sync::pull::min_retained_seq(&mut PgConnection, Uuid) -> AppResult<i64>`,
  `sync::touch_last_sync(&PgPool, Uuid) -> sqlx::Result<()>`, `ratelimit::sync_layer()`.

- [ ] **Step 1: Write the failing tests**

`crates/server/tests/sync_pull.rs` (NEW):

```rust
/// The order the roadmap fixes: an initial pass is dependency rank first, then seq, so
/// a client's cache has every parent before its children (A164).
#[sqlx::test(migrations = "../../migrations")]
async fn the_initial_pass_is_rank_ordered(pool: PgPool) { … }

/// A change committed while the pass is running must not shift a row out of the pass.
/// This is the defect the anchor exists for: a new rank-1 workspace inserted between
/// two pages moves every later row one position right, and an unanchored offset paging
/// would skip the row that fell across the boundary — then advance the cursor past it,
/// losing it permanently.
#[sqlx::test(migrations = "../../migrations")]
async fn a_write_during_the_initial_pass_cannot_shift_a_row_out_of_it(pool: PgPool) { … }

/// The named risk of this phase, as a test. A pull for A contains no seq that belongs
/// to B, and no row id that B owns.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pull_never_contains_another_accounts_seq(pool: PgPool) { … }

/// The same risk one layer down: a planted `change_log` row for A whose entity_id is
/// B's task must not produce B's row. It produces `data: null`, which the client
/// applies as a delete (A188), never a cross-tenant read.
#[sqlx::test(migrations = "../../migrations")]
async fn the_payload_loader_cannot_be_pointed_at_another_users_row(pool: PgPool) { … }

/// `410 resync_required` is the contract's code and it is already the error's mapping
/// (A166): no new variant, no new body shape.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cursor_below_the_watermark_is_410_resync_required(pool: PgPool) { … }

/// A tombstone is `op: "delete"` with `data: null`, and the seq of a tombstone is
/// returned like any other change.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tombstone_carries_no_payload(pool: PgPool) { … }

/// Paging: `limit` rows, `has_more` true while more remain, and `next_cursor` equal to
/// the last seq of the page.
#[sqlx::test(migrations = "../../migrations")]
async fn a_page_is_limited_and_has_more_says_whether_to_ask_again(pool: PgPool) { … }

/// Pulling writes `devices.last_sync_at` (A180), which is the only source of the
/// Devices screen's "Last synced".
#[sqlx::test(migrations = "../../migrations")]
async fn a_pull_records_when_this_device_last_synced(pool: PgPool) { … }

/// A `user_settings` change is keyed by the user id, because that is what
/// `log_change_by_user` writes into `entity_id` (A179).
#[sqlx::test(migrations = "../../migrations")]
async fn a_settings_change_is_keyed_by_the_user_id(pool: PgPool) { … }

/// A limit of 0 is a 400, and a limit over the cap is clamped rather than refused.
#[sqlx::test(migrations = "../../migrations")]
async fn the_page_size_is_validated_and_clamped(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test sync_pull` → `error[E0433]: could not find sync in
stvdy_server`.

- [ ] **Step 3: Implement the module root and the route**

`crates/server/src/sync/mod.rs`:

```rust
//! The sync routes: pull, push, stream, plus the compaction job's shared helpers.
//!
//! The per-mutation decision is NOT here. `sync::push::decide` landed in Phase 7 (A150)
//! and this module adds only the transport around it: the advisory lock, the
//! idempotency log, the apply, and the results.

pub mod apply;
pub mod log;
pub mod pull;
// Tasks 4, 5 and 6 add `pub mod push;`, `pub mod apply;` and `pub mod stream;` together
// with their route lines, so each of those tasks compiles and passes on its own.

use axum::Router;
use axum::routing::get;
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    // One `.route(...)` per task that lands it: `post("/sync/push", …)` in Task 4 and
    // `get("/sync/stream", …)` in Task 6 join this call, so the module never declares a
    // handler that does not exist yet.
    Router::new().route("/sync/pull", get(pull::pull))
}

/// Records that this device just talked to the server.
///
/// The Devices screen's "Last synced" reads `devices.last_sync_at`, and nothing wrote
/// it before this phase (A180). A locally invented time would be a number the app did
/// not compute, which is what roadmap gate 7(a) forbids.
pub async fn touch_last_sync(pool: &PgPool, device_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("update devices set last_sync_at = now(), last_seen_at = now() where id = $1")
        .bind(device_id)
        .execute(pool)
        .await
        .map(|_| ())
}
```

`crates/server/src/sync/pull.rs` carries the two queries the cursor needs:

```rust
/// The page of `change_log` rows this cursor asks for.
///
/// `Initial` anchors at the user's `max(seq)` so the pass set cannot change under it
/// (A164); `Incremental` walks `seq > cursor` ascending. Both are scoped by `user_id`
/// in the same statement — never filtered afterwards — because that is this phase's
/// named risk.
pub async fn load_page(conn: &mut PgConnection, user_id: Uuid, q: &PullQuery) -> AppResult<Page>;

/// The current row for each id, restricted to the entity's payload allowlist (A168).
///
/// `where user_id = $1 and id = any($2)` is the whole defence: an id that belongs to
/// another account loads nothing, and the caller emits `data: null` (A188).
pub async fn load_payload(conn: &mut PgConnection, user_id: Uuid, entity: Entity, ids: &[Uuid])
    -> AppResult<HashMap<Uuid, Value>>;
```

The per-entity select is built from `Entity::payload_columns()`, so a column added to a
table and not to the allowlist is *not* shipped (the safe direction), and the query is
written with an explicit column list — never `SELECT *`, which the roadmap's risk
register names because sqlx 0.9 macros cannot map `vector`/`tsvector`.

The handler is one transaction: read `min_retained_seq`, refuse a stale cursor with
`AppError::ResyncRequired`, load the page, load the payloads per entity present in the
page, `touch_last_sync`, commit. It logs `latency_ms`, `rows`, `cursor` and `has_more`
in one `tracing::info!` (the roadmap requires sync pull latency to be logged).

- [ ] **Step 4: Register the route, classify it, and rate-limit it**

- `crates/server/src/lib.rs` gains `pub mod sync;` (alphabetical, before `telemetry`).
- `crates/server/src/routes/mod.rs` gains `.merge(sync::routes())` beside the other
  merges (`:118-128`).
- `crates/server/tests/route_sweep.rs` gains, in `read_only_rows()` (`:61`):

```rust
        ro(
            "GET",
            "/sync/pull",
            "reads the caller's own change log; a pull that needed a plan would break account sync on free",
        ),
```

- `crates/server/src/ratelimit.rs` gains `sync_layer()` keyed per account at 600
  requests/minute (A185), applied to the `/sync` routes in `routes()`.

- [ ] **Step 5: Run the tests**

`cargo test -p stvdy-server --test sync_pull` → green.
`cargo test -p stvdy-server --test route_sweep` → green (`/sync/pull` is classified).

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/sync crates/server/src/{lib.rs,routes/mod.rs,ratelimit.rs} \
        crates/server/tests/{sync_pull.rs,route_sweep.rs}
git commit -m "feat(server): GET /sync/pull, anchored, ranked, and scoped by user in the statement itself"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change. (Gate 6's data half is
exercised in Task 4 and Task 11.)

---

## Task 4: `crates/server/src/sync/push.rs` — `POST /sync/push`, idempotent

**Files:**
- Create: `crates/server/src/sync/push.rs`, `crates/server/src/sync/apply.rs`,
  `crates/server/tests/sync_push.rs`
- Modify: `crates/server/tests/route_sweep.rs`

**Interfaces:**
- Consumes: `sync::push::decide` and `push_features` (Phase 7, A150), `Entity`,
  `Mutation`, `PushRequest`, `PushResponse`, `MutationResult`, `ErrorCode::as_str`,
  `AuthUser::{device_id, user_id}`.
- Produces: `sync::push::push(State, AuthUser, Json<PushRequest>) ->
  AppResult<Json<PushResponse>>`, `sync::push::apply_batch(&AppState, &AuthUser,
  &PushRequest) -> AppResult<PushResponse>`, `sync::apply::{Outcome, apply_one}`.

`apply.rs` is created here with the plain paths — insert, update, tombstone, and the
append-only `on conflict (id) do nothing` — so this task's batch compiles and its tests
pass alone. Task 5 extends it with the field-level merge; it does not introduce the
module.

- [ ] **Step 1: Write the failing tests**

`crates/server/tests/sync_push.rs` (NEW):

```rust
/// The phase's second named risk, asserted as facts and not as a response shape: a
/// replay returns the stored result AND applies nothing — the row's `updated_at` is
/// unchanged and `change_log` gained no row.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_mutation_applies_nothing(pool: PgPool) { … }

/// A rejected mutation does not abort the batch: one batch carries a `Message`
/// (read_only_entity), a `Source` (read_only_entity) and a `Task` (applied), and the
/// task lands.
#[sqlx::test(migrations = "../../migrations")]
async fn one_rejected_mutation_does_not_abort_the_batch(pool: PgPool) { … }

/// The threat model's two rows, through HTTP rather than through `decide`: `Message`
/// and `Source` are `read_only_entity`, with the exact code.
#[sqlx::test(migrations = "../../migrations")]
async fn a_message_and_a_source_are_refused_over_the_wire(pool: PgPool) { … }

/// `flashcards.ease/interval/due` is `read_only_entity` (§5), and a `flashcard_review`
/// — which is pushable — is not confused with it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_derived_flashcard_field_is_refused_and_a_review_is_not(pool: PgPool) { … }

/// The lock-order rule, as a test: two devices push a two-mutation batch touching the
/// same rows concurrently (a task plus its subtask) and both succeed — no `40P01`.
#[sqlx::test(migrations = "../../migrations")]
async fn two_devices_editing_overlapping_rows_both_succeed(pool: PgPool) { … }

/// A mutation whose id belongs to another account is `not_found`, and the other
/// account's row is untouched (A173).
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_row_is_not_found_and_never_written(pool: PgPool) { … }

/// A `mutation_id` already recorded for a different account is a conflict, and the
/// first account's stored result is not returned (A172).
#[sqlx::test(migrations = "../../migrations")]
async fn a_mutation_id_belonging_to_another_account_is_a_conflict(pool: PgPool) { … }

/// A body claiming another device's id is 403 (A171): `origin_device_id` is only worth
/// recording if the claim is checkable.
#[sqlx::test(migrations = "../../migrations")]
async fn a_body_cannot_claim_another_device(pool: PgPool) { … }

/// A duplicate append-only id is ignored and reports the row's existing seq, so a
/// retried review does not read as a new change (A174).
#[sqlx::test(migrations = "../../migrations")]
async fn a_duplicate_review_is_ignored_and_reports_the_existing_seq(pool: PgPool) { … }

/// An Indonesian task title and notes survive the round trip byte-exact, in both
/// directions (roadmap gate 6, the data half).
#[sqlx::test(migrations = "../../migrations")]
async fn indonesian_text_round_trips_byte_exact(pool: PgPool) { … }

/// A push writes `devices.last_sync_at` (A180) and sets `origin_device_id` to the
/// pushing device.
#[sqlx::test(migrations = "../../migrations")]
async fn a_push_records_the_device_and_the_origin(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test sync_push` → `error[E0433]`.

- [ ] **Step 3: Implement the batch**

```rust
/// One transaction for the whole batch, and the order inside it is the contract:
///
///   1. `select pg_advisory_xact_lock(hashtext($1::text))` — the roadmap's lock-order
///      rule. The per-user lock also lives inside the AFTER trigger, so a batch that
///      wrote rows first would acquire it *after* its row locks and two devices could
///      deadlock (40P01). Taking it first makes the order uniform everywhere.
///   2. `select set_config('app.device_id', $1, true)` — so `origin_device_id` is
///      populated without a device column on every table.
///   3. Per mutation: a SAVEPOINT, the replay lookup, `decide`, the apply, the
///      `mutation_log` insert, then release. A failed apply rolls back to its savepoint
///      and the batch continues (A170).
pub async fn apply_batch(state: &AppState, user: &AuthUser, body: &PushRequest)
    -> AppResult<PushResponse>;
```

The per-mutation sequence, exactly:

1. `log::replay(tx, user.user_id, m.mutation_id)` → `Some(result)` returns the stored
   `MutationResult` and applies nothing.
2. `entitlements::snapshot_in` for this user, then `decide(&snapshot, m)` (Phase 7's
   function, not a copy). `Err(result)` is pushed into the response and the savepoint is
   rolled back.
3. `apply::apply_one(tx, user.user_id, m)`.
4. `log::record(tx, user.user_id, user.device_id, m.mutation_id, &result)`.

`touch_last_sync` runs once, after the loop, in the same transaction.

- [ ] **Step 4: Register the route and classify it**

`crates/server/src/sync/mod.rs` gains `pub mod push;` and
`.route("/sync/push", post(push::push))` on the router (with `post` added to the
`axum::routing` import); `crates/server/src/sync/apply.rs` and `log.rs` are the two
modules the batch needs and Task 5 extends.

`crates/server/tests/route_sweep.rs` gains a `POST /sync/push` row. Phase 7 introduces
`Class::SweptPerMutation` for it (A149); if that variant is absent when this task runs,
add it here with the driver case, whose assertion is the interesting one: an account
whose every limit is used still gets its task **applied**, because all six pushable
entities are free (`push_features` returns `&[]`, R1) — and the refusal path is asserted
per mutation instead, with a `Message`.

- [ ] **Step 5: Run the tests**

`cargo test -p stvdy-server --test sync_push` → green.
`cargo test -p stvdy-server --test route_sweep` → green.
`cargo test -p stvdy-server --test tamper` → green (Phase 7's rows are now enforced
through the route as well as through `decide`).

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/sync/push.rs crates/server/tests/{sync_push.rs,route_sweep.rs}
git commit -m "feat(server): POST /sync/push takes the lock first and applies a replayed mutation never"
```

**Gates:** 5 — no UI change. 6 — the data half is asserted here (Indonesian text
byte-exact through push and pull); the copy half lands in Task 10. 7 — no UI change.

---

## Task 5: `crates/server/src/sync/apply.rs` — the conflict table, decided once

**Files:**
- Extend: `crates/server/src/sync/apply.rs` (created in Task 4)
- Create: `crates/sync/src/client/mod.rs`, `crates/sync/src/client/conflict.rs`
- Modify: `crates/server/src/sync/log.rs`, `crates/sync/src/lib.rs` (`pub mod client;`)
- Test: `crates/server/tests/sync_push.rs` (new cases)

**Interfaces:**
- Consumes: `Entity`, `Mutation`, `ErrorCode`.
- Produces: `sync::apply::apply_one(&mut PgConnection, Uuid, &Mutation) ->
  AppResult<Outcome>`, `sync::apply::merge_patch(&Value, Option<&Value>, &Value) ->
  Merge`, `enum Merge { Disjoint(Value), Overlap, Tombstone }`, `enum Outcome`,
  `stvdy_sync::client::conflict::{Rule, rule_for}`.

The conflict table lands in `crates/sync` here rather than in Task 8, because the
server's apply is its first reader and A175's whole point is that there is one copy.
`crates/sync/src/client/mod.rs` is created with `pub mod conflict;` and its re-exports;
Task 8 adds the remaining modules to it.

- [ ] **Step 1: Write the failing tests**

Append to `crates/server/tests/sync_push.rs`:

```rust
/// Two devices edit disjoint fields of one task and both edits survive — the
/// field-level merge the conflict table promises, computed from the patch and the base
/// the client sent (A176/A177).
#[sqlx::test(migrations = "../../migrations")]
async fn disjoint_task_edits_both_survive(pool: PgPool) { … }

/// Two devices edit the same field: the incoming patch wins whole, because a
/// per-field tie-break would need a per-field clock the schema does not have.
#[sqlx::test(migrations = "../../migrations")]
async fn overlapping_task_edits_resolve_by_server_receive_time(pool: PgPool) { … }

/// The exit gate's "tombstone vs edit": a patch against a tombstoned task is a
/// `conflict` whose `winner` is the tombstone, so the editing device is told rather
/// than silently reverted.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tombstone_beats_a_concurrent_edit(pool: PgPool) { … }

/// The one exception the contract names: an explicit restore (`deleted_at: null`) wins
/// and logs an upsert.
#[sqlx::test(migrations = "../../migrations")]
async fn an_explicit_restore_wins_and_logs_an_upsert(pool: PgPool) { … }

/// A patch that omits `deleted_at` does not resurrect a tombstone (A178).
#[sqlx::test(migrations = "../../migrations")]
async fn a_patch_that_omits_deleted_at_does_not_resurrect(pool: PgPool) { … }

/// A settings push upserts the caller's single row and echoes the user id as the
/// entity id, matching what `log_change_by_user` writes (A179).
#[sqlx::test(migrations = "../../migrations")]
async fn a_settings_push_upserts_the_callers_row(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test sync_push disjoint` → the test exists and fails on
the merge (the first implementation applies the patch whole, so the other device's field
is lost).

- [ ] **Step 3: Implement, with the conflict table in `crates/sync`**

`crates/sync/src/client/conflict.rs` is created here rather than in Task 8, because the
server's apply is its first reader and A175's whole point is that there is one copy:

```rust
/// What the conflict rules say about one entity, as data.
///
/// The roadmap's table is normative and this is its single encoding: the server's apply
/// reads it, the client's reaction reads it, and neither can drift into a second
/// convention (A175). The *decision* is still the server's (A150).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Insert; a duplicate id is ignored. `flashcard_reviews`, `quiz_attempts`,
    /// `study_sessions`.
    AppendOnly,
    /// Record-level last-write-wins on server receive time, with a field-level merge
    /// when the changed field sets are disjoint. `tasks`, `events`.
    LwwWithFieldMerge,
    /// Record-level last-write-wins. `user_settings`.
    Lww,
}

pub fn rule_for(entity: Entity) -> Rule;
```

`apply.rs` then holds the three shapes:

```rust
pub enum Outcome {
    Applied { seq: i64 },
    Rejected { code: ErrorCode, message: String },
    Conflict { winner: Value },
}

pub enum Merge {
    /// The other writer's changed set and this patch's are disjoint: apply the patch.
    Disjoint(Value),
    /// They overlap, or there is no base to compare against: the patch wins whole.
    Overlap,
    /// The stored row is tombstoned and this patch does not restore it.
    Tombstone,
}
```

`apply_one` dispatches on `rule_for(entity)` and, for `Task`/`Event`, on
`Entity::payload_shape() == PatchWithBase` (A176). Every write is
`where id = $1 and user_id = $2` in the statement itself (A173). An append-only insert
is `on conflict (id) do nothing`, and when nothing was inserted the seq is read from
`change_log` so the response reports the existing change (A174).

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test sync_push` → green.
`cargo test -p stvdy-sync` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/sync/apply.rs crates/server/src/sync/log.rs \
        crates/sync/src/client/conflict.rs crates/server/tests/sync_push.rs
git commit -m "feat(server): the conflict table in one place, and the merge a patch and a base make decidable"
```

**Gates:** 5 — no UI change. 6 — no UI change (the text round trip is Task 4). 7 — no UI
change.

---

## Task 6: `crates/server/src/sync/stream.rs` — `GET /sync/stream`

**Files:**
- Create: `crates/server/src/sync/stream.rs`, `crates/server/tests/sync_stream.rs`
- Modify: `crates/server/tests/route_sweep.rs`

**Interfaces:**
- Consumes: `axum::response::sse::{Event, Sse}`, `crate::chat::sse::HEARTBEAT` (15s, the
  one constant the chat stream already uses).
- Produces: `sync::stream::stream(State, AuthUser) -> AppResult<Sse<impl Stream<…>>>`,
  `sync::stream::seq_event(i64) -> Event`, `sync::stream::POLL_INTERVAL` (2s).

- [ ] **Step 1: Write the failing test**

`crates/server/tests/sync_stream.rs` (NEW):

```rust
/// The hint's first frame is the current `max(seq)`, so a device that reconnects after
/// a gap pulls at once instead of waiting for the next change (A181).
#[sqlx::test(migrations = "../../migrations")]
async fn the_stream_opens_with_the_current_seq(pool: PgPool) { … }

/// A change committed after the connection opens reaches the stream within the poll
/// interval, as `event: seq`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_later_change_is_emitted_as_a_seq_event(pool: PgPool) { … }

/// The hint carries the caller's own clock and nothing else: another account's change
/// does not move this stream.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_change_does_not_move_this_stream(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test sync_stream` → `error[E0433]`.

- [ ] **Step 3: Implement**

`crates/server/src/sync/mod.rs` gains `pub mod stream;` and
`.route("/sync/stream", get(stream::stream))`.

An `async_stream::stream!` loop: emit the current `max(seq)` immediately, then every
`POLL_INTERVAL` (2s) re-read `max(seq)` and emit only when it advanced, interleaved with
the 15s heartbeat comment. It is a per-connection poll rather than `LISTEN/NOTIFY`
because that is what makes it correct with more than one server instance and needs no
cross-instance bus (A181). The stream ends when the client disconnects; nothing else
terminates it. The handler logs connection open and close with the user id and the
reason, and it is classified `ReadOnly` in `route_sweep.rs` ("an SSE hint; it reads the
caller's own clock").

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-server --test sync_stream` → green.
`cargo test -p stvdy-server --test route_sweep` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/sync/stream.rs crates/server/tests/{sync_stream.rs,route_sweep.rs}
git commit -m "feat(server): GET /sync/stream, a poll of max(seq) that works behind more than one instance"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 7: `compact_change_log` — the job, the watermark, and the 410 it creates

**Files:**
- Create: `crates/server/src/jobs/handlers/compact.rs`,
  `crates/server/tests/sync_compaction.rs`
- Modify: `crates/server/src/jobs/handlers/mod.rs`, `crates/server/src/jobs/worker.rs`,
  `crates/server/src/jobs/schedule.rs`, `crates/server/src/config.rs`, `.env.example`

**Interfaces:**
- Consumes: `jobs::{enqueue, Failure}`, `Queue`, `PgPool`.
- Produces: `jobs::handlers::compact::run(&AppState, &Job) -> Result<(), Failure>`,
  `compact::compact_user(&mut PgConnection, Uuid, Retention) -> sqlx::Result<Option<i64>>`,
  `config::AppConfig::{change_log_retention_days, change_log_tombstone_retention_days}`.

- [ ] **Step 1: Write the failing tests**

`crates/server/tests/sync_compaction.rs` (NEW):

```rust
/// Upserts older than the retention window go; tombstones younger than the tombstone
/// window stay, because a device that has been offline for three months must be told
/// that a row was removed rather than silently keep it (A184).
#[sqlx::test(migrations = "../../migrations")]
async fn compaction_keeps_tombstones_longer_than_it_keeps_upserts(pool: PgPool) { … }

/// The watermark is the highest seq the job actually removed, so a cursor at or above
/// it is still servable and a cursor below it is 410 (A165).
#[sqlx::test(migrations = "../../migrations")]
async fn the_watermark_is_the_first_seq_that_survived(pool: PgPool) { … }

/// An account whose history is entirely inside the window is untouched, and its
/// watermark stays 0 — which is what keeps a new device's first pull from being 410.
#[sqlx::test(migrations = "../../migrations")]
async fn an_account_inside_the_window_is_untouched(pool: PgPool) { … }

/// The job is a singleton: two enqueues in one day produce one row, because the unique
/// index on `jobs.dedupe_key` is the mechanism the roadmap names.
#[sqlx::test(migrations = "../../migrations")]
async fn the_job_is_a_singleton_per_day(pool: PgPool) { … }

/// A cursor below the watermark gets 410 through the route, and a client that resets
/// to 0 then receives the whole surviving history.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_cursor_is_told_to_resync_and_cursor_zero_always_works(pool: PgPool) { … }

/// Another account's rows are not touched by this account's compaction, and one
/// account's watermark does not 410 another's cursor.
#[sqlx::test(migrations = "../../migrations")]
async fn compaction_is_scoped_to_one_account(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test sync_compaction` → `error[E0433]`.

- [ ] **Step 3: Implement the handler**

One transaction per user: delete `op = 'upsert'` rows older than
`CHANGE_LOG_RETENTION_DAYS`, delete `op = 'delete'` rows older than
`CHANGE_LOG_TOMBSTONE_RETENTION_DAYS`, then write
`change_log_watermark.min_retained_seq = coalesce(min(seq) over the account's remaining
rows, 0)`. The two config keys default to `90` and `180` (A184) and join the explicit
key list in `crates/server/src/config.rs` and `.env.example`, because
`AppConfig::from_env_at` falls back to the process environment for anything not listed
and a developer's `.env` must not silently set a retention window.

- [ ] **Step 4: Schedule it**

`jobs/schedule.rs` (Phase 7) gains a daily arm:
`enqueue(pool, queue, "compact_change_log", json!({}), Some(&format!("compact_change_log:{}", Utc::now().date_naive())))`.
`jobs/worker.rs::dispatch` gains the arm, and `jobs/handlers/mod.rs` the `pub mod`.
A failure is `Failure::transient` (a lock or a connection is worth a retry); a bad
config is permanent and surfaces at boot, not here.

- [ ] **Step 5: Run the tests**

`cargo test -p stvdy-server --test sync_compaction` → green.

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/jobs crates/server/src/config.rs .env.example \
        crates/server/tests/sync_compaction.rs
git commit -m "feat(server): compact_change_log, its two retention windows, and the watermark it writes"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 8: `crates/sync/src/client/` — the engine, the outbox, the apply

**Files:**
- Create: `crates/sync/src/client/{store,transport,outbox,apply,engine}.rs`,
  `crates/sync/tests/engine.rs`
- Modify: `crates/sync/src/client/mod.rs` (created by Task 5 with `conflict`),
  `crates/sync/src/lib.rs`

**Interfaces:**
- Produces: `LocalStore`, `ApplyReport`, `StoreError`, `Transport`, `TransportError`,
  `outbox::{next_batch, Retry}`, `apply::{plan, ChangeSet}`, `Engine`,
  `Engine::{sync_once, status, enqueue}`, `SyncState`.

- [ ] **Step 1: Write the failing tests**

`crates/sync/tests/engine.rs` (NEW), over an in-memory `LocalStore` and a scripted
`Transport` (both in the test file, so the traits are exercised by a second
implementation before the shell's SQLite one exists):

```rust
/// One pass pulls until `has_more` is false, applies every page, and leaves the cursor
/// at the last seq — the property the whole client exists for.
#[tokio::test]
async fn a_pass_pulls_until_has_more_is_false_and_advances_the_cursor() { … }

/// The apply is one transaction with `defer_foreign_keys` on, so a page whose child
/// arrives before its parent still lands. The store records the transaction count, so
/// "one transaction" is asserted rather than assumed.
#[tokio::test]
async fn a_page_is_applied_in_one_transaction_with_deferred_foreign_keys() { … }

/// A row whose parent is still missing after the whole pass is deferred, not dropped,
/// and is retried on the next pull — the roadmap's rule.
#[tokio::test]
async fn a_row_whose_parent_is_missing_is_deferred_and_retried() { … }

/// A 410 wipes the local state, resets the cursor to 0 and re-pulls the whole history
/// in the same pass (the exit gate's resync).
#[tokio::test]
async fn a_410_resyncs_from_zero_without_surfacing_an_error() { … }

/// A 401 on this device is `Revoked`: the store is wiped including the outbox, because
/// a pending offline write from a revoked device must not be replayed by the next
/// account that signs in on this machine (A186).
#[tokio::test]
async fn a_401_wipes_the_cache_and_the_outbox() { … }

/// A network failure leaves the outbox intact and the status `offline`; nothing is
/// dropped and the next pass retries in order.
#[tokio::test]
async fn a_network_failure_keeps_the_outbox_in_order() { … }

/// The outbox drains in enqueue order, one mutation per call to the store, and a
/// mutation that is `rejected` is removed with its reason recorded while the rest
/// continue (the server decides; the client only reacts).
#[tokio::test]
async fn the_outbox_drains_in_order_and_a_rejection_does_not_stop_it() { … }

/// Backoff is bounded and monotonic, so a flapping network does not turn into a hot
/// loop against the server.
#[tokio::test]
async fn retry_backoff_is_monotonic_and_capped() { … }

/// Status is derived, never asserted: `synced` requires an empty outbox, a successful
/// pull and a known `last_synced_at` (A187). This is gate 7(b) as a test — a status
/// the app cannot compute must not be shown.
#[tokio::test]
async fn synced_requires_an_empty_outbox_and_a_successful_pass() { … }

/// `data: null` on an `upsert` is applied as a delete, because a row this device
/// cannot see must never be created on it (A188).
#[tokio::test]
async fn a_null_payload_is_applied_as_a_delete() { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-sync --test engine` → `error[E0432]`.

- [ ] **Step 3: Implement the traits and the pass**

```rust
/// Everything the engine needs from a local database, so the engine has no idea
/// whether it is talking to SQLite or to a `HashMap` (A182).
pub trait LocalStore: Send {
    fn cursor(&self) -> Result<i64, StoreError>;
    fn set_cursor(&mut self, cursor: i64) -> Result<(), StoreError>;
    /// Applies a page in ONE transaction with `PRAGMA defer_foreign_keys=ON`.
    fn apply_page(&mut self, changes: &[Change]) -> Result<ApplyReport, StoreError>;
    fn enqueue(&mut self, mutation: Mutation) -> Result<(), StoreError>;
    fn pending(&self, limit: usize) -> Result<Vec<Mutation>, StoreError>;
    fn resolve(&mut self, mutation_id: Uuid, result: &MutationResult) -> Result<(), StoreError>;
    fn outbox_len(&self) -> Result<usize, StoreError>;
    fn last_synced_at(&self) -> Result<Option<DateTime<Utc>>, StoreError>;
    fn set_last_synced_at(&mut self, at: DateTime<Utc>) -> Result<(), StoreError>;
    fn wipe(&mut self) -> Result<(), StoreError>;
}

pub struct ApplyReport {
    pub applied: usize,
    /// Rows whose parent is still missing after the page: retried on the next pull.
    pub deferred: Vec<(Entity, Uuid)>,
}

pub trait Transport: Send + Sync {
    async fn pull(&self, query: PullQuery) -> Result<PullResponse, TransportError>;
    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError>;
}

pub enum TransportError {
    /// Nothing was reached: keep the outbox, retry with backoff, status `offline`.
    Network(String),
    /// This device's session is gone (A186).
    Revoked,
    /// The cursor is older than retained history: wipe and resync (A166).
    Stale,
    Server { status: u16, code: String },
}
```

`sync_once` is: flush the outbox (one push per batch, stopping on `Network`), then pull
pages until `has_more` is false or a page budget is hit, applying each page in one
transaction, then record `last_synced_at` from the transport's own clock... no — from the
local clock at the moment the pass succeeded. That is a number the app computed (the
pass happened), which is what gate 7(b) requires; the server's `devices.last_sync_at`
is what the Devices screen shows for the device list.

- [ ] **Step 4: Run the tests**

`cargo test -p stvdy-sync` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/sync/src/client crates/sync/src/lib.rs crates/sync/tests/engine.rs
git commit -m "feat(sync): the client engine, generic over the store and the transport"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — (a)/(b) are asserted here as tests
(the status derivation), which is the mechanical half of the gate; the screen is
Task 10.

---

## Task 9: `src-tauri` — the SQLite cache, the outbox, and the wipe

**Files:**
- Create: `src-tauri/src/cache/{mod,schema,queries}.rs`,
  `src-tauri/src/sync/{mod,transport,commands}.rs`, `src-tauri/tests/cache.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/src/commands.rs`

**Interfaces:**
- Produces: `cache::{open, Connection, wipe}`, `cache::queries::SqliteStore`
  (implementing `stvdy_sync::client::LocalStore`), `sync::transport::HttpTransport`
  (implementing `Transport`), the IPC commands `sync_status`, `sync_read`, `sync_write`,
  `sync_tombstone`, `sync_now`, `sync_reset`, `sync_configure`.

- [ ] **Step 1: Write the failing tests**

`src-tauri/tests/cache.rs` (NEW). These run under
`cargo test --manifest-path src-tauri/Cargo.toml`, which CI already runs on macOS:

```rust
/// The schema is created on open and opening twice is a no-op, because the app opens
/// the cache on every launch and a migration that only works once is a launch crash.
#[test]
fn the_schema_is_created_and_reopening_is_idempotent() { … }

/// The pragma the roadmap names, asserted on the connection rather than assumed:
/// `PRAGMA defer_foreign_keys` reads back as 1 inside the apply transaction.
#[test]
fn the_apply_transaction_defers_foreign_keys() { … }

/// A child that arrives before its parent in the same page lands, because the FK check
/// is deferred to commit, by which point the parent is there.
#[test]
fn a_child_before_its_parent_in_one_page_still_lands() { … }

/// A row whose parent is missing after the page is deferred and survives a restart: it
/// is retried on the next pull rather than dropped.
#[test]
fn a_deferred_row_survives_a_restart_and_is_retried() { … }

/// The outbox survives a restart in order, so an offline write made before the app was
/// killed is pushed when it comes back.
#[test]
fn the_outbox_survives_a_restart_in_order() { … }

/// The wipe removes the cache AND the outbox, and leaves no row behind that the next
/// account could read (A186).
#[test]
fn a_wipe_removes_the_cache_and_the_outbox() { … }

/// The access token never reaches the database: the file is scanned for the token
/// string after a configure + pass, because the keychain is the only place a token may
/// live (the spec's "tokens only in keychain/keystore").
#[test]
fn the_access_token_is_not_written_to_the_database() { … }

/// The cache holds only the user's own synced entities: a row for another account is
/// rejected by the store's own scoping rather than stored and filtered on read.
#[test]
fn the_store_rejects_a_row_for_another_account() { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test --manifest-path src-tauri/Cargo.toml --test cache` → the test file cannot
compile: no `stvdy_shell_lib::cache`.

- [ ] **Step 3: Add the dependencies and the cache**

`src-tauri/Cargo.toml` gains:

```toml
stvdy-sync = { path = "../crates/sync" }
rusqlite = { version = "0.37", features = ["bundled"] }
reqwest = { version = "0.13.5", default-features = false, features = ["json", "rustls"] }
tokio = { version = "1.53.2", features = ["rt", "macros", "time", "sync"] }
uuid = { version = "1.28.0", features = ["v4", "v7", "serde"] }
chrono = { version = "0.4.45", features = ["serde"] }
```

`bundled` is deliberate: it is what `docs/ARCHITECTURE.md` § Local cache already states,
and it removes a system SQLite from the platform matrix. `rusqlite` and `reqwest` live
here and not in `crates/sync`, so the server's test binaries never link a C SQLite
(A182). The shell has no HTTP client today (verified), so `reqwest` is new here.

`cache/schema.rs` creates one table per `Entity` (columns from the same allowlist the
pull uses, plus `seq`), the outbox (`mutation_id`, `entity`, `op`, `id`, `payload`,
`client_ts`, `state`, `attempts`, `last_error`), the deferred-children table, and the
single-row `sync_state` (cursor, last_synced_at, last_error, device_id).

- [ ] **Step 4: The commands, and the token handshake**

The SPA owns the session, so the shell is handed the access token rather than storing
it: `sync_configure { base_url, access_token, device_id }` sets them in memory, and the
engine emits `stvdy://sync-auth-expired` when it needs a fresh one. The token is never
written to SQLite (asserted in Step 1). `sync_status` returns the `SyncState` plus the
outbox length; `sync_read`/`sync_write`/`sync_tombstone` are the SPA's local-first path;
`sync_now` runs one pass; `sync_reset` wipes and resyncs.

- [ ] **Step 5: Run the tests**

`cargo test --manifest-path src-tauri/Cargo.toml` → green.
`cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` and `cargo clippy
--manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` → clean.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src src-tauri/tests
git commit -m "feat(shell): the SQLite cache, the outbox, and the wipe a revoked device needs"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — (a): the cache stores no number the
app did not compute, asserted by the token-scan test.

---

## Task 10: The SPA — status, the Devices screen, and the online-only web path

**Files:**
- Create: `app/src/features/sync/{SyncStatus.tsx,SyncStatus.test.tsx}`,
  `app/src/features/devices/{DevicesScreen.tsx,DevicesScreen.test.tsx}`,
  `app/src/api/queries/{sync.ts,devices.ts}`
- Modify: `app/src/platform/{types.ts,web.ts,desktop.ts,mobile.ts}`,
  `app/src/components/layout/{Shell,TabBar,Sidebar}.tsx`, `app/src/api/sse.ts`,
  `app/src/router.tsx`, `app/src/i18n/locales/{en,id}.json`

**Interfaces:**
- Produces: `SyncApi { status, read, write, tombstone, syncNow, reset, onStatus }` on
  `Platform`, `SyncStatus` union gaining `'needs_attention'`, `useSyncStatus()`,
  `useDevices()`, `DevicesScreen`.

- [ ] **Step 1: Write the failing tests**

`app/src/features/sync/SyncStatus.test.tsx`:

```tsx
/**
 * Gate 7(d) at the level the component can prove: a status is never rendered as
 * "Synced" while the outbox is non-empty, and a rejection shows the server's code.
 */
it('never says Synced while the outbox has a pending write', …)
it('shows the server code for a rejected mutation and marks the row not saved', …)
it('renders "Last synced" from the stored timestamp, never from a fresh clock read', …)
it('is reachable and operable by keyboard alone, with a visible focus ring', …)
```

`app/src/features/devices/DevicesScreen.test.tsx`:

```tsx
it('lists devices with their platform and last-synced time', …)
it('revokes a device after confirmation and removes it from the list', …)
it('offers "Reset local data and resync" and says what it deletes', …)
```

- [ ] **Step 2: Run them and watch them fail**

`npm --prefix app test -- SyncStatus` → `Cannot find module '@/features/sync/SyncStatus'`.

- [ ] **Step 3: Implement**

- `types.ts`: `SyncStatus` gains `'needs_attention'` (the spec's four states; the union
  already had three) and `Platform` gains `sync: SyncApi`.
- `web.ts`: the online-only implementation (A183) — `status()` returns
  `{ state: 'synced', lastSyncedAt: null, outbox: 0 }`, `write`/`tombstone` throw with a
  message that names the REST path, `reset()` is a no-op.
- `desktop.ts`/`mobile.ts`: the IPC-backed implementation, plus the
  `stvdy://sync-status`, `stvdy://sync-revoked` and `stvdy://sync-auth-expired` events.
- `SyncStatus.tsx`: one control showing the state word, "Last synced …" and, when a
  mutation was rejected, the server's code. `Shell` renders it in both form factors
  (sidebar foot on desktop, a chip in the tab bar row on phone), because the phone has
  no sidebar and `/settings` is still a `NotBuiltRoute` (`router.tsx:92`) — Settings is
  Phase 9's (A189).
- `DevicesScreen.tsx` at `/devices`, reached from the status control: the device list
  with platform and last-synced time, rename, revoke with confirmation, and "Reset local
  data and resync" with the copy saying what it deletes (the cache **and** the outbox,
  A186). No sixth nav item: the sidebar and the tab bar are both built from the same five
  labels (A189).
- `app/src/api/sse.ts` gains a `streamSync(...)` sibling to `streamChat`: same reader and
  same parser, a GET instead of a POST, and one event (`seq`).
- Both locale files gain `sync.*` and `devices.*`; `app/src/i18n/i18n.test.ts` enforces
  key and placeholder parity, so an Indonesian string that drops `{{count}}` fails.

- [ ] **Step 4: Run the tests, the build and the CDN check**

`npm --prefix app test` → green. `npm --prefix app run build` → clean.
`node scripts/check-no-cdn.mjs` → clean.

- [ ] **Step 5: Commit**

```bash
git add app/src
git commit -m "feat(app): the sync status, the Devices screen, and the online-only web path"
```

**Gates:** 5 — measured in Task 12. 6 — the copy half lands here; the data half is
Task 4. 7 — (a) no invented time, (b) the status traces to the outbox and the stored
timestamp, (d) a rejected mutation names its code, (f) the Devices screen shows no
price or upgrade control anywhere.

---

## Task 11: `crates/server/tests/sync_convergence.rs` — the exit gate

**Files:**
- Create: `crates/server/tests/sync_convergence.rs`
- Modify: nothing

**Interfaces:**
- Consumes: `stvdy_sync::client::{Engine, LocalStore, Transport, SyncState}`,
  `stvdy_server::testing::TestState`, `common::{signed_up, json}`.
- Produces: no production API. The suite's contract is that it fails when the exit gate
  stops holding.

- [ ] **Step 1: Write the suite**

The transport here is a `Transport` implementation that calls the real router
(`tower::ServiceExt::oneshot`) with the device's bearer token, and the store is the
in-memory one from `crates/sync/tests/engine.rs` — moved into
`crates/sync/src/client/testing.rs` so both suites use one copy (the pattern
`crates/core/src/traits/testing.rs` already sets for test doubles).

```rust
/// The exit gate, first clause. Three devices, each with its own cache and outbox:
/// device A writes a task offline, device B edits it, device C adds a review; after one
/// pass each, all three hold the same rows at the same seq. Convergence is asserted on
/// the *content*, not on the cursor alone.
#[sqlx::test(migrations = "../../migrations")]
async fn three_devices_converge(pool: PgPool) { … }

/// The exit gate, second clause: a replayed mutation applies nothing, through the
/// client as well as through the route — the engine's outbox is retried after a
/// simulated lost response and the second push changes no row and adds no change_log
/// entry.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_mutation_through_the_outbox_applies_nothing(pool: PgPool) { … }

/// The exit gate, third clause: a tombstone and an edit from two devices, in both
/// arrival orders, end with the tombstone winning and the editing device told.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tombstone_beats_a_concurrent_edit_in_both_arrival_orders(pool: PgPool) { … }

/// The exit gate, fourth clause: compaction moves the watermark past a device's cursor,
/// the device gets 410, and its cache is rebuilt from zero with nothing lost.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_cursor_resyncs_from_zero_and_loses_nothing(pool: PgPool) { … }

/// The exit gate, fifth clause: a revoked device is wiped — its cache and its outbox —
/// and the wipe is asserted on the store, not on the HTTP status alone.
#[sqlx::test(migrations = "../../migrations")]
async fn a_revoked_device_is_wiped(pool: PgPool) { … }

/// The phase's named risk, at the engine level: two accounts syncing concurrently in
/// one test, and neither store ever holds a row or a seq belonging to the other.
#[sqlx::test(migrations = "../../migrations")]
async fn two_accounts_never_cross(pool: PgPool) { … }

/// The lock-order rule end to end: two devices push a two-mutation batch (a task plus
/// its subtask) against the same rows concurrently and both succeed. The roadmap calls
/// this the core multi-device scenario, not an edge case.
#[sqlx::test(migrations = "../../migrations")]
async fn two_devices_pushing_overlapping_batches_both_succeed(pool: PgPool) { … }

/// Offline reads: with the transport failing, the store still answers a read from the
/// rows already applied, and the status is `offline` rather than `error`.
#[sqlx::test(migrations = "../../migrations")]
async fn offline_reads_work_and_the_status_says_offline(pool: PgPool) { … }
```

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-server --test sync_convergence` → the file does not compile until
`crates/sync/src/client/testing.rs` exists.

- [ ] **Step 3: Move the doubles into the crate**

`crates/sync/src/client/testing.rs` (a `pub mod testing;` behind no feature gate: the
crate is `publish = false` and its only consumers are tests and the shell) holds the
in-memory store, the scripted transport and the router-backed transport's request
shapes. `crates/sync/tests/engine.rs` then imports them instead of defining its own, so
the convergence suite and the engine suite cannot drift.

- [ ] **Step 4: Run the suite**

`cargo test -p stvdy-server --test sync_convergence` → green.

- [ ] **Step 5: Commit**

```bash
git add crates/server/tests/sync_convergence.rs crates/sync/src/client/testing.rs \
        crates/sync/tests/engine.rs
git commit -m "test(sync): three devices converge, and a replayed mutation applies nothing"
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change. This is roadmap gate 3
for this phase; its output is quoted in the phase summary.

---

## Task 12: Docs, the three roadmap gates, and the smoke run

**Files:**
- Modify: `docs/SYNC.md`, `docs/API.md`, `docs/ARCHITECTURE.md`,
  `docs/ASSUMPTIONS.md`, `docs/THREAT_MODEL.md` (§9's Phase 8 rows)
- Create: `scripts/smoke-phase8.sh`

**Interfaces:** no production API.

- [ ] **Step 1: `docs/SYNC.md` is the normative protocol document, and it is updated here**

It already carries the cursor model, the advisory-lock argument, the lock-order rule,
the pull/push shapes, the allowlist, the conflict table, the offline scope, the cache
contents and the compaction paragraph. This task adds, in the same voice:

- The **initial-pass anchor** and the `offset` parameter, with the failure it prevents
  (A164) — including the sentence that an interrupted pass may restart at offset 0.
- The **watermark table** and the exact staleness rule (A165), replacing the current
  prose "A cursor older than retained history returns 410" with the predicate.
- The **payload allowlist** and what it excludes (A168), and the `data: null` rule
  (A188).
- The **payload shapes** per entity (A176) with one worked example of the disjoint-field
  merge and one of the overlap (A177).
- The **retention windows** (A184) as two numbers rather than "default 90 days".
- The **stream frame** (A181): `event: seq`, `data: {"seq":N}`, a 2s poll, a 15s
  heartbeat.
- The **outbox, retry and status** (A187), and the **revoked wipe** (A186) with the
  sentence that the outbox is deleted too and why.
- The **isolation rule** as a contract line: every sync read is scoped by `user_id` in
  the statement that reads it, and a `change_log` row whose `entity_id` is not this
  account's loads no payload.

- [ ] **Step 2: `docs/API.md`**

Replace the one-line `## Sync (Phase 8)` stub (`:207`) with the three routes in the
document's own style: the query parameters with their defaults and caps, `next_cursor`
and `has_more`, the four `MutationResult` bodies, the `read_only_entity` table, the 410
and its remedy, the 403 for a body that claims another device, and the SSE frame. Note
in the `### SSE` section that `event: seq` is the sync stream's event.

- [ ] **Step 3: `docs/ARCHITECTURE.md`**

Extend § Local cache (`:841`) with the cache/outbox design: the table set, the
`defer_foreign_keys` apply and the two mechanisms that make out-of-order arrival land
(in-page deferral, cross-pull retry), the outbox's ordering and backoff, the wipe, and
why `rusqlite` and `reqwest` live in the shell rather than in `crates/sync` (A182). Add
one section on the seq clock: why the advisory lock makes `seq` order equal commit
order, and why the push route takes the lock first.

- [ ] **Step 4: `docs/ASSUMPTIONS.md`**

Append A164–A189 (§ "Pinned decisions"), add `change_log_watermark` to § "Schema
deviations from the spec", and record the two new config keys.

- [ ] **Step 5: The three roadmap gates, measured**

- **Gate 5.** In a browser at 390×844 and at 1365px, both themes: tab to the sync
  control and the Devices screen's controls with a visible focus ring; measure the
  contrast of the status word, the "Last synced" line and the revoke button against
  their surfaces; measure the status control's and the revoke button's box at phone
  width; confirm no state is carried by colour alone (the status is a word); re-render at
  200% text and assert zero clipped and zero overflowing elements and no horizontal page
  overflow. Record the numbers, not an opinion.
- **Gate 6.** The data half is `indonesian_text_round_trips_byte_exact` (Task 4) and the
  tombstone leg in Task 11; the copy half is the two locale files, checked by
  `npm --prefix app test -- i18n`. Also drive the Devices screen in Indonesian and read
  the status line in the browser.
- **Gate 7.** (a) the status and "Last synced" trace to `devices.last_sync_at` and the
  outbox length; (b) `grep` the shipped copy for the banned claims (no "100%", no
  "verified", no percentage) and exercise the screen; (c) not applicable — no chat
  composer on these screens, stated explicitly rather than skipped; (d) no generative
  action here, so no credit cost is shown, stated explicitly; (e) no AI proposal on
  these screens, stated explicitly; (f) the Devices screen and the status control carry
  no price, no upgrade button and no external link on any platform.

- [ ] **Step 6: The smoke run**

`scripts/smoke-phase8.sh` drives the running server with `curl` and needs no model stub:

1. sign up + verify two accounts (A and B);
2. device 1 pushes a task with an Indonesian title; assert `applied{seq}`;
3. device 2 pulls and sees the row, with the same seq;
4. replay the same `mutation_id`; assert the identical result and that the task's
   `updated_at` and the `change_log` count are unchanged;
5. device 2 pushes a tombstone; device 1 pulls and sees `op: delete` with `data: null`;
6. device 1 pushes an edit against the tombstone; assert `conflict` with the tombstone
   as the winner;
7. push a `Message` and a `Source`; assert `read_only_entity` for both, and that the
   batch's task still applied;
8. account B pulls; assert none of A's seqs appear, and vice versa;
9. force the watermark past device 1's cursor (a tiny retention window), pull, assert
   410 `resync_required`, then pull from 0 and assert the surviving history arrives;
10. revoke device 2, assert its pull is 401;
11. print `=== PASS`.

Add a `smoke-phase8` recipe to the `justfile` beside the others.

- [ ] **Step 7: The full quality gate**

`just check` (which is `check-rust check-web check-shell`), plus `cargo sqlx prepare
--workspace --check`. Quote the output in the phase summary.

- [ ] **Step 8: Commit**

```bash
git add docs scripts/smoke-phase8.sh justfile
git commit -m "docs(phase 8): the sync protocol, the three routes, the cache design, and the gates measured"
```

**Gates:** 5/6/7 — recorded here as measured results.

---

## Phase 8 exit checklist

- [ ] `crates/sync/src/proto/cursor.rs` exists; the default and cap are 200/1000; an
      `offset` is legal only on the initial pass; a cursor of 0 is never stale.
- [ ] The initial pass is anchored at the user's `max(seq)`, and a write during the pass
      cannot shift a row out of it.
- [ ] `crates/sync/src/client/` exists with the engine, the outbox, the apply, the
      conflict table and the transport trait, and `crates/sync` links neither `rusqlite`
      nor `reqwest`.
- [ ] `GET /sync/pull` pages by seq ascending, is rank-ordered on the initial pass,
      ships only the allowlisted columns, and answers 410 `resync_required` below the
      watermark.
- [ ] A pull for account A contains no seq and no row id belonging to account B, and the
      payload loader is scoped by `user_id` in its own statement.
- [ ] `POST /sync/push` takes the advisory lock as its first statement, sets
      `app.device_id` second, uses a savepoint per mutation, and calls Phase 7's `decide`
      rather than a copy of it.
- [ ] A replayed `mutation_id` returns the stored result and applies nothing.
- [ ] A rejected mutation does not abort the batch; `Message`, `Source` and a derived
      flashcard field are `read_only_entity` over the wire.
- [ ] A mutation whose id belongs to another account is `not_found` and writes nothing; a
      `mutation_id` belonging to another account is a conflict and never returns that
      account's result.
- [ ] A body claiming another device's id is 403.
- [ ] Append-only duplicates are ignored and report the existing seq.
- [ ] Disjoint task/event edits both survive; overlapping edits resolve by server receive
      time; a tombstone beats an edit and the loser is told; an explicit restore wins.
- [ ] `GET /sync/stream` emits `event: seq` with the current max on connect, polls every
      2s, and carries only the caller's own clock.
- [ ] `compact_change_log` keeps tombstones longer than upserts, writes the watermark,
      runs as a per-day singleton, and its 410 has a working full-resync path.
- [ ] The shell has a SQLite cache, an outbox, a `defer_foreign_keys` apply path, a
      deferred-child retry, a wipe, and no access token in the database.
- [ ] The SPA has a sync status, a Devices screen, and an online-only web path; the
      locale files carry every string in both languages.
- [ ] Three simulated devices converge; the isolation test passes; two devices pushing
      overlapping batches both succeed.
- [ ] Roadmap gate 5 (accessibility) performed and recorded, with measurements.
- [ ] Roadmap gate 6 (Indonesian) performed and recorded, both copy and data.
- [ ] Roadmap gate 7 (honesty) performed and recorded, all six clauses.
- [ ] `docs/{SYNC,API,ARCHITECTURE,ASSUMPTIONS,THREAT_MODEL}.md` updated in the same
      phase.
- [ ] `bash scripts/smoke-phase8.sh` prints `=== PASS`.
- [ ] The full quality gate is green.

### Exit checklist table (gate | command | result)

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --all -- --check` + `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` | pending |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` + the `src-tauri` manifest | pending |
| Rust tests | `cargo test --workspace` | pending |
| Shell tests | `cargo test --manifest-path src-tauri/Cargo.toml` | pending |
| Protocol rules | `cargo test -p stvdy-sync --test protocol` | pending |
| The engine | `cargo test -p stvdy-sync --test engine` | pending |
| Pull | `cargo test -p stvdy-server --test sync_pull` | pending |
| Push | `cargo test -p stvdy-server --test sync_push` | pending |
| Stream | `cargo test -p stvdy-server --test sync_stream` | pending |
| Compaction | `cargo test -p stvdy-server --test sync_compaction` | pending |
| **The exit gate** | `cargo test -p stvdy-server --test sync_convergence` | pending |
| The route sweep | `cargo test -p stvdy-server --test route_sweep` | pending |
| The schema contract | `cargo test -p stvdy-server --test schema` | pending |
| The tamper suite | `cargo test -p stvdy-server --test tamper` | pending |
| Offline queries | `cargo sqlx prepare --workspace --check` | pending |
| Web tests | `npm --prefix app test` | pending |
| Web build | `npm --prefix app run build` | pending |
| No CDN | `node scripts/check-no-cdn.mjs` | pending |
| Roadmap gate 3 (this phase's acceptance) | `bash scripts/smoke-phase8.sh` | pending |
| Roadmap gate 5 (accessibility) | measured in Chromium at 390×844, both themes, 200% text | pending |
| Roadmap gate 6 (Indonesian) | the byte-exact round trip + the two locale files | pending |
| Roadmap gate 7 (honesty) | clauses (a)–(f), each with the check named | pending |
| Everything | `just check` | pending |

---

## Execution notes

1. **Phase 7 must be merged before Task 3.** Tasks 1 and 2 do not need it; Task 4 is a
   transport around `sync::push::decide` and cannot start without it. Task 1 Step 1 is
   the gate: read the landed file, record its signature, and stop if it is absent rather
   than writing a second decision path.
2. **Migration numbering.** Phase 6 took `0023` and `0024`; Phase 7's plan takes `0025`
   and `0026`. This phase takes `0027`, and it adds no `+sync` table, so
   `0017_sync_triggers.sql` stays the last trigger-attaching migration and its array is
   untouched.
3. **Do not add a sixth nav item.** The sidebar and the phone tab bar are both built
   from the same five labels; Devices is reached from the sync status control, which
   `Shell` renders in both form factors (A189). `/settings` stays a `NotBuiltRoute`:
   Settings is Phase 9's.
4. **`crates/sync` must stay free of `rusqlite`, `reqwest` and `sqlx`.** The server's
   test binaries link it, and a C SQLite in that graph is a build problem on every
   platform for no benefit. If a task finds itself wanting a driver in `crates/sync`,
   the trait is the thing to add.
5. **The isolation rule is a code rule, not a review rule.** Every sync read carries
   `where user_id = $1` in the same statement. A `select` followed by a filter in Rust
   is a defect even when it returns the right rows, because the next edit is what makes
   it wrong.
6. **`data: null` is a delete.** Both ends implement it (A188). It is what keeps a
   pathological `change_log` row from becoming a cross-tenant read, and it is why the
   client's apply does not dispatch on `op` alone.
7. **Log the latency.** The roadmap's constraint names sync pull/push explicitly; one
   `tracing::info!` per request with `latency_ms`, `rows`/`mutations`, `cursor` and the
   request id.
8. **`seq` is not a wall clock.** Nothing may compare `seq` across users, and nothing
   may order by `changed_at` where the cursor is the point: `changed_at` is `now()` at
   insert time and is not the ordering the cursor guarantees.

---

## Self-Review

- **Spec coverage.** `docs/SPEC.md` § "Sync protocol": pull by cursor with
  `next_cursor`/`has_more` and dependency-safe initial order (Tasks 1/3), push with
  per-mutation validation, idempotent replay via `mutation_log` and per-mutation results
  (Tasks 4/5), the realtime SSE hint (Task 6), "clients write locally first, update the
  UI immediately, then push; retry with exponential backoff; show sync status" (Tasks
  8/9/10), and compaction with the 90-day window and the resync fallback (Tasks 7/12).
  § "Conflict rules": append-only inserts with duplicate ids ignored, server-derived
  flashcard scheduling, record-level LWW with field-level merge for tasks and events, a
  tombstone beating a concurrent edit, and server-owned entities read-only except through
  their endpoints (Task 5, plus Phase 7's `decide`). § "What is cached locally on native
  clients": the synced entities only, no chunks or embeddings, plus "Reset local data and
  resync" (Tasks 9/10). § "Auth across devices": a revoked device gets 401 and wipes its
  cache on next contact, and the Devices screen lists, renames and revokes (Task 10;
  the spec's IA nests Devices under settings, and A189 records why it hangs off the
  status control until Phase 9 builds that screen).
  § "Entitlements engine": "including sync push mutations" (Task 4, via `decide`).
  § "AUTH AND SECURITY": every query scoped to the authenticated user including sync
  pull/push, rate-limited sync, and tokens only in the keychain (Tasks 3/9).
  `docs/THREAT_MODEL.md` §5's `Message`/`Source`/derived-field rows and its cross-tenant
  row each gain their Phase 8 case.
- **Placeholders:** none. Every task names its files, its exact test names, its run
  command and its commit. The choices the spec and the roadmap leave open are pinned as
  A164–A189 rather than waved at, and each names the failure it prevents.
- **Type consistency.** `PullQuery`, `CursorError`, `CursorKind`, `is_stale`,
  `PayloadShape`, `payload_columns`, `LocalStore`, `ApplyReport`, `Transport`,
  `TransportError`, `SyncState`, `Rule`, `Merge`, `Outcome`, `Page`, `apply_batch`,
  `apply_one`, `merge_patch`, `load_page`, `load_payload`, `min_retained_seq`,
  `touch_last_sync`, `SyncApi` are each defined once and referenced by name in the
  server, the shell, the SPA and the tests. `Mutation`, `MutationResult`, `PushRequest`,
  `PushResponse`, `Change`, `Entity`, `Op` and `ErrorCode` come from `crates/sync` and
  are never redefined. `AppError::ResyncRequired` is reused rather than replaced.
  `decide`/`push_features` are Phase 7's and are called, not copied (A150).
- **The two bugs this plan exists to avoid.** (1) Cross-tenant leakage: the phase's named
  risk, defended by scoping in the statement and by three tests that would fail if a
  filter moved into Rust. (2) A replay that applies twice: defended by asserting the
  row's `updated_at` and the `change_log` count, not the response shape. A third is
  written down because it is silent: the unanchored initial pass, which loses a row per
  mid-pass insert and only on a device that is syncing for the first time.
- **Deviations from the contract, each recorded:** A164 (the anchor and the `offset`
  parameter, an addition the contract's single-integer cursor cannot express), A168 (a
  payload allowlist the contract does not spell), A176 (the patch+base payload shape),
  A181 (`event: seq`, which is not in the roadmap §4 list because that list covers the
  chat stream), A184 (two retention windows where the contract says one), A188 (`data:
  null` on an `upsert`), A189 (Devices hangs off the status control rather than a
  Settings screen, which does not exist yet).
- **Open items.**
  1. **The scale-out path in `change_log` is not taken.** `docs/SYNC.md` names the
     `txid` + `pg_snapshot_xmin` alternative to the per-user advisory lock. It is not
     needed at this write rate and taking it would change the cursor's meaning; it stays
     a documented option.
  2. **`change_log` is compacted per user on a daily schedule, not incrementally.** A
     single account with millions of changes would do that work in one transaction. The
     retention window bounds it at 90 days of one account's writes, and the alternative
     (a batched delete with a resumable cursor) is real work for a case no plan limit
     permits today.
  3. **The client's conflict table is data, but its *application* is only the server's.**
     A device that goes offline for a week and reconnects replays a queue the server may
     refuse row by row; the client shows the refusals (A187) but has no per-row "keep
     mine" override. The spec does not ask for one, and adding it would be a second
     conflict resolver.
  4. **The web build has no offline reads.** `docs/SPEC.md` scopes the cache to native
     clients and `docs/ARCHITECTURE.md` already says so, but the consequence — a browser
     tab that loses connectivity cannot read the last page it saw — is worth stating in
     the phase summary rather than discovering in Phase 9.
  5. **B1 (QRIS gateway docs)** is still needed at Phase 13, not before.
