# Phase 0. Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A compiling, tested, CI-green skeleton: Cargo workspace (`core`, `sync`, `server`), the complete Postgres schema with sync triggers, config loaders for `plans.toml` + `billing_policy.toml`, docker-compose (pgvector + MinIO), an Axum `/health` server, the React SPA with a `platform` adapter, and a Tauri 2 shell that builds for macOS and Windows.

**Architecture:** Four artifacts in one repo. `crates/core` is pure logic with no web/db/io dependency (config loaders live here so both server and later phases share them). `crates/sync` holds the wire DTOs shared by server and Tauri shell. `crates/server` is the Axum binary plus the sqlx migration runner. `app/` is the single SPA consumed by the web build and by Tauri; `src-tauri/` is the native shell whose `frontendDist` points at `app/dist`.

**Tech Stack (exact pins, verified 2026-10-11):** Rust 1.94.1, edition 2024 · axum 0.8.9 · tokio 1.53.2 · sqlx 0.9.0 (`runtime-tokio` + `tls-rustls-ring`) · pgvector 0.4.2 · serde 1.0.229 · thiserror 2.0.21 · tracing 0.1.44 / tracing-subscriber 0.3.23 · tower 0.5.3 · tower-http 0.7.1 · toml 1.1.8 · React 19.3.0 · Vite 8.3.4 · TypeScript 7.0.2 · Tailwind 4.3.3 · React Router 8.4.0 · TanStack Query 5.104.1 · Vitest 5.0.3 · Tauri 2.12.3 / CLI 2.12.2 · Node 24.

## Global Constraints

Inherited from `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md` § Global Constraints. Phase-0-specific additions:

- Rust toolchain is pinned to **1.94.1** (sqlx 0.9 needs ≥1.94.0, `aws-sdk-s3` ≥1.94.1). Edition 2024, workspace resolver 3.
- **No `unwrap()`/`expect()` in non-test code.** Config loaders return typed errors; `main` may use `anyhow` at the top level only.
- Never `SELECT *` in any query that can touch `chunks` (`embedding vector`, `tsv tsvector`): sqlx 0.9's compile-time macros cannot map either type. Phase 0 writes no such query, but the rule starts now.
- `SQLX_OFFLINE=true` + a committed `.sqlx` directory: CI lint jobs must compile without a database.
- Frontend: no heavy state library. Server state goes through TanStack Query; local UI state uses React state or one tiny `useSyncExternalStore` module. No Redux/MobX.
- All UI code reaches platform capabilities **only** through `app/src/platform/*`. Nothing outside that folder may import `@tauri-apps/api`.
- Every migration file is numbered sequentially (`0001_`…) and is append-only once committed.

---

## File Structure

| Path | Responsibility |
|---|---|
| `Cargo.toml` | Workspace members, shared dependency versions (`[workspace.dependencies]`), lints |
| `rust-toolchain.toml` | Pin 1.94.1 + rustfmt/clippy |
| `clippy.toml` | Relax `unwrap_used`/`expect_used`/`panic` inside test code |
| `.cargo/config.toml` | `SQLX_OFFLINE` note, alias for `cargo sqlx prepare` |
| `justfile` | `db-up`, `db-search`, `migrate`, `server`, `web`, `tauri`, `check`, `test` |
| `docker-compose.yml` | postgres (pgvector image), minio, bucket-init, searxng (search tier 2, internal only) |
| `searxng/settings.yml` | SearXNG config: `formats: [html, json]`, limiter off, broken engines disabled (A3) |
| `.env.example` | Every env var the server reads, documented |
| `crates/core/Cargo.toml` | Pure crate: no axum, no sqlx |
| `crates/core/src/lib.rs` | Module tree + re-exports |
| `crates/core/src/error.rs` | `CoreError` (config/validation) |
| `crates/core/src/config/mod.rs` | `load_plans`, `load_billing_policy`, `ConfigError` |
| `crates/core/src/config/plans.rs` | `PlansFile`, `PlanConfig`, `PlanId`, `Credits`, `Entitlements`-shaped limits |
| `crates/core/src/config/billing_policy.rs` | `BillingPolicy`, `Platform`, `PlatformPolicy` |
| `crates/core/tests/plans_matrix.rs` | Plan×limit matrix snapshot + monotonicity + validation tests |
| `crates/core/tests/billing_policy.rs` | Defaults + all-5-platforms-present test |
| `config/plans.toml` | Prices, limits, credit costs, cost ceilings |
| `config/billing_policy.toml` | Per-platform purchase policy |
| `crates/sync/Cargo.toml` | DTO crate; no sqlx, no axum |
| `crates/sync/src/proto/{mod,entities,pull,push,error}.rs` | `Entity`, `Op`, `Change`, `Mutation`, `MutationResult`, `PullResponse`, `PushRequest/Response`, `ErrorCode` |
| `crates/sync/tests/wire.rs` | Serde round-trip + entity↔table-name bijection tests |
| `crates/server/Cargo.toml` | Axum binary |
| `crates/server/src/{main,lib,config,state,error,telemetry,db}.rs` | Startup, env config, shared state, error→status mapping, tracing, pool + migrations |
| `crates/server/src/routes/mod.rs` | Router assembly |
| `crates/server/src/routes/health.rs` | `GET /health` |
| `crates/server/tests/schema.rs` | Sync-column + trigger + change_log behaviour tests (live DB) |
| `migrations/0001..0016*.sql` | Full schema (mapping below) |
| `app/package.json`, `vite.config.ts`, `tsconfig.json`, `vitest.config.ts`, `index.html` | SPA build config |
| `app/src/styles.css` | Tailwind v4 `@theme` tokens + mobile base layer |
| `app/src/main.tsx`, `App.tsx`, `router.tsx` | Entry, providers, routes |
| `app/src/platform/{types,index,web,desktop,mobile}.ts` | The only place Tauri APIs are touched |
| `app/src/lib/{time,id}.ts` | Timezone-safe formatting, UUIDv7 |
| `app/src/components/layout/{Shell,Sidebar,TabBar}.tsx` | Adaptive shell (desktop sidebar / phone tab bar) |
| `app/src/features/*/` | Placeholder route components for Phase 0 |
| `src-tauri/{Cargo.toml,tauri.conf.json,build.rs}` | Shell config |
| `src-tauri/src/{main,lib}.rs` | `run()`, plugin registration, `AppState` |
| `src-tauri/src/keychain.rs` | `keyring-core` + per-platform store, `cfg`-gated |
| `src-tauri/src/commands/mod.rs` | `health_ping`, `secret_set`, `secret_get`, `secret_delete` |
| `src-tauri/capabilities/{default,desktop,mobile}.json` | Capability files with `platforms` filters |
| `.github/workflows/ci.yml` | fmt/clippy/test + web build + macOS/Windows bundle |
| `docs/{ARCHITECTURE,API,SYNC,RELEASE}.md` | Stubs with the sections later phases fill in |

### Migration file mapping (bodies are normative in `docs/SPEC.md` § DATABASE SCHEMA)

| File | Contents | Deltas vs spec |
|---|---|---|
| `0001_extensions.sql` | `create extension if not exists vector;` `pgcrypto;` |. |
| `0002_plans.sql` | `plans` |. |
| `0003_users.sql` | `users` | `+ is_admin boolean not null default false` |
| `0004_devices_auth_push.sql` | `devices`, `auth_sessions`, `push_tokens` | `+ index on auth_sessions(refresh_token_hash)`, `+ index on auth_sessions(user_id, revoked_at)` |
| `0005_change_log_mutation_log.sql` | `change_log`, `mutation_log` |. |
| `0006_sync_functions.sql` | `touch_updated_at()`, `log_change()`, `log_change_by_user()` | new (spec mandates the behaviour) |
| `0007_workspaces_sources_chunks.sql` | `workspaces`, `sources`, `chunks` + HNSW/GIN/hash indexes | `+ index sources(user_id, workspace_id)`, `+ index sources(status)`, `+ chunks.embedder text not null`, and the spec's `content_hash` index becomes a **non-unique** `index on chunks (embedder, content_hash)`. It must not be unique: `chunks` has no `user_id`, so a global constraint would let the first user to upload a shared textbook break ingestion for every other user, and would leak content existence across tenants. Dedupe is a per-user lookup that copies the existing embedding (A2). |
| `0008_messages.sql` | `messages` | `+ index messages(user_id, workspace_id, created_at)` |
| `0009_decks_flashcards_reviews.sql` | `decks`, `flashcards`, `flashcard_reviews` | `+ index flashcards(user_id, due_at)` |
| `0010_quizzes_attempts.sql` | `quizzes`, `quiz_questions`, `quiz_attempts` | `+ index quiz_attempts(user_id, quiz_id)` |
| `0011_reports.sql` | `reports`, `report_revisions` | `report_revisions` is new (loser-of-conflict copy, spec § conflict rules) |
| `0012_tasks_goals_sessions_settings_actions.sql` | `tasks`, `goals`, `study_sessions`, `user_settings`, `pending_actions` | `+ index tasks(user_id, status, due_at)`, `+ index pending_actions(user_id, status)` |
| `0013_calendar_events.sql` | `calendar_connections`, `events` | `events.task_id uuid references tasks(id) on delete set null`; `+ index events(user_id, starts_at)`, `+ unique(connection_id, external_id)` |
| `0014_billing.sql` | `subscriptions`, `payments`, `webhook_events`, `credit_ledger`, `usage_events`, `discount_codes`, `user_api_keys` | `+ index credit_ledger(user_id, bucket)`, `+ index usage_events(user_id, action, created_at)` |
| `0015_jobs.sql` | `jobs` | `+ dedupe_key text` + `unique index on jobs(dedupe_key) where dedupe_key is not null` |
| `0016_cheatsheets.sql` | `cheatsheets` (+sync metadata) + indexes | new. The print-PDF feature needs a persisted document. Artifacts (rendered HTML, PDF) live in object storage and are referenced by key, not stored in Postgres. |
| `0017_sync_triggers.sql` | Trigger attachment for every `+sync` table, including `cheatsheets` | new. **This migration must always be last**, because it attaches triggers to every table created before it; a new `+sync` table is added by inserting a migration *above* it and appending the table name to its array. |

`0012` must precede `0013` because `events.task_id` references `tasks`. `0017` must be last. No other ordering constraint exists.

---

## Task 1: Workspace skeleton, toolchain, dev entry points

**Files:**
- Create: `rust-toolchain.toml`, `Cargo.toml`, `.gitignore`, `.cargo/config.toml`, `justfile`, `README.md`
- Create: `crates/core/Cargo.toml`, `crates/core/src/lib.rs`
- Create: `crates/sync/Cargo.toml`, `crates/sync/src/lib.rs`
- Create: `crates/server/Cargo.toml`, `crates/server/src/main.rs`, `crates/server/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: workspace crate names `stvdy-core`, `stvdy-sync`, `stvdy-server` (lib names `stvdy_core`, `stvdy_sync`, `stvdy_server`); `[workspace.dependencies]` that every later task extends.

- [ ] **Step 1: Write the toolchain + workspace manifests**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.94.1"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

Root `Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/core", "crates/sync", "crates/server"]
# src-tauri is its own workspace: it has a different toolchain target set and
# `tauri_build::build()` must not run during `cargo clippy --workspace`
# (it panics when the bundle icon files are absent, and the lint CI job has
# neither Node nor a built dist/). It path-depends on crates/sync and keeps its
# own committed Cargo.lock.
exclude = ["src-tauri"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.94.1"
license = "proprietary"
publish = false

[workspace.dependencies]
# --- shared, version-pinned once ---
stvdy-core = { path = "crates/core" }
stvdy-sync = { path = "crates/sync" }

serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.152"
toml = "1.1.8"
thiserror = "2.0.21"
anyhow = "1.0.104"
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter", "json"] }
chrono = { version = "0.4.45", features = ["serde"] }
chrono-tz = { version = "0.10.4", features = ["serde"] }
uuid = { version = "1.28.0", features = ["v4", "v7", "serde"] }
hex = "0.4.3"
base64 = "0.23.1"
rand = "0.10.3"
secrecy = "0.10.3"
async-trait = "0.1.92"
futures = "0.3.34"
bytes = "1.12.1"
tokio-util = "0.7.20"
unicode-segmentation = "1.13.3"

tokio = { version = "1.53.2", features = ["rt-multi-thread", "macros", "net", "time", "signal", "sync", "fs"] }
axum = { version = "0.8.9", features = ["macros"] }
axum-extra = { version = "0.12.6", features = ["typed-header"] }
tower = { version = "0.5.3", features = ["util", "timeout", "limit"] }
tower-http = { version = "0.7.1", features = ["cors", "trace", "limit", "timeout", "request-id", "catch-panic", "sensitive-headers"] }
tower_governor = { version = "0.8.0", default-features = false, features = ["axum"] }

sqlx = { version = "0.9.0", default-features = false, features = [
  "runtime-tokio", "tls-rustls-ring", "postgres", "macros", "migrate", "uuid", "chrono", "json",
] }
pgvector = { version = "0.4.2", features = ["sqlx"] }

[workspace.lints.rust]
unsafe_code = "forbid"
unused_must_use = "deny"

[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
dbg_macro = "deny"
```

`.gitignore`:
```
/target
/app/node_modules
/app/dist
/src-tauri/target
.env
.env.local
var/
.fastembed_cache/
```

Note: `/src-tauri/gen` is deliberately **not** ignored. Tauri's CLI writes its own `.gitignore` containing `*` inside `gen/<project>/.tauri/`, which implies the surrounding generated project tree is meant to be tracked. And it is where the Phase 14 native work lives (iOS Share Extension target, `Info.plist` `BGTaskSchedulerPermittedIdentifiers` + `UIBackgroundModes`, App Group entitlements). Ignoring it would silently drop hand-written native files.

`clippy.toml`:
```toml
# The workspace denies unwrap/expect/panic in non-test code (see
# [workspace.lints.clippy]). `cargo clippy --all-targets` also compiles test
# code, which legitimately unwraps and panics, so those lints must be relaxed
# inside tests or CI can never be green.
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
allow-dbg-in-tests = true
```

Clippy's "in tests" detection keys off the function's attributes. It covers `#[test]`, but a proc-macro harness such as `#[sqlx::test]` is not guaranteed to be recognised on every toolchain, so every integration test file that uses a non-`#[test]` harness must additionally start with:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
```

That header is already present in `crates/server/tests/schema.rs` and `crates/server/tests/health.rs`. Any new integration test using a macro harness must add it too.

`.cargo/config.toml`:
```toml
[env]
SQLX_OFFLINE = "false"   # CI sets SQLX_OFFLINE=true for lint/build jobs
```

`justfile`:
```make
set dotenv-load := true

db-up:
    docker compose up -d postgres minio minio-init searxng

db-search:
    docker compose exec searxng wget -qO- "http://127.0.0.1:8080/search?q=test&format=json" | head -c 400

db-down:
    docker compose down

migrate:
    sqlx migrate run

server:
    cargo run -p stvdy-server

web:
    npm --prefix app run dev

tauri:
    cargo tauri dev

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    npm --prefix app run build

prepare:
    cargo sqlx prepare --workspace
```

- [ ] **Step 2: Create the three crate manifests**

`crates/core/Cargo.toml`:
```toml
[package]
name = "stvdy-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
toml.workspace = true
thiserror.workspace = true
chrono.workspace = true
chrono-tz.workspace = true
uuid.workspace = true
async-trait.workspace = true

[lints]
workspace = true
```

`crates/sync/Cargo.toml`:
```toml
[package]
name = "stvdy-sync"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true

[lints]
workspace = true
```

`crates/server/Cargo.toml`:
```toml
[package]
name = "stvdy-server"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
stvdy-core.workspace = true
stvdy-sync.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
axum.workspace = true
axum-extra.workspace = true
tower.workspace = true
tower-http.workspace = true
sqlx.workspace = true
pgvector.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
chrono.workspace = true
secrecy.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["full"] }

[lints]
workspace = true
```

- [ ] **Step 3: Minimal crate roots so the workspace compiles**

`crates/core/src/lib.rs`:
```rust
//! Pure domain logic: no web, no database, no IO.
pub mod error;
```

`crates/core/src/error.rs`:
```rust
use std::path::PathBuf;

/// Errors raised by pure core logic (config loading, validation, planning).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("config file {path} could not be read: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config file {path} is not valid TOML: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("config is invalid: {0}")]
    ConfigInvalid(String),

    #[error("validation failed for {field}: {reason}")]
    Validation { field: String, reason: String },
}

pub type CoreResult<T> = Result<T, CoreError>;
```

`crates/sync/src/lib.rs`:
```rust
//! Sync protocol DTOs shared by the server and the native client shell.
pub mod proto;
```

`crates/sync/src/proto/mod.rs`:
```rust
pub mod entities;
pub mod pull;
pub mod push;

pub use entities::{Change, Entity, Op};
pub use pull::PullResponse;
pub use push::{Mutation, MutationResult, PushRequest, PushResponse};
```

`crates/server/src/lib.rs`:
```rust
pub mod config;
pub mod error;
pub mod state;
pub mod telemetry;
pub mod db;
pub mod routes;
```

`crates/server/src/main.rs` (placeholder; Task 9 fills it in):
```rust
fn main() -> anyhow::Result<()> {
    println!("stvdy-server: scaffold");
    Ok(())
}
```

- [ ] **Step 4: Verify the workspace compiles**

Run: `cargo build --workspace`
Expected: compiles; `Cargo.lock` created. If a pinned version does not exist, the error names the crate and the available versions. Fix the pin rather than loosening it to `*`.

- [ ] **Step 5: Verify lints are enforced**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Then temporarily add `let x: Option<u8> = None; x.unwrap();` to `crates/server/src/main.rs` and re-run: expected failure with `used `unwrap()` on an `Option` value`. Remove the line.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: cargo workspace skeleton with pinned toolchain and lint gates"
```

---

## Task 2: `plans.toml` + typed loader with the full plan matrix

**Files:**
- Create: `config/plans.toml`
- Create: `crates/core/src/config/mod.rs`, `crates/core/src/config/plans.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod config;`)
- Test: `crates/core/tests/plans_matrix.rs`

**Interfaces:**
- Consumes: `CoreError`/`CoreResult` from Task 1.
- Produces:
  - `PlanId` (`Free|Pro|Power`, serde `lowercase`)
  - `PlansFile { meta, credits, provider_cost_ceiling_usd, plans }`
  - `PlanConfig` with every limit as an `Option<u32>`/`Option<i64>` where `None` = unlimited, plus `prices()`, `export_formats`, `planner`, `model_tier`
  - `Credits` with `cost(CreditAction) -> i64` and the long-document multiplier
  - `load_plans(path: &Path) -> CoreResult<PlansFile>` and `PlansFile::validate()`
  - `CreditAction` enum used by Phase 7's ledger

- [ ] **Step 1: Write the config file**

`config/plans.toml`:
```toml
[meta]
version = 1
currency = "IDR"

# Credit prices are in *credits*, not currency. They are calibrated against measured
# provider cost per action, not guessed (A41). Reference figures at 2026-10-11 rates,
# IDR/USD 17,889:
#   one chat message  ~= 1500 system + 4000 retrieved + 500 history in, 600 out
#   deepseek-v4.1-flash   $0.00119 blended   (off-peak $0.00104 / peak $0.00207)
#   gemini-2.5-flash-lite $0.00073           (no peak/off-peak split)
#   search                $0.00              (A3: academic APIs keyless, SearXNG self-hosted)
# A credit is therefore worth about $0.0011-0.0019 of revenue depending on plan, and
# the cheapest model costs less than one credit's worth of revenue per message.
# Consequence: `chat_message = 1` is correct and must NOT be raised to cover search --
# search is now free, and raising it would only make the paid tiers look worse.
#
# Off-peak matters: DeepSeek peak is Mon-Fri 01:00-04:00 and 06:00-10:00 UTC, which is
# 08:00-11:00 and 13:00-17:00 WIB. The Indonesian student study window (19:00-23:00 WIB)
# falls entirely in off-peak, so the realistic blended cost is the cheaper column.
# Keep the system prompt a byte-stable prefix: DeepSeek cache hits bill $0.003/1M
# off-peak vs $0.15/1M on a miss, a 50x difference, at no extra cost to write.

[credits]
chat_message = 1
web_search = 2
flashcard_batch_10 = 4
quiz_10 = 6
report_outline = 3
report_section = 6
task_breakdown = 2
syllabus_import = 8
plan_generation = 2
agent_turn = 2
long_doc_threshold_tokens = 20000
long_doc_credits_per_1k_tokens = 1

# Hard per-user monthly provider-cost stop, in USD. These are runaway backstops, not
# the normal operating point: at the cheapest tier the free plan's full 60 credits cost
# about $0.044 against a $0.50 ceiling, and power's 2000 credits about $2.39 against
# $6.00. Exceeding one pauses expensive actions with a clear message rather than
# silently spending (docs/SPEC.md § Entitlements engine, "Margin safety"). They are what makes an unattended free tier
# survivable, so they MUST be enforced in Phase 7, not merely configured.

[provider_cost_ceiling_usd]
free = 0.50
pro = 2.00
power = 6.00

[plans.free]
name = "Free"
price_idr_monthly = 0
price_idr_yearly = 0
student_percent_off = 0
workspaces = 2
sources_total = 20
max_file_mb = 20
storage_mb = 200
devices_max = 2
credits_per_month = 60
chat_msgs_per_day = 20
web_searches_per_day = 3
flashcards_per_month = 30
quizzes_per_month = 2
reports_per_month = 1
export_formats = ["md"]
task_ai_breakdown = false
quick_capture = false
google_calendar = false
google_accounts_max = 0
syllabus_imports_total = 1
planner = "manual"
agent_actions = false
reminder_channels = ["web", "push"]
model_tier = "cheapest"
byok = false
advanced_analytics = false

[plans.pro]
name = "Pro"
price_idr_monthly = 10000
price_idr_yearly = 100000
student_percent_off = 50
workspaces = 0            # 0 = unlimited
sources_total = 150
max_file_mb = 50
storage_mb = 2000
devices_max = 5
credits_per_month = 500
chat_msgs_per_day = 0
web_searches_per_day = 30
flashcards_per_month = 0
quizzes_per_month = 0
reports_per_month = 0
export_formats = ["md", "pdf", "docx"]
task_ai_breakdown = true
quick_capture = true
google_calendar = true
google_accounts_max = 1
syllabus_imports_total = 0
planner = "auto"
agent_actions = true
reminder_channels = ["web", "push", "email"]
model_tier = "cheap"
byok = false
advanced_analytics = false

[plans.power]
name = "Power"
price_idr_monthly = 25000
price_idr_yearly = 250000
student_percent_off = 50
workspaces = 0
sources_total = 500
max_file_mb = 100
storage_mb = 10000
devices_max = 10
credits_per_month = 2000
chat_msgs_per_day = 0
web_searches_per_day = 100
flashcards_per_month = 0
quizzes_per_month = 0
reports_per_month = 0
export_formats = ["md", "pdf", "docx", "csv", "json"]
task_ai_breakdown = true
quick_capture = true
google_calendar = true
google_accounts_max = 0
syllabus_imports_total = 0
planner = "auto+balancer+weakness_blocks"
agent_actions = true
reminder_channels = ["web", "push", "email", "telegram", "discord"]
model_tier = "best"
byok = true
advanced_analytics = true
```

