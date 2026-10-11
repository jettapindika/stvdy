# Study OS MVP — Master Implementation Roadmap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> This file is the **index and contract document**. Each phase gets its own task-level plan at `docs/superpowers/plans/YYYY-MM-DD-phase-N-<slug>.md`, written immediately before that phase starts, using the contracts fixed here. Phase 0's plan already exists: `docs/superpowers/plans/2026-10-11-phase-0-scaffold.md`.

**Goal:** Ship an AI study OS — source-grounded research workspaces (NotebookLM-style) plus a study planner (tasks, calendar, exam-aware planning, confirm-gated assistant), sold as 3 prepaid QRIS plans, on macOS, Windows, iOS, Android and web, with one account syncing everything.

**Architecture:** Rust/Axum + Postgres (pgvector + tsvector) is the single source of truth and does all heavy work. One React SPA is packaged by Tauri 2 for the four native platforms and deployed as the web build. Native clients keep a SQLite cache + outbox and sync through a cursor-based change log with idempotent mutation replay. All external capabilities (LLM, embeddings, search, payments, push, calendar) sit behind traits so providers are config, not code.

**Tech Stack:** Rust stable (Axum, Tokio, sqlx/Postgres, serde, reqwest, tracing, tower-http, argon2, JWT + rotating refresh), Postgres 16 + pgvector + MinIO, React + Vite + TypeScript + Tailwind, TanStack Query, React Router, Tauri 2 (deep-link, notification, updater, dialog/fs, opener, os, global-shortcut, keychain), `rusqlite` local cache, GitHub Actions CI.

---

## Global Constraints

Every task in every phase implicitly includes these. Values are copied from the spec; do not re-derive or "improve" them.

- **Server is the only source of truth.** Clients never call LLM, search, or Google APIs directly. Only BYOK keys travel client→server, per request, never stored unencrypted on device.
- **All heavy work is server-side:** ingestion, embeddings, retrieval, LLM, planner, Google Calendar sync, billing. Ingestion and generation are async with visible progress.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts, study sessions, settings. Everything else requires connectivity and queues nothing.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`). Local-date math only through `chrono-tz`.
- **Prices (IDR):** free `0`; pro `10000`/mo, `100000`/yr; power `25000`/mo, `250000`/yr. Prepaid periods only: 30 days or 365 days. No auto-renew, no stored payment methods.
- **Plan limits:** free `workspaces=2, sources_total=20, max_file_mb=20, storage_mb=200, devices_max=2, credits_per_month=60, chat_msgs_per_day=20, web_searches_per_day=3, ai_flashcards_per_month=30, quizzes_per_month=2, reports_per_month=1, export=[md], planner=manual, no google_calendar, no agent_actions`; pro `unlimited, 150, 50, 2000, 5, 500, unlimited chat, 30, unlimited, unlimited, unlimited, [md,pdf,docx], planner=auto, 1 Google account two-way, agent_actions=true, reminders=[web,push,email]`; power `pro + sources_total=500, max_file_mb=100, storage_mb=10000, devices_max=10, credits_per_month=2000, web_searches_per_day=100, multi-Google, planner=auto+balancer+weakness_blocks, reminders + telegram/discord, advanced analytics, better models, byok=true`.
- **Credit costs (tunable, `config/plans.toml`):** `chat_message=1`, `web_search=2` (the spec's value is truncated — `web_sear,` — so this is an assumption, A25), `flashcard_batch(10)=4`, `quiz(10 questions)=6`, `report_outline=3`, `report_section=6`, `task_breakdown=2`, `syllabus_import=8`, `plan_generation=2`, `agent_turn=2`. Long-document ops scale with input tokens: `base + ceil(max(0, input_tokens - 20000) / 1000) * 1` (the spec says "simple multiplier" without a formula — A25/A2 pin it in `config/plans.toml` as `long_doc_threshold_tokens` + `long_doc_credits_per_1k_tokens`).
- **Account-wide entitlements, never per device.** Paying on web/desktop unlocks every device as soon as payment is verified. Account sync works on ALL plans; only device count and storage differ.
- **`config/billing_policy.toml` defaults:** `allow_purchase_ui`, `show_prices`, `allow_purchase_cta_in_locked_messages` = `true` for web/macos/windows, `false` for ios/android. Mobile builds contain no checkout screen, no purchase buttons, no off-store purchase links or prices.
- **Never trust client-reported payment status.** Never store card/wallet data. Gateway secrets only in env. Webhook = signature-verified, idempotent, amount-checked.
- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes. Prompts live in versioned modules, never inline. `tracing` structured logs with request ids; log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Quality gate at every phase:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, web build, desktop build (mobile builds from Phase 14). CI must be green before a phase is called done.
- **UI language English**, but Indonesian documents and queries must work end to end (`simple` tsvector config + multilingual embeddings).

---

## Planned Repository Tree

```
Stvdy/
├── Cargo.toml                     # [workspace] members = crates/*
├── rust-toolchain.toml            # stable, pinned
├── justfile                       # dev entry points (db-up, migrate, server, web, tauri, check)
├── docker-compose.yml             # postgres:16 + pgvector, minio, createbucket
├── .env.example
├── .github/workflows/ci.yml       # fmt, clippy, test, web build, macos+windows bundle
├── config/
│   ├── plans.toml                 # plans, limits, credit costs, model tiers, cost ceilings
│   └── billing_policy.toml        # per-platform purchase-UI policy
├── prompts/                       # versioned prompt modules, compiled via include_str!
│   ├── README.md                  # composition, versioning and variable contract
│   ├── base.v1.md
│   ├── json_contract.v1.md
│   └── {flashcards,quiz,report_outline,report_section,syllabus,
│        task_breakdown,quick_capture,planner_effort,cheatsheet}.v1.md
├── templates/                     # print templates, compiled via include_str!
│   └── cheatsheet.v1.html
├── crates/
│   ├── core/                      # pure logic, NO web/db/io
│   │   ├── src/lib.rs
│   │   ├── error.rs
│   │   ├── config/{mod.rs,plans.rs,billing_policy.rs}
│   │   ├── traits/{mod.rs,llm.rs,embedder.rs,search.rs,payment.rs,push.rs,clock.rs,storage.rs}
│   │   ├── chunk.rs               # ~480 tokens, ~50 overlap, sentence-safe (A2: model max_length is 512)
│   │   ├── retrieval.rs           # vector+keyword RRF merge
│   │   ├── citations.rs           # label->chunk resolver: validate [1]/[W1], drop unknown
│   │   ├── sm2.rs                 # deterministic review replay
│   │   ├── entitlements/{mod.rs,feature.rs,check.rs,usage.rs}
│   │   ├── planner/{mod.rs,model.rs,slots.rs,allocate.rs,balance.rs,replan.rs}
│   │   ├── prompts/{mod.rs,registry.rs}   # include_str! of prompts/*.md + strict {{var}} composer
│   │   ├── generation/{mod.rs,flashcards.rs,quiz.rs,report.rs,syllabus.rs,capture.rs,cheatsheet.rs}
│   │   ├── render/{mod.rs,sanitize.rs,cheatsheet.rs}  # typed blocks -> template HTML, escaping
│   │   └── agent/{mod.rs,tools.rs,gate.rs}
│   ├── sync/                      # shared protocol + client engine (server AND tauri)
│   │   ├── src/lib.rs
│   │   ├── proto/{mod.rs,entities.rs,pull.rs,push.rs,error.rs,cursor.rs}
│   │   └── client/{mod.rs,engine.rs,outbox.rs,apply.rs,conflict.rs,transport.rs}
│   └── server/
│       ├── src/main.rs
│       └── src/{lib.rs,config.rs,state.rs,error.rs,telemetry.rs,db.rs,util.rs}
│           ├── auth/{mod.rs,routes.rs,tokens.rs,password.rs,email_verify.rs,extract.rs}
│           ├── devices.rs
│           ├── sync/{mod.rs,pull.rs,push.rs,stream.rs,apply.rs,log.rs}
│           ├── workspaces.rs
│           ├── sources/{mod.rs,routes.rs,ingest.rs,parse.rs,store.rs,status.rs}
│           ├── chunks.rs
│           ├── chat/{mod.rs,routes.rs,sse.rs,retrieve.rs,prompt.rs}
│           ├── flashcards.rs
│           ├── quizzes.rs
│           ├── reports.rs
│           ├── tasks.rs
│           ├── events.rs
│           ├── goals.rs
│           ├── planner/{mod.rs,routes.rs,apply.rs}
│           ├── syllabus.rs
│           ├── calendar/{mod.rs,google.rs,oauth.rs,ics_in.rs,ics_out.rs,sync.rs}
│           ├── actions.rs
│           ├── study.rs
│           ├── dashboard.rs
│           ├── settings.rs
│           ├── account.rs
│           ├── admin.rs
│           ├── billing/{mod.rs,routes.rs,webhook.rs,activate.rs,periods.rs,discount.rs,credits.rs,usage.rs,margin.rs}
│           ├── jobs/{mod.rs,worker.rs,handlers/{mod.rs,ingest.rs,generate.rs,calendar.rs,reminders.rs,push.rs,planner.rs,grants.rs,payments.rs,retention.rs}}
│           └── integrations/{mod.rs,llm.rs,embedder.rs,search.rs,push.rs,payment.rs,storage.rs,mailer.rs,clock.rs}
├── migrations/                    # sqlx, ordered
│   ├── 0001_extensions.sql
│   ├── 0002_plans.sql
│   ├── 0003_users.sql
│   ├── 0004_devices_auth_push.sql
│   ├── 0005_change_log_mutation_log.sql
│   ├── 0006_sync_functions.sql
│   ├── 0007_workspaces_sources_chunks.sql
│   ├── 0008_messages.sql
│   ├── 0009_decks_flashcards_reviews.sql
│   ├── 0010_quizzes_attempts.sql
│   ├── 0011_reports.sql
│   ├── 0012_tasks_goals_sessions_settings_actions.sql
│   ├── 0013_calendar_events.sql      # after 0012: events.task_id references tasks(id)
│   ├── 0014_billing.sql
│   ├── 0015_jobs.sql
│   ├── 0016_cheatsheets.sql
│   └── 0017_sync_triggers.sql      # MUST BE LAST: attaches log_change/touch_updated_at to every +sync table
├── app/                           # the single UI for web + all Tauri targets
│   ├── index.html  vite.config.ts  tsconfig.json  package.json  vitest.config.ts
│   └── src/
│       ├── main.tsx  App.tsx  router.tsx  styles.css
│       ├── platform/{types.ts,index.ts,web.ts,desktop.ts,mobile.ts}
│       ├── api/{client.ts,sse.ts,errors.ts,queries/*.ts}
│       ├── lib/{time.ts,format.ts,id.ts,cache.ts}
│       ├── components/{ui/*,layout/{Shell,Sidebar,TabBar,ThreePanel}.tsx}
│       └── features/
│           ├── auth/ workspaces/ sources/ chat/ studio/{flashcards,quiz,reports}/
│           ├── study/ today/ calendar/ tasks/ syllabus/ dashboard/
│           └── settings/ devices/ billing/ admin/
├── src-tauri/
│   ├── Cargo.toml  tauri.conf.json  build.rs
│   ├── capabilities/{default.json,desktop.json,mobile.json}
│   ├── src/{main.rs,lib.rs,keychain.rs,cache/{mod.rs,schema.rs,queries.rs},commands/{mod.rs,sync.rs,auth.rs,files.rs,notify.rs}}
│   ├── gen/apple/  gen/android/    # generated mobile projects (iOS: Swift push bridge)
│   └── plugins/push/               # custom Swift/Kotlin push plugin IF no viable official one
└── docs/{ASSUMPTIONS.md,ARCHITECTURE.md,API.md,SYNC.md,RELEASE.md,superpowers/plans/*}
```

---

## Cross-Cutting Contracts

These are fixed now so that parallel work in later phases cannot diverge. Phase plans implement them; they may not redefine them.

### 1. Provider traits (`crates/core/src/traits/`)

```rust
#[async_trait] pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports_tools(&self) -> bool;
    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, LlmError>;
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError>;   // mpsc of StreamEvent
}
// ChatRequest { model, messages: Vec<ChatMsg>, tools: Vec<ToolSpec>, temperature, max_tokens, json_mode }
// StreamEvent::Token(String) | ToolCall(ToolCall) | Usage{in,out,provider_cost_usd} | Done
// ChatResponse { text, tool_calls, usage }

#[async_trait] pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;                    // MUST equal chunks.embedding dimension
    fn model_id(&self) -> &str;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

#[async_trait] pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError>;
}
// SearchHit { title, url, snippet, published_at: Option<DateTime<Utc>>,
//             source_kind: SourceKind, doi: Option<String>, provider: &'static str,
//             score: Option<f32>, license: Option<String> }
// SourceKind = Web | Academic        // drives citation rendering and ranking, not the UI badge
// SearchError is *per provider*, never fatal for the request: a tier that errors is
// recorded and the composite moves on. Only "every tier failed" is a user-visible error.
// `published_at` is populated by the academic tier (OpenAlex/Crossref/DOAJ/ERIC carry a
// real date) and by Exa; Tavily and SearXNG returned null in 20/20 measured results — so
// any "recent sources" filter MUST read this field, never assume it is present.

// The composite is itself a SearchProvider, so callers cannot tell tiers apart:
//   CompositeSearch { tiers: Vec<Box<dyn SearchProvider>> }
//   search(): for each tier in order -> hit Postgres cache (A40) -> on miss call the tier ->
//             if results are non-empty, cache, rank, return; else record `unresponsive`
//             and continue. Every tier's outcome lands on one `usage_events` row.
// Ranking (crates/core/src/search/rank.rs, pure + unit-tested): dedupe by
//   normalized URL, then DOI; then sort by (kind_priority: Academic > Web,
//   has_doi, published_at desc if present, tier order, provider score). Deterministic —
//   a tie resolves by URL so two runs of the same query return the same order.
// A query that produces zero results from *every* tier is not an error: the chat turn
// proceeds without web context and the `citations` event is empty (A17 drops unknown labels).

#[async_trait] pub trait PageFetcher: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError>;
}   // FetchedPage { final_url, title, text, fetched_at }
// Used for the "save to workspace" path and for expanding a citation's page. Order:
//   1. Exa /contents  — $1 per 1,000 pages, returns clean text, honours robots upstream
//   2. moli CLI       — `moli fetch --dump markdown --timeout 15000 --block-private-networks`,
//                       JS-rendered pages; exit code 1 + stderr reason is a real failure
//   3. reqwest + readability — static pages, cheapest
// `--block-private-networks` is mandatory on every path that fetches a user-supplied URL:
// ingestion accepts arbitrary URLs, so an unguarded fetch is an SSRF primitive (A18/A34).
// Note moli's own help states proxy-resolved targets bypass that client-side policy — do not
// combine `--block-private-networks` with `--http-proxy` on this path.

#[async_trait] pub trait PaymentProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn create_invoice(&self, req: InvoiceRequest) -> Result<Invoice, PaymentError>; // order_id, amount_idr, description, expires_at
    async fn get_status(&self, order_id: &str) -> Result<GatewayStatus, PaymentError>;
    fn parse_and_verify_webhook(&self, headers: &HeaderMap, raw_body: &[u8]) -> Result<WebhookEvent, PaymentError>;
    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError>; // record-only for MVP
}

#[async_trait] pub trait PushProvider: Send + Sync {
    fn platform(&self) -> PushPlatform;  // Apns | Fcm
    async fn send(&self, token: &str, msg: PushMessage) -> Result<(), PushError>;
}

pub trait Clock: Send + Sync { fn now(&self) -> DateTime<Utc>; }        // injected into planner + SM-2
#[async_trait] pub trait Storage: Send + Sync {
    async fn put(&self, key: &str, body: Bytes, mime: &str) -> Result<(), StorageError>;
    async fn get(&self, key: &str) -> Result<Bytes, StorageError>;
    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, StorageError>;
    async fn presign_put(&self, key: &str, ttl: Duration) -> Result<String, StorageError>;
    async fn delete(&self, key: &str) -> Result<(), StorageError>;
}
```

Test doubles live in `crates/core/src/traits/testing.rs` (mock LLM that emits scripted tokens/tool calls, mock embedder with deterministic hashing, mock payment gateway, mock push, fixed clock) and are reused by integration tests in every later phase.

### 2. Sync protocol (`crates/sync/src/proto/`)

```rust
pub struct PullResponse { pub changes: Vec<Change>, pub next_cursor: i64, pub has_more: bool }
pub struct Change { pub seq: i64, pub entity: Entity, pub id: Uuid, pub op: Op, pub data: Option<serde_json::Value> }
pub enum Entity { Workspace, Source, Message, Deck, Flashcard, FlashcardReview, Quiz, QuizQuestion,
                  QuizAttempt, Report, Event, Task, Goal, StudySession, UserSettings, PendingAction }
pub enum Op { Upsert, Delete }
pub struct PushRequest { pub device_id: Uuid, pub mutations: Vec<Mutation> }
pub struct Mutation { pub mutation_id: Uuid, pub entity: Entity, pub op: Op, pub id: Uuid,
                      pub payload: Option<serde_json::Value>, pub client_ts: DateTime<Utc> }
pub struct PushResponse { pub results: Vec<MutationResult> }
pub enum MutationResult { Applied{ seq: i64 }, Rejected{ code: ErrorCode, message: String }, Conflict{ winner: serde_json::Value } }
```

Rules the implementation must satisfy (tested in Phase 8 and re-used by every later phase):

- Pull pages by `seq`, always `order by seq asc`, `limit` default 200 / max 1000. Cursor older than retained history → `410 resync_required`.
- **`seq` is a real clock only because of a per-user advisory lock.** `change_log.seq` is a `bigserial` assigned by `nextval()` inside the trigger, so it is assigned at INSERT time but rows become visible at COMMIT time; two concurrent transactions for one user can take seq 10 and 11 yet commit 11 first. A client that reads `seq > cursor`, sees 11 and advances to 11 would never receive 10 → permanent loss on that device. Both `log_change()` and `log_change_by_user()` therefore take `pg_advisory_xact_lock(hashtext(user_id::text))` **before** the `change_log` insert. The lock is held to commit, so for any committed row of a user every lower seq is already committed, and a reader can only ever observe a contiguous prefix. Accepted cost: one user's sync writes serialize against each other (free at this write rate). Same key as the credit-spend lock (A9), so sync writes and credit spends serialize rather than deadlock. Scale-out path if it ever matters: add a `txid` column and only return rows below `pg_snapshot_xmin(pg_current_snapshot())`.
- Initial sync (`cursor=0`) is ordered by **dependency rank** then `seq`: `workspace(1), source(2), deck(3), flashcard(4), quiz(5), quiz_question(6), report(7), task(8), goal(9), event(10), study_session(11), message(12), flashcard_review(13), quiz_attempt(14), user_settings(15), pending_action(16)`.
- Push is idempotent per `mutation_id` via `mutation_log`; a replay returns the stored result and applies nothing.
- Push validates entitlements + ownership **per mutation**; a rejected mutation does not abort the batch.
- Local apply runs in one SQLite transaction with `PRAGMA defer_foreign_keys=ON`, so out-of-order arrivals still land; anything whose parent is still missing is retried on the next pull.
- Every synced table carries `user_id`, `updated_at`, `deleted_at`; the server attaches two triggers per table from `migrations/0006_sync_functions.sql` + `migrations/0017_sync_triggers.sql`. **Those files are the single source of truth for the trigger bodies** — this document deliberately does not duplicate them, because a stale copy of `log_change()` is how the seq-ordering bug below would silently reappear. The trigger migration must remain the **last** migration, since it attaches triggers to every synced table created before it; a new `+sync` table is added by inserting its migration above it and appending the table name to the array. The invariants the bodies must preserve:
  - `log_change()` reads `old`/`new` only through columns that exist on every `+sync` table (`user_id`, `id`, `deleted_at`). `user_settings` has neither `id` nor `deleted_at`, so it uses the separate `log_change_by_user()` function; referencing `new.id` there raises "record new has no field id".
  - Both functions take `pg_advisory_xact_lock(hashtext(user_id::text))` **before** inserting into `change_log` — this is what makes `seq` order equal commit order (see the cursor rule above).
  - The device that caused a change comes from `current_setting('app.device_id', true)`, set per push transaction with `select set_config('app.device_id', $1, true)`.

- **Lock order is always advisory-then-rows.** The per-user advisory lock lives inside the AFTER trigger, so a transaction acquires it *after* its row locks. A push batch that writes several rows (a task plus its subtasks, or a task plus its linked event) can therefore deadlock against another device touching the same rows: tx A holds the advisory lock from row Y and waits on row X, while tx B holds row X and waits for the advisory lock in its trigger. Postgres aborts one with `40P01` and the push 500s — the outbox retries so it self-heals, but this is the core multi-device scenario, not an edge case. Rule: **`POST /sync/push` takes `select pg_advisory_xact_lock(hashtext($user_id::text))` as the FIRST statement of its transaction, before applying any mutation**, so the advisory lock always precedes row locks and the acquisition order is uniform. The trigger keeps its own lock as the backstop for the dedicated REST endpoints, which write a single row and therefore cannot invert the order. Phase 8's convergence tests must include a two-mutation batch edited concurrently from two devices (task + subtask) and assert both pushes succeed.

- `app.device_id` is set once per push transaction (`select set_config('app.device_id', $1, true)`), so `origin_device_id` is populated without adding a device column to every table`.

**Pushable-entity allowlist (normative).** `POST /sync/push` accepts mutations for **exactly six** entities — the spec's offline-write scope — and rejects everything else with `Rejected{ code: "read_only_entity" }`:

| Entity | Push ops allowed |
|---|---|
| `Task` | `upsert`, `delete` |
| `Event` | `upsert`, `delete` |
| `FlashcardReview` | `upsert` only (append-only) |
| `QuizAttempt` | `upsert` only (append-only) |
| `StudySession` | `upsert` only (append-only) |
| `UserSettings` | `upsert` only (no tombstones) |

Everything else is written through its dedicated endpoint, never through push: `Workspace`, `Deck`, `Flashcard`, `Quiz`, `QuizQuestion`, `Report`, `Goal`, `Source`, `Message`, `PendingAction`. Two of these are security-relevant, not just tidiness:

- **`Message` must be rejected.** Messages are persisted server-side by `POST /workspaces/:id/chat`. Accepting a client push would create a second write path that skips the credit spend *and* lets a client forge assistant turns with fabricated citations. Offline chat is explicitly out of scope.
- **`Source` must be rejected.** `sources.status`, `file_key`, `read_only` and the ingestion lifecycle are server-owned; a client must not be able to mark a source `ready` or bypass storage quotas.

`Flashcard`, `Quiz`, `QuizQuestion`, `Deck` and `Report` are server-generated (LLM output) or created via their own endpoints, so a client push would bypass credit accounting and the strict-JSON validation that generation performs.

Online edits to the six pushable entities may go through either path — the dedicated REST endpoints or push — because both funnel through the same table triggers and therefore produce the same `change_log` entries and the same LWW outcome.

- Conflict rules, fixed:

| Entity | Rule |
|---|---|
| `flashcard_reviews`, `quiz_attempts`, `study_sessions` | Append-only. Insert; duplicate id ignored (`on conflict do nothing`). |
| `flashcards` (ease/interval/due) | Server-derived by replaying reviews. Clients treat as read-only; a client push of these fields is rejected. |
| `tasks`, `events`, `user_settings` | Record-level LWW on server receive time; field-level merge for `tasks` and `events` when changed field sets are disjoint. |
| `goals`, `decks`, `reports`, `workspaces` | LWW on server receive time, reached through their dedicated endpoints (not push — see the allowlist). |
| Delete vs concurrent edit | Tombstone wins unless the user explicitly restores (restore = push with `deleted_at=null`). |
| Long-text (`reports.body_md`) edited on two devices | Keep latest, store the losing body as a recoverable copy row in `report_revisions`. |
| `sources` status, subscription, credits, planner output, `messages`, `pending_actions` | Server-owned; read-only for clients except via their dedicated endpoints. |

### 3. Entitlements (`crates/core/src/entitlements/`)

```rust
pub enum Feature {
    CreateWorkspace, UploadSource{ size_bytes: i64 }, IngestSource, ChatMessage, WebSearch,
    CreateDeck, FlashcardBatch{ n: u32 }, CreateQuiz{ n: u32 }, Report, ExportFormat(ExportFormat),
    TaskBreakdown, QuickCapture, GoogleCalendar, SyllabusImport, PlanGeneration, AgentAction,
    DeviceLogin, Byok, ReminderChannel(Channel), AdvancedAnalytics,
}
pub enum EntitlementError {
    PlanRequired { feature: Feature, min_plan: PlanId },
    LimitReached { feature: Feature, limit: i64, used: i64, min_plan: PlanId },
    InsufficientCredits { needed: i64, available: i64 },
    FeatureLocked { feature: Feature },
}
pub struct UsageSnapshot { pub plan: PlanId, pub subscription_status: SubStatus, pub credits: Credits,
                           pub storage_bytes: i64, pub workspaces: i64, pub sources: i64, pub devices: i64,
                           pub chat_msgs_today: i64, pub web_searches_today: i64,
                           pub flashcards_this_month: i64, pub quizzes_this_month: i64,
                           pub reports_this_month: i64, pub syllabus_imports: i64 }
pub fn check(u: &UsageSnapshot, f: Feature) -> Result<(), EntitlementError>;
```

- Every handler — including **each sync push mutation** — calls `check`. No limit is hardcoded anywhere else; the matrix is generated from `config/plans.toml` and a generated test asserts the whole plan×feature matrix.
- Errors serialize to `{ code, message, min_plan?, limit?, used? }`; the UI renders a precise upgrade prompt, except on mobile where `billing_policy` forces neutral copy.
- Credits: `pg_advisory_xact_lock(hashtext(user_id::text))` → read bucket sums → reject if insufficient → insert negative `credit_ledger` row + `usage_events` row, all in one transaction. Monthly bucket first, then purchased.

### 4. SSE contract

`POST /workspaces/:id/chat` and `POST /workspaces/:id/chat` with agent tools enabled both answer `text/event-stream`:

```
event: token      data: {"t":"..."}
event: citations  data: {"citations":[{"type":"chunk","chunk_id":"...","source_id":"...","page":12}]}
event: action     data: {"pending_action_id":"...","tool":"create_task","args":{...}}   // WRITE tools only
event: usage      data: {"credits_spent":1,"credits_remaining":412}
event: done       data: {"message_id":"...","used_web":false}
event: error      data: {"code":"insufficient_credits","message":"...","min_plan":null}
```

Client side uses `fetch` + `ReadableStream` (never `EventSource`: chat needs POST + `Authorization`). Heartbeat comment every 15s keeps proxies from closing idle streams.

### 5. HTTP error mapping (`crates/server/src/error.rs`)

| Variant | Status | `code` |
|---|---|---|
| `Unauthorized`, `TokenExpired` | 401 | `unauthorized`, `token_expired` |
| `Forbidden` / `EntitlementError::PlanRequired` / `FeatureLocked` | 403 | `plan_required`, `feature_locked` |
| `NotFound` | 404 | `not_found` |
| `Conflict` / `MutationResult::Conflict` | 409 | `conflict` |
| `Gone` (stale cursor) | 410 | `resync_required` |
| `LimitReached` | 422 | `limit_reached` |
| `InsufficientCredits` | 402 | `insufficient_credits` |
| `Validation` | 400 | `validation` |
| `RateLimited` | 429 | `rate_limited` |
| `Provider` / `Internal` | 502 / 500 | `provider_error`, `internal` |

### 6. Deterministic planner (`crates/core/src/planner/`) — pure function, no DB, no clock

```rust
pub struct PlannerInput {
    pub now: DateTime<Utc>, pub tz: Tz, pub settings: StudySettings,
    pub goals: Vec<GoalInput>,          // target_date, topics: Vec<Topic{ source_id, remaining_min, weight }>
    pub due_review_min: i64,            // from due flashcards
    pub busy: Vec<Interval>,            // merged, from events (kind != study_block) + tasks with due dates
    pub pinned: Vec<Interval>,          // user-dragged blocks kept as-is (replan)
    pub config: PlannerConfig,          // block_len_min, break_min, deadline_buffer_min, revisit_days, spread_k, balancer_enabled
}
pub struct PlanProposal { pub blocks: Vec<ProposedBlock>, pub day_load: Vec<DayLoad>,
                          pub deficit_min: i64, pub warnings: Vec<PlanWarning> }
```

Algorithm (fixed order, fully deterministic — same input ⇒ same output):

1. Horizon = `ceil_to_block(now)` … `max(goal.target_date) + 1 day`.
2. Per local date (via `chrono-tz`; non-existent local times on DST-spring days are skipped, ambiguous times take the earlier offset) → study window `[start_hour, end_hour)` → subtract merged `busy` ∪ `pinned` → free intervals, each clipped to `max_study_min_per_day` and chunked into `block_len_min` blocks separated by `break_min`.
3. Demand `D = Σ_topic remaining_min × weight(topic)` where `weight = 1 + spread_k × (1 − accuracy)` (weak topics weigh more), plus `due_review_min`.
4. Reserve a review pass: for each goal, the last `revisit_days` days before `target_date` reserve one block per day, and the final `deadline_buffer_min` before `target_date` is reserved for review only (no new-material blocks).
5. Allocate remaining capacity day-by-day, goals by `target_date` asc (tie: `created_at`), topics by weight desc, capping each topic per day at `max(block_len_min, ceil(remaining_min / days_left))` so work spreads instead of front-loading.
6. If `Σ capacity < D`, still emit a best-effort plan and report `deficit_min` + `PlanWarning::Overloaded{days}` — never silently drop demand.
7. Balancer (Power): while any day load exceeds `overload_pct` of `max_study_min_per_day` and a movable block exists, move the latest movable block to the least-loaded day; bounded to 100 iterations to guarantee termination.

Unit tests: no overlap with busy time; study-hours respected incl. a DST-transition day; `max_study_min_per_day` respected; block/break lengths exact; deadline buffer; weak-topic weighting; spread cap; determinism; balancer termination; replan keeps pinned blocks.

### 7. Prompts and document rendering

**Prompts are versioned content, not code.** Each module is a Markdown file under `prompts/`, compiled in with `include_str!`, composed by `crates/core/src/prompts/`. Rules (full detail in `prompts/README.md`):

- Composition is one of exactly two shapes: chat = `base.v1.md` + sources + tool schemas; a strict-JSON task = base **trimmed** by section allowlist + `json_contract.v1.md` + the module + sources. The composer asserts the trimmed result still contains `<grounding_and_citations>` and `<security_untrusted_content>` — a task prompt must never lose its grounding or injection defences.
- An unfilled `{{variable}}` is an **error, not an empty string**, so a literal `{{count}}` can never ship. Two variables per module are declared optional with a stated default.
- User-controlled variables (`user_name`, `workspace_name`, `goal_title`, `task_title`, `notes`, `topic`, `capture_text`) are **never interpolated bare**. The composer wraps them in a `<user_context_data>` block labelled as data, because a display name is a user-editable string and "ignore all previous instructions" belongs in it just as much as in an uploaded PDF.
- Every prompt has a version (`chat.v1`), stored on each `usage_events` row so a quality or cost regression can be attributed to a prompt change instead of guessed at. A shipped version is never edited — changes create `.v2`.
- `crates/core/tests/prompts.rs` fails CI if a module has an undeclared variable, a leftover `{{...}}`, a missing grounding/security section, or a documented JSON example that does not parse into its Rust struct.

**Document rendering is a pipeline of typed data, never model-authored HTML.**

```
generate (cheatsheet.v1.md -> strict JSON blocks)
  -> validate (serde, deny_unknown_fields; drop blocks with unknown kinds)
  -> resolve labels -> real chunk ids (unknown labels dropped, A17)
  -> render (crates/core/src/render/cheatsheet.rs: escape every field, emit template markup)
  -> template (templates/cheatsheet.v1.html, include_str!)
  -> print (server-side Chromium -> PDF, or the client's print dialog)
```

The security property that makes this safe: **the model produces structured JSON, never markup.** Every string it emits passes through HTML escaping, and SVG blocks are sanitised against an element/attribute allowlist (`svg,g,rect,circle,ellipse,line,polyline,polygon,path,text,tspan,defs,marker,title`; no `script`, `style`, `on*`, `href`, or `foreignObject`). This matters because source text is attacker-controlled: a document containing `</div><script>` or an `<svg onload=...>` must be inert in the rendered PDF. Two further rules the renderer enforces:

- Every `id` inside an inline SVG is prefixed with the block index, and every `url(#...)` reference is rewritten to match. Bare ids like `a` are document-global, so the second diagram would silently reuse the first one's marker.
- The template is compiled in with `include_str!`, so a PDF can never render against a template file that is missing or drifted from the binary.

**PDF engine:** server-side headless Chromium via `headless_chrome`, with `printBackground: true` (without it every coloured box prints white), `preferCSSPageSize: true` (A4 comes from the template's `@page`), `generateDocumentOutline: true` (real PDF bookmarks from the headings) and `generateTaggedPDF: true` (accessibility). Verified in this repo: Chrome honours `@page` margin boxes and `@page :first`, so the running header and `Halaman N dari M` footer come from CSS; CDP `headerTemplate`/`footerTemplate` is the fallback for per-job overrides. Chromium runs as a pooled, sandboxed child process; the render step is a `jobs` row so a large cheatsheet never blocks a request, and it is gated by plan (A39).

### 8. Job kinds and schedule

`ingest_source`, `generate_flashcards`, `generate_quiz`, `generate_report_section`, `generate_cheatsheet`, `render_cheatsheet_pdf`, `plan_goal`, `sync_calendar`, `send_reminder`, `send_push`, `poll_payment`, `reconcile_payments`, `monthly_grant`, `renewal_reminder`, `reset_counters`, `compact_change_log`, `purge_deleted_accounts`. Worker: `select … where status='queued' and run_at<=now() order by run_at for update skip locked limit N`; `attempts` with exponential backoff; singleton jobs use `dedupe_key`. The two cheatsheet jobs are separate so a PDF failure does not discard a successfully generated HTML cheatsheet.

---

## Phase Plan

| Phase | Goal | Exit gate (observable) |
|---|---|---|
| 0 | Scaffold compiles on all targets | `just check` green; `docker compose up` + `sqlx migrate run` clean; `/health` returns ok; Tauri builds for macOS + Windows; CI green |
| 1 | Auth + devices + ingestion + mobile capability spike | Signup→verify→login on 2 "devices"; PDF upload → chunks+embeddings → `ready`; iOS+Android device builds proving push-token registration and file intake, results in ARCHITECTURE.md |
| 2 | Retrieval + streaming chat | Cited answer streams, unknown chunk ids dropped, message persisted and visible on second device |
| 3 | Web search | Toggle on → cited results with URLs **and DOIs**; repeated query bills once (cache hit); academic tier answers with SearXNG + both fallback keys unavailable; "save to workspace" creates a `kind=web` source |
| 4 | Flashcards + SRS | Generated deck; reviews from two clients replay to identical ease/interval/due |
| 5 | Quiz | Generated quiz; attempts scored; weak-topic summary grouped by source/page |
| 6 | Reports + cheatsheet/PDF pipeline | Outline edit → section generation with citations → `.md` export; cheatsheet JSON → template HTML → A4 PDF with header, page numbers and TOC |
| 7 | Entitlements + credits | Every route goes through `check`; plan matrix test; concurrent credit spend never double-spends; downgrade → read-only; manual plan switch via admin |
| 8 | Sync engine | 3 simulated devices converge; replayed mutation idempotent; tombstone vs edit; 410 → full resync; revoked device wiped |
| 9 | Tasks/events/Today/timer | Offline task+event+session writes sync after reconnect; Today view + focus timer work offline |
| 10 | ICS + Google Calendar | `/cal/:token.ics` validates; external ICS imported with recurrence; Google two-way sync incremental + 410 full resync |
| 11 | Syllabus + planner | Syllabus → review screen → events/tasks/goals; planner proposal respects busy time, daily max, study hours, buffer |
| 12 | Agent + reminders + dashboard | WRITE tool creates `pending_actions`, confirm on device B executes it; reminder dedupe across devices; dashboard metrics |
| 13 | QRIS payments | Sandbox invoice → verified webhook → plan active on second device; duplicate/mismatch/late/expired cases tested; mobile checkout blocked |
| 14 | Mobile | iOS + Android builds; no purchase UI anywhere in the mobile bundle; push tokens register; deep links round-trip |
| 15 | Release | Signed+notarized macOS universal, signed Windows installer, updater with signed manifests, store checklists, server Docker image |

### Phase detail

**Phase 0 — Scaffold.** Cargo workspace (`core`, `sync`, `server`), config loaders with tests, full schema migrations incl. sync triggers, docker-compose (postgres+pgvector, minio), `/health`, React+Vite+TS+Tailwind SPA with `platform` adapter skeleton, Tauri 2 shell (macOS + Windows config, capabilities, plugins registered), CI matrix, docs stubs. *Risk:* schema mistakes are expensive later → the migration test asserts every `+sync` table has the three sync columns **and** both triggers, driven by a table list in the test.

**Phase 1 — Auth, devices, ingestion, mobile capability spike.** argon2id, email verification, JWT access + rotating refresh bound to `devices`, reuse detection revoking the family, devices CRUD, `POST /workspaces/:id/sources` (multipart), object storage presign, ingest job (PDF via pdfium + text), 480/50 sentence-safe chunking, `(embedder, content_hash)` dedupe lookup, batched embeddings, status polling. Native prototype: keychain token storage + deep-link round trip on macOS and iOS simulator; findings into `docs/ARCHITECTURE.md`. *Risk:* pdfium binary packaging in Docker → test the Docker image build in CI, not just the host.

**Phase 1 mobile capability spike (do not defer to Phase 14).** The spec requires the riskiest native pieces be prototyped early, and Phase 1 currently only covers keychain + deep link. Push and iOS file intake are the two capabilities whose failure forces a Swift/Kotlin plugin — or a framework change, which requires asking. Discover that in Phase 1, not after 13 phases. Minimal, throwaway-proportion spike on a physical device (and simulator where meaningful):

| Probe | Pass condition | If it fails |
|---|---|---|
| APNs token registration (iOS) | `registerForRemoteNotifications` yields a device token surfaced to Rust/JS | Adopt a custom Swift plugin; escalate before Phase 14 |
| FCM token registration (Android) | `FirebaseMessagingService` yields a token; `google-services.json` + Gradle plugin wired | Same, Kotlin side |
| Foreground push receipt + tap-to-open (both) | Tapping a push deep-links into the right screen, including cold start | Tap-to-open is the only reliable JS surface; background *execution* is not achievable (A33) |
| iOS "Open in…" document intake | A PDF opened from Files reaches the app via `RunEvent::Opened` | Test cold start specifically — cold-start delivery under the scene lifecycle is **UNVERIFIED** in the capability matrix |
| iOS share-sheet row | App appears in the share sheet and the file arrives | Requires a Share Extension + App Group (custom Swift); confirm the App Group entitlement path early |
| Android `content://` share/import | `bundle.fileAssociations[]` with a concrete `mimeType` delivers a URI the fs plugin can read | `mimeType: "*/*"` never matches; set concrete types |

Record every result — pass **and** fail — in `docs/ARCHITECTURE.md` § Native prototypes, with the exact plugin/native shim chosen. Anything that fails becomes a Phase 14 prerequisite, and a framework-level failure is a question for the user before proceeding.

**Phase 2 — Retrieval + chat.** Hybrid vector+tsvector top-k with RRF, source-scoped filter, chunk-labeled prompt, streaming SSE, citation validation, message persistence. *Risk:* citation hallucination → validation is a pure function with its own tests, and a golden test asserts a fabricated id is dropped.

**Phase 3 — Web search.** `SearchProvider` as a **composite** (A3): academic tier first (OpenAlex, ERIC, Crossref, DOAJ, arXiv, PubMed), then the self-hosted SearXNG service, then Exa, then Tavily — each tier only consulted on empty results or transport error. Postgres result cache with `cache_hit` accounting and a `usage_events` row per search (A40). Ranking + URL/DOI dedupe as a pure function. `use_web` per message, cleaned/truncated context as a separate "web" block, URL **and DOI** citations, `PageFetcher` (Exa `/contents` → moli → reqwest+readability, all with `--block-private-networks`) for save-as-source. **Acceptance:** the same query twice bills one credit (second is a cache hit); with SearXNG stopped and the fallback keys unset, the request still answers from the academic tier; a query that hits nothing returns an empty `citations` array rather than an error.

**Phase 4 — Flashcards.** Strict-JSON generation with one retry, chunk-linked cards, append-only reviews, SM-2 replay in `(reviewed_at, id)` order, due-cards endpoint, client-side due computation from synced state, swipe study UI.

**Phase 5 — Quiz.** MCQ + short answer, count/difficulty, explanation + chunk per question, append-only attempts, score + weak-topic summary.

**Phase 6 — Reports, cheatsheets and the PDF pipeline.** Editable outline → per-section generation with citations → Markdown storage, `.md` export first. Then the document-rendering pipeline that the rest of the app reuses:

- `crates/core/src/prompts/` composer + registry (`include_str!`), with the strict variable contract and the grounding/security assertion. `crates/core/tests/prompts.rs` lands here and covers every module, not just the report ones, so later phases add a prompt and get the checks for free.
- `crates/core/src/render/` — typed blocks → template markup, HTML escaping, SVG element/attribute allowlist, and `id`/`url(#…)` prefixing per block.
- `generate_cheatsheet` and `render_cheatsheet_pdf` jobs; `headless_chrome` pooled renderer with `print_background`, `prefer_css_page_size`, `generate_document_outline`, `generate_tagged_pdf`.
- Routes: `POST /workspaces/:id/cheatsheets` (body: scope, exam_date, depth, source_ids), `GET /cheatsheets/:id`, `GET /cheatsheets/:id/html`, `GET /cheatsheets/:id/pdf`, `POST /cheatsheets/:id/re-render`, and `GET /reports/:id/export?format=` extended with `pdf`. `docs/API.md` gets a `## Cheatsheets` section.
- **Re-render without re-generating.** The validated JSON blocks are persisted (`cheatsheets.blocks_key`), so `POST /cheatsheets/:id/re-render` re-runs only the render + print steps. This exists because the template will change after launch — the first render of `cheatsheet.v1.html` in this repo already surfaced five defects (blocked KaTeX, SVG scale, orphaned answers, SVG `id` collisions, a split `.gap-note`) — and re-generating through the LLM would cost credits *and* produce different content for a document the student may already have printed. `template_version` records which template produced each artifact, so an admin can list stale documents. The renderer must therefore be a pure function of (blocks, template version) with no clock, no network, and no model call.

*Risk:* this is the first phase with a native dependency in the server image. Chromium must be baked into the Docker image and pinned, the renderer must run sandboxed as a pooled child process, and the CI job must build that image — otherwise the pipeline works on a laptop and fails on deploy. PDF rendering is plan-gated (`export_formats`), so a free user gets HTML and prints it themselves.

*Verification:* the phase's acceptance test renders `templates/cheatsheet.v1.html` with sample data and asserts a 6-page A4 PDF whose text contains the running header, a `Halaman N dari 6` footer, the chapter titles, and **no raw `$` delimiters** (which would mean KaTeX failed). That last assertion is the one that catches a broken asset reference or an SRI mistake, both of which fail silently otherwise.

**Phase 7 — Entitlements, credits, limits.** Entitlements engine + generated plan matrix test, credit ledger + advisory-lock spend, `usage_events` with `provider_cost_usd`, daily/monthly counters with timezone-aware reset, monthly grant job, storage accounting, downgrade → grace → read-only, device limits, rate limits, PPTX/DOCX/URL ingestion, admin plan switch. All earlier routes are retrofitted to call `check` — the phase is not done until a grep test shows no bypass.

**Phase 8 — Sync engine.** `crates/sync` protocol + client engine, `/sync/pull`, `/sync/push`, `/sync/stream`, mutation idempotency, SQLite cache + outbox + `defer_foreign_keys` apply, offline reads, sync-status UI, Devices screen, 410 resync, compaction job, convergence tests with 2–3 simulated devices. *Risk:* this is the phase most likely to leak cross-tenant data → a test asserts a pull for user A never contains user B's `seq`.

**Phase 9 — Tasks, events, Today, timer, settings.** Sync-native CRUD, subtasks, quick-capture (LLM → draft → confirm), break-down, Today view, focus timer logging to `study_sessions` surviving restart, settings incl. quiet hours.

**Phase 10 — Calendar.** ICS feed out (`/cal/:token.ics`), ICS import/subscribe with rrule + tz, then Google OAuth (system browser + deep link, PKCE + state), server-side encrypted refresh tokens, incremental sync via `syncToken` with 410 full resync, dedicated "Study Plan" calendar, conflict policy (Google wins for non-app events), calendar UI week/month.

**Phase 11 — Syllabus + planner.** Syllabus extraction to structured JSON with confidence/evidence → review screen → confirmed events/tasks/goals; deterministic planner per the contract above; proposal preview + drag-adjust + accept; replan; balancer + weak-topic blocks (Power).

**Phase 12 — Agent, reminders, dashboard.** Tool definitions + read/write split, `pending_actions` confirm/reject cross-device, expiry, prompt-injection defense (retrieved text is data, never instructions), reminder engine with timezone + quiet hours + cross-device dedupe, APNs/FCM/desktop/in-app/email channels, local notifications from synced data, dashboard metrics.

**Phase 13 — QRIS payments.** `PaymentProvider` + `qris_gateway` (blocked on B1 docs; env-driven mapping), checkout with discount/student pricing, QR screen with countdown, verified idempotent webhook (one transaction: mark paid → activate/extend → grant credits → record fee → notify), polling fallback + reconciliation, period extension/upgrade-proration/downgrade-at-period-end, credit packs, renewal reminders at 7/3/1/0, billing + pricing pages for web/desktop, billing_policy enforcement, BYOK, admin payment tools + margin report.

**Phase 14 — Mobile.** Tauri mobile builds, phone/tablet adaptive UI, read-only plan screen (no purchase UI), push plugins with token registration, universal links/app links, secure storage, file picker + share-sheet import, local notifications, TestFlight + Play internal testing, mobile test checklist + store-policy risk note in `docs/RELEASE.md`.

**Phase 15 — Release.** macOS signing + notarization + universal binary, Windows code signing, Tauri updater with signed manifests, Android direct-APK channel, store submission checklists (privacy labels, in-app account deletion, review notes), README + deployment notes + server Docker image.

---

## Risk Register

| Risk | Impact | Mitigation |
|---|---|---|
| Tauri mobile gaps (push, share target, background sync) | Phase 14 rework | Verified in `docs/research/2026-10-11-tauri-2-capability-matrix.md`: mobile is **GA**, but three items need custom Swift/Kotlin — APNs/FCM receive (no official plugin exists; the official notification plugin **cannot** receive remote push), the iOS share-sheet **Share Extension** (+App Group; a share cannot be expressed as a deep link), and the background scheduler (`BGAppRefreshTask`/`BGProcessingTask` on iOS, `WorkManager` ≥15 min on Android). Also verified impossible: running SPA JS while backgrounded/terminated on either platform. Treat push + share extension + background scheduler as **one native-work package** in Phase 14, not three independent tasks; prototype keychain + deep link in Phase 1 as planned. Never switch frameworks without asking. |
| `vector(768)` vs local embedder dimension | Blocks Phase 1 | Column dimension is one migration constant; the chosen model (`MultilingualE5Base`, 768) is asserted equal to it at startup (fail fast). Changing dimension = new column + full re-embed + index rebuild (no widening cast exists). |
| Sync correctness (conflicts, cross-tenant leak) | Data loss / security | Protocol DTOs shared in one crate; convergence + isolation tests in Phase 8; conflict table above is normative. Verified constraint: sqlx 0.9 macros cannot map `vector`/`tsvector`, so `chunks` and generic sync paths use runtime `query_as` with explicit column lists and never `SELECT *`. |
| Planner complexity | Slipping Phase 11 | Pure deterministic function, no IO, unit-tested against a fixed clock; LLM only estimates effort. Recurrence iteration must be bounded (`rrule` `all(limit)` + check `limited`) — unbounded iteration is a real server hang. |
| QRIS gateway undocumented (B1) | Blocks Phase 13 | Trait-first implementation; only field mapping + signature scheme remain once docs arrive. |
| Store review rejection for QRIS-only mobile | Launch delay | No purchase UI in mobile bundle (config + server enforcement, structurally enforced by `BillingPolicy::validate`); risk documented in `docs/RELEASE.md`; `PaymentProvider` extensible to store billing. |
| Cost overrun vs IDR pricing | Margin loss | `usage_events.provider_cost_usd` on every action, per-user cost ceiling with cheap-model fallback, admin margin report. |
| Single-instance assumptions (rate limits, SSE) | Scale limits | Documented; SSE works multi-instance by design (seq polling), rate limits are the only per-instance piece. |
| Toolchain floor surprises (Rust 1.94.1, Node 24) | CI/local drift | Pinned in `rust-toolchain.toml`, `.nvmrc`/`engines`, and CI; `sqlx` 0.9 needs ≥1.94.0 and `aws-sdk-s3` ≥1.94.1. |

---

## Verification Gates (apply to every phase)

1. `cargo fmt --check` · `cargo clippy --all-targets -- -D warnings` · `cargo test` — clean.
2. `npm run build` (web) + `npm run tauri build -- --no-bundle` (desktop) compile; from Phase 14 also `tauri ios build` / `tauri android build`.
3. The phase's own acceptance test from the table above runs and its output is quoted in the phase summary.
4. Docs updated in the same phase: `docs/API.md` for new routes, `docs/SYNC.md` for protocol/conflict changes, `docs/ARCHITECTURE.md` for prototype results, `docs/RELEASE.md` for distribution/policy changes, `docs/ASSUMPTIONS.md` when a decision changes.
5. Phase summary posted; then wait for "continue".

## Self-Review

- **Spec coverage:** all 16 phases from the spec are present with exit gates; every differentiator (multi-source retrieval, model-agnostic traits, exportability, streaming, planner↔content feedback, clean UI + Indonesian support, IDR pricing + cost tracking, one account every device) maps to a contract section or phase. Every deviation from the spec's DDL and crate list, plus the three unspecified values (the truncated `web_sear` credit cost, the long-document multiplier formula, and the embedding dimension), are recorded in `docs/ASSUMPTIONS.md`.
- **Placeholders:** none — unknowns are either pinned (assumption table) or flagged as blockers with a workaround. Version pins live in the Phase 0 plan.
- **Type consistency:** `Feature`, `EntitlementError`, `Mutation`, `MutationResult`, `PlanProposal`, `StreamEvent` are defined once here and referenced by name in every phase.
- **Open item:** B1 (QRIS gateway docs) — needed at Phase 13, not before.