- [ ] **Step 2: Write the failing test**

`crates/core/tests/plans_matrix.rs`:
```rust
use std::path::Path;
use stvdy_core::config::{load_plans, CreditAction, ModelTier, PlanId};
use stvdy_core::error::CoreError;

const PLANS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/plans.toml");

#[test]
fn loads_the_shipped_plan_file() {
    let plans = load_plans(Path::new(PLANS_PATH)).expect("shipped plans.toml must load");
    assert_eq!(plans.plans.len(), 3);
    assert_eq!(plans.plans[&PlanId::Free].price_idr_monthly, 0);
    assert_eq!(plans.plans[&PlanId::Pro].price_idr_monthly, 10_000);
    assert_eq!(plans.plans[&PlanId::Pro].price_idr_yearly, 100_000);
    assert_eq!(plans.plans[&PlanId::Power].price_idr_monthly, 25_000);
    assert_eq!(plans.plans[&PlanId::Power].price_idr_yearly, 250_000);
}

#[test]
fn zero_means_unlimited() {
    let plans = load_plans(Path::new(PLANS_PATH)).unwrap();
    assert_eq!(plans.plans[&PlanId::Free].workspaces, Some(2));
    assert_eq!(plans.plans[&PlanId::Pro].workspaces, None);
    assert_eq!(plans.plans[&PlanId::Pro].chat_msgs_per_day, None);
    assert_eq!(plans.plans[&PlanId::Free].chat_msgs_per_day, Some(20));
}

#[test]
fn plan_limits_are_monotonic_free_le_pro_le_power() {
    let p = load_plans(Path::new(PLANS_PATH)).unwrap();
    let (free, pro, power) = (&p.plans[&PlanId::Free], &p.plans[&PlanId::Pro], &p.plans[&PlanId::Power]);

    // A missing value means unlimited, which is >= any concrete value.
    let ge = |lo: Option<u32>, hi: Option<u32>| match (lo, hi) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => b >= a,
    };

    for (field, f, m, pw) in [
        ("sources_total", free.sources_total, pro.sources_total, power.sources_total),
        ("storage_mb", free.storage_mb, pro.storage_mb, power.storage_mb),
        ("devices_max", free.devices_max, pro.devices_max, power.devices_max),
        ("credits_per_month", free.credits_per_month, pro.credits_per_month, power.credits_per_month),
        ("web_searches_per_day", free.web_searches_per_day, pro.web_searches_per_day, power.web_searches_per_day),
    ] {
        assert!(ge(f, m), "{field}: free must not exceed pro");
        assert!(ge(m, pw), "{field}: pro must not exceed power");
    }

    // Boolean features must be monotonic too.
    assert!(free.agent_actions == false && pro.agent_actions && power.agent_actions);
    assert!(free.byok == false && pro.byok == false && power.byok);
    assert!(free.quick_capture == false && pro.quick_capture && power.quick_capture);
    assert!(free.google_calendar == false && pro.google_calendar && power.google_calendar);

    // Model tier must not go backwards: a higher plan never gets a cheaper model.
    // `ModelTier` derives `Ord` precisely so this is a comparison, not three string
    // matches that silently stop covering a newly added variant.
    assert!(free.model_tier <= pro.model_tier, "free must not outrank pro");
    assert!(pro.model_tier <= power.model_tier, "pro must not outrank power");
    assert_eq!(free.model_tier, ModelTier::Cheapest);
    assert_eq!(power.model_tier, ModelTier::Best);
}

#[test]
fn credit_costs_match_the_spec() {
    let p = load_plans(Path::new(PLANS_PATH)).unwrap();
    let c = &p.credits;
    assert_eq!(c.cost(CreditAction::ChatMessage), 1);
    assert_eq!(c.cost(CreditAction::WebSearch), 2);
    assert_eq!(c.cost(CreditAction::FlashcardBatch10), 4);
    assert_eq!(c.cost(CreditAction::Quiz10), 6);
    assert_eq!(c.cost(CreditAction::ReportOutline), 3);
    assert_eq!(c.cost(CreditAction::ReportSection), 6);
    assert_eq!(c.cost(CreditAction::TaskBreakdown), 2);
    assert_eq!(c.cost(CreditAction::SyllabusImport), 8);
    assert_eq!(c.cost(CreditAction::PlanGeneration), 2);
    assert_eq!(c.cost(CreditAction::AgentTurn), 2);
}

#[test]
fn long_document_operations_scale_with_input_tokens() {
    let p = load_plans(Path::new(PLANS_PATH)).unwrap();
    // At or below the threshold: base cost only.
    assert_eq!(p.credits.long_doc_cost(CreditAction::ChatMessage, 20_000), 1);
    // Above it: base + 1 credit per started 1k tokens over the threshold.
    assert_eq!(p.credits.long_doc_cost(CreditAction::ChatMessage, 22_500), 1 + 3);
}

#[test]
fn a_missing_file_is_a_read_error_not_a_panic() {
    let err = load_plans(Path::new("/nonexistent/plans.toml")).unwrap_err();
    assert!(matches!(err, CoreError::ConfigRead { .. }));
}

#[test]
fn unknown_keys_are_rejected() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(&path, "[meta]\nversion = 1\ncurrency = \"IDR\"\n[credits]\nchat_message = 1\n[plans.free]\nname = \"Free\"\nprice_idr_monthly = 0\nprice_idr_yearly = 0\nstudent_percent_off = 0\ntypo_workspaces = 2\n").unwrap();
    let err = load_plans(&path).unwrap_err();
    assert!(matches!(err, CoreError::ConfigParse { .. }), "got {err:?}");
}

#[test]
fn validation_requires_free_pro_and_power() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(&path, "[meta]\nversion = 1\ncurrency = \"IDR\"\n[credits]\nchat_message = 1\n").unwrap();
    let err = load_plans(&path).unwrap_err();
    match err {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("free"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run the tests to confirm they fail**

Run: `cargo test -p stvdy-core --test plans_matrix`
Expected: FAIL to compile. `unresolved import stvdy_core::config`. `uuid` must also be added as a dev-dependency of `stvdy-core` for the test (add `uuid.workspace = true` under `[dev-dependencies]`).

- [ ] **Step 4: Implement the loader**

`crates/core/src/config/mod.rs`:
```rust
mod billing_policy;
mod plans;

use std::path::Path;

use crate::error::{CoreError, CoreResult};

pub use billing_policy::{BillingPolicy, Platform, PlatformPolicy};
pub use plans::{CreditAction, Credits, ModelTier, PlanConfig, PlanId, PlansFile};

fn read_to_string(path: &Path) -> CoreResult<String> {
    std::fs::read_to_string(path).map_err(|source| CoreError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load_plans(path: &Path) -> CoreResult<PlansFile> {
    let raw = read_to_string(path)?;
    let parsed: PlansFile = toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })?;
    parsed.validate()?;
    Ok(parsed)
}
```

`crates/core/src/config/plans.rs`:
```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanId {
    Free,
    Pro,
    Power,
}

impl PlanId {
    pub const ALL: [PlanId; 3] = [PlanId::Free, PlanId::Pro, PlanId::Power];

    pub fn as_str(self) -> &'static str {
        match self {
            PlanId::Free => "free",
            PlanId::Pro => "pro",
            PlanId::Power => "power",
        }
    }

    /// The cheapest paid plan, used by `EntitlementError::PlanRequired` hints.
    pub fn min_paid(self) -> PlanId {
        match self {
            PlanId::Free => PlanId::Pro,
            other => other,
        }
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub version: u32,
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credits {
    pub chat_message: i64,
    pub web_search: i64,
    pub flashcard_batch_10: i64,
    pub quiz_10: i64,
    pub report_outline: i64,
    pub report_section: i64,
    pub task_breakdown: i64,
    pub syllabus_import: i64,
    pub plan_generation: i64,
    pub agent_turn: i64,
    pub long_doc_threshold_tokens: i64,
    pub long_doc_credits_per_1k_tokens: i64,
}

impl Credits {
    pub fn cost(&self, action: CreditAction) -> i64 {
        match action {
            CreditAction::ChatMessage => self.chat_message,
            CreditAction::WebSearch => self.web_search,
            CreditAction::FlashcardBatch10 => self.flashcard_batch_10,
            CreditAction::Quiz10 => self.quiz_10,
            CreditAction::ReportOutline => self.report_outline,
            CreditAction::ReportSection => self.report_section,
            CreditAction::TaskBreakdown => self.task_breakdown,
            CreditAction::SyllabusImport => self.syllabus_import,
            CreditAction::PlanGeneration => self.plan_generation,
            CreditAction::AgentTurn => self.agent_turn,
        }
    }

    /// Base cost plus a linear surcharge for long inputs (spec: "long-document
    /// operations scale with input tokens (simple multiplier)").
    pub fn long_doc_cost(&self, action: CreditAction, input_tokens: i64) -> i64 {
        let base = self.cost(action);
        let over = input_tokens.saturating_sub(self.long_doc_threshold_tokens);
        if over <= 0 {
            return base;
        }
        let thousands = over.div_ceil(1000);
        base + thousands * self.long_doc_credits_per_1k_tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditAction {
    ChatMessage,
    WebSearch,
    FlashcardBatch10,
    Quiz10,
    ReportOutline,
    ReportSection,
    TaskBreakdown,
    SyllabusImport,
    PlanGeneration,
    AgentTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlannerMode {
    Manual,
    Auto,
    /// `auto+balancer+weakness_blocks`. Parsed as one variant so the config
    /// string is copied verbatim from the spec.
    #[serde(rename = "auto+balancer+weakness_blocks")]
    AutoBalancerWeaknessBlocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderChannel {
    Web,
    Push,
    Email,
    Telegram,
    Discord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    /// Free only. The cheapest model that still passes the citation/grounding
    /// contract. A free user costs real money and has no revenue behind them,
    /// so this tier exists to cap that exposure (A41).
    Cheapest,
    Cheap,
    Best,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanConfig {
    pub name: String,
    pub price_idr_monthly: i64,
    pub price_idr_yearly: i64,
    pub student_percent_off: u8,

    /// `None` (or `0` in TOML) means unlimited. Deserialized through `limit_of`.
    #[serde(deserialize_with = "limit_of")]
    pub workspaces: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub sources_total: Option<u32>,
    pub max_file_mb: u32,
    pub storage_mb: u32,
    pub devices_max: u32,
    pub credits_per_month: i64,
    #[serde(deserialize_with = "limit_of")]
    pub chat_msgs_per_day: Option<u32>,
    pub web_searches_per_day: u32,
    #[serde(deserialize_with = "limit_of")]
    pub flashcards_per_month: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub quizzes_per_month: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub reports_per_month: Option<u32>,

    pub export_formats: Vec<ExportFormat>,
    pub task_ai_breakdown: bool,
    pub quick_capture: bool,
    pub google_calendar: bool,
    #[serde(deserialize_with = "limit_of")]
    pub google_accounts_max: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub syllabus_imports_total: Option<u32>,
    pub planner: PlannerMode,
    pub agent_actions: bool,
    pub reminder_channels: Vec<ReminderChannel>,
    pub model_tier: ModelTier,
    pub byok: bool,
    pub advanced_analytics: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Md,
    Pdf,
    Docx,
    Csv,
    Json,
}

/// `0` in the TOML means "unlimited" and becomes `None`.
fn limit_of<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = u32::deserialize(deserializer)?;
    Ok(if raw == 0 { None } else { Some(raw) })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlansFile {
    pub meta: Meta,
    pub credits: Credits,
    pub provider_cost_ceiling_usd: BTreeMap<PlanId, f64>,
    pub plans: BTreeMap<PlanId, PlanConfig>,
}

impl PlansFile {
    pub fn get(&self, plan: PlanId) -> &PlanConfig {
        // validate() guarantees presence; a lookup miss is a programming error
        // and must not panic in request paths, so fall back to the free plan.
        self.plans.get(&plan).unwrap_or(&self.plans[&PlanId::Free])
    }

    pub fn monthly_price(&self, plan: PlanId) -> i64 {
        self.get(plan).price_idr_monthly
    }

    pub fn validate(&self) -> CoreResult<()> {
        for required in PlanId::ALL {
            if !self.plans.contains_key(&required) {
                return Err(CoreError::ConfigInvalid(format!(
                    "plans.toml must define a [{required}] plan"
                )));
            }
        }
        for (plan, config) in &self.plans {
            if config.price_idr_monthly < 0 || config.price_idr_yearly < 0 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: prices must not be negative"
                )));
            }
            if config.credits_per_month < 0 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: credits_per_month must not be negative"
                )));
            }
            if config.export_formats.is_empty() {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: export_formats must not be empty"
                )));
            }
            if config.student_percent_off > 100 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: student_percent_off must be 0..=100"
                )));
            }
        }
        if self.plans[&PlanId::Free].price_idr_monthly != 0 {
            return Err(CoreError::ConfigInvalid(
                "the free plan must cost 0 IDR".into(),
            ));
        }
        for plan in PlanId::ALL {
            if !self.provider_cost_ceiling_usd.contains_key(&plan) {
                return Err(CoreError::ConfigInvalid(format!(
                    "provider_cost_ceiling_usd is missing an entry for {plan}"
                )));
            }
        }
        Ok(())
    }
}
```

`crates/core/src/config/billing_policy.rs`. Declared in `mod.rs`, implemented in Task 3. For this task add a temporary stub so the module compiles, then Task 3 replaces it:
```rust
// replaced in Task 3
pub struct BillingPolicy;
pub struct Platform;
pub struct PlatformPolicy;
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test -p stvdy-core --test plans_matrix`
Expected: PASS (8 tests).

- [ ] **Step 6: Commit**

```bash
git add config/plans.toml crates/core
git commit -m "feat(core): typed plans.toml loader with plan matrix tests"
```

---

## Task 3: `billing_policy.toml` + platform policy loader

**Files:**
- Create: `config/billing_policy.toml`
- Modify: `crates/core/src/config/billing_policy.rs` (replace the Task 2 stub), `crates/core/src/config/mod.rs` (add `load_billing_policy`)
- Test: `crates/core/tests/billing_policy.rs`

**Interfaces:**
- Consumes: `CoreError`, `read_to_string` from Task 2.
- Produces:
  - `Platform { Web, Macos, Windows, Ios, Android }` with `Platform::ALL` and `as_str()`
  - `PlatformPolicy { allow_purchase_ui, show_prices, allow_purchase_cta_in_locked_messages }`
  - `BillingPolicy::for_platform(&self, Platform) -> &PlatformPolicy`, `BillingPolicy::purchase_allowed(&self, Platform) -> bool`
  - `load_billing_policy(path: &Path) -> CoreResult<BillingPolicy>`

- [ ] **Step 1: Write the config file**

`config/billing_policy.toml`:
```toml
# Controls where purchase UI may appear. iOS and Android must never sell:
# QRIS-only checkout is web/desktop, and the plan then syncs to the account.
[platforms.web]
allow_purchase_ui = true
show_prices = true
allow_purchase_cta_in_locked_messages = true

[platforms.macos]
allow_purchase_ui = true
show_prices = true
allow_purchase_cta_in_locked_messages = true

[platforms.windows]
allow_purchase_ui = true
show_prices = true
allow_purchase_cta_in_locked_messages = true

[platforms.ios]
allow_purchase_ui = false
show_prices = false
allow_purchase_cta_in_locked_messages = false

[platforms.android]
allow_purchase_ui = false
show_prices = false
allow_purchase_cta_in_locked_messages = false
```

- [ ] **Step 2: Write the failing test**

`crates/core/tests/billing_policy.rs`:
```rust
use std::path::Path;

use stvdy_core::config::{load_billing_policy, Platform};
use stvdy_core::error::CoreError;

const POLICY_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../config/billing_policy.toml"
);

#[test]
fn shipped_policy_allows_selling_on_web_and_desktop_only() {
    let policy = load_billing_policy(Path::new(POLICY_PATH)).unwrap();
    for platform in [Platform::Web, Platform::Macos, Platform::Windows] {
        let p = policy.for_platform(platform);
        assert!(p.allow_purchase_ui, "{platform:?} must allow purchase UI");
        assert!(p.show_prices, "{platform:?} must show prices");
        assert!(
            p.allow_purchase_cta_in_locked_messages,
            "{platform:?} must allow the upgrade CTA"
        );
    }
    for platform in [Platform::Ios, Platform::Android] {
        let p = policy.for_platform(platform);
        assert!(!p.allow_purchase_ui, "{platform:?} must not allow purchase UI");
        assert!(!p.show_prices, "{platform:?} must not show prices");
        assert!(
            !p.allow_purchase_cta_in_locked_messages,
            "{platform:?} must not allow the upgrade CTA"
        );
    }
    assert!(!policy.purchase_allowed(Platform::Ios));
    assert!(!policy.purchase_allowed(Platform::Android));
    assert!(policy.purchase_allowed(Platform::Web));
}

#[test]
fn every_platform_must_be_declared() {
    let dir = std::env::temp_dir().join(format!("stvdy-bp-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("billing_policy.toml");
    std::fs::write(
        &path,
        "[platforms.web]\nallow_purchase_ui = true\nshow_prices = true\nallow_purchase_cta_in_locked_messages = true\n",
    )
    .unwrap();
    match load_billing_policy(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("macos"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn mobile_must_not_be_more_permissive_than_desktop() {
    let dir = std::env::temp_dir().join(format!("stvdy-bp-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("billing_policy.toml");
    let mut body = String::new();
    for (name, allow) in [
        ("web", true), ("macos", true), ("windows", true), ("ios", true), ("android", false),
    ] {
        body.push_str(&format!(
            "[platforms.{name}]\nallow_purchase_ui = {allow}\nshow_prices = {allow}\nallow_purchase_cta_in_locked_messages = {allow}\n"
        ));
    }
    std::fs::write(&path, body).unwrap();
    match load_billing_policy(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("ios"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}
```

- [ ] **Step 3: Run the test to confirm it fails**

Run: `cargo test -p stvdy-core --test billing_policy`
Expected: FAIL to compile. `load_billing_policy` does not exist.

- [ ] **Step 4: Implement the loader**

`crates/core/src/config/billing_policy.rs` (replace the stub):
```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Web,
    Macos,
    Windows,
    Ios,
    Android,
}

impl Platform {
    pub const ALL: [Platform; 5] = [
        Platform::Web,
        Platform::Macos,
        Platform::Windows,
        Platform::Ios,
        Platform::Android,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Web => "web",
            Platform::Macos => "macos",
            Platform::Windows => "windows",
            Platform::Ios => "ios",
            Platform::Android => "android",
        }
    }

    pub fn is_mobile(self) -> bool {
        matches!(self, Platform::Ios | Platform::Android)
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPolicy {
    pub allow_purchase_ui: bool,
    pub show_prices: bool,
    pub allow_purchase_cta_in_locked_messages: bool,
}

impl PlatformPolicy {
    /// The policy used when a platform key is absent. Deny-by-default: a
    /// missing entry must never accidentally enable selling on mobile.
    pub const DENY: PlatformPolicy = PlatformPolicy {
        allow_purchase_ui: false,
        show_prices: false,
        allow_purchase_cta_in_locked_messages: false,
    };
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BillingPolicy {
    pub platforms: BTreeMap<Platform, PlatformPolicy>,
}

impl BillingPolicy {
    pub fn for_platform(&self, platform: Platform) -> &PlatformPolicy {
        self.platforms
            .get(&platform)
            .unwrap_or(&PlatformPolicy::DENY)
    }

    pub fn purchase_allowed(&self, platform: Platform) -> bool {
        self.for_platform(platform).allow_purchase_ui
    }

    pub fn validate(&self) -> CoreResult<()> {
        for platform in Platform::ALL {
            let policy = self.platforms.get(&platform).ok_or_else(|| {
                CoreError::ConfigInvalid(format!(
                    "billing_policy.toml must declare [platforms.{platform}]"
                ))
            })?;
            if platform.is_mobile() && policy.allow_purchase_ui {
                return Err(CoreError::ConfigInvalid(format!(
                    "{platform} must not allow purchase UI: QRIS checkout is web/desktop only \
                     (App Store / Play billing is out of scope for the MVP)"
                )));
            }
            if platform.is_mobile() && policy.show_prices {
                return Err(CoreError::ConfigInvalid(format!(
                    "{platform} must not show prices"
                )));
            }
            if policy.allow_purchase_cta_in_locked_messages && !policy.allow_purchase_ui {
                return Err(CoreError::ConfigInvalid(format!(
                    "{platform}: a purchase CTA is meaningless when purchase UI is disabled"
                )));
            }
        }
        Ok(())
    }
}
```

Add to `crates/core/src/config/mod.rs`:
```rust
pub fn load_billing_policy(path: &Path) -> CoreResult<BillingPolicy> {
    let raw = read_to_string(path)?;
    let parsed: BillingPolicy = toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })?;
    parsed.validate()?;
    Ok(parsed)
}
```

- [ ] **Step 5: Run the tests to confirm they pass**

Run: `cargo test -p stvdy-core`
Expected: PASS (11 tests total across both integration test binaries).

- [ ] **Step 6: Commit**

```bash
git add config/billing_policy.toml crates/core
git commit -m "feat(core): billing_policy loader that structurally forbids mobile selling"
```

---

## Task 4: Migrations 0001–0006 (extensions, plans, users, devices, sync log, sync functions)

**Files:**
- Create: `migrations/0001_extensions.sql` … `migrations/0006_sync_functions.sql`

**Interfaces:**
- Consumes: nothing.
- Produces: Postgres functions `touch_updated_at()`, `log_change()`, `log_change_by_user()`; tables `plans`, `users`, `devices`, `auth_sessions`, `push_tokens`, `change_log`, `mutation_log`.

- [ ] **Step 1: Create migrations 0001–0005**

`docs/SPEC.md` § DATABASE SCHEMA is a **sketch, not a script**. It says so in its own heading ("create as migrations; add sensible indexes and FKs"), states the sync columns as a *rule* rather than writing them into each table body, and describes the trigger behaviour in prose. So: **copy each `create table` body as the source, then apply the deltas below *and* the sync-column rule.** Do not copy the section verbatim. A verbatim copy produces tables with no `user_id`/`updated_at`/`deleted_at`, and Task 7's schema test plus the exit checklist (32 triggers, three sync columns on every `+sync` table) would both fail.

**The sync-column rule, applied to every table marked `-- +sync` in this and the following two tasks:**
```sql
  user_id uuid not null references users on delete cascade,  -- denormalized for fast scoping
  updated_at timestamptz not null default now(),             -- maintained by touch_updated_at
  deleted_at timestamptz,                                    -- tombstone; deletes sync as tombstones
```
`user_settings` is the one exception: it is keyed by `user_id` already and has neither `id` nor `deleted_at`, which is why `0006` provides a separate `log_change_by_user()`.

- `0001_extensions.sql`: the two `create extension if not exists` statements.
- `0002_plans.sql`: the `plans` table.
- `0003_users.sql`: the `users` table **plus** (a) `is_admin boolean not null default false` after `student_verified`, and (b) the A48 deltas for Google login: **`password_hash` must be nullable** (the spec says `not null`; a Google-only user has no password), plus
  ```sql
  google_sub text unique,             -- stable Google identifier; the ONLY safe link key
  email_verified_at timestamptz,      -- when verification happened (A43 needs a timestamp)
  create unique index on users (lower(email));
  ```
- `0004_devices_auth_push.sql`: `devices`, `auth_sessions`, `push_tokens` **plus**:
  ```sql
  create index on auth_sessions (refresh_token_hash);
  create index on auth_sessions (user_id, revoked_at);
  ```
- `0005_change_log_mutation_log.sql`: `change_log` + its index, `mutation_log`.

- [ ] **Step 2: Write `0006_sync_functions.sql`**

```sql
-- Sync plumbing. Two behaviours, applied to every +sync table by 0016:
--   1. touch_updated_at: maintain updated_at on UPDATE (server receive time).
--   2. log_change: append a change_log row in the same transaction, so a pull
--      cursor is a strict per-user clock.
-- The device that caused a change is taken from a transaction-local setting so
-- no per-table device column is needed:
--   select set_config('app.device_id', $1, true);
--
-- WHY THE ADVISORY LOCK IS REQUIRED (correctness, not tuning):
--   change_log.seq is a bigserial, so it is assigned at INSERT time, but rows
--   only become visible at COMMIT time. Two concurrent transactions for the
--   same user can therefore be assigned seq 10 and seq 11 yet commit 11 first.
--   A pull that reads `seq > cursor`, sees 11 and advances the cursor to 11
--   would never return 10 once it commits: a permanently skipped change.
--   Taking a per-user transaction-scoped advisory lock BEFORE inserting the
--   change_log row serialises the nextval() calls against commit order, so for
--   any committed row of a user, every lower seq is already committed. That is
--   exactly the property the cursor depends on. The lock is held to commit, so
--   a reader can only ever observe a contiguous prefix.
--   Cost: writes for one user serialise against each other. At this write rate
--   that is free. Scale-out alternative if it ever matters: add a `txid` column
--   and only return rows below `pg_snapshot_xmin(pg_current_snapshot())`.
--   Note this uses the same key as credit spending (docs/ASSUMPTIONS.md A9),
--   which is deliberate: one user's sync writes and credit spends serialise,
--   which also removes any deadlock ordering between the two paths.

create or replace function touch_updated_at() returns trigger
language plpgsql as $$
begin
  new.updated_at := now();
  return new;
end $$;

-- For tables whose primary key is `id`.
create or replace function log_change() returns trigger
language plpgsql as $$
declare
  uid uuid;
  eid uuid;
  op  text;
begin
  if tg_op = 'DELETE' then
    uid := old.user_id;
    eid := old.id;
    op  := 'delete';
  elsif tg_op = 'INSERT' then
    uid := new.user_id;
    eid := new.id;
    op  := case when new.deleted_at is null then 'upsert' else 'delete' end;
  else
    uid := new.user_id;
    eid := new.id;
    op  := case
             when new.deleted_at is not null and old.deleted_at is null then 'delete'
             else 'upsert'
           end;
  end if;

  -- Must precede the insert: this is what makes seq order == commit order.
  perform pg_advisory_xact_lock(hashtext(uid::text));

  insert into change_log (user_id, entity, entity_id, op, origin_device_id)
  values (uid, tg_table_name, eid, op,
          nullif(current_setting('app.device_id', true), '')::uuid);

  return null;
end $$;

-- For tables whose primary key IS user_id (user_settings), which has no
-- `id` column and no tombstones: updates only, always `upsert`. A separate
-- function is required because `new.id` / `new.deleted_at` do not exist there
-- and referencing them raises "record new has no field id".
create or replace function log_change_by_user() returns trigger
language plpgsql as $$
begin
  perform pg_advisory_xact_lock(hashtext(new.user_id::text));

  insert into change_log (user_id, entity, entity_id, op, origin_device_id)
  values (new.user_id, tg_table_name, new.user_id, 'upsert',
          nullif(current_setting('app.device_id', true), '')::uuid);

  return null;
end $$;
```

- [ ] **Step 3: Start the database and apply the migrations**

Run: `docker compose up -d postgres` (the compose file lands in Task 8; if it does not exist yet, run the container directly):
```bash
docker run -d --name stvdy-pg -p 5432:5432 -e POSTGRES_PASSWORD=postgres -e POSTGRES_USER=postgres -e POSTGRES_DB=stvdy pgvector/pgvector:pg16
```
Then:
```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/stvdy
cargo install --locked sqlx-cli --no-default-features --features postgres,rustls-ring 2>/dev/null || cargo install sqlx-cli --no-default-features --features postgres,rustls-ring
sqlx migrate run
```
Expected: `0001`…`0006` applied without error.

Note: `cargo install --locked sqlx-cli` no longer works in sqlx 0.9 (Cargo.lock is untracked), hence the fallback command.

- [ ] **Step 4: Verify the schema by inspection**

Run:
```bash
psql "$DATABASE_URL" -c "\df touch_updated_at" -c "\df log_change" -c "\df log_change_by_user" -c "\d change_log"
```
Expected: three functions present; `change_log` has `seq bigserial` PK and the `(user_id, seq)` index.

- [ ] **Step 5: Commit**

```bash
git add migrations
git commit -m "feat(db): extensions, plans, users, devices, sync log and sync functions"
```

---

## Task 5: Migrations 0007–0011 (content tables)

**Files:**
- Create: `migrations/0007_workspaces_sources_chunks.sql`, `0008_messages.sql`, `0009_decks_flashcards_reviews.sql`, `0010_quizzes_attempts.sql`, `0011_reports.sql`

**Interfaces:**
- Consumes: `0003_users.sql` (FK target `users`).
- Produces: `workspaces`, `sources`, `chunks`, `messages`, `decks`, `flashcards`, `flashcard_reviews`, `quizzes`, `quiz_questions`, `quiz_attempts`, `reports`, `report_revisions`.

- [ ] **Step 1: Create the five migrations**

Same rule as Task 4: `docs/SPEC.md` supplies the table bodies; you supply the sync columns (the rule in Task 4 Step 1 applies to every `-- +sync` table here), the indexes below, and the FKs the spec's heading asks for but does not write. Apply these deltas:

- `0007`: add `create index on sources (user_id, workspace_id);` and `create index on sources (status);`
- `0008`: add `create index on messages (user_id, workspace_id, created_at);`
- `0009`: add `create index on flashcards (user_id, due_at);`
- `0010`: add `create index on quiz_attempts (user_id, quiz_id);`
- `0011`: the `reports` table plus:
  ```sql
  -- Holds the losing body when the same report is edited on two devices
  -- (spec § Conflict rules: "store the losing version as a recoverable copy").
  create table report_revisions (
    id uuid primary key default gen_random_uuid(),
    report_id uuid not null references reports on delete cascade,
    user_id uuid not null references users on delete cascade,
    body_md text not null,
    replaced_at timestamptz not null default now()
  );
  create index on report_revisions (report_id, replaced_at desc);
  ```

The `chunks` migration must keep the exact index set from the spec (HNSW on `embedding` with `vector_cosine_ops`, GIN on `tsv`, btree on `content_hash`).

- [ ] **Step 2: Apply and inspect**

Run: `sqlx migrate run && psql "$DATABASE_URL" -c "\d chunks" -c "\d report_revisions"`
Expected: `chunks.embedding` is `vector(768)`, `chunks.tsv` is `tsvector` with `GENERATED ALWAYS ... STORED`; both indexes listed. `report_revisions` exists with its index.

- [ ] **Step 3: Commit**

```bash
git add migrations
git commit -m "feat(db): content tables (workspaces, sources, chunks, chat, study tools, reports)"
```

---

## Task 6: Migrations 0012–0016 (study OS, billing, jobs, triggers)

**Files:**
- Create: `migrations/0012_tasks_goals_sessions_settings_actions.sql`, `0013_calendar_events.sql`, `0014_billing.sql`, `0015_jobs.sql`, `0016_cheatsheets.sql`, `0017_sync_triggers.sql`

**Interfaces:**
- Consumes: `log_change()`, `log_change_by_user()`, `touch_updated_at()` from Task 4.
- Produces: `tasks`, `goals`, `study_sessions`, `user_settings`, `pending_actions`, `calendar_connections`, `events`, `subscriptions`, `payments`, `webhook_events`, `credit_ledger`, `usage_events`, `discount_codes`, `user_api_keys`, `jobs`; triggers on all 16 sync tables.

- [ ] **Step 1: Create migrations 0012–0015**

`docs/SPEC.md` supplies the table bodies; the sync-column rule from Task 4 Step 1 applies to every `-- +sync` table here (`tasks`, `goals`, `study_sessions`, `pending_actions`, `events`, `cheatsheets`). Add these deltas:

- `0012`: add
  ```sql
  create index on tasks (user_id, status, due_at);
  create index on pending_actions (user_id, status);
  ```
- `0013`: `events.task_id` becomes `task_id uuid references tasks(id) on delete set null`; add
  ```sql
  create unique index on events (connection_id, external_id) where external_id is not null;
  ```
  (the spec's `create index on events (user_id, starts_at)` stays).
- `0014`: add
  ```sql
  create index on credit_ledger (user_id, bucket);
  create index on usage_events (user_id, action, created_at);
  ```
  and, on `usage_events`, the column
  ```sql
  prompt_version text,   -- which prompt module version produced this action (A38)
  ```
- `0015`: `jobs` plus
  ```sql
  dedupe_key text,   -- column added to the create table
  ```
  and
  ```sql
  -- Singleton scheduled jobs (monthly_grant, compact_change_log, ...) must not
  -- duplicate across restarts or instances.
  create unique index on jobs (dedupe_key) where dedupe_key is not null;
  ```

- [ ] **Step 2: Write `0016_cheatsheets.sql`**

```sql
-- The cheatsheet document itself (print/PDF export). Only metadata syncs: the
-- rendered HTML and PDF are large, never edited on-device, and live in object
-- storage behind the keys below.
create table cheatsheets (               -- +sync
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  scope text not null,                   -- e.g. "UTS Jaringan Komputer"
  exam_date date,
  depth text not null default 'normal',  -- brief | normal | thorough
  source_ids uuid[] not null default '{}',
  status text not null default 'pending',-- pending | generating | ready | failed
  error text,
  -- The validated JSON blocks, stored in object storage rather than as a jsonb
  -- column: this table is +sync, so a jsonb body would ship tens of KB of
  -- document text to every device on every change. The blocks are the source of
  -- truth for the document, so a template fix can re-render every existing
  -- cheatsheet deterministically, for free and without re-invoking the model
  -- (which would cost credits and produce different content for a document the
  -- student may already have printed).
  blocks_key text,
  template_version text,                 -- which templates/cheatsheet.*.html rendered it
  html_key text,                         -- object storage key of the rendered HTML
  pdf_key text,                          -- object storage key of the rendered PDF
  page_count int,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create index on cheatsheets (user_id, workspace_id);
create index on cheatsheets (user_id, status);
create index on cheatsheets (user_id, created_at desc);
```

- [ ] **Step 3: Write `0017_sync_triggers.sql`**

This migration **must be last**, because it attaches triggers to every synced table
created before it. A future `+sync` table is added by inserting its migration above this
one and appending its name to the array below. Never by creating a new migration after
this one.

```sql
-- Attach the sync triggers to every +sync table. Table list is the contract:
-- crates/server/tests/schema.rs asserts exactly these tables carry both triggers.
do $$
declare
  t text;
  id_tables text[] := array[
    'workspaces', 'sources', 'messages', 'decks', 'flashcards',
    'flashcard_reviews', 'quizzes', 'quiz_questions', 'quiz_attempts',
    'reports', 'cheatsheets', 'tasks', 'goals', 'events', 'study_sessions',
    'pending_actions'
  ];
begin
  foreach t in array id_tables loop
    execute format(
      'create trigger %I_touch_updated_at before update on %I
         for each row execute function touch_updated_at()', t, t);
    execute format(
      'create trigger %I_log_change after insert or update or delete on %I
         for each row execute function log_change()', t, t);
  end loop;
end $$;

-- user_settings is keyed by user_id, has no tombstones, and only needs updates
-- to reach other devices.
create trigger user_settings_touch_updated_at
  before update on user_settings
  for each row execute function touch_updated_at();

create trigger user_settings_log_change
  after insert or update on user_settings
  for each row execute function log_change_by_user();
```

- [ ] **Step 4: Apply and verify trigger coverage**

Run:
```bash
sqlx migrate run
psql "$DATABASE_URL" -c "select c.relname, t.tgname from pg_trigger t join pg_class c on c.oid = t.tgrelid where not t.tgisinternal order by 1,2;"
```
Expected: 34 rows (17 tables × 2 triggers), with `user_settings` showing `user_settings_touch_updated_at` and `user_settings_log_change`. The only tables with no triggers are the non-sync ones: `change_log`, `mutation_log`, `chunks`, `jobs`, the billing tables, and `report_revisions`. `cheatsheets` **does** carry both triggers like every other sync table. It is created by `0016_cheatsheets.sql`, which runs before this migration.

Then confirm the sync columns actually landed, since the spec states them as a rule and it is easy to copy a table body without them:
```bash
psql "$DATABASE_URL" -c "
select c.relname, count(*) filter (where a.attname in ('user_id','updated_at','deleted_at')) as sync_cols
from pg_class c join pg_attribute a on a.attrelid = c.oid
where c.relkind = 'r' and a.attnum > 0 and not a.attisdropped
  and c.relname in ('workspaces','sources','messages','decks','flashcards','flashcard_reviews',
                    'quizzes','quiz_questions','quiz_attempts','reports','cheatsheets','tasks',
                    'goals','events','study_sessions','pending_actions')
group by 1 having count(*) filter (where a.attname in ('user_id','updated_at','deleted_at')) <> 3;"
```
Expected: **zero rows.** Any row returned is a `+sync` table missing one of the three columns, which Task 7's schema test would catch later. Better to see it here.

- [ ] **Step 5: Commit**

```bash
git add migrations
git commit -m "feat(db): study OS, calendar, billing, jobs, cheatsheets tables and sync triggers"
```

---

## Task 7: Schema integration test. Sync columns, triggers, change_log behaviour

**Files:**
- Create: `crates/server/tests/schema.rs`

**Interfaces:**
- Consumes: all migrations; `sqlx::test` (which creates a fresh database per test and applies `migrations/`).
- Produces: the guard that keeps every later phase's sync tables honest.

- [ ] **Step 1: Write the failing test**

`crates/server/tests/schema.rs`:
```rust
// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Task 1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Guards the sync contract: every +sync table carries the sync columns and
//! both triggers, and a write produces exactly one change_log row.
//!
//! Requires DATABASE_URL pointing at a Postgres 16 server with pgvector.
//! `sqlx::test` creates a scratch database per test and runs ../../migrations.

use std::collections::BTreeSet;

use sqlx::{PgPool, Row};
use uuid::Uuid;

const SYNC_TABLES: &[&str] = &[
    "workspaces", "sources", "messages", "decks", "flashcards",
    "flashcard_reviews", "quizzes", "quiz_questions", "quiz_attempts",
    "reports", "cheatsheets", "tasks", "goals", "events", "study_sessions",
    "pending_actions",
];

async fn insert_user(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into users (id, email, password_hash) values ($1, $2, 'x')",
    )
    .bind(id)
    .bind(format!("{id}@example.test"))
    .execute(pool)
    .await
    .expect("insert user");
    id
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_sync_table_has_the_sync_columns(pool: PgPool) {
    for table in SYNC_TABLES {
        let rows = sqlx::query(
            "select column_name from information_schema.columns
             where table_schema = 'public' and table_name = $1",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .expect("query columns");
        let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();

        for required in ["user_id", "updated_at", "deleted_at"] {
            assert!(
                columns.iter().any(|c| c == required),
                "{table} is missing the sync column {required} (has {columns:?})"
            );
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_settings_has_only_updated_at(pool: PgPool) {
    let rows = sqlx::query(
        "select column_name from information_schema.columns
         where table_schema = 'public' and table_name = 'user_settings'",
    )
    .fetch_all(&pool)
    .await
    .expect("query columns");
    let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();
    assert!(columns.iter().any(|c| c == "updated_at"));
    assert!(columns.iter().any(|c| c == "user_id"));
    assert!(!columns.iter().any(|c| c == "deleted_at"), "user_settings has no tombstones");
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_sync_table_has_both_triggers(pool: PgPool) {
    for table in SYNC_TABLES {
        let names: Vec<String> = sqlx::query_scalar(
            "select t.tgname from pg_trigger t
             join pg_class c on c.oid = t.tgrelid
             where not t.tgisinternal and c.relname = $1",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .expect("query triggers");

        assert!(
            names.iter().any(|n| n == &format!("{table}_touch_updated_at")),
            "{table} is missing touch_updated_at (has {names:?})"
        );
        assert!(
            names.iter().any(|n| n == &format!("{table}_log_change")),
            "{table} is missing log_change (has {names:?})"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_sync_tables_have_no_triggers(pool: PgPool) {
    for table in ["chunks", "change_log", "mutation_log", "jobs", "usage_events", "credit_ledger"] {
        let count: i64 = sqlx::query_scalar(
            "select count(*) from pg_trigger t
             join pg_class c on c.oid = t.tgrelid
             where not t.tgisinternal and c.relname = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("count triggers");
        assert_eq!(count, 0, "{table} must not be synced");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn insert_appends_one_change_log_row_with_the_device(pool: PgPool) {
    let user = insert_user(&pool).await;
    let device = Uuid::now_v7();
    sqlx::query("insert into devices (id, user_id, name, platform) values ($1, $2, 't', 'macos')")
        .bind(device)
        .bind(user)
        .execute(&pool)
        .await
        .expect("insert device");

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("select set_config('app.device_id', $1, true)")
        .bind(device.to_string())
        .execute(&mut *tx)
        .await
        .expect("set device");
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'Algorithms')")
        .bind(ws)
        .bind(user)
        .execute(&mut *tx)
        .await
        .expect("insert workspace");
    tx.commit().await.expect("commit");

    let row = sqlx::query(
        "select entity, entity_id, op, origin_device_id from change_log
         where user_id = $1 order by seq desc limit 1",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .expect("change_log row");

    assert_eq!(row.get::<String, _>("entity"), "workspaces");
    assert_eq!(row.get::<Uuid, _>("entity_id"), ws);
    assert_eq!(row.get::<String, _>("op"), "upsert");
    assert_eq!(row.get::<Option<Uuid>, _>("origin_device_id"), Some(device));
}

#[sqlx::test(migrations = "../../migrations")]
async fn soft_delete_logs_a_tombstone_and_touch_sets_updated_at(pool: PgPool) {
    let user = insert_user(&pool).await;
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(ws)
        .bind(user)
        .execute(&pool)
        .await
        .expect("insert workspace");

    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select updated_at from workspaces where id = $1")
            .bind(ws)
            .fetch_one(&pool)
            .await
            .expect("updated_at");

    sqlx::query("update workspaces set deleted_at = now() where id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .expect("soft delete");

    let (op, after): (String, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "select cl.op, w.updated_at from change_log cl
         join workspaces w on w.id = cl.entity_id
         where cl.entity_id = $1 order by cl.seq desc limit 1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .expect("latest change");

    assert_eq!(op, "delete");
    assert!(after > before, "touch_updated_at must advance updated_at");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_update_of_a_soft_deleted_row_stays_a_tombstone(pool: PgPool) {
    let user = insert_user(&pool).await;
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(ws).bind(user).execute(&pool).await.unwrap();
    sqlx::query("update workspaces set deleted_at = now() where id = $1")
        .bind(ws).execute(&pool).await.unwrap();
    sqlx::query("update workspaces set name = 'renamed' where id = $1")
        .bind(ws).execute(&pool).await.unwrap();

    let op: String = sqlx::query_scalar(
        "select op from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(op, "delete");
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_settings_update_logs_an_upsert_keyed_by_user(pool: PgPool) {
    let user = insert_user(&pool).await;
    sqlx::query("insert into user_settings (user_id) values ($1)")
        .bind(user)
        .execute(&pool)
        .await
        .expect("insert settings");
    sqlx::query("update user_settings set study_start_hour = 9 where user_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("update settings");

    let (entity, entity_id, op): (String, Uuid, String) = sqlx::query_as(
        "select entity, entity_id, op from change_log
         where user_id = $1 and entity = 'user_settings' order by seq desc limit 1",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .expect("settings change");
    assert_eq!(entity, "user_settings");
    assert_eq!(entity_id, user);
    assert_eq!(op, "upsert");
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_table_in_the_spec_ddl_exists(pool: PgPool) {
    // The spec's schema block is exhaustive, so it is the oracle: a table
    // dropped from a migration must fail here rather than pass silently
    // because a hand-written list was updated to match the mistake.
    let spec = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/SPEC.md"),
    )
    .expect("docs/SPEC.md must be readable");

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for line in spec.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("create table ") else {
            continue;
        };
        let name = rest
            .trim()
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or_default();
        if !name.is_empty() {
            expected.insert(name.to_string());
        }
    }
    assert!(
        expected.len() > 25,
        "expected to extract the spec's table list, got {expected:?}"
    );

    let actual: BTreeSet<String> = sqlx::query_scalar(
        "select table_name from information_schema.tables
         where table_schema = 'public' and table_type = 'BASE TABLE'",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables")
    .into_iter()
    .collect();

    let missing: Vec<&String> = expected.difference(&actual).collect();
    assert!(missing.is_empty(), "spec tables missing from the schema: {missing:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn tables_added_beyond_the_spec_are_deliberate(pool: PgPool) {
    // Anything not in the spec's DDL must be a documented deviation, so this
    // test fails when someone invents a table without recording it.
    let documented = ["report_revisions", "cheatsheets"];
    let spec_tables = SYNC_TABLES
        .iter()
        .map(|t| t.to_string())
        .chain(
            [
                "plans", "users", "devices", "auth_sessions", "push_tokens", "change_log",
                "mutation_log", "chunks", "calendar_connections", "subscriptions", "payments",
                "webhook_events", "credit_ledger", "usage_events", "discount_codes",
                "user_api_keys", "jobs", "user_settings",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .chain(documented.iter().map(|t| t.to_string()))
        .collect::<BTreeSet<_>>();

    let actual: BTreeSet<String> = sqlx::query_scalar(
        "select table_name from information_schema.tables
         where table_schema = 'public' and table_type = 'BASE TABLE'",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables")
    .into_iter()
    .collect();

    let undocumented: Vec<&String> = actual.difference(&spec_tables).collect();
    assert!(
        undocumented.is_empty(),
        "tables exist that are neither in the spec nor a documented deviation: {undocumented:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_concurrent_writer_blocks_so_seq_order_matches_commit_order(pool: PgPool) {
    // The cursor rule depends on this: seq is assigned by nextval() at insert
    // time but rows become visible at commit time, so a second transaction for
    // the same user MUST be held back until the first commits. If this lock
    // ever disappears, a pull can observe seq 11 while seq 10 is still
    // uncommitted, advance its cursor past 10, and lose that change forever.
    let user = insert_user(&pool).await;

    let mut tx_a = pool.begin().await.unwrap();
    sqlx::query("select set_config('app.device_id', '', true)")
        .execute(&mut *tx_a)
        .await
        .unwrap();
    let ws_a = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'a')")
        .bind(ws_a)
        .bind(user)
        .execute(&mut *tx_a)
        .await
        .unwrap();
    let seq_a: i64 = sqlx::query_scalar(
        "select seq from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws_a)
    .fetch_one(&mut *tx_a)
    .await
    .unwrap();

    // A second connection writing for the same user must block on the advisory
    // lock. Bound the wait so the test cannot hang.
    let mut tx_b = pool.begin().await.unwrap();
    sqlx::query("select set_config('app.device_id', '', true)")
        .execute(&mut *tx_b)
        .await
        .unwrap();
    sqlx::query("set local lock_timeout = '250ms'")
        .execute(&mut *tx_b)
        .await
        .unwrap();
    let blocked = sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'b')")
        .bind(Uuid::now_v7())
        .bind(user)
        .execute(&mut *tx_b)
        .await;

    assert!(
        blocked.is_err(),
        "a concurrent write for the same user must block on the per-user advisory lock"
    );
    tx_b.rollback().await.unwrap();

    tx_a.commit().await.unwrap();

    // With A committed, B proceeds and is ordered strictly after A.
    let mut tx_b = pool.begin().await.unwrap();
    let ws_b = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'b')")
        .bind(ws_b)
        .bind(user)
        .execute(&mut *tx_b)
        .await
        .unwrap();
    let seq_b: i64 = sqlx::query_scalar(
        "select seq from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws_b)
    .fetch_one(&mut *tx_b)
    .await
    .unwrap();
    tx_b.commit().await.unwrap();

    assert!(seq_b > seq_a, "seq {seq_b} must be ordered after {seq_a}");

    // A reader that has seen seq_b can never be missing seq_a.
    let seen: Vec<i64> = sqlx::query_scalar(
        "select seq from change_log where user_id = $1 and seq <= $2 order by seq",
    )
    .bind(user)
    .bind(seq_b)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(seen.contains(&seq_a), "seq_a must be visible once seq_b is: {seen:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn change_log_is_scoped_to_one_user(pool: PgPool) {
    let (a, b) = (insert_user(&pool).await, insert_user(&pool).await);
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'a')")
        .bind(Uuid::now_v7()).bind(a).execute(&pool).await.unwrap();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'b')")
        .bind(Uuid::now_v7()).bind(b).execute(&pool).await.unwrap();

    let a_seqs: Vec<i64> =
        sqlx::query_scalar("select seq from change_log where user_id = $1 order by seq")
            .bind(a).fetch_all(&pool).await.unwrap();
    let b_seqs: Vec<i64> =
        sqlx::query_scalar("select seq from change_log where user_id = $1 order by seq")
            .bind(b).fetch_all(&pool).await.unwrap();

    assert_eq!(a_seqs.len(), 1, "user A sees only its own change");
    assert_eq!(b_seqs.len(), 1, "user B sees only its own change");
    assert!(a_seqs[0] < b_seqs[0], "seq is globally monotonic");
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_user_cascades_its_change_log(pool: PgPool) {
    let user = insert_user(&pool).await;
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(Uuid::now_v7()).bind(user).execute(&pool).await.unwrap();
    sqlx::query("delete from users where id = $1").bind(user).execute(&pool).await.unwrap();

    let count: i64 = sqlx::query_scalar("select count(*) from change_log where user_id = $1")
        .bind(user).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 2: Run the tests to confirm the sync contract holds**

Run: `cargo test -p stvdy-server --test schema`
Expected: PASS (9 tests). If `every_sync_table_has_the_sync_columns` fails, the named table is missing a column in its migration. Fix the migration, do not weaken the test.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/schema.rs
git commit -m "test(db): assert sync columns, trigger coverage and change_log semantics"
```

---

## Task 8: docker-compose (incl. SearXNG), env template, and the DB pool + plan seeding

**Files:**
- Create: `docker-compose.yml`, `.env.example`, `searxng/settings.yml`
- Create: `crates/server/src/config.rs`, `crates/server/src/db.rs`
- Modify: `crates/server/src/lib.rs`

**Interfaces:**
- Consumes: `stvdy_core::config::{load_plans, load_billing_policy}`.
- Produces:
  - `server::config::AppConfig::from_env() -> anyhow::Result<AppConfig>` with fields `database_url`, `bind_addr`, `plans: PlansFile`, `billing_policy: BillingPolicy`, `env: Env`, `jwt_secret: SecretString`
  - `server::db::connect(cfg) -> anyhow::Result<PgPool>`, `server::db::migrate(&pool)`, `server::db::seed_plans(&pool, &PlansFile)`

- [ ] **Step 1: Write `docker-compose.yml`**

```yaml
services:
  postgres:
    image: pgvector/pgvector:pg16
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
      POSTGRES_DB: stvdy
    ports: ["5432:5432"]
    volumes: ["pgdata:/var/lib/postgresql/data"]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres -d stvdy"]
      interval: 5s
      timeout: 3s
      retries: 20

  minio:
    # `minio/minio` was REMOVED from Docker Hub (A15). Verified 404 on both the
    # Hub API and `docker manifest inspect`. This is the community fork/mirror;
    # same S3 API, same `server /data` CLI, amd64+arm64. Verified working:
    # bucket create + object put/get via the matching `pgsty/mc`.
    image: pgsty/minio:latest
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin
    ports: ["9000:9000", "9001:9001"]
    volumes: ["miniodata:/data"]
    healthcheck:
      test: ["CMD", "mc", "ready", "local"]
      interval: 5s
      timeout: 3s
      retries: 20

  minio-init:
    image: pgsty/mc:latest   # `minio/mc` is gone too (A15); this is the mirror
    depends_on:
      minio:
        condition: service_healthy
    entrypoint: >
      /bin/sh -c "
      mc alias set local http://minio:9000 minioadmin minioadmin &&
      mc mb --ignore-existing local/stvdy-sources &&
      mc anonymous set none local/stvdy-sources &&
      echo 'bucket ready'
      "

  # Search tier 2 (A3). Internal only: there is deliberately NO `ports:` mapping,
  # so the container is reachable only by the server over the compose network at
  # http://searxng:8080. Add `ports: ["8088:8080"]` locally only to poke the UI.
  searxng:
    image: docker.io/searxng/searxng:latest   # dev only; pin a digest in prod
    environment:
      # SearXNG maps `SEARXNG_*` env vars onto settings paths, so this sets
      # server.secret_key and is the ONE source of that value. Settings.yml
      # deliberately omits it so no secret is ever committed. Verified: the
      # container starts with no `secret_key` in settings.yml as long as this
      # variable is present.
      SEARXNG_SECRET: ${SEARXNG_SECRET:?set SEARXNG_SECRET in .env}
      SEARXNG_BASE_URL: http://searxng:8080/
    volumes:
      - ./searxng:/etc/searxng:ro
    healthcheck:
      # Verified against the image: /usr/sbin/wget exists, curl does not, and
      # GET /healthz returns 200 with body "OK".
      test: ["CMD", "wget", "-q", "-O", "/dev/null", "http://127.0.0.1:8080/healthz"]
      interval: 10s
      timeout: 3s
      retries: 10
      start_period: 20s
    restart: unless-stopped

volumes:
  pgdata:
  miniodata:
```

- [ ] **Step 1b: Write `searxng/settings.yml`**

`searxng/settings.yml`:
```yaml
use_default_settings: true

server:
  # `secret_key` is deliberately absent: it comes from the SEARXNG_SECRET env var
  # in docker-compose.yml, so no secret is ever committed to the repository.
  limiter: false          # server-to-server only; there are no browser clients to rate-limit
  public_instance: false

search:
  formats: [html, json]   # REQUIRED. `json` is off by default; without it the API returns 403.

engines:
  # Engine list is tuned for the PRODUCTION host (A45), not the dev laptop.
  # Measured 2026-10-11 on the VPS: google cse 20 results, mojeek 10, mwmbl 12-55
  # -> 30-83 results/query at 0.9-1.1s. On the dev laptop the same config yields
  # only google cse (20), because the ISP intercepts duckduckgo and rate-limits
  # the rest. Do NOT re-tune this against a laptop.
  - name: brave
    disabled: true        # 429 "too many requests" in BOTH environments -> dead
  - name: duckduckgo
    disabled: true        # laptop: ISP TLS interception; VPS: CAPTCHA -> dead
  - name: qwant
    disabled: true        # CAPTCHA in both environments -> dead
  - name: startpage
    inactive: false       # upstream default is inactive; enabled then CAPTCHA'd
    disabled: true        # -> keep off, it is CAPTCHA in both environments
  # These two ARE the production workhorses. `mojeek` is `inactive: true`
  # upstream and must be explicitly re-enabled; `mwmbl` is enabled by default.
  # Both answered consistently across 6 consecutive queries on the VPS.
  - name: mojeek
    inactive: false
  - name: mwmbl
    disabled: false
  #
  # `google cse` (enabled by default) is the third production engine, but note
  # what it is: it scrapes a hardcoded third-party CX id (blackle.com) with no
  # API key. It is the single largest result contributor AND the one with a ToS
  # question attached, which is why it must never be the only engine (A3).
  # Verify after any image upgrade: `just db-search` should show >= 3 engines.
```

**Verification for Step 1b**. With the stack up:
```bash
curl -s "http://127.0.0.1:8088/search?q=spaced+repetition&format=json" | head -c 200   # only with the dev port mapping
docker compose exec searxng wget -qO- "http://127.0.0.1:8080/search?q=test&format=json" | head -c 200
```
Expected: a JSON object with a `results` array, not the `Verifying your browser…` HTML page (which is what a *public* SearXNG instance returns to an unauthenticated JSON client).

- [ ] **Step 2: Write `.env.example`**

```dotenv
# --- runtime ---
APP_ENV=dev                      # dev | prod
BIND_ADDR=127.0.0.1:8080
RUST_LOG=info,stvdy_server=debug,tower_http=debug

# --- database / storage ---
# Dev: either the compose postgres (below) or Supabase. If Supabase, use the
# TRANSACTION POOLER (port 6543) and note that db.rs must set
# statement_cache_capacity(0). See A47. Production must NOT be Supabase free
# (500 MB cannot hold a pgvector workload; ~7 paying users would fill it).
DATABASE_URL=postgres://postgres:postgres@localhost:5432/stvdy
# Supabase example (pooler, transaction mode):
# DATABASE_URL=postgres://postgres.<ref>:<pw>@aws-0-<region>.pooler.supabase.com:6543/postgres

# --- object storage (A15: minio/* images were REMOVED from Docker Hub) ---
# Dev: the compose service uses pgsty/minio (community mirror, same S3 API).
# Prod: Cloudflare R2 free tier. 10 GB storage, 1M Class A / 10M Class B ops, $0 egress.
S3_ENDPOINT=http://localhost:9000
S3_REGION=us-east-1
S3_BUCKET=stvdy-sources
S3_ACCESS_KEY_ID=minioadmin
S3_SECRET_ACCESS_KEY=minioadmin
S3_FORCE_PATH_STYLE=true
# R2 example (S3_FORCE_PATH_STYLE stays true):
# S3_ENDPOINT=https://<accountid>.r2.cloudflarestorage.com

# --- auth (A48: Google login is server-side OIDC; NOT Supabase Auth) ---
# Google Cloud console -> Credentials -> OAuth client (Web application).
# Register the redirect URI per environment; never ship the secret to a client.
GOOGLE_LOGIN_CLIENT_ID=
GOOGLE_LOGIN_CLIENT_SECRET=
GOOGLE_LOGIN_REDIRECT_URI=http://localhost:8080/auth/google/callback

# --- secrets (dev values only; generate real ones for prod) ---
JWT_SECRET=dev-only-change-me
ENCRYPTION_KEY_HEX=0000000000000000000000000000000000000000000000000000000000000000

# --- config files ---
PLANS_CONFIG=config/plans.toml
BILLING_POLICY_CONFIG=config/billing_policy.toml
ADMIN_EMAIL=admin@example.test

# --- providers (Phase 2+) ---
LLM_BASE_URL=
LLM_API_KEY=
LLM_MODEL=
EMBEDDER=fastembed             # fastembed | http

# --- search (A3: composite = academic -> searxng -> exa -> tavily) ---
SEARCH_PROVIDER=composite       # composite | academic | searxng | exa | tavily
SEARCH_CACHE_TTL_HOURS=24       # A40: repeated query serves from cache, no second credit charge

# Tier 1 - academic APIs. No key required. OPENALEX_MAILTO puts us in the
# "polite pool" (10 req/s) and a free key raises the daily budget 10x.
ACADEMIC_PROVIDERS=openalex,eric,crossref,doaj,arxiv,pubmed
OPENALEX_MAILTO=                # e.g. admin@stvdy.app
OPENALEX_API_KEY=               # optional; free from openalex.org/settings/api
NCBI_API_KEY=                   # optional; raises PubMed 3 req/s -> 10 req/s

# Tier 2 - self-hosted SearXNG (compose service; internal network only)
SEARXNG_SECRET=dev-only-change-me-searxng-secret   # prod: 32+ random bytes; never commit the real one
SEARXNG_URL=http://searxng:8080
SEARXNG_ENGINE_ALLOWLIST=google_cse

# Tier 3 - fallback APIs, consulted only when tier 1 and 2 return nothing.
# Exa: $10 free credit/month (~1,428 searches, ~10,000 page fetches), no card.
EXA_API_KEY=
# Tavily: 1,000 free credits/month (~33 searches/day), no card.
SEARCH_API_KEY=

# --- page fetching for save-as-source / citation expansion (Phase 3) ---
MOLI_BIN=moli                   # JS-rendered pages; --block-private-networks is always on
PAGE_FETCH_TIMEOUT_SECS=15

# --- email (Phase 1; dev writes to var/mail) ---
SMTP_URL=
MAIL_FROM=no-reply@stvdy.app

# --- payments (Phase 13) ---
QRIS_BASE_URL=
QRIS_API_KEY=
QRIS_CALLBACK_TOKEN=
```

- [ ] **Step 3: Write the failing test for config loading**

`crates/server/tests/config.rs`:
```rust
use stvdy_server::config::{AppConfig, Env};

#[test]
fn loads_config_files_and_defaults_the_bind_address() {
    // Config-file paths are relative to the workspace root, so run from there.
    let cfg = AppConfig::from_env_at(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").as_path(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ],
    )
    .expect("config must load");

    assert_eq!(cfg.bind_addr.to_string(), "127.0.0.1:8080");
    assert_eq!(cfg.env, Env::Dev);
    assert_eq!(cfg.plans.plans.len(), 3);
    assert!(!cfg.billing_policy.purchase_allowed(stvdy_core::config::Platform::Ios));
    assert!(cfg.billing_policy.purchase_allowed(stvdy_core::config::Platform::Web));
}

#[test]
fn a_missing_plans_file_is_reported_with_its_path() {
    let err = AppConfig::from_env_at(
        std::path::Path::new("/tmp"),
        &[("PLANS_CONFIG", "does-not-exist.toml")],
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("does-not-exist.toml"), "got: {msg}");
}
```

- [ ] **Step 4: Run the test to confirm it fails**

Run: `cargo test -p stvdy-server --test config`
Expected: FAIL to compile. `stvdy_server::config::AppConfig` does not exist.

- [ ] **Step 5: Implement `config.rs`**

```rust
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use secrecy::SecretString;
use stvdy_core::config::{load_billing_policy, load_plans, BillingPolicy, PlansFile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    Dev,
    Prod,
}

impl Env {
    fn from_str(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "dev" | "development" | "test" => Ok(Env::Dev),
            "prod" | "production" => Ok(Env::Prod),
            other => anyhow::bail!("APP_ENV must be dev or prod, got {other}"),
        }
    }

    pub fn is_dev(self) -> bool {
        matches!(self, Env::Dev)
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub env: Env,
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub jwt_secret: SecretString,
    pub encryption_key_hex: String,
    pub plans: PlansFile,
    pub billing_policy: BillingPolicy,
    pub admin_email: Option<String>,
    pub root: PathBuf,
}

impl AppConfig {
    /// Reads the process environment. `root` is the directory that relative
    /// config paths resolve against (the workspace root in dev, `/app` in a
    /// container).
    pub fn from_env() -> anyhow::Result<Self> {
        let root = std::env::current_dir().context("resolving the working directory")?;
        Self::from_env_at(&root, &[])
    }

    /// Testable variant: `overrides` replaces process env for the given keys.
    pub fn from_env_at(root: &Path, overrides: &[(&str, &str)]) -> anyhow::Result<Self> {
        let get = |key: &str| -> Option<String> {
            overrides
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
                .or_else(|| std::env::var(key).ok())
        };
        let get_or = |key: &str, default: &str| get(key).unwrap_or_else(|| default.to_string());

        let env = Env::from_str(&get_or("APP_ENV", "dev"))?;

        let bind_addr: SocketAddr = get_or("BIND_ADDR", "127.0.0.1:8080")
            .parse()
            .context("BIND_ADDR must be host:port")?;

        let plans_path = root.join(get_or("PLANS_CONFIG", "config/plans.toml"));
        let plans = load_plans(&plans_path)
            .with_context(|| format!("loading plans config at {}", plans_path.display()))?;

        let policy_path = root.join(get_or("BILLING_POLICY_CONFIG", "config/billing_policy.toml"));
        let billing_policy = load_billing_policy(&policy_path).with_context(|| {
            format!("loading billing policy at {}", policy_path.display())
        })?;

        let database_url = get_or("DATABASE_URL", "postgres://postgres:postgres@localhost:5432/stvdy");

        let jwt_secret = SecretString::from(get_or("JWT_SECRET", "dev-only-change-me"));
        if !env.is_dev() {
            // A prod deploy with the dev secret is a silent full-auth bypass.
            anyhow::ensure!(
                get("JWT_SECRET").is_some_and(|s| s.len() >= 32),
                "JWT_SECRET must be set to at least 32 characters when APP_ENV=prod"
            );
        }

        Ok(AppConfig {
            env,
            bind_addr,
            database_url,
            jwt_secret,
            encryption_key_hex: get_or("ENCRYPTION_KEY_HEX", &"0".repeat(64)),
            plans,
            billing_policy,
            admin_email: get("ADMIN_EMAIL"),
            root: root.to_path_buf(),
        })
    }
}
```

- [ ] **Step 6: Implement `db.rs`**

```rust
use anyhow::Context;
use sqlx::postgres::{PgPool, PgPoolOptions};
use stvdy_core::config::{PlanId, PlansFile};

use crate::config::AppConfig;

pub async fn connect(cfg: &AppConfig) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&cfg.database_url)
        .await
        .context("connecting to Postgres")?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .context("running migrations")?;
    Ok(())
}

/// Mirrors `config/plans.toml` into the `plans` table so billing rows keep a
/// valid FK and the entitlements snapshot stays auditable. The TOML remains the
/// authority: entitlement checks read the typed config, never this table.
pub async fn seed_plans(pool: &PgPool, plans: &PlansFile) -> anyhow::Result<()> {
    for plan_id in PlanId::ALL {
        let config = plans.get(plan_id);
        let entitlements = serde_json::to_value(config).context("serializing plan config")?;
        sqlx::query(
            "insert into plans (id, name, price_idr_monthly, price_idr_yearly, entitlements)
             values ($1, $2, $3, $4, $5)
             on conflict (id) do update
               set name = excluded.name,
                   price_idr_monthly = excluded.price_idr_monthly,
                   price_idr_yearly = excluded.price_idr_yearly,
                   entitlements = excluded.entitlements",
        )
        .bind(plan_id.as_str())
        .bind(&config.name)
        .bind(config.price_idr_monthly as i32)
        .bind(config.price_idr_yearly as i32)
        .bind(entitlements)
        .execute(pool)
        .await
        .with_context(|| format!("seeding plan {plan_id}"))?;
    }
    Ok(())
}
```

`PlanConfig` must derive `Serialize` for `seed_plans`. Add `Serialize` to its derive list and to `PlansFile`'s, and `#[serde(serialize_with)]` is not needed because `limit_of` only affects deserialization.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p stvdy-server --test config`
Expected: PASS (2 tests).

- [ ] **Step 8: Verify migrations + seeding against the real database**

Run:
```bash
docker compose up -d postgres
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/stvdy
sqlx migrate run
cargo test -p stvdy-server --test schema -- --nocapture
```
Expected: schema tests PASS against the compose database.

- [ ] **Step 9: Commit**

```bash
git add docker-compose.yml .env.example crates/server
git commit -m "feat(server): env config, DB pool, migration runner and plan seeding"
```

---

## Task 9: Axum server with `/health`, request-id tracing, and error mapping

**Files:**
- Create: `crates/server/src/telemetry.rs`, `crates/server/src/error.rs`, `crates/server/src/state.rs`, `crates/server/src/routes/mod.rs`, `crates/server/src/routes/health.rs`
- Modify: `crates/server/src/main.rs`, `crates/server/src/lib.rs`
- Test: `crates/server/tests/health.rs`

**Interfaces:**
- Consumes: `AppConfig`, `connect`, `migrate`, `seed_plans`.
- Produces:
  - `AppState { pool: PgPool, config: Arc<AppConfig> }` (later phases add provider handles here)
  - `AppError` with the status/code mapping from the roadmap § HTTP error mapping, and `AppResult<T>`
  - `routes::router(state: AppState) -> axum::Router`
  - `telemetry::init(&str)`

- [ ] **Step 1: Write the failing test**

`crates/server/tests/health.rs`:
```rust
// See Task 1: `#[sqlx::test]` is a macro harness, so clippy.toml's
// test relaxations are not guaranteed to apply.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`

#[sqlx::test(migrations = "../../migrations")]
async fn health_reports_ok_and_the_plan_count(pool: sqlx::PgPool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = stvdy_server::config::AppConfig::from_env_at(
        &root,
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ],
    )
    .unwrap();
    stvdy_server::db::seed_plans(&pool, &config.plans).await.unwrap();

    let state = stvdy_server::state::AppState {
        pool,
        config: Arc::new(config),
    };
    let app = stvdy_server::routes::router(state);

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["plans"], 3);
    assert!(json["version"].is_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_routes_return_the_typed_not_found_error(pool: sqlx::PgPool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = stvdy_server::config::AppConfig::from_env_at(
        &root,
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ],
    )
    .unwrap();
    let state = stvdy_server::state::AppState { pool, config: Arc::new(config) };
    let app = stvdy_server::routes::router(state);

    let response = app
        .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "not_found");
}

#[test]
fn error_mapping_matches_the_documented_table() {
    use stvdy_server::error::AppError;
    for (error, status, code) in [
        (AppError::Unauthorized, 401, "unauthorized"),
        (AppError::TokenExpired, 401, "token_expired"),
        (AppError::Forbidden, 403, "forbidden"),
        (AppError::NotFound, 404, "not_found"),
        (AppError::Conflict("x".into()), 409, "conflict"),
        (AppError::ResyncRequired, 410, "resync_required"),
        (AppError::RateLimited, 429, "rate_limited"),
    ] {
        assert_eq!(error.status().as_u16(), status, "{code}");
        assert_eq!(error.code(), code);
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cargo test -p stvdy-server --test health`
Expected: FAIL to compile. `stvdy_server::routes` does not exist.

- [ ] **Step 3: Implement `error.rs`**

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub type AppResult<T> = Result<T, AppError>;

/// Every error the API can return. The status/code pairs are the contract
/// documented in the roadmap § HTTP error mapping; keep the two in sync.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("authentication required")]
    Unauthorized,

    #[error("access token expired")]
    TokenExpired,

    #[error("not permitted")]
    Forbidden,

    #[error("not found")]
    NotFound,

    #[error("{0}")]
    Validation(String),

    #[error("{0}")]
    Conflict(String),

    #[error("sync cursor is older than retained history")]
    ResyncRequired,

    #[error("rate limited")]
    RateLimited,

    #[error("{message}")]
    Entitlement {
        code: &'static str,
        message: String,
        min_plan: Option<&'static str>,
    },

    #[error("provider error: {0}")]
    Provider(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::Unauthorized | AppError::TokenExpired => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::ResyncRequired => StatusCode::GONE,
            AppError::Entitlement { code, .. } if *code == "insufficient_credits" => {
                StatusCode::PAYMENT_REQUIRED
            }
            AppError::Entitlement { .. } => StatusCode::FORBIDDEN,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::Provider(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::Unauthorized => "unauthorized",
            AppError::TokenExpired => "token_expired",
            AppError::Forbidden => "forbidden",
            AppError::NotFound => "not_found",
            AppError::Validation(_) => "validation",
            AppError::Conflict(_) => "conflict",
            AppError::ResyncRequired => "resync_required",
            AppError::RateLimited => "rate_limited",
            AppError::Entitlement { code, .. } => code,
            AppError::Provider(_) => "provider_error",
            AppError::Internal(_) => "internal",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();

        // Internal causes are logged, never returned: they can carry SQL text.
        let message = match &self {
            AppError::Internal(cause) => {
                tracing::error!(error = %cause, "internal error");
                "internal error".to_string()
            }
            other => other.to_string(),
        };

        let mut body = json!({ "code": code, "message": message });
        if let AppError::Entitlement { min_plan, .. } = &self {
            body["min_plan"] = json!(min_plan);
        }

        (status, Json(body)).into_response()
    }
}
```

- [ ] **Step 4: Implement `state.rs`, `routes/*`, `telemetry.rs`, `main.rs`**

`state.rs`:
```rust
use std::sync::Arc;

use sqlx::PgPool;

use crate::config::AppConfig;

/// Shared application state. Provider handles (LLM, embedder, search, push,
/// payments, storage) are added here by the phases that introduce them.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<AppConfig>,
}
```

`routes/mod.rs`:
```rust
pub mod health;

use axum::routing::get;
use axum::Router;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new().route("/health", get(health::health));

    Router::new()
        .merge(api)
        .fallback(crate::routes::health::not_found)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(SetSensitiveRequestHeadersLayer::new(std::iter::once(
            axum::http::header::AUTHORIZATION,
        )))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        // Dev-only permissive CORS so the Vite dev server can call the API.
        // Production origins are configured when the web build is deployed.
        .layer(CorsLayer::permissive())
        .with_state(state)
}
```

`routes/health.rs`:
```rust
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::state::AppState;

/// Liveness plus a cheap readiness probe: the pool must answer and the plans
/// config must have loaded.
pub async fn health(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    sqlx::query_scalar::<_, i32>("select 1")
        .fetch_one(&state.pool)
        .await
        .map_err(|e| AppError::Internal(anyhow::Error::from(e)))?;

    Ok(Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "env": if state.config.env.is_dev() { "dev" } else { "prod" },
        "plans": state.config.plans.plans.len(),
    })))
}

pub async fn not_found() -> AppError {
    AppError::NotFound
}
```

`telemetry.rs`:
```rust
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Structured logs. JSON in prod (so a collector can parse fields), human
/// readable in dev.
pub fn init(env: &str, default_filter: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter));

    let registry = tracing_subscriber::registry().with(filter);
    if env == "prod" {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer().with_target(true)).init();
    }
}
```

`main.rs`:
```rust
use std::sync::Arc;

use anyhow::Context;
use stvdy_server::{config::AppConfig, db, routes, state::AppState, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::from_env()?;
    telemetry::init(
        if config.env.is_dev() { "dev" } else { "prod" },
        "info,stvdy_server=debug,tower_http=debug",
    );

    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    db::seed_plans(&pool, &config.plans).await?;

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config.clone()),
    };
    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "stvdy-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
```

Add `pub mod routes;` to `lib.rs` (Task 1 already lists it).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p stvdy-server`
Expected: PASS (schema 9 + config 2 + health 3).

- [ ] **Step 6: Smoke-test the running server**

Run:
```bash
docker compose up -d postgres
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/stvdy
cargo run -p stvdy-server &
sleep 3
curl -s localhost:8080/health
curl -s -o /dev/null -w '%{http_code}\n' localhost:8080/nope
kill %1
```
Expected: `{"status":"ok","version":"0.1.0","env":"dev","plans":3}` and `404`. This is the phase's real smoke test: the server boots, migrates, seeds and answers.

- [ ] **Step 7: Commit**

```bash
git add crates/server
git commit -m "feat(server): axum app with health, request-id tracing and typed error mapping"
```

---

## Task 10: `crates/sync` protocol DTOs

**Files:**
- Create: `crates/sync/src/proto/entities.rs`, `pull.rs`, `push.rs`, `error.rs`
- Modify: `crates/sync/src/proto/mod.rs`, `crates/sync/src/lib.rs`
- Test: `crates/sync/tests/wire.rs`

**Interfaces:**
- Consumes: `serde`, `uuid`, `chrono`.
- Produces (normative names used by every later phase):
  - `Entity` (17 variants) with `Entity::table_name()`, `Entity::from_table_name(&str)`, `Entity::dependency_rank()`, `Entity::is_client_writable()`, `Entity::accepts_push_op(Op)`
  - `Op { Upsert, Delete }`
  - `Change { seq, entity, id, op, data: Option<Value> }`
  - `PullResponse { changes, next_cursor, has_more }`
  - `Mutation { mutation_id, entity, op, id, payload, client_ts }`
  - `PushRequest { device_id, mutations }`, `PushResponse { results }`
  - `MutationResult { Applied{seq}, Rejected{code,message}, Conflict{winner} }`
  - `ErrorCode` (`serde` snake_case) matching the server's error codes

- [ ] **Step 1: Write the failing test**

`crates/sync/tests/wire.rs`:
```rust
use chrono::{TimeZone, Utc};
use serde_json::json;
use stvdy_sync::proto::{
    Change, Entity, Mutation, MutationResult, Op, PullResponse, PushRequest, PushResponse,
};
use uuid::Uuid;

#[test]
fn only_the_offline_scope_is_client_writable() {
    // Compare order-independently: `Entity::ALL` is in declaration order, which
    // is not the order of the writable subset, and the assertion should also
    // survive a future enum reordering.
    let mut writable: Vec<&str> = Entity::ALL
        .into_iter()
        .filter(|e| e.is_client_writable())
        .map(|e| e.table_name())
        .collect();
    writable.sort_unstable();

    let mut expected = vec![
        "tasks",
        "events",
        "flashcard_reviews",
        "quiz_attempts",
        "study_sessions",
        "user_settings",
    ];
    expected.sort_unstable();

    assert_eq!(
        writable, expected,
        "the pushable set must be exactly the spec's offline-write scope"
    );
}

#[test]
fn server_owned_entities_reject_client_pushes() {
    // Security-relevant: messages are persisted by the chat endpoint, so a
    // client push would skip the credit spend and allow forged assistant turns.
    for entity in [
        Entity::Message,
        Entity::Source,
        Entity::Flashcard,
        Entity::Deck,
        Entity::Quiz,
        Entity::QuizQuestion,
        Entity::Report,
        Entity::Goal,
        Entity::Workspace,
        Entity::PendingAction,
    ] {
        assert!(!entity.is_client_writable(), "{entity:?} must not be client-writable");
        assert!(!entity.accepts_push_op(Op::Upsert), "{entity:?} must reject upserts");
        assert!(!entity.accepts_push_op(Op::Delete), "{entity:?} must reject deletes");
    }
}

#[test]
fn append_only_entities_accept_upserts_but_not_deletes() {
    for entity in [Entity::FlashcardReview, Entity::QuizAttempt, Entity::StudySession] {
        assert!(entity.is_append_only());
        assert!(entity.accepts_push_op(Op::Upsert));
        assert!(!entity.accepts_push_op(Op::Delete), "{entity:?} is append-only");
    }
}

#[test]
fn mutable_offline_entities_accept_both_ops_and_settings_has_no_tombstone() {
    for entity in [Entity::Task, Entity::Event] {
        assert!(entity.accepts_push_op(Op::Upsert));
        assert!(entity.accepts_push_op(Op::Delete));
    }
    // user_settings is synced but has no deleted_at column.
    assert!(Entity::UserSettings.accepts_push_op(Op::Upsert));
    assert!(!Entity::UserSettings.accepts_push_op(Op::Delete));
}

#[test]
fn every_entity_round_trips_through_its_table_name() {
    for entity in Entity::ALL {
        let table = entity.table_name();
        assert_eq!(
            Entity::from_table_name(table),
            Some(entity),
            "{table} did not map back to {entity:?}"
        );
    }
}

#[test]
fn dependency_ranks_put_parents_before_children() {
    let rank = |e: Entity| e.dependency_rank();
    assert!(rank(Entity::Workspace) < rank(Entity::Source));
    assert!(rank(Entity::Source) < rank(Entity::Deck));
    assert!(rank(Entity::Deck) < rank(Entity::Flashcard));
    assert!(rank(Entity::Flashcard) < rank(Entity::FlashcardReview));
    assert!(rank(Entity::Quiz) < rank(Entity::QuizQuestion));
    assert!(rank(Entity::QuizQuestion) < rank(Entity::QuizAttempt));
    assert!(rank(Entity::Workspace) < rank(Entity::Cheatsheet));
    assert!(rank(Entity::Source) < rank(Entity::Cheatsheet));
    assert!(rank(Entity::Task) < rank(Entity::Event));
    // Ranks are a dense 1..=N sequence so initial sync can just sort by them.
    let mut ranks: Vec<u8> = Entity::ALL.iter().map(|e| e.dependency_rank()).collect();
    ranks.sort_unstable();
    assert_eq!(ranks, (1..=Entity::ALL.len() as u8).collect::<Vec<_>>());
}

#[test]
fn change_serializes_with_a_camel_case_free_contract() {
    let id = Uuid::now_v7();
    let change = Change {
        seq: 42,
        entity: Entity::Task,
        id,
        op: Op::Upsert,
        data: Some(json!({ "title": "Baca bab 3" })),
    };
    let value = serde_json::to_value(&change).unwrap();
    assert_eq!(value["seq"], 42);
    assert_eq!(value["entity"], "task");
    assert_eq!(value["op"], "upsert");
    assert_eq!(value["id"], id.to_string());

    let back: Change = serde_json::from_value(value).unwrap();
    assert_eq!(back.entity, Entity::Task);
    assert_eq!(back.op, Op::Upsert);
}

#[test]
fn a_tombstone_has_no_data() {
    let change = Change {
        seq: 7,
        entity: Entity::Workspace,
        id: Uuid::now_v7(),
        op: Op::Delete,
        data: None,
    };
    let value = serde_json::to_value(&change).unwrap();
    assert!(value["data"].is_null());
}

#[test]
fn pull_response_round_trips() {
    let response = PullResponse {
        changes: vec![],
        next_cursor: 128,
        has_more: true,
    };
    let text = serde_json::to_string(&response).unwrap();
    assert!(text.contains("\"next_cursor\":128"));
    assert!(text.contains("\"has_more\":true"));
    let back: PullResponse = serde_json::from_str(&text).unwrap();
    assert_eq!(back.next_cursor, 128);
    assert!(back.has_more);
}

#[test]
fn push_request_round_trips_with_client_generated_ids() {
    let request = PushRequest {
        device_id: Uuid::now_v7(),
        mutations: vec![Mutation {
            mutation_id: Uuid::now_v7(),
            entity: Entity::StudySession,
            op: Op::Upsert,
            id: Uuid::now_v7(),
            payload: Some(json!({ "duration_min": 25 })),
            client_ts: Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap(),
        }],
    };
    let text = serde_json::to_string(&request).unwrap();
    let back: PushRequest = serde_json::from_str(&text).unwrap();
    assert_eq!(back.mutations.len(), 1);
    assert_eq!(back.mutations[0].entity, Entity::StudySession);
    assert_eq!(back.mutations[0].client_ts.timestamp(), 1_760_173_200);
}

#[test]
fn mutation_results_carry_the_rejection_reason() {
    let applied = MutationResult::Applied { seq: 9 };
    assert_eq!(serde_json::to_value(&applied).unwrap()["status"], "applied");

    let rejected = MutationResult::Rejected {
        code: "limit_reached".into(),
        message: "sources_total".into(),
    };
    let value = serde_json::to_value(&rejected).unwrap();
    assert_eq!(value["status"], "rejected");
    assert_eq!(value["code"], "limit_reached");

    let response = PushResponse { results: vec![applied, rejected] };
    let back: PushResponse = serde_json::from_value(serde_json::to_value(&response).unwrap()).unwrap();
    assert_eq!(back.results.len(), 2);
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cargo test -p stvdy-sync`
Expected: FAIL to compile. The `proto` modules are empty.

- [ ] **Step 3: Implement the DTOs**

`crates/sync/src/proto/entities.rs`:
```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Every synced entity. `table_name()` must equal the Postgres table name,
/// because `change_log.entity` stores `tg_table_name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entity {
    Workspace,
    Source,
    Message,
    Deck,
    Flashcard,
    FlashcardReview,
    Quiz,
    QuizQuestion,
    QuizAttempt,
    Report,
    Cheatsheet,
    Event,
    Task,
    Goal,
    StudySession,
    UserSettings,
    PendingAction,
}

impl Entity {
    pub const ALL: [Entity; 17] = [
        Entity::Workspace,
        Entity::Source,
        Entity::Message,
        Entity::Deck,
        Entity::Flashcard,
        Entity::FlashcardReview,
        Entity::Quiz,
        Entity::QuizQuestion,
        Entity::QuizAttempt,
        Entity::Report,
        Entity::Cheatsheet,
        Entity::Event,
        Entity::Task,
        Entity::Goal,
        Entity::StudySession,
        Entity::UserSettings,
        Entity::PendingAction,
    ];

    pub fn table_name(self) -> &'static str {
        match self {
            Entity::Workspace => "workspaces",
            Entity::Source => "sources",
            Entity::Message => "messages",
            Entity::Deck => "decks",
            Entity::Flashcard => "flashcards",
            Entity::FlashcardReview => "flashcard_reviews",
            Entity::Quiz => "quizzes",
            Entity::QuizQuestion => "quiz_questions",
            Entity::QuizAttempt => "quiz_attempts",
            Entity::Report => "reports",
            Entity::Cheatsheet => "cheatsheets",
            Entity::Event => "events",
            Entity::Task => "tasks",
            Entity::Goal => "goals",
            Entity::StudySession => "study_sessions",
            Entity::UserSettings => "user_settings",
            Entity::PendingAction => "pending_actions",
        }
    }

    pub fn from_table_name(table: &str) -> Option<Entity> {
        Entity::ALL.into_iter().find(|e| e.table_name() == table)
    }

    /// Dense 1..=17 rank used to order the initial sync so parents land before
    /// children. Lower is applied first.
    pub fn dependency_rank(self) -> u8 {
        match self {
            Entity::Workspace => 1,
            Entity::Source => 2,
            Entity::Deck => 3,
            Entity::Flashcard => 4,
            Entity::Quiz => 5,
            Entity::QuizQuestion => 6,
            Entity::Report => 7,
            // Cheatsheet depends on workspaces + sources only, but appending it
            // last keeps the dense 1..=N property the wire test asserts.
            Entity::Cheatsheet => 17,
            Entity::Task => 8,
            Entity::Goal => 9,
            Entity::Event => 10,
            Entity::StudySession => 11,
            Entity::Message => 12,
            Entity::FlashcardReview => 13,
            Entity::QuizAttempt => 14,
            Entity::UserSettings => 15,
            Entity::PendingAction => 16,
        }
    }

    /// Whether a client may write this entity through `POST /sync/push`.
    ///
    /// This is exactly the spec's offline-write scope. Everything else is
    /// written through its dedicated endpoint. Two entries are security
    /// relevant rather than tidy:
    ///   - `Message` is persisted by `POST /workspaces/:id/chat`; accepting a
    ///     push would skip the credit spend and let a client forge assistant
    ///     turns with fabricated citations.
    ///   - `Source` status/file_key/read_only are server-owned; a client must
    ///     not be able to mark a source `ready` or bypass storage quotas.
    pub fn is_client_writable(self) -> bool {
        matches!(
            self,
            Entity::Task
                | Entity::Event
                | Entity::FlashcardReview
                | Entity::QuizAttempt
                | Entity::StudySession
                | Entity::UserSettings
        )
    }

    /// Append-only entities accept only `Op::Upsert`; their rows are never
    /// tombstoned, and a duplicate id is ignored rather than overwriting.
    pub fn is_append_only(self) -> bool {
        matches!(
            self,
            Entity::FlashcardReview | Entity::QuizAttempt | Entity::StudySession
        )
    }

    /// Whether `op` is acceptable for this entity through `/sync/push`.
    pub fn accepts_push_op(self, op: Op) -> bool {
        if !self.is_client_writable() {
            return false;
        }
        match op {
            Op::Upsert => true,
            // Append-only rows are never deleted, and `user_settings` has no
            // tombstone column at all.
            Op::Delete => !self.is_append_only() && self != Entity::UserSettings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Upsert,
    Delete,
}

/// One row of `change_log` plus the entity's current state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub seq: i64,
    pub entity: Entity,
    pub id: Uuid,
    pub op: Op,
    /// `None` for tombstones.
    pub data: Option<Value>,
}
```

`crates/sync/src/proto/pull.rs`:
```rust
use serde::{Deserialize, Serialize};

use super::entities::Change;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullResponse {
    pub changes: Vec<Change>,
    pub next_cursor: i64,
    pub has_more: bool,
}
```

`crates/sync/src/proto/push.rs`:
```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::entities::{Entity, Op};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mutation {
    /// Idempotency key: replaying the same `mutation_id` must not apply twice.
    pub mutation_id: Uuid,
    pub entity: Entity,
    pub op: Op,
    pub id: Uuid,
    pub payload: Option<Value>,
    pub client_ts: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRequest {
    pub device_id: Uuid,
    pub mutations: Vec<Mutation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MutationResult {
    Applied { seq: i64 },
    Rejected { code: String, message: String },
    Conflict { winner: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushResponse {
    pub results: Vec<MutationResult>,
}
```

`crates/sync/src/proto/error.rs`:
```rust
use serde::{Deserialize, Serialize};

/// Wire error codes. Mirrors `AppError::code()` on the server and
/// `ErrorCode` on the client, so both ends can switch exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    TokenExpired,
    Forbidden,
    NotFound,
    Validation,
    Conflict,
    ResyncRequired,
    LimitReached,
    InsufficientCredits,
    PlanRequired,
    FeatureLocked,
    /// The entity is not client-writable (see the pushable-entity allowlist in
    /// the roadmap): messages, sources, flashcards, quizzes, decks, reports,
    /// goals, workspaces and pending_actions are written through their own
    /// endpoints, never through `/sync/push`.
    ReadOnlyEntity,
    RateLimited,
    ProviderError,
    Internal,
}
```

Update `crates/sync/src/proto/mod.rs` to also `pub mod error;` and re-export `ErrorCode`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p stvdy-sync`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/sync
git commit -m "feat(sync): shared protocol DTOs with entity/table bijection tests"
```

---

## Task 11: SPA scaffold (Vite 8 + React 19 + TS 7 + Tailwind 4 + Router 8 + Query 5)

**Files:**
- Create: `app/package.json`, `app/vite.config.ts`, `app/tsconfig.json`, `app/vitest.config.ts`, `app/index.html`, `app/src/vite-env.d.ts`, `app/src/main.tsx`, `app/src/App.tsx`, `app/src/router.tsx`, `app/src/styles.css`, `app/src/test/setup.ts`
- Test: `app/src/lib/time.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `npm --prefix app run dev|build|test`; `app/dist` as Tauri's `frontendDist`; the `@/` path alias to `app/src`; `src/test/setup.ts` with jest-dom matchers.

- [ ] **Step 1: Write `package.json`**

```json
{
  "name": "stvdy-app",
  "private": true,
  "type": "module",
  "engines": { "node": ">=22.22.0" },
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "preview": "vite preview",
    "test": "vitest run",
    "test:watch": "vitest",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tanstack/react-query": "5.104.1",
    "@tauri-apps/api": "2.12.3",
    "@tauri-apps/plugin-deep-link": "2.6.1",
    "@tauri-apps/plugin-dialog": "2.8.2",
    "@tauri-apps/plugin-fs": "2.7.0",
    "@tauri-apps/plugin-notification": "2.6.0",
    "@tauri-apps/plugin-opener": "2.7.1",
    "@tauri-apps/plugin-os": "2.4.0",
    "eventsource-parser": "4.1.1",
    "react": "19.3.0",
    "react-dom": "19.3.0",
    "react-router": "8.4.0"
  },
  "devDependencies": {
    "@tailwindcss/vite": "4.3.3",
    "@tauri-apps/cli": "2.12.2",
    "@testing-library/dom": "10.4.2",
    "@testing-library/jest-dom": "7.0.1",
    "@testing-library/react": "16.3.3",
    "@testing-library/user-event": "14.6.7",
    "@types/node": "26.6.5",
    "@types/react": "19.3.0",
    "@types/react-dom": "19.3.0",
    "@vitejs/plugin-react": "6.1.2",
    "jsdom": "30.1.2",
    "tailwindcss": "4.3.3",
    "typescript": "7.0.2",
    "vite": "8.3.4",
    "vitest": "5.0.3"
  }
}
```

- [ ] **Step 2: Write the build config**

`app/vite.config.ts`:
```ts
/// <reference types="vitest/config" />
import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Set by the Tauri CLI when running on a physical device.
const host = process.env.TAURI_DEV_HOST
const isDebug = Boolean(process.env.TAURI_ENV_DEBUG)
const isWindows = process.env.TAURI_ENV_PLATFORM === 'windows'

export default defineConfig({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },

  // Tauri prints Rust compiler output to the same terminal; never wipe it.
  clearScreen: false,

  // Vite's envPrefix is a startsWith check, so 'TAURI_ENV_*' would never match.
  envPrefix: ['VITE_', 'TAURI_ENV_'],

  server: {
    port: 5173,        // must equal build.devUrl in src-tauri/tauri.conf.json
    strictPort: true,
    host: host || false,
    ws: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },

  build: {
    // Tailwind 4 requires Safari 16.4+, which is the honest floor for the
    // macOS/iOS WKWebView; Windows/Android use Chromium.
    target: isWindows ? 'chrome111' : 'safari16.4',
    minify: isDebug ? false : 'oxc',
    sourcemap: isDebug,
  },

  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
})
```

`app/vitest.config.ts`:
```ts
// Vitest reads the `test` block from vite.config.ts; this file exists so
// editors and `vitest` invocations from the app directory resolve the same
// config without duplicating it.
export { default } from './vite.config'
```

`app/tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2023",
    "lib": ["ES2023", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "noUncheckedIndexedAccess": true,
    "verbatimModuleSyntax": true,
    "skipLibCheck": true,
    "isolatedModules": true,
    "noEmit": true,
    "types": ["vite/client", "node", "vitest/globals"],
    "baseUrl": ".",
    "paths": { "@/*": ["./src/*"] }
  },
  "include": ["src", "vite.config.ts", "vitest.config.ts"]
}
```

Note: TypeScript 7 defaults `types` to `[]` and enables `noUncheckedSideEffectImports`, so `vite/client` must be listed explicitly for `import './styles.css'` to typecheck.

`app/index.html`:
```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <!-- viewport-fit=cover is required for env(safe-area-inset-*) to be non-zero. -->
    <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
    <title>Stvdy</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

`app/src/vite-env.d.ts`:
```ts
/// <reference types="vite/client" />
```

`app/src/test/setup.ts`:
```ts
import '@testing-library/jest-dom/vitest'
```

- [ ] **Step 3: Write `styles.css` with Tailwind v4 tokens**

```css
@import "tailwindcss";

/* Tailwind 4 is CSS-first: tokens live here, there is no tailwind.config.js. */
@theme {
  --color-brand-50: oklch(0.97 0.02 250);
  --color-brand-500: oklch(0.62 0.16 250);
  --color-brand-900: oklch(0.32 0.09 250);
  --font-sans: ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;

  /* Safe-area insets as spacing tokens -> pt-safe-top / pb-safe-bottom. */
  --spacing-safe-top: env(safe-area-inset-top, 0px);
  --spacing-safe-bottom: env(safe-area-inset-bottom, 0px);
  --spacing-safe-left: env(safe-area-inset-left, 0px);
  --spacing-safe-right: env(safe-area-inset-right, 0px);
}

@layer base {
  html {
    overscroll-behavior: none;
    -webkit-text-size-adjust: 100%;
  }
  body {
    min-height: 100dvh;
    -webkit-tap-highlight-color: transparent;
  }
  #root {
    min-height: 100dvh;
  }
}

/* Hover styles only where a real pointer exists (Tailwind's hover: sticks on touch). */
@custom-variant pointer-fine (@media (pointer: fine));
```

- [ ] **Step 4: Write the failing test**

`app/src/lib/time.test.ts`:
```ts
import { describe, expect, it } from 'vitest'
import { formatInZone, localDateKey, zoneOffsetLabel } from './time'

describe('time helpers', () => {
  const jakarta = 'Asia/Jakarta'

  it('renders an instant in the user timezone, not the machine timezone', () => {
    // 2026-10-11T00:00:00Z is 07:00 in Jakarta (UTC+7, no DST).
    const instant = new Date('2026-10-11T00:00:00.000Z')
    expect(formatInZone(instant, jakarta, { hour: '2-digit', minute: '2-digit', hour12: false })).toBe('07:00')
  })

  it('keys a day by the local date, so a late-night session stays on its day', () => {
    // 2026-10-11T17:30:00Z is 2026-10-12T00:30 in Jakarta.
    expect(localDateKey(new Date('2026-10-11T17:30:00.000Z'), jakarta)).toBe('2026-10-12')
    expect(localDateKey(new Date('2026-10-11T16:30:00.000Z'), jakarta)).toBe('2026-10-11')
  })

  it('labels the offset for the given instant', () => {
    expect(zoneOffsetLabel(new Date('2026-10-11T00:00:00.000Z'), jakarta)).toBe('UTC+7')
  })
})
```

- [ ] **Step 5: Run the test to confirm it fails**

Run: `cd app && npm install && npm test`
Expected: FAIL. `./time` cannot be resolved. (`npm install` may need `npm install --force` if a peer range conflicts; resolve by aligning versions rather than disabling peer checks.)

- [ ] **Step 6: Implement the helpers and the app shell**

`app/src/lib/time.ts`:
```ts
/**
 * Timezone helpers. All instants are UTC on the wire; every local-time
 * computation goes through the user's IANA zone (default Asia/Jakarta).
 */

export const DEFAULT_TIMEZONE = 'Asia/Jakarta'

export function formatInZone(
  instant: Date,
  timeZone: string,
  options: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' },
): string {
  return new Intl.DateTimeFormat('en-GB', { ...options, timeZone }).format(instant)
}

/** The calendar day the instant falls on, in the user's zone (YYYY-MM-DD). */
export function localDateKey(instant: Date, timeZone: string): string {
  // en-CA renders as YYYY-MM-DD.
  return new Intl.DateTimeFormat('en-CA', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    timeZone,
  }).format(instant)
}

/** e.g. "UTC+7" for Asia/Jakarta at the given instant. */
export function zoneOffsetLabel(instant: Date, timeZone: string): string {
  const part = new Intl.DateTimeFormat('en-GB', { timeZone, timeZoneName: 'longOffset' })
    .formatToParts(instant)
    .find((p) => p.type === 'timeZoneName')?.value

  // "GMT+07:00" -> "UTC+7"; "GMT" -> "UTC+0".
  if (!part) return 'UTC+0'
  const match = /GMT([+-])(\d{2}):(\d{2})/.exec(part)
  if (!match) return 'UTC+0'
  const [, sign, hours, minutes] = match
  const h = Number(hours)
  return minutes === '00' ? `UTC${sign}${h}` : `UTC${sign}${h}:${minutes}`
}
```

`app/src/main.tsx`:
```tsx
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider } from 'react-router'

import { router } from './router'
import './styles.css'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
      // The sync engine owns freshness; do not refetch on every focus change.
      refetchOnWindowFocus: false,
    },
  },
})

const container = document.getElementById('root')
if (!container) throw new Error('#root is missing from index.html')

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
```

`app/src/router.tsx`:
```tsx
import { createBrowserRouter } from 'react-router'

import { App } from './App'
import { Placeholder } from './components/Placeholder'

/** Phase 0 routes are placeholders; later phases replace each element. */
export const router = createBrowserRouter([
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <Placeholder title="Today" phase={9} /> },
      { path: 'workspaces', element: <Placeholder title="Workspaces" phase={1} /> },
      { path: 'calendar', element: <Placeholder title="Calendar" phase={10} /> },
      { path: 'tasks', element: <Placeholder title="Tasks" phase={9} /> },
      { path: 'settings', element: <Placeholder title="Settings" phase={9} /> },
      { path: 'billing', element: <Placeholder title="Plan and usage" phase={13} /> },
      { path: '*', element: <Placeholder title="Not found" phase={0} /> },
    ],
  },
])
```

`app/src/App.tsx`:
```tsx
import { Outlet } from 'react-router'

import { Shell } from './components/layout/Shell'

export function App() {
  return (
    <Shell>
      <Outlet />
    </Shell>
  )
}
```

`app/src/components/Placeholder.tsx`:
```tsx
export function Placeholder({ title, phase }: { title: string; phase: number }) {
  return (
    <section className="p-6">
      <h1 className="text-2xl font-semibold text-brand-900">{title}</h1>
      <p className="mt-2 text-sm text-gray-500">
        Implemented in Phase {phase}.
      </p>
    </section>
  )
}
```

`app/src/components/layout/Shell.tsx`:
```tsx
import type { ReactNode } from 'react'

import { platform } from '@/platform'
import { Sidebar } from './Sidebar'
import { TabBar } from './TabBar'

/**
 * Adaptive shell: sidebar on desktop/tablet, bottom tab bar on phones.
 * The platform adapter decides which; no Tauri import appears here.
 */
export function Shell({ children }: { children: ReactNode }) {
  const isPhone = platform.formFactor() === 'phone'

  return (
    <div className="flex min-h-[100dvh] bg-white text-gray-900 dark:bg-neutral-950 dark:text-neutral-100">
      {!isPhone && <Sidebar />}
      <main className="flex-1 pb-safe-bottom pt-safe-top">{children}</main>
      {isPhone && <TabBar />}
    </div>
  )
}
```

`app/src/components/layout/Sidebar.tsx` and `TabBar.tsx`:
```tsx
import { NavLink } from 'react-router'

const links = [
  { to: '/', label: 'Today' },
  { to: '/workspaces', label: 'Workspaces' },
  { to: '/calendar', label: 'Calendar' },
  { to: '/tasks', label: 'Tasks' },
  { to: '/settings', label: 'Settings' },
]

export function Sidebar() {
  return (
    <nav aria-label="Main" className="hidden w-56 shrink-0 border-r border-gray-200 p-4 sm:block dark:border-neutral-800">
      <ul className="space-y-1">
        {links.map((link) => (
          <li key={link.to}>
            <NavLink
              to={link.to}
              end={link.to === '/'}
              className={({ isActive }) =>
                `block rounded-md px-3 py-2 text-sm ${
                  isActive ? 'bg-brand-50 font-medium text-brand-900' : 'text-gray-600 hover:bg-gray-50'
                }`
              }
            >
              {link.label}
            </NavLink>
          </li>
        ))}
      </ul>
    </nav>
  )
}
```

```tsx
import { NavLink } from 'react-router'

const tabs = [
  { to: '/', label: 'Today' },
  { to: '/workspaces', label: 'Work' },
  { to: '/calendar', label: 'Calendar' },
  { to: '/tasks', label: 'Tasks' },
  { to: '/settings', label: 'More' },
]

/** Bottom navigation for phones. Targets are >=44px tall. */
export function TabBar() {
  return (
    <nav
      aria-label="Main"
      className="fixed inset-x-0 bottom-0 flex border-t border-gray-200 bg-white pb-safe-bottom sm:hidden dark:border-neutral-800 dark:bg-neutral-950"
    >
      {tabs.map((tab) => (
        <NavLink
          key={tab.to}
          to={tab.to}
          end={tab.to === '/'}
          className={({ isActive }) =>
            `flex min-h-11 flex-1 items-center justify-center text-xs ${
              isActive ? 'font-semibold text-brand-900' : 'text-gray-500'
            }`
          }
        >
          {tab.label}
        </NavLink>
      ))}
    </nav>
  )
}
```

- [ ] **Step 7: Run the tests and the build**

Run: `cd app && npm test && npm run build`
Expected: 3 tests PASS; `dist/` created. Then `npm run dev` and load `http://localhost:5173`. The shell renders with the sidebar and the "Today" placeholder.

- [ ] **Step 8: Commit**

```bash
git add app
git commit -m "feat(app): vite+react+tailwind SPA scaffold with adaptive shell and timezone helpers"
```

---

## Task 12: `platform` adapter (the only place Tauri APIs are touched)

**Files:**
- Create: `app/src/platform/types.ts`, `index.ts`, `web.ts`, `desktop.ts`, `mobile.ts`
- Test: `app/src/platform/platform.test.ts`

**Interfaces:**
- Consumes: `@tauri-apps/api` (only inside `desktop.ts`/`mobile.ts`), `@tauri-apps/plugin-os` for the platform name.
- Produces: a `Platform` interface with `formFactor()`, `kind()`, `secureStore`, `notifications`, `files`, `deepLinks`, `openExternal()`, `updater`, `haptics`; a singleton `platform`; `isTauri()`.

- [ ] **Step 1: Write the failing test**

`app/src/platform/platform.test.ts`:
```ts
import { describe, expect, it } from 'vitest'

import { detectRuntime, formFactorFor, isTauri } from './index'

describe('runtime detection', () => {
  it('detects the plain web build', () => {
    expect(detectRuntime({})).toBe('web')
    expect(detectRuntime({ __TAURI_INTERNALS__: {} })).toBe('tauri')
  })

  it('does not claim Tauri in the browser', () => {
    expect(isTauri()).toBe(false)
  })
})

describe('form factor', () => {
  it('maps platforms to a layout form factor', () => {
    expect(formFactorFor('ios')).toBe('phone')
    expect(formFactorFor('android')).toBe('phone')
    expect(formFactorFor('macos')).toBe('desktop')
    expect(formFactorFor('windows')).toBe('desktop')
    expect(formFactorFor('web')).toBe('web')
  })
})
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cd app && npm test -- platform`
Expected: FAIL. Cannot resolve `./index`.

- [ ] **Step 3: Implement the adapter**

`app/src/platform/types.ts`:
```ts
export type PlatformKind = 'web' | 'macos' | 'windows' | 'ios' | 'android'
export type FormFactor = 'web' | 'desktop' | 'tablet' | 'phone'
export type Runtime = 'web' | 'tauri'

export type SyncStatus = 'synced' | 'syncing' | 'offline' | 'error'

export interface SecureStore {
  /** Stores a secret in the OS keychain/keystore. Web throws. */
  set(key: string, value: string): Promise<void>
  get(key: string): Promise<string | null>
  delete(key: string): Promise<void>
}

export interface NotificationsApi {
  /** Requests permission and returns whether notifications may be shown. */
  requestPermission(): Promise<boolean>
  notify(title: string, body: string): Promise<void>
  /**
   * Schedules a local notification. Mobile honours `at`; desktop and web
   * resolve to `notify` immediately being unsupported, so callers must treat
   * scheduling as best-effort (see docs/ASSUMPTIONS.md A32).
   */
  schedule(title: string, body: string, at: Date): Promise<void>
}

export interface FilesApi {
  /** Opens a picker and returns local paths or null when cancelled. */
  pick(options?: { multiple?: boolean; mimeTypes?: string[] }): Promise<string[] | null>
  /** Reads a picked file as bytes. */
  read(path: string): Promise<Uint8Array>
}

export interface DeepLinksApi {
  /** Current URL at cold start, if any. */
  current(): Promise<string | null>
  onOpen(handler: (url: string) => void): Promise<() => void>
}

export interface Platform {
  kind(): PlatformKind
  runtime(): Runtime
  formFactor(): FormFactor
  secureStore: SecureStore
  notifications: NotificationsApi
  files: FilesApi
  deepLinks: DeepLinksApi
  openExternal(url: string): Promise<void>
  /** Haptics are a no-op off mobile. */
  haptic(kind: 'light' | 'medium' | 'success'): Promise<void>
  /** Desktop only; resolves false elsewhere. */
  checkForUpdate(): Promise<boolean>
}
```

`app/src/platform/index.ts`:
```ts
import type { FormFactor, Platform, PlatformKind, Runtime } from './types'
import { webPlatform } from './web'

export type { FormFactor, Platform, PlatformKind, Runtime, SyncStatus } from './types'

export function detectRuntime(scope: Record<string, unknown>): Runtime {
  return '__TAURI_INTERNALS__' in scope ? 'tauri' : 'web'
}

export function isTauri(): boolean {
  return detectRuntime(globalThis as unknown as Record<string, unknown>) === 'tauri'
}

export function formFactorFor(kind: PlatformKind): FormFactor {
  switch (kind) {
    case 'ios':
    case 'android':
      return 'phone'
    case 'macos':
    case 'windows':
      return 'desktop'
    case 'web':
      return 'web'
  }
}

/**
 * The active adapter. The web implementation is the default; the Tauri shell
 * installs its own via `installPlatform` during startup, so no module outside
 * `platform/` ever imports a Tauri package.
 */
let active: Platform = webPlatform()

export function installPlatform(next: Platform): void {
  active = next
}

export const platform: Platform = new Proxy({} as Platform, {
  get(_target, prop: string) {
    const value = active[prop as keyof Platform]
    return typeof value === 'function' ? value.bind(active) : value
  },
})
```

`app/src/platform/web.ts`:
```ts
import type {
  DeepLinksApi,
  FilesApi,
  NotificationsApi,
  Platform,
  PlatformKind,
  SecureStore,
} from './types'

const unsupported = (what: string): never => {
  throw new Error(`${what} is not available in the web build`)
}

const secureStore: SecureStore = {
  set: async () => unsupported('secure storage'),
  get: async () => null,
  delete: async () => unsupported('secure storage'),
}

const notifications: NotificationsApi = {
  async requestPermission() {
    if (typeof Notification === 'undefined') return false
    const result = await Notification.requestPermission()
    return result === 'granted'
  },
  async notify(title, body) {
    if (typeof Notification === 'undefined') return
    if (Notification.permission === 'granted') new Notification(title, { body })
  },
  // The web build cannot schedule; it fires immediately so the call is never lost.
  async schedule(title, body) {
    await notifications.notify(title, body)
  },
}

const files: FilesApi = {
  async pick() {
    return unsupported('file picking')
  },
  async read() {
    return unsupported('file reading')
  },
}

const deepLinks: DeepLinksApi = {
  async current() {
    return null
  },
  async onOpen() {
    return () => {}
  },
}

export function webPlatform(kind: PlatformKind = 'web'): Platform {
  return {
    kind: () => kind,
    runtime: () => 'web',
    formFactor: () => (kind === 'web' ? 'web' : 'phone'),
    secureStore,
    notifications,
    files,
    deepLinks,
    async openExternal(url) {
      window.open(url, '_blank', 'noopener,noreferrer')
    },
    async haptic() {},
    async checkForUpdate() {
      return false
    },
  }
}
```

`app/src/platform/desktop.ts`:
```ts
import { invoke } from '@tauri-apps/api/core'
import { platform as osPlatform } from '@tauri-apps/plugin-os'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'
import { open } from '@tauri-apps/plugin-dialog'
import { readFile } from '@tauri-apps/plugin-fs'
import { onOpenUrl } from '@tauri-apps/plugin-deep-link'
import { check } from '@tauri-apps/plugin-updater'

import { webPlatform } from './web'
import type { Platform, PlatformKind } from './types'

/** Tauri adapter for macOS and Windows. Secrets go through the Rust keychain. */
export function desktopPlatform(kind: PlatformKind): Platform {
  const base = webPlatform(kind)

  return {
    ...base,
    runtime: () => 'tauri',
    formFactor: () => 'desktop',

    secureStore: {
      set: (key, value) => invoke('secret_set', { key, value }),
      get: (key) => invoke<string | null>('secret_get', { key }),
      delete: (key) => invoke('secret_delete', { key }),
    },

    notifications: {
      async requestPermission() {
        if (await isPermissionGranted()) return true
        return (await requestPermission()) === 'granted'
      },
      async notify(title, body) {
        if (await isPermissionGranted()) sendNotification({ title, body })
      },
      // Desktop cannot schedule; deliver immediately (docs/ASSUMPTIONS.md A32).
      async schedule(title, body) {
        await this.notify(title, body)
      },
    },

    files: {
      async pick(options) {
        const selected = await open({
          multiple: options?.multiple ?? false,
          filters: options?.mimeTypes?.map((mime) => ({ name: mime, extensions: [] })),
        })
        if (selected === null) return null
        return Array.isArray(selected) ? selected : [selected]
      },
      read: (path) => readFile(path),
    },

    deepLinks: {
      current: async () => {
        const urls = await invoke<string[] | null>('plugin:deep-link|get_current')
        return urls?.[0] ?? null
      },
      onOpen: async (handler) => {
        const unlisten = await onOpenUrl((urls) => urls.forEach(handler))
        return unlisten
      },
    },

    openExternal: (url) => openUrl(url),

    async checkForUpdate() {
      const update = await check()
      return update !== null
    },
  }
}

/** Exposed so the shell can branch on the OS without importing plugin-os elsewhere. */
export function currentOsPlatform(): PlatformKind {
  const name = osPlatform()
  return (['macos', 'windows', 'ios', 'android'] as const).includes(name as 'macos')
    ? (name as PlatformKind)
    : 'web'
}
```

`app/src/platform/mobile.ts`:
```ts
import { invoke } from '@tauri-apps/api/core'
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'
import { openUrl } from '@tauri-apps/plugin-opener'
import { onOpenUrl } from '@tauri-apps/plugin-deep-link'
import { hapticImpact, hapticNotification } from '@tauri-apps/plugin-haptics'

import { desktopPlatform } from './desktop'
import type { Platform, PlatformKind } from './types'

/** Tauri adapter for iOS and Android. Scheduling works offline here. */
export function mobilePlatform(kind: PlatformKind): Platform {
  const base = desktopPlatform(kind)

  return {
    ...base,
    formFactor: () => 'phone',

    notifications: {
      async requestPermission() {
        if (await isPermissionGranted()) return true
        return (await requestPermission()) === 'granted'
      },
      async notify(title, body) {
        if (await isPermissionGranted()) sendNotification({ title, body })
      },
      async schedule(title, body, at) {
        if (!(await isPermissionGranted())) return
        // Local scheduling survives app restarts and works offline.
        await invoke('plugin:notification|notify', {
          options: {
            title,
            body,
            schedule: { at: at.toISOString(), allowWhileIdle: true },
          },
        })
      },
    },

    deepLinks: {
      current: async () => {
        const urls = await invoke<string[] | null>('plugin:deep-link|get_current')
        return urls?.[0] ?? null
      },
      onOpen: async (handler) => {
        const unlisten = await onOpenUrl((urls) => urls.forEach(handler))
        return unlisten
      },
    },

    openExternal: (url) => openUrl(url),

    async haptic(kind) {
      if (kind === 'success') await hapticNotification({ type: 'Success' })
      else await hapticImpact({ style: kind === 'light' ? 'Light' : 'Medium' })
    },

    async checkForUpdate() {
      // Mobile updates ship through TestFlight / Play.
      return false
    },
  }
}
```

Add `@tauri-apps/plugin-haptics` `2.4.2` to `package.json` dependencies.

- [ ] **Step 4: Enforce the adapter boundary**

Add `app/src/platform/boundary.test.ts`:
```ts
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const SRC = join(import.meta.dirname, '..')

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

describe('platform boundary', () => {
  it('only the platform adapter imports @tauri-apps packages', () => {
    const offenders = walk(SRC)
      .filter((path) => /\.[tj]sx?$/.test(path))
      .filter((path) => !path.includes('/platform/'))
      .filter((path) => !path.endsWith('.test.ts') && !path.endsWith('.test.tsx'))
      .filter((path) => /from\s+['"]@tauri-apps\//.test(readFileSync(path, 'utf8')))

    expect(offenders).toEqual([])
  })
})
```

- [ ] **Step 5: Run the tests**

Run: `cd app && npm test`
Expected: PASS (5 tests: 3 time + 2 platform + 1 boundary).

- [ ] **Step 6: Commit**

```bash
git add app
git commit -m "feat(app): platform adapter with enforced Tauri import boundary"
```

---

## Task 13: Tauri 2 shell (macOS + Windows build, keychain, commands)

**Files:**
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/keychain.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/capabilities/default.json`, `src-tauri/capabilities/desktop.json`, `src-tauri/icons/` (generated)
- Modify: `app/src/main.tsx` (install the platform adapter at startup)

**Interfaces:**
- Consumes: `app/dist` as `frontendDist`; the Task 12 `installPlatform`.
- Produces: `tauri dev`/`tauri build` for macOS and Windows; IPC commands `health_ping`, `secret_set`, `secret_get`, `secret_delete`.

- [ ] **Step 1: Scaffold the shell and inspect the generated config**

Run:
```bash
cd app && npm run tauri init -- \
  --app-name Stvdy \
  --window-title Stvdy \
  --frontend-dist ../app/dist \
  --dev-url http://localhost:5173 \
  --before-dev-command "npm run dev" \
  --before-build-command "npm run build"
cd ..
```
Expected: `src-tauri/` created. Because `frontendDist` is `../app/dist`, Tauri runs the before-commands with cwd `<root>/app`, which is where `package.json` lives (verified in the CLI source; see docs/ASSUMPTIONS.md A36).

- [ ] **Step 2: Write `src-tauri/tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Stvdy",
  "version": "0.1.0",
  "identifier": "app.stvdy.desktop",
  "build": {
    "beforeDevCommand": "npm run dev",
    "beforeBuildCommand": "npm run build",
    "devUrl": "http://localhost:5173",
    "frontendDist": "../app/dist"
  },
  "app": {
    "windows": [
      {
        "title": "Stvdy",
        "width": 1280,
        "height": 840,
        "minWidth": 960,
        "minHeight": 600,
        "resizable": true
      }
    ],
    "security": {
      "csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' asset: http://asset.localhost data:; font-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost http://localhost:8080 https://api.stvdy.app; frame-ancestors 'none'; object-src 'none'"
    }
  },
  "bundle": {
    "active": true,
    "targets": ["app", "dmg", "nsis"],
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "createUpdaterArtifacts": true,
    "macOS": {
      "minimumSystemVersion": "11.0"
    },
    "windows": {
      "nsis": { "installMode": "perMachine" }
    }
  },
  "plugins": {
    "deep-link": {
      "desktop": { "schemes": ["stvdy"] }
    }
  }
}
```

`connect-src` must list the API origin, or `fetch` from the webview fails; `ipc:` and `http://ipc.localhost` are required by Tauri's own IPC.

- [ ] **Step 3: Write `src-tauri/Cargo.toml` and `build.rs`**

`src-tauri/build.rs`:
```rust
fn main() {
    tauri_build::build()
}
```

`src-tauri/Cargo.toml`:
```toml
[package]
name = "stvdy-shell"
version = "0.1.0"
edition = "2024"
rust-version = "1.94.1"
publish = false

[lib]
name = "stvdy_shell_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2.7.2", features = [] }

[dependencies]
tauri = { version = "2.12.3", features = ["protocol-asset"] }
tauri-plugin-deep-link = "2.6.1"
tauri-plugin-notification = "2.6.0"
tauri-plugin-dialog = "2.8.2"
tauri-plugin-fs = "2.7.0"
tauri-plugin-opener = "2.7.1"
tauri-plugin-os = "2.4.0"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.152"
thiserror = "2.0.21"
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter"] }

# Secure storage: keyring-core plus the per-platform store. The `keyring` crate
# defaults are desktop-only, so each target wires its own store (A30).
keyring-core = "1.0.0"

# macOS uses the legacy keychain (works unsigned); iOS has only the protected
# data store. The crate exposes them as separate feature-gated modules.
[target.'cfg(target_os = "macos")'.dependencies]
apple-native-keyring-store = { version = "1.0.2", features = ["keychain"] }

[target.'cfg(target_os = "ios")'.dependencies]
apple-native-keyring-store = { version = "1.0.2", features = ["protected"] }

[target.'cfg(target_os = "windows")'.dependencies]
windows-native-keyring-store = "1.1.0"

[target.'cfg(target_os = "android")'.dependencies]
android-native-keyring-store = "1.0.0"

# Desktop-only plugins must not be linked into mobile builds.
[target.'cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))'.dependencies]
tauri-plugin-updater = "2.13.3"
tauri-plugin-global-shortcut = "2.4.1"
tauri-plugin-single-instance = "2.4.0"

[target.'cfg(any(target_os = "ios", target_os = "android"))'.dependencies]
tauri-plugin-haptics = "2.4.2"

[features]
default = ["custom-protocol"]
custom-protocol = ["tauri/custom-protocol"]

[profile.release]
codegen-units = 1
lto = true
opt-level = "s"
panic = "abort"
strip = true
```

- [ ] **Step 4: Write the shell source**

`src-tauri/src/keychain.rs`:
```rust
//! Secure token storage. Only the refresh token is stored here; the access
//! token lives in memory for the process lifetime.
//!
//! `keyring-core` keeps a process-wide default store; each platform installs
//! its own before the first `Entry` is created. Verified against
//! keyring-core 1.0.0 (`set_default_store(Arc<CredentialStore>)`,
//! `Entry::{new,set_password,get_password,delete_credential}`,
//! `Error::{NoEntry,NoDefaultStore}`) and apple-native-keyring-store 1.0.2
//! (`Store::new() -> Result<Arc<Self>>`, features `keychain`/`protected`).

use std::sync::Arc;

use keyring_core::{CredentialStore, Entry, Error as KeyringError};

pub const SERVICE: &str = "app.stvdy";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("no secure store is available on this platform")]
    NoStore,
    #[error("secure storage failed: {0}")]
    Backend(String),
}

/// Installs the platform store exactly once, before any Entry is created.
pub fn init() -> Result<(), SecretError> {
    #[cfg(target_os = "macos")]
    {
        // The legacy keychain works for unsigned dev builds; the protected
        // data store requires a provisioning profile.
        let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(target_os = "ios")]
    {
        // iOS has only the protected data store.
        let store: Arc<CredentialStore> = apple_native_keyring_store::protected::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(target_os = "android")]
    {
        // No shim needed: android-activity (via tao, which Tauri uses) calls
        // ndk_context::initialize_android_context before main, and this store's
        // own README lists Tauri Mobile as a framework that already provides
        // that initialization. Calling it ourselves would be a SECOND call,
        // and ndk-context asserts the call happens exactly once
        // (`assert!(previous.is_none())`). I.e. it would panic at startup.
        // If a device prototype ever shows Store::new() returning Err here,
        // that is the trigger to revisit (see docs/ASSUMPTIONS.md A30).
        let store: Arc<CredentialStore> = android_native_keyring_store::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(not(any(
        target_os = "macos", target_os = "ios", target_os = "windows", target_os = "android"
    )))]
    {
        Err(SecretError::NoStore)
    }
}

fn entry(key: &str) -> Result<Entry, SecretError> {
    Entry::new(SERVICE, key).map_err(|e| match e {
        KeyringError::NoDefaultStore => SecretError::NoStore,
        other => SecretError::Backend(other.to_string()),
    })
}

pub fn set(key: &str, value: &str) -> Result<(), SecretError> {
    entry(key)?
        .set_password(value)
        .map_err(|e| SecretError::Backend(e.to_string()))
}

pub fn get(key: &str) -> Result<Option<String>, SecretError> {
    match entry(key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(other) => Err(SecretError::Backend(other.to_string())),
    }
}

pub fn delete(key: &str) -> Result<(), SecretError> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(other) => Err(SecretError::Backend(other.to_string())),
    }
}
```

`src-tauri/src/commands/mod.rs`:
```rust
use crate::keychain;

/// Round-trips through the IPC boundary so the frontend can prove the shell is
/// alive (used by the Phase 1 native prototype checklist).
#[tauri::command]
pub fn health_ping() -> String {
    format!("stvdy-shell {}", env!("CARGO_PKG_VERSION"))
}

#[tauri::command]
pub fn secret_set(key: String, value: String) -> Result<(), String> {
    keychain::set(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn secret_get(key: String) -> Result<Option<String>, String> {
    keychain::get(&key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn secret_delete(key: String) -> Result<(), String> {
    keychain::delete(&key).map_err(|e| e.to_string())
}
```

`src-tauri/src/lib.rs`:
```rust
pub mod commands;
pub mod keychain;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Err(error) = keychain::init() {
        tracing::warn!(%error, "secure storage unavailable; tokens will not persist");
    }

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .invoke_handler(tauri::generate_handler![
            commands::health_ping,
            commands::secret_set,
            commands::secret_get,
            commands::secret_delete,
        ]);

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_focus();
                }
            }))
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_global_shortcut::Builder::new().build());
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        builder = builder.plugin(tauri_plugin_haptics::init());
    }

    builder
        .setup(|app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                app.deep_link().register_all()?;
            }
            let _ = app.handle();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Stvdy shell");
}
```

`src-tauri/src/main.rs`:
```rust
// Prevent a console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    stvdy_shell_lib::run();
}
```

- [ ] **Step 5: Write the capability files**

`src-tauri/capabilities/default.json`:
```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Baseline capabilities available on every platform.",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "os:default",
    "opener:default",
    "notification:default",
    "dialog:default",
    "fs:default",
    "deep-link:default"
  ]
}
```

`src-tauri/capabilities/desktop.json`:
```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "desktop-only",
  "description": "Updater and global shortcut exist on desktop only.",
  "windows": ["main"],
  "platforms": ["macOS", "windows", "linux"],
  "permissions": [
    "updater:default",
    "global-shortcut:default"
  ]
}
```

`src-tauri/capabilities/mobile.json`:
```json
{
  "$schema": "../gen/schemas/mobile-schema.json",
  "identifier": "mobile-only",
  "description": "Haptics are mobile only.",
  "windows": ["main"],
  "platforms": ["iOS", "android"],
  "permissions": ["haptics:default"]
}
```

- [ ] **Step 6: Install the adapter at startup**

Replace the render block in `app/src/main.tsx` so the Tauri adapter is chosen before the first render:
```tsx
import { installPlatform, isTauri } from './platform'
import { currentOsPlatform, desktopPlatform } from './platform/desktop'
import { mobilePlatform } from './platform/mobile'

async function bootstrap() {
  if (isTauri()) {
    const kind = currentOsPlatform()
    installPlatform(kind === 'ios' || kind === 'android' ? mobilePlatform(kind) : desktopPlatform(kind))
  }
  // ...existing createRoot(...).render(...) call moved here
}

void bootstrap()
```

- [ ] **Step 7: Build and smoke-test the desktop app**

Run:
```bash
cd app && npm run tauri build -- --no-bundle
```
Expected: a release binary under `src-tauri/target/release/`. Then:
```bash
npm run tauri dev
```
Expected: the window opens showing the shell; in the devtools console `await window.__TAURI_INTERNALS__.invoke('health_ping')` returns `stvdy-shell 0.1.0`, and `secret_set`/`secret_get` round-trip a value (on macOS this may prompt for keychain access the first time. That is expected and is the Phase 1 prototype data point).

- [ ] **Step 8: Commit**

```bash
git add src-tauri app/src/main.tsx app/package.json app/package-lock.json
git commit -m "feat(shell): tauri 2 desktop shell with keychain, plugins and capabilities"
```

---

## Task 14: CI workflow (fmt, clippy, test, web build, desktop bundle matrix)

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `.sqlx/` (generated, committed)

**Interfaces:**
- Consumes: `justfile` targets, `app/package.json` scripts, `src-tauri`.
- Produces: a green CI badge; `.sqlx` so lint jobs compile without a database.

- [ ] **Step 1: Generate and commit the offline query cache**

Run:
```bash
docker compose up -d postgres
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/stvdy
sqlx migrate run
cargo sqlx prepare --workspace
git add .sqlx
```
Expected: `.sqlx/` contains one JSON file per compile-time-checked query. Phase 0 has none yet, so the directory may be created empty. That is fine and later phases add entries. `cargo sqlx prepare --check --workspace` must exit 0.

- [ ] **Step 2: Write the workflow**

`.github/workflows/ci.yml`:
```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  lint:
    name: fmt + clippy
    runs-on: ubuntu-24.04
    env:
      # No database in this job: compile against the committed .sqlx cache.
      SQLX_OFFLINE: "true"
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@1.94.1
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    name: cargo test
    runs-on: ubuntu-24.04
    services:
      postgres:
        image: pgvector/pgvector:pg16
        env:
          POSTGRES_USER: postgres
          POSTGRES_PASSWORD: postgres
          POSTGRES_DB: stvdy
        ports: ["5432:5432"]
        options: >-
          --health-cmd "pg_isready -U postgres"
          --health-interval 5s
          --health-timeout 3s
          --health-retries 20
    env:
      DATABASE_URL: postgres://postgres:postgres@localhost:5432/stvdy
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@1.94.1
      - uses: Swatinem/rust-cache@v2
      - name: Install sqlx-cli
        run: cargo install sqlx-cli --no-default-features --features postgres,rustls-ring
      - run: sqlx migrate run
      - run: cargo sqlx prepare --workspace --check
      - run: cargo test --workspace

  web:
    name: web build
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-node@v5
        with:
          node-version: 24
          cache: npm
          cache-dependency-path: app/package-lock.json
      - run: npm ci --prefix app
      - run: npm --prefix app test
      - run: npm --prefix app run build

  desktop:
    name: desktop bundle (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: macos-15
            args: --target universal-apple-darwin --no-bundle
          - os: windows-2025
            args: --no-bundle
    env:
      SQLX_OFFLINE: "true"
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@1.94.1
        with:
          targets: ${{ matrix.os == 'macos-15' && 'aarch64-apple-darwin,x86_64-apple-darwin' || '' }}
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri
      - uses: actions/setup-node@v5
        with:
          node-version: 24
          cache: npm
          cache-dependency-path: app/package-lock.json
      - run: npm ci --prefix app
      - name: Build shell
        run: npm --prefix app run tauri build -- ${{ matrix.args }}
```

- [ ] **Step 3: Verify the workflow locally as far as possible**

Run:
```bash
cargo fmt --all -- --check
SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix app test && npm --prefix app run build
```
Expected: all clean. The `SQLX_OFFLINE=true` clippy run proves the lint job will pass without a database.

- [ ] **Step 4: Commit and push to see CI run**

```bash
git add .github .sqlx
git commit -m "ci: fmt, clippy, test, web build and desktop bundle matrix"
git push -u origin main
```
Expected: all four jobs green. The `desktop` job is the phase's macOS/Windows proof.

---

## Task 15: Documentation stubs

**Files:**
- Create: `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/SYNC.md`, `docs/RELEASE.md`
- Verify in place: `docs/THREAT_MODEL.md`. Already written; this task only confirms it is present and linked from `docs/ARCHITECTURE.md` and `README.md`, so it cannot drift out of the doc set
- Modify: `README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the documents later phases append to; each has the section headings the phase table promises.

- [ ] **Step 1: Write the four stubs**

Each file gets its headings plus an explicit "filled by phase N" marker, so a later phase cannot silently skip documentation:

`docs/ARCHITECTURE.md`: `## Layers`, `## Provider traits`, `## Config`, `## Trust boundary` (one paragraph + a link to `docs/THREAT_MODEL.md`, which is the normative statement), `## Native capability matrix` (link `docs/research/2026-10-11-tauri-2-capability-matrix.md`), `## Native prototypes` (Phase 1: keychain, deep link; Phase 14: push, share target, background sync), `## Local cache` (SQLCipher flag, default off).

`docs/API.md`: `## Conventions` (auth header, error shape, pagination, SSE event names), `## Auth`, `## Devices`, `## Sync`, `## Workspaces`, `## Sources`, `## Chat`, `## Flashcards`, `## Quiz`, `## Reports`, `## Tasks`, `## Events`, `## Goals and planner`, `## Syllabus`, `## Calendar`, `## Actions`, `## Study`, `## Settings`, `## Account`, `## Billing`, `## Admin`. Each with a "Phase N" note.

`docs/SYNC.md`: `## Cursor model`, `## Pull`, `## Push and idempotency`, `## Realtime hint`, `## Conflict rules` (copy the normative table from the roadmap), `## Offline scope`, `## Compaction and resync`, `## Local cache contents`.

`docs/RELEASE.md`: `## Desktop distribution`, `## Mobile distribution`, `## Store policy risk` (QRIS-only mobile selling; note the risk that stores may require their own billing for digital subscriptions), `## Privacy labels`, `## Account deletion`, `## Tax and PPN` (unconfirmed; see ASSUMPTIONS B2), `## Versioning`, and the operations sections a *public* launch requires. These are not distribution concerns and must not be skipped, because the app is unreleased today but a paying user is a promise to keep running:

- `## Hosting` (Phase 15). The provisioned Oracle Cloud VM (`ubuntu@168.110.202.106`, Ubuntu 24.04, 2 vCPU / 11 GB / 45 GB, deployment root `~/stvdy/`), one compose stack, and the sizing note that Chromium (Phase 6) was what set the original 4 GB floor (moot on this box, but the moli-vs-Chromium decision stands on its own merits). **Must include the two-layer firewall fact, because it is the first thing that will look like a broken deploy:** Oracle's VCN security list currently admits only port 22, and the host's own `iptables` INPUT chain (`/etc/iptables/rules.v4`, netfilter-persistent) also accepts only `state NEW tcp dpt:22` and REJECTs everything else. **Both layers need an explicit rule for 80/443**. Fixing only one leaves the service unreachable from outside, which is indistinguishable from a crashed app. Also: the instance region supersedes the earlier "ap-southeast" preference, so the UU PDP data-residency line must be updated to name where the data actually lives.
- `## Backups and restore` (Phase 15). What is backed up (Postgres dump + object storage), frequency, retention, **where the restore is tested**, and the restore-drill cadence. An untested backup is not a backup.
- `## Monitoring and alerting` (Phase 15). The signals that mean the service is broken: job worker stalled (oldest `queued` row age), webhook failures, `unresponsive_engines` on every search (A3 degradation), provider-cost ceiling hits, disk and connection saturation, and the alert route for each.
- `## Incident response` (Phase 15). The UU PDP 72-hour breach clock: how a breach is detected, who decides it is reportable, what is logged for the report, and the fact that notification goes to both data subjects **and** the authority. Also the on-call reality: one operator, so the runbook must be executable by a tired person at 03:00.
- `## Abuse control` (Phase 7). The A43 controls as deployed values: signup rate limits (per IP, per domain), the per-account global cap, the verification-before-spending rule, and the manual unblock path for R5 false positives.
- `## Data retention` (Phase 15). Per-category retention (change_log 90 days per A19; uploads and chunks for the life of the workspace; `usage_events` and billing rows as required for margin and tax; account deletion semantics and the scheduled hard delete). UU PDP requires a stated schedule, not just a delete button.
- `## Data processing agreements` (Phase 15). The vendor list needing a DPA before public launch: LLM provider, Exa/Tavily (A3), VPS host, object storage, SMTP, and the payment gateway (B1). UU PDP explicitly requires formalized vendor contracts.

- [ ] **Step 2: Write `README.md`**

Include: what the project is (one paragraph), the architecture in five bullets, the trust boundary in two sentences (server is the only enforcement point; see `docs/THREAT_MODEL.md`), prerequisites (Rust 1.94.1, Node 24, Docker, Xcode for iOS, Android Studio + NDK for Android), the quick start (`docker compose up -d`, `cp .env.example .env`, `sqlx migrate run`, `cargo run -p stvdy-server`, `npm --prefix app install`, `npm --prefix app run dev`, `npm --prefix app run tauri dev`), the `justfile` targets, the CI gates, and pointers to `docs/SPEC.md`, `docs/ASSUMPTIONS.md`, `docs/THREAT_MODEL.md` and the plan files.

- [ ] **Step 3: Verify the links resolve**

Run: `ls docs/ docs/research/ docs/superpowers/plans/ config/ migrations/`
Expected: every file referenced from `README.md` exists.

- [ ] **Step 4: Commit**

```bash
git add README.md docs
git commit -m "docs: architecture, API, sync and release stubs plus README quick start"
```

---

## Phase 0 Exit Checklist

All must be true before the phase is called done:

- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean, **and** the `unwrap_used`/`expect_used` lint demonstrably fires (Task 1 Step 5).
- [ ] `cargo test --workspace` green: plans matrix (8), billing policy (3), schema (9), config (2), health (3), sync wire (7), plus app tests (6).
- [ ] `npm --prefix app test` and `npm --prefix app run build` green.
- [ ] `docker compose up -d` then `sqlx migrate run` applies `0001`–`0017` with no error; **34** sync triggers exist (17 tables × 2. 16 `log_change` tables plus `user_settings` via `log_change_by_user`), and no non-sync table has one. The Task 6 Step 4 column query returns zero rows.
- [ ] `docker compose up -d` reports `searxng` **healthy**, and `just db-search` prints a JSON object whose `results` array is non-empty (not the `Verifying your browser…` HTML page). On the **production host** it must also have an empty `unresponsive_engines`. Verify the engine count too, since the tuned list depends on two upstream-disabled engines being re-enabled: `curl -s localhost:8088/config | jq '[.engines[] | select(.enabled and (.categories | index("general")))] | length'` should report **≥ 3 answering engines** on the production host (`google cse`, `mojeek`, `mwmbl`). See A3 for the measured baseline. If only `google cse` answers, check first that `brave`, `duckduckgo`, `qwant` and `startpage` are absent from `unresponsive_engines`: if they appear, the tuned list is not in effect. If they are absent, the config is correct and the shortfall is the dev laptop's own network (it blocks `mojeek`), which understates this tier by ~3.6× and must not be tuned around.
- [ ] `cargo run -p stvdy-server` + `curl localhost:8080/health` returns `{"status":"ok",...,"plans":3}`; `/nope` returns 404 with `{"code":"not_found"}`.
- [ ] `npm --prefix app run tauri build -- --no-bundle` succeeds on macOS; the Windows job in CI is green.
- [ ] `cargo sqlx prepare --workspace --check` exits 0 with `SQLX_OFFLINE=true`.
- [ ] `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/SYNC.md`, `docs/RELEASE.md` exist with their headings and phase markers; `docs/THREAT_MODEL.md` exists and is linked from `docs/ARCHITECTURE.md` § Trust boundary and from `README.md`.
- [ ] The SPA shell passes the accessibility gate (roadmap § Verification Gates 5) on the screens it has: keyboard-only navigation reaches every control with a visible focus ring, the default theme meets contrast, and no interactive element is below the 44px touch target on a phone-width viewport.
- [ ] Phase summary posted; wait for "continue".

## Self-Review

**Spec coverage for Phase 0's scope**. The spec's Phase 0 line asks for: Cargo workspace ✓ (Task 1), React+Vite app ✓ (Task 11), Tauri 2 shell building for macOS and Windows ✓ (Task 13 + CI matrix in Task 14), docker-compose with Postgres+pgvector and MinIO ✓ (Task 8), migrations ✓ (Tasks 4–6), plans.toml + billing_policy.toml loaders ✓ (Tasks 2–3), health endpoint ✓ (Task 9), CI with fmt/clippy/test and the web + macOS + Windows build matrix ✓ (Task 14), `platform` adapter skeleton ✓ (Task 12). The full schema is created in Phase 0 rather than Phase 1 because the spec's sync columns and triggers must exist "from the start" and retrofitting triggers onto live tables is the expensive mistake this ordering avoids.

**Placeholders**. None in the sense of unfinished work. Two honest qualifications: (1) migration bodies are authored from `docs/SPEC.md` § DATABASE SCHEMA, which is a *sketch*. Its own heading says "create as migrations; add sensible indexes and FKs", it states the sync columns as a rule rather than writing them into each table body, and it describes the trigger behaviour in prose. Tasks 4–6 therefore carry the sync-column rule, the index/FK deltas and a verification query, and the exit checklist asserts the resulting counts (34 triggers, three sync columns on every `+sync` table) rather than trusting the copy. This is deliberate: one authoritative sketch plus explicit deltas is stronger than duplicating 400 lines that would drift, but it is not "copy the file". (2) `docs/THREAT_MODEL.md` is written and normative; `docs/ARCHITECTURE.md`, `API.md`, `SYNC.md` and `RELEASE.md` are created as stubs in Task 15 with the headings and phase markers later phases must fill.

**Type consistency**. `PlanId`, `PlanConfig`, `BillingPolicy`, `Platform`, `AppConfig`, `AppState`, `AppError`, `Entity`, `Op`, `Change`, `Mutation`, `MutationResult`, `PullResponse`, `PushRequest/Response`, `Platform`/`PlatformKind`/`FormFactor` are each defined exactly once and referenced by name thereafter. `Entity::ALL` has 17 variants and `dependency_rank()` returns a dense 1..=17, asserted by the wire test. The schema test's `SYNC_TABLES` list (16 tables) plus `user_settings` matches `0017_sync_triggers.sql` exactly, and `tables_added_beyond_the_spec_are_deliberate` carries `report_revisions` + `cheatsheets` as the documented deviations.

**Known risks carried into execution**. (1) exact crate versions are pinned to values verified on 2026-10-11; if one is unavailable on the target platform the fix is to re-verify that pin, never to loosen it to `*`; (2) `keyring-core` + per-platform store wiring is the Phase 1 prototype precisely because the research flagged iOS/Android cross-compilation as unverified. The API surface itself is now verified from crate sources, and the Android `ndk-context` initialization is settled (android-activity provides it; do **not** add a shim, which would be a second call and panic); (3) `#[sqlx::test]` needs a superuser connection to `create extension vector`. The CI service and the compose file both use `postgres`, which is superuser; (4) the `desktop` CI job builds `--no-bundle` so unsigned artifacts are produced without certificates, which arrive in Phase 15; (5) `npm run tauri init` output is a starting point, not gospel. Task 13 Step 2 replaces `tauri.conf.json` wholesale, so review the generated file rather than trusting it.

## Execution notes (Phase 0, as built)

Deviations and discoveries from actually running the plan. The plan text above is left as written so the next phase can see what changed and why.

1. **`sqlx-cli` feature name.** The plan said `--features postgres,rustls-ring`; the real feature is `rustls`. Also `cargo install --locked sqlx-cli` no longer works in 0.9.
2. **`src-tauri` placement.** `tauri init` creates `app/src-tauri`, but the CLI resolves `src-tauri` only as a *subfolder of the working directory*. It does not search upward. The shell lives at the repo root and `justfile` invokes `./app/node_modules/.bin/tauri` from there, which is what makes `frontendDist: "../app/dist"` and the `beforeBuildCommand` cwd (A36) correct.
3. **`protocol-asset` needs config.** Enabling the feature without `app.security.assetProtocol` fails the build ("features do not match the allowlist").
4. **The updater cannot ship in Phase 0.** It panics at startup without a `plugins.updater` config, and it needs a signing keypair plus a release endpoint. Registering it with placeholder config would be a fake capability, so it moved to Phase 15. Along with `createUpdaterArtifacts: true`, which needs `TAURI_SIGNING_PRIVATE_KEY` at build time.
5. **`plans` must be seeded by the migration, not only at boot.** `users.plan_id` is `not null default 'free'` and references `plans`, so the default must be satisfiable by the schema alone; otherwise every `insert into users` fails with `users_plan_id_fkey` (10 tests caught this).
6. **Cascade deletes must not be logged.** Deleting a user cascades to its rows, whose AFTER DELETE triggers then tried to insert a `change_log` row for a user that no longer exists. FK violation, making account deletion impossible. `log_change()` now skips the log when the owner is gone; the rows cascade away anyway, so those tombstones could never be pulled.
7. **`PlansFile.plans` is `#[serde(default)]`** so a config with no plan tables reports "must define a [free] plan" instead of serde's "missing field `plans`".
8. **TypeScript 7 removed `baseUrl`.** `paths` now resolves relative to `tsconfig.json`.
9. **Haptics exports are `impactFeedback`/`notificationFeedback`**, not `hapticImpact`/`hapticNotification`. Caught by `tsc`.
10. **`i64::div_ceil` is unstable**; the long-document multiplier uses manual ceiling division.
11. **Tests must not mutate the process environment** (it is global and racy), so `AppConfig::from_env_at` takes explicit overrides.
12. **Plugin config in `tauri.conf.json` is validated only by the Tauri CLI, and only when the plugin is rebuilt.** The CLI sets `TAURI_{PLUGIN}_PLUGIN_CONFIG` for every plugin in the config with no target gating, and each plugin's `build.rs` calls `plugin_config::<Config>("...")`, which panics on a shape mismatch. Two consequences:
    - `cargo build`/`cargo clippy` can never catch it. They do not set the variable.
    - **Changing `tauri.conf.json` does not re-run the plugin's `build.rs`.** Cargo re-runs the *shell's* build script (it declares `rerun-if-changed` on the config), but nothing tells cargo the plugin's script depends on our config, so a wrong shape can sit undetected until a fresh checkout or `cargo clean -p <plugin>`.

    Verified the hard way: `"mobile": [{ "scheme": "stvdy" }]` built cleanly on an incremental `tauri build`, then panicked with `failed to parse configuration: Error("invalid type: string \"stvdy\", expected a sequence")` once `cargo clean -p tauri-plugin-deep-link` forced the script to re-run. `AssociatedDomain.scheme` is `Vec<String>`.
    **Rule for later phases:** after editing any `plugins.*` block, verify with `cargo clean -p <the-plugin>` followed by `./app/node_modules/.bin/tauri build --debug --no-bundle`. An incremental build proves nothing.

### Phase 0 verification (as executed)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean; `unwrap_used` proven to fire on a deliberate probe, and proven relaxed inside tests |
| `cargo test --workspace` | **62 passing**, 0 failing |
| `npm --prefix app test` | **11 passing** |
| `npm --prefix app run build` | clean (348 kB / 110 kB gzip) |
| `src-tauri` build + `cargo test` | builds; keychain round-trip **PASS** (real macOS keychain) |
| Shell launch | `tauri build --debug --no-bundle` → runs and holds a window |
| Fresh DB `sqlx migrate run` | 17 migrations, **34 triggers**, `insert into users` works with no app code |
| Server smoke | `/health` → `{env:dev,plans:3,status:ok,version:0.1.0}`; `/nope` → typed 404; `x-request-id` propagated |
| Web render | verified in headless Chrome: shell renders, deep route returns 200 |
