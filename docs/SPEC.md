# Study OS — Product & Technical Spec (source of record)

This file is the requirement document of record for the project. Every phase plan in `docs/superpowers/plans/` derives from it. Where a plan deviates, the deviation is recorded in `docs/ASSUMPTIONS.md`.

---

# ROLE
You are a senior Rust + full-stack engineer. Build an MVP of an AI "study OS": a NotebookLM-style source-grounded research workspace PLUS a study planner (tasks, schedule, Google Calendar integration, exam-aware planning, an action-taking assistant), sold through 3 subscription plans paid via QRIS, shipped as native apps for macOS, Windows, iOS, and Android (plus a web build), where one account syncs everything across all devices. Work incrementally, keep the code compiling and tested at the end of every phase, and explain key decisions briefly. Don't ask me questions unless you're truly blocked; make reasonable assumptions and list them in docs/ASSUMPTIONS.md.

# PRODUCT
Users create workspaces (e.g. one per course), upload sources, and chat with them with cited answers. The AI can optionally search the web. From selected sources users generate flashcards (spaced repetition), quizzes, and reports. On top of that the app manages tasks and schedule: it imports syllabi, syncs with Google Calendar and ICS calendars, auto-plans study blocks backward from exam dates using quiz weaknesses and due flashcards, and has an assistant that can take actions (create tasks, schedule blocks) only after user confirmation. Access is governed by 3 plans (free, pro, power) with monthly AI credits.

The app runs on macOS, Windows, iOS, Android (and web). A user signs in with ONE account on any device and sees the same data everywhere: workspaces, sources, chat history, flashcards and review progress, quizzes, reports, tasks, events, settings, and subscription status. Changes made on one device (including offline edits to tasks, events, flashcard reviews, quiz attempts, and study sessions) appear on the others.

Billing: users pay with QRIS through a payment gateway I already have (configured via env). There is NO Apple App Store / Google Play billing in this MVP. Subscriptions are prepaid periods, not auto-renewing.

Differentiators (keep in mind in every design choice):
1. One workspace holds many sources with high limits; retrieval works across sources (no isolated notebooks).
2. Model-agnostic: LLM, embedding, search, and payment providers all sit behind traits.
3. Everything is exportable (Markdown/JSON, ICS). Never lock user data in.
4. Fast and streaming: LLM responses stream via SSE; ingestion and generation are async with visible progress. Native shell and local cache make the UI feel instant.
5. Planner and content feed each other (quiz weaknesses -> review blocks; syllabus -> calendar + tasks).
6. Clean, student-friendly UI. UI in English for MVP, but Indonesian documents and queries must work (use the 'simple' text search config and multilingual embeddings). Default user timezone Asia/Jakarta; store all times in UTC plus the user's IANA timezone.
7. Priced for Indonesian students (IDR), with margins protected by credits and cost tracking.
8. One account, every device: seamless sync, offline-tolerant, with per-device session management.

# PLATFORMS AND ARCHITECTURE
- Server (Rust/Axum + Postgres) is the single source of truth. All heavy work (ingestion, embeddings, retrieval, LLM calls, planner, Google Calendar sync, billing) runs on the server. Clients never call LLM, search, or Google APIs directly (exception: BYOK keys are sent to the server per request, never stored on device unencrypted).
- Clients: ONE shared UI codebase (React + Vite + TypeScript + Tailwind, SPA) packaged by Tauri 2 for macOS, Windows, iOS, and Android, and also deployed as a web app (used for admin, checkout, and as a fallback).
- Native shell (src-tauri, Rust) contains: the sync engine (crate `sync`), a local SQLite cache + outbox, secure token storage (OS keychain/keystore), notifications, deep links, file access, and platform plugins. Web build is online-first (no local DB; in-memory cache only).
- A small TypeScript `platform` adapter interface hides platform differences (secure storage, notifications, file picker/share target, deep links, open-external, updater, global shortcut, haptics) with implementations for web, desktop, and mobile. UI code never calls Tauri APIs directly outside this adapter.
- Prototype the riskiest native pieces EARLY (keychain storage, deep-link OAuth callback, push notifications on iOS/Android, file picker/share-target on iOS) and record results in docs/ARCHITECTURE.md. If Tauri mobile lacks a required capability, write a thin Tauri plugin in Swift/Kotlin. Do not switch frameworks without asking me.
- Offline scope: native clients can READ all synced data offline and WRITE offline only for: tasks, events, flashcard reviews, quiz attempts, study sessions, settings. Chat, generation, web search, uploads/ingestion, checkout, and calendar connect require connectivity (show a clear offline state, queue nothing for these).
- Distribution: desktop = direct download (signed + notarized macOS universal build, signed Windows installer) with Tauri updater and signed update manifests. Mobile = TestFlight/App Store and Play internal testing/Google Play; an Android direct-APK download from my website is an allowed alternative channel. Mac App Store / Microsoft Store are out of scope.
- Purchase policy for mobile: because payment is QRIS only, the iOS and Android apps DO NOT sell anything: no checkout, no purchase buttons, no links or instructions pointing to a website purchase, no off-store prices. Users buy on the web or desktop app; the plan then syncs to their account on every device. On mobile, locked features show a neutral message ("This feature isn't available on your current plan") with no purchase call-to-action. This is controlled by config/billing_policy.toml (default: purchase UI enabled on web/macos/windows, disabled on ios/android). Document the app-store review risk of this approach (stores may require their own billing for digital subscriptions in many regions) in docs/RELEASE.md, and keep the PaymentProvider trait extensible so store billing could be added later.

# TECH STACK (fixed)
- Backend: Rust (stable), Axum, Tokio, sqlx (Postgres), serde, reqwest, tracing, thiserror/anyhow, tower-http, argon2, JWT access tokens + rotating refresh tokens.
- Database: Postgres 16 with pgvector and tsvector (hybrid search). Object storage (S3-compatible, e.g. MinIO in dev) for original uploaded files; clients get short-lived signed URLs.
- Parsing: pdfium-render or lopdf for PDF (keep page numbers); zip/xml parsing for PPTX and DOCX.
- Embeddings: trait `Embedder` with impls: (a) `fastembed` local multilingual model, (b) configurable HTTP API. Choose via env. Embeddings and chunks stay server-side.
- LLM: trait `LlmProvider` with streaming AND tool/function calling. Implement one provider first (base URL + API key + model via env). Support a per-request override key (BYOK).
- Web search: trait `SearchProvider`, one impl first (Tavily or Brave via env).
- Calendar: `oauth2` (Google OAuth, offline access, PKCE), `icalendar` (generate ICS), `ical` (parse ICS), `chrono` + `chrono-tz`, `rrule`.
- Payments: trait `PaymentProvider` with ONE implementation now: `qris_gateway`, for my existing gateway. GATEWAY = <FILL IN: gateway name and link to its API docs>. Implement against that gateway's documented API (create QRIS invoice, status query, webhook/callback signature verification). Everything gateway-specific (base URL, keys, callback token/signature scheme, fee rates) comes from env/config. Keep the trait generic so other providers (including store billing) can be added later.
- Push: APNs (iOS) and FCM (Android) via a `PushProvider` trait; desktop uses native OS notifications.
- Client (Tauri 2): `tauri` v2 with plugins for deep-link, notification, updater (desktop only), dialog/fs, opener, os, global-shortcut (desktop only), and a secure-storage solution (keychain/keystore); `rusqlite` (or sqlx-sqlite) for the local cache.
- Frontend: React + Vite + TypeScript + Tailwind, React Router, TanStack Query (server-state + cache) is allowed; no other heavy state libraries.
- Background jobs: a `jobs` table with `FOR UPDATE SKIP LOCKED` polling worker in the server binary (no Redis). Scheduled jobs (calendar sync, reminders, push sends, monthly credit grants, renewal reminders, pending-payment status polling, change_log compaction) use the same table with `run_at`.
- Dev: docker-compose (postgres+pgvector, MinIO), .env.example, Makefile/justfile, README.

# REPO STRUCTURE
/crates/core      -> pure logic, NO web code: chunking, retrieval, prompts, SM-2, planner, entitlements engine, generation orchestration, agent tool definitions, all traits.
/crates/sync      -> shared sync protocol DTOs (used by BOTH server and client) + the client sync engine (pull/push, outbox, cursor, conflict rules) usable in the Tauri shell.
/crates/server    -> Axum app: routes, auth/sessions, SSE, job worker, Google/ICS integration, QRIS payments/webhooks, config.
/app              -> shared React + Vite UI for all platforms (web build and Tauri frontend). Includes `platform/` adapters.
/src-tauri        -> Tauri 2 shell for macOS, Windows, iOS, Android: commands, local SQLite cache, plugins, mobile native plugins (Swift/Kotlin) for push notifications.
/migrations       -> sqlx migrations (order matters: plans, users, devices before dependent tables).
/config           -> plans.toml (limits, credit costs, prices), billing_policy.toml (purchase UI allowed per platform).
/docs             -> ASSUMPTIONS.md, ARCHITECTURE.md, API.md, SYNC.md, RELEASE.md.

# DATABASE SCHEMA (create as migrations; add sensible indexes and FKs)

SYNC COLUMNS: every table marked `-- +sync` ALSO has these columns and a `log_change` AFTER trigger that appends to `change_log` in the same transaction:
  user_id uuid not null references users on delete cascade   (denormalized onto child tables for fast scoping and sync)
  updated_at timestamptz not null default now()               (maintained by a BEFORE `touch_updated_at` trigger)
  deleted_at timestamptz                                      (soft-delete tombstone; deletes are synced as tombstones)
IDs may be generated by clients (UUIDv7 preferred); the server accepts client-supplied ids. NOT synced to clients: chunks, embeddings, jobs, billing internals, credit_ledger, usage_events, payments (clients get balances, plan status, and payment status via API).

```sql
create extension if not exists vector;
create extension if not exists pgcrypto;

-- Billing reference data (create first)
create table plans (
  id text primary key,                 -- free | pro | power
  name text not null,
  price_idr_monthly int not null,
  price_idr_yearly int not null,
  entitlements jsonb not null          -- mirrors plans.toml for auditing
);

create table users (
  id uuid primary key default gen_random_uuid(),
  email text unique not null,
  password_hash text not null,
  timezone text not null default 'Asia/Jakarta',
  plan_id text not null default 'free' references plans,
  email_verified boolean not null default false,
  student_verified boolean not null default false,   -- verified .ac.id email
  ics_feed_token text unique,                        -- revocable secret for the ICS feed
  deleted_at timestamptz,                            -- account deletion (scheduled hard delete)
  created_at timestamptz default now()
);

-- Multi-device
create table devices (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  name text not null,                    -- e.g. "Jetta's MacBook"
  platform text not null,                -- macos | windows | ios | android | web
  app_version text,
  last_seen_at timestamptz,
  last_sync_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz default now()
);

create table auth_sessions (             -- rotating refresh tokens, one per device login
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  device_id uuid not null references devices on delete cascade,
  refresh_token_hash text not null,      -- store hash only; detect reuse of rotated tokens
  family_id uuid not null,               -- token family for reuse detection
  expires_at timestamptz not null,
  revoked_at timestamptz,
  created_at timestamptz default now()
);

create table push_tokens (
  device_id uuid primary key references devices on delete cascade,
  provider text not null,                -- apns | fcm
  token text not null,
  updated_at timestamptz default now()
);

create table change_log (                -- the sync clock: monotonic per-server seq, filtered by user
  seq bigserial primary key,
  user_id uuid not null references users on delete cascade,
  entity text not null,                  -- table name
  entity_id uuid not null,
  op text not null,                      -- upsert | delete
  origin_device_id uuid,
  changed_at timestamptz not null default now()
);
create index on change_log (user_id, seq);

create table mutation_log (              -- idempotency for offline outbox replays
  mutation_id uuid primary key,
  user_id uuid not null references users on delete cascade,
  device_id uuid references devices on delete set null,
  result jsonb not null,
  created_at timestamptz default now()
);

-- Core content
create table workspaces (                -- +sync
  id uuid primary key default gen_random_uuid(),
  name text not null,
  created_at timestamptz default now()
);

create table sources (                   -- +sync (metadata only; chunks stay on server)
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  kind text not null,              -- pdf | pptx | docx | url | text | web | syllabus
  title text not null,
  url text,
  file_key text,                   -- object storage key of the original file
  file_size bigint,
  mime text,
  status text not null default 'pending', -- pending | processing | ready | failed
  error text,
  read_only boolean not null default false, -- set when over plan limits after downgrade
  created_at timestamptz default now()
);

create table chunks (                    -- server-only, not synced
  id uuid primary key default gen_random_uuid(),
  source_id uuid not null references sources on delete cascade,
  page int,
  idx int not null,
  content text not null,
  content_hash text not null,
  tsv tsvector generated always as (to_tsvector('simple', content)) stored,
  embedding vector(768)            -- must match the configured embedder dimension
);
create index on chunks using hnsw (embedding vector_cosine_ops);
create index on chunks using gin (tsv);
create index on chunks (content_hash);

create table messages (                  -- +sync (chat history, one thread per workspace)
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  role text not null,              -- user | assistant
  content text not null,
  citations jsonb not null default '[]',  -- [{type:"chunk",chunk_id,source_id,page} | {type:"web",url,title}]
  used_web boolean not null default false,
  created_at timestamptz default now()
);

create table decks (                     -- +sync
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  source_ids uuid[] not null
);

create table flashcards (                -- +sync (ease/interval/due are DERIVED by replaying flashcard_reviews; clients treat them as read-only)
  id uuid primary key default gen_random_uuid(),
  deck_id uuid not null references decks on delete cascade,
  front text not null,
  back text not null,
  chunk_id uuid references chunks on delete set null,
  ease real not null default 2.5,
  interval_days int not null default 0,
  due_at timestamptz not null default now()
);

create table flashcard_reviews (         -- +sync, APPEND-ONLY, client-generated id
  id uuid primary key,
  flashcard_id uuid not null references flashcards on delete cascade,
  rating text not null,            -- again | hard | good | easy
  reviewed_at timestamptz not null,
  device_id uuid references devices on delete set null
);
create index on flashcard_reviews (flashcard_id, reviewed_at);

create table quizzes (                   -- +sync
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  difficulty text
);

create table quiz_questions (            -- +sync
  id uuid primary key default gen_random_uuid(),
  quiz_id uuid not null references quizzes on delete cascade,
  kind text not null,              -- mcq | short
  prompt text not null,
  options jsonb,
  answer text not null,
  explanation text,
  chunk_id uuid references chunks on delete set null
);

create table quiz_attempts (             -- +sync, APPEND-ONLY, client-generated id
  id uuid primary key,
  quiz_id uuid not null references quizzes on delete cascade,
  question_id uuid not null references quiz_questions on delete cascade,
  user_answer text,
  correct boolean,
  answered_at timestamptz default now()
);

create table reports (                   -- +sync
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  outline jsonb,
  body_md text,
  created_at timestamptz default now()
);

-- Study OS
create table calendar_connections (      -- server-side only; devices never hold Google tokens
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  provider text not null,                -- google | ics
  account_email text,
  refresh_token_enc bytea,               -- encrypted (aes-gcm)
  sync_token text,
  ics_url text,
  last_synced_at timestamptz
);

create table events (                    -- +sync
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid references workspaces on delete set null,
  kind text not null,                    -- class | exam | study_block | deadline | external
  title text not null,
  starts_at timestamptz not null,
  ends_at timestamptz not null,
  external_id text,
  etag text,
  connection_id uuid references calendar_connections on delete set null,
  source text not null default 'app',    -- app | google | ics
  task_id uuid
);
create index on events (user_id, starts_at);

create table tasks (                     -- +sync
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid references workspaces on delete set null,
  parent_id uuid references tasks on delete cascade,
  title text not null,
  notes text,
  due_at timestamptz,
  estimate_min int,
  priority int not null default 2,       -- 1 high, 2 normal, 3 low
  status text not null default 'todo',   -- todo | doing | done
  created_at timestamptz default now()
);

create table goals (                     -- +sync (exams/big targets that drive the planner)
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  title text not null,
  target_date timestamptz not null,
  source_ids uuid[]
);

create table study_sessions (            -- +sync, APPEND-ONLY, client-generated id
  id uuid primary key,
  workspace_id uuid references workspaces on delete set null,
  task_id uuid references tasks on delete set null,
  started_at timestamptz not null,
  duration_min int not null
);

create table user_settings (             -- synced; only updated_at (no tombstones)
  user_id uuid primary key references users on delete cascade,
  study_start_hour int not null default 8,
  study_end_hour int not null default 22,
  max_study_min_per_day int not null default 240,
  block_len_min int not null default 50,
  break_min int not null default 10,
  reminder_channels jsonb not null default '["web"]',
  updated_at timestamptz not null default now()
);

create table pending_actions (           -- agent proposals awaiting confirmation; surfaced on every device
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  tool text not null,
  args jsonb not null,
  status text not null default 'pending',-- pending | confirmed | rejected | expired
  created_at timestamptz default now()
);

-- Billing and usage (QRIS)
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
-- Prepaid periods: no auto-renew. Rule: at most ONE active subscription per user.

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

create table webhook_events (            -- idempotency + audit
  id uuid primary key default gen_random_uuid(),
  provider text not null,
  event_id text not null,
  payload jsonb not null,
  processed_at timestamptz,
  unique (provider, event_id)
);

create table credit_ledger (             -- balance = sum(delta); spend inside a transaction
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  delta int not null,                    -- negative = spend, positive = grant/top-up
  bucket text not null default 'monthly',-- monthly | purchased (purchased never expires)
  reason text not null,                  -- monthly_grant | credit_pack | usage:<action> | refund | admin
  ref_id uuid,
  created_at timestamptz default now()
);
create index on credit_ledger (user_id, created_at);

create table usage_events (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users on delete cascade,
  action text not null,
  credits int not null default 0,
  provider_cost_usd numeric(10,6),
  tokens_in int, tokens_out int,
  byok boolean not null default false,
  created_at timestamptz default now()
);
create index on usage_events (user_id, created_at);

create table discount_codes (
  code text primary key,
  percent_off int not null,
  valid_until timestamptz,
  max_redemptions int,
  redeemed int not null default 0
);

create table user_api_keys (             -- BYOK (power plan)
  user_id uuid primary key references users on delete cascade,
  provider text not null,
  key_enc bytea not null,                -- aes-gcm encrypted
  created_at timestamptz default now()
);

create table jobs (
  id uuid primary key default gen_random_uuid(),
  kind text not null,                    -- ingest_source | generate_* | sync_calendar | send_reminder | send_push | plan_goal | monthly_grant | renewal_reminder | poll_payment | compact_change_log
  payload jsonb not null,
  status text not null default 'queued', -- queued | running | done | failed
  attempts int not null default 0,
  error text,
  run_at timestamptz not null default now(),
  created_at timestamptz default now()
);
create index on jobs (status, run_at);
```

# CORE BEHAVIOR: SOURCES, CHAT, STUDY TOOLS

## Ingestion
- Accept PDF, PPTX, DOCX, pasted text, and URL (fetch + readability-style cleanup). Extract text with page/slide numbers. Store the original file in object storage (counts against the plan's storage quota).
- Chunk ~500 tokens with ~50 overlap, avoid splitting mid-sentence. Skip re-embedding chunks whose content_hash already exists. Embed in batches. Update source.status, expose progress, store readable errors on failure.
- Clients can open the original file via a short-lived signed URL, and resolve a citation to its chunk text and page via `GET /chunks/:id`.

## Retrieval and chat
- Hybrid retrieval: vector top-k + keyword (tsvector) top-k merged with reciprocal rank fusion, optionally limited to user-selected sources.
- Prompt with chunks labeled by ID; require citations by chunk ID; VALIDATE citations after generation and drop IDs not in the provided context. If the sources don't contain the answer, say so instead of guessing.
- Stream tokens over SSE; send citations as a final structured event; persist messages (they then sync to the user's other devices).
- Per-chat toggle `use_web`: run SearchProvider, clean/truncate results, include as separate "web" context with URL citations. Provide "save to workspace" to turn a web result into a source (kind=web).

## Flashcards
- Generate from selected sources. Strict JSON validated with serde; retry once on parse failure; each card links to its source chunk.
- Reviews are APPEND-ONLY events (`flashcard_reviews`). The server derives each card's ease, interval_days, and due_at by replaying that card's reviews in `reviewed_at` order with SM-2 (deterministic, unit-tested). This makes reviews from multiple devices conflict-free. Study endpoint returns due cards; clients may also compute due cards locally from synced state.

## Quiz
- MCQ and short answer, configurable count and difficulty, strict JSON, explanation + linked chunk per question. Record attempts (append-only); return score and weak-topic summary grouped by source/page.

## Reports
- Step 1: editable outline. Step 2: generate each section separately with citations. Store Markdown; export .md first (PDF/DOCX later, gated by plan).

# CORE BEHAVIOR: STUDY OS

## Tasks
- CRUD with subtasks, priority, due date, estimate, workspace link. Quick-capture endpoint accepting free text ("finish crypto assignment friday") parsed by the LLM into a task draft.
- "Break down": LLM proposes subtasks with time estimates for a big task; user confirms before saving.

## Calendar integration
- Google Calendar stays SERVER-SIDE: one connection per account works on every device. OAuth2 with offline access, minimal scopes, PKCE + state parameter; refresh tokens encrypted (aes-gcm). Incremental sync via syncToken; on 410 do a full resync. Optionally register watch channels (webhook) with polling via jobs as fallback. Maintain our own `events` rows with external_id + etag. Conflict policy: Google wins for events we didn't create; our study blocks go to a dedicated "Study Plan" calendar so they're easy to hide or delete.
- Native OAuth flow: open the system browser (never an embedded webview), return via deep link / universal link / app link, validate state and PKCE.
- ICS feed: `GET /cal/:token.ics` outputs tasks with due dates, study blocks, and exams. Token is per-user, revocable, regenerable.
- ICS import/subscribe: user adds an external ICS URL (e.g. academic calendar); poll periodically; treat events as busy time. Support recurring events (rrule) and timezones correctly.

## Syllabus import
- User uploads a syllabus/course schedule (source kind=syllabus). LLM extracts structured JSON: class meetings (recurrence), assignment deadlines, exam dates, each with a confidence/evidence field. Show a REVIEW screen where the user edits/accepts items; only then create events, tasks, and goals.

## Planner (deterministic, in crates/core, heavily unit-tested)
- Input: goals (exam dates), sources/topics per goal, quiz weakness data, due flashcards, existing events (busy time), tasks with estimates, user_settings.
- Algorithm: work backward from each goal date; compute remaining effort per topic (weight weak topics higher); place study blocks into free slots within study hours, respecting max minutes/day, block length, breaks, and never overlapping busy time; spread evenly with spaced-repetition-style revisits before the exam; prefer earlier slots for higher priority; leave a buffer before deadlines.
- The LLM may only help estimate effort and break down tasks. Slot placement is plain Rust: deterministic and fast. Planner runs on the server; the result syncs to all devices as events.
- Output a PROPOSED plan the user previews, tweaks, and accepts (then write events + optionally push to Google). Replan endpoint reschedules unfinished blocks when things change.
- Workload balancer (Power): detect overloaded days/weeks and propose moves or splits. Weak-topic auto review blocks (Power).

## Agentic assistant
- Chat gets tool calling. Tools: `create_task`, `update_task`, `list_tasks`, `create_event`, `find_free_slots`, `schedule_study_block`, `create_flashcards`, `create_quiz`, `get_due_flashcards`, `get_upcoming_deadlines`.
- READ tools execute immediately. WRITE tools never execute directly: they create a `pending_actions` row and stream a confirmation card to the UI (human-in-the-loop). Only an explicit user confirm executes it. Expire pending actions after a timeout. Pending actions are visible and confirmable from any signed-in device.
- Treat retrieved document text and web content as untrusted data: it must never trigger tool calls on its own (prompt-injection defense).

## Focus timer, reminders, dashboard
- Pomodoro-style timer logging to study_sessions (optionally tied to a task). Timer state survives app restarts.
- Reminders via server jobs: push notifications (APNs/FCM) to mobile devices, native notifications on desktop, in-app, email, Telegram/Discord optional (Power); for upcoming events, due tasks, due flashcards. Respect timezone and quiet hours. Avoid duplicate notifications across devices (deliver to the most recently active device, or dedupe by reminder id). Mobile and desktop clients may also schedule local notifications from synced events/tasks so reminders work offline.
- Dashboard: time per course, streaks, quiz mastery per topic, weak-topic list that can auto-create review blocks (Power).

# MULTI-DEVICE ACCOUNT AND SYNC (document in docs/SYNC.md)

## Auth across devices
- Login returns a short-lived access token (e.g. 15 min) and a rotating refresh token bound to a `device`. Store the refresh token only in the OS keychain/keystore on native clients. Refresh-token reuse detection: if a rotated token is reused, revoke the whole token family and require login.
- Devices screen: list devices (name, platform, last seen), rename, sign out a device remotely (revoke), sign out all. Revoked devices get 401 and wipe their local cache on next contact. Enforce `devices_max` per plan (error includes the option to revoke an old device).

## Sync protocol (types in crates/sync, shared by server and client)
- Pull: `GET /sync/pull?cursor=<seq>&limit=<n>` returns changes for the user since `seq` as `[{seq, entity, id, op, data?}]` plus `next_cursor` and `has_more`. Initial sync starts at cursor 0 and pages through in dependency-safe order. If the cursor is older than retained history, return `410 resync_required` and the client does a full resync.
- Push: `POST /sync/push` with `device_id` and a list of mutations `{mutation_id (uuid), entity, op, id, payload, client_ts}` from the client's outbox. The server validates each mutation against entitlements and ownership, applies it in a transaction, records it in `mutation_log` (IDEMPOTENT: replaying a mutation_id returns the stored result), and returns per-mutation results (applied / rejected with reason / conflict).
- Realtime hint: `GET /sync/stream` (SSE) emits `{seq}` when the user's change_log advances so other online devices pull immediately. Mobile apps in the background rely on push notifications (silent or visible) and pull on foreground.
- Clients write locally first (SQLite + outbox), update the UI immediately, then push; retry with exponential backoff; show sync status (synced / syncing / offline / error) in the UI.
- Compaction: a job trims change_log older than the retention window (default 90 days) but keeps tombstones at least that long; document the resync fallback.

## Conflict rules
- Append-only entities (flashcard_reviews, quiz_attempts, study_sessions, messages): just insert; ids are client-generated, duplicates ignored.
- Flashcard scheduling state is server-derived from reviews (no conflicts possible).
- Mutable entities (tasks, events, goals, decks, reports, workspaces, settings): record-level last-write-wins using server receive time, with field-level merge for tasks and events when the changed fields don't overlap. A delete (tombstone) beats a concurrent edit unless the user explicitly restores. Report/long-text edits on two devices: keep the latest and store the losing version as a recoverable copy.
- Server-owned entities (sources/ingestion status, subscription, credits, planner output) are read-only for clients except via their dedicated endpoints.
- Document these rules and test them (see quality bar).

## What is cached locally on native clients
Synced entities above (not chunks/embeddings), plus downloaded original files on demand (user can "make available offline" per source). Provide "Reset local data and resync" in settings. Optional encrypted local DB (SQLCipher) behind a config flag.

# SUBSCRIPTIONS, PLANS, ENTITLEMENTS

## Plans (config/plans.toml, loaded at startup; defaults below, tune after measuring real cost)
Prices (IDR): free=0; pro monthly=10000, yearly=100000; power monthly=25000, yearly=250000. Optional student-verified promo prices configurable per plan. Account sync works on ALL plans; only device count and storage differ.

free:  workspaces=2, sources_total=20, max_file_mb=20, storage_mb=200, devices_max=2, credits_per_month=60, chat_msgs_per_day=20, web_searches_per_day=3, ai_flashcards_per_month=30, quizzes_per_month=2, reports_per_month=1, export_formats=[md], task_ai_breakdown=false, quick_capture=false, google_calendar=false (ICS feed + import only), syllabus_imports_total=1, planner="manual", agent_actions=false, reminder_channels=[web,push], model="cheap default", byok=false

pro:   workspaces=unlimited, sources_total=150, max_file_mb=50, storage_mb=2000, devices_max=5, credits_per_month=500, web_searches_per_day=30, export_formats=[md,pdf,docx], task_ai_breakdown=true, quick_capture=true, google_calendar=true (1 account, two-way), syllabus_imports=unlimited, planner="auto", agent_actions=true, reminder_channels=[web,push,email], model="cheap default", byok=false

power: everything in pro plus sources_total=500, max_file_mb=100, storage_mb=10000, devices_max=10, credits_per_month=2000, web_searches_per_day=100, multiple Google accounts, planner="auto+balancer+weakness_blocks", reminder_channels add telegram/discord, advanced analytics + CSV/JSON export, better models selectable, byok=true (user supplies their own LLM API key, encrypted at rest; BYOK usage consumes no credits and has no credit limit)

## Credit costs (config, tunable)
chat_message=1, web_sear, flashcard_batch(10)=4, quiz(10 questions)=6, report_outline=3, report_section=6, task_breakdown=2, syllabus_import=8, plan_generation=2, agent_turn=2. Long-document operations scale with input tokens (simple multiplier).

## Entitlements engine (crates/core)
- `Entitlements` built from plan + subscription status. One function `check(user, Feature) -> Result<(), EntitlementError>` used by EVERY handler (including sync push mutations); no limits hardcoded anywhere else. Structured errors (`plan_required`, `limit_reached`, `insufficient_credits`) include the minimum plan needed so the UI can show a precise upgrade prompt (on mobile builds, the prompt text follows billing_policy: no purchase CTA).
- Entitlements are per ACCOUNT, never per device: paying on the web or desktop unlocks every device as soon as the payment is verified.
- Credit accounting: monthly grant job at period start (no rollover by default, configurable). Spend inside a DB transaction against `credit_ledger` (no negative balance, safe under concurrency); spend monthly bucket first, then purchased. Purchased credit-pack credits never expire.
- Daily/monthly counters (chat msgs, web searches, quizzes, reports) via usage_events or a counters table; reset by timezone-aware jobs.
- Downgrade/expiry: never delete user data. After a 3-day grace period (configurable), over-limit workspaces/sources become read-only (viewable, exportable); features above the plan are locked with clear messaging; extra devices beyond the new limit are flagged read-only until the user revokes some.
- Margin safety: log provider_cost_usd per action in usage_events; admin margin report (revenue minus provider cost minus gateway fees, per plan and per user); a hard monthly provider-cost ceiling per user per plan (config) that falls back to the cheap model or pauses expensive actions with a clear message.

## Payments: QRIS via my gateway (trait `PaymentProvider`: create_invoice, get_status, parse_and_verify_webhook, refund_note)
- QRIS is a one-time payment, so billing is PREPAID PERIODS only: 30 days (monthly) or 365 days (yearly). No auto-renew, no stored payment methods. Every renewal or credit pack is a new QRIS invoice.
- Checkout flow:
  1. `POST /billing/checkout` {plan_id, period} or {credit_pack}, optional discount_code. Server computes the amount (plan price, discount, student promo), creates a `payments` row (status=pending, unique `order_id`, `expires_at`, default 15-30 minutes, configurable) and calls the gateway to create a QRIS invoice.
  2. Return the QR payload/image URL and expiry. Client renders the QR with a countdown (and an "open e-wallet" deep link if the gateway provides one), then waits via `GET /billing/payments/:id` polling and/or the sync SSE hint.
  3. Gateway webhook marks the payment paid. The handler verifies the signature/callback token, checks that order_id exists and belongs to a pending payment, and that the paid amount EQUALS the expected `amount_idr` and currency is IDR; then, in ONE transaction and idempotently (`webhook_events` unique key): marks paid, activates/extends the subscription, grants monthly credits (or adds the purchased credits), records the gateway fee, and notifies the user (push/in-app: "Pro is active").
  4. Fallback: a `poll_payment` job queries the gateway's status API for pending payments until expiry, in case the webhook is delayed or lost. A reconciliation job compares recent gateway transactions with our `payments` table and flags mismatches.
- Edge cases to handle and test: duplicate webhooks, amount mismatch or over/under-payment (mark `review`, never activate automatically, alert admin), payment confirmed after invoice expiry (default: honor it if the gateway confirms it was paid; configurable), expired/failed invoices, user paying twice for the same period, webhook arriving before the client polls.
- Period rules: paying while a subscription is active EXTENDS it starting at `current_period_end`; paying after expiry starts at payment time. Upgrade (pro -> power) switches the plan immediately and charges the prorated price difference for the remaining days (configurable); downgrade takes effect at period end.
- Renewal reminders at 7, 3, and 1 day before expiry and on expiry day (push, email, in-app on web/desktop; on mobile builds reminders only state the plan is ending, with no purchase link, per billing_policy). 3-day grace after expiry, then downgrade to free behavior (data kept).
- Support: monthly/yearly toggle (default yearly at checkout, since fixed per-transaction costs hurt at low prices), one-time credit packs, discount codes, student discount (auto-apply when `.ac.id` email is verified via email link). Admin tools: manually mark a payment paid or refunded, extend a subscription, grant credits. Refunds are processed outside the app via the gateway; record them in `payments`. Require verified email before checkout.
- Never trust client-reported payment status. Never store card or wallet data. Gateway secrets only in env. Restrict the webhook endpoint (signature verification, plus IP allowlist if the gateway offers one), and rate-limit checkout.
- config/billing_policy.toml: per platform {web, macos, windows, ios, android} flags: `allow_purchase_ui`, `show_prices`, `allow_purchase_cta_in_locked_messages`. Defaults: true for web/macos/windows; false for ios/android. The UI reads this on every platform; mobile builds must contain no checkout screen, no purchase buttons, and no links or text directing users to buy elsewhere.

# AUTH AND SECURITY
- Email+password signup/login, argon2, email verification, per-device sessions (see sync section). Every query scoped to the authenticated user (no cross-tenant access), including sync pull/push and signed file URLs. Validate and sanitize all inputs; restrict file types and sizes per plan; never log secrets or tokens; encrypt OAuth refresh tokens and BYOK keys; rate-limit auth, sync, checkout, and agent endpoints. Request minimal Google scopes and document that Google app verification is required beyond test users.
- Tauri hardening: strict CSP, minimal capabilities/permissions per window, no remote code loading, validate every IPC command input, tokens only in keychain/keystore, validate deep-link payloads (state/PKCE), HTTPS only.
- Account deletion: in-app `Delete account` (also required by app store rules) that deletes or anonymizes all user data, revokes sessions, and cancels pending jobs. There are no recurring charges to cancel; document what happens to remaining paid time. Also `Export my data` (JSON + Markdown + ICS).
- Keep ToS/privacy placeholder pages and note that tax/PPN handling and store privacy-label disclosures must be confirmed before launch.

# API (document in docs/API.md)
Auth: POST /auth/signup, /auth/login, /auth/refresh, /auth/logout, /auth/logout-all, /auth/verify-email
Devices: GET /devices, PATCH /devices/:id, DELETE /devices/:id (revoke), PUT /devices/:id/push-token
Sync: GET /sync/pull, POST /sync/push, GET /sync/stream (SSE)
Workspaces: CRUD /workspaces
Sources: POST /workspaces/:id/sources (multipart or JSON), GET list, DELETE, GET /sources/:id/status, GET /sources/:id/file (signed URL), GET /chunks/:id
Chat: GET /workspaces/:id/messages, POST /workspaces/:id/chat (SSE; body: message, source_ids?, use_web)
Flashcards: POST /workspaces/:id/decks, GET /decks/:id, GET /decks/:id/due, POST /flashcards/:id/review (writes a flashcard_reviews row)
Quiz: POST /workspaces/:id/quizzes, GET /quizzes/:id, POST /quizzes/:id/attempts, GET /quizzes/:id/results
Reports: POST /workspaces/:id/reports/outline, POST /reports/:id/generate, PATCH /reports/:id, GET /reports/:id/export?format=
Tasks: CRUD /tasks, POST /tasks/quick-capture, POST /tasks/:id/breakdown
Events: CRUD /events, GET /events?from=&to=
Goals/Planner: CRUD /goals, POST /goals/:id/plan (proposal), POST /plans/accept, POST /plan/replan
Syllabus: POST /workspaces/:id/syllabus, GET /syllabus/:id/draft, POST /syllabus/:id/confirm
Calendar: GET /calendar/google/connect, GET /calendar/google/callback, POST /calendar/ics, DELETE /calendar/connections/:id, POST /calendar/sync, POST /calendar/ics-token/regenerate, GET /cal/:token.ics
Assistant actions: GET /actions/pending, POST /actions/:id/confirm, POST /actions/:id/reject
Study: POST /sessions, GET /dashboard
Settings: GET/PUT /settings, PUT/DELETE /settings/api-key (BYOK, power only)
Account: DELETE /account, GET /account/export
Billing: GET /plans, GET /billing/me (plan, period end, credits, usage; works on all platforms), POST /billing/checkout (blocked by policy on ios/android), GET /billing/payments/:id, GET /billing/invoices, POST /webhooks/qris
Admin: users, plan overrides, manual payment mark/refund, manual credit grants, margin report

# FRONTEND (one adaptive UI for all platforms)
Screens:
1. Auth pages + email verification.
2. Workspace list.
3. Workspace view: Sources | Chat (streaming, clickable citations that open the source at the right page, web toggle, confirmation cards for agent actions) | Studio (Flashcards, Quiz, Report).
4. Study mode: flashcard review; quiz-taking with results and explanations.
5. Report editor: outline editing, generate, Markdown preview, export.
6. TODAY view (default home): today's tasks, study blocks, due flashcards, upcoming exams, quick-capture input, focus timer.
7. Calendar page: week/month view of events + study blocks, connect Google, add ICS URL, copy ICS feed link, plan preview/accept flow with drag-to-adjust.
8. Tasks page: list/board, subtasks, break-down button.
9. Syllabus review screen.
10. Dashboard: time per course, streaks, weak topics.
11. Settings: timezone, study hours, daily limits, reminder channels, connections, BYOK key (Power).
12. Devices and sync: device list with sign-out, last sync time, sync status, "Reset local data and resync".
13. Pricing page + billing page for web/macOS/Windows: plan cards, monthly/yearly toggle (default yearly), student discount banner, QRIS checkout screen (QR, countdown, status), payment history, renewal date, credit meter, buy credits. On iOS/Android: a read-only "Plan and usage" screen (current plan, renewal date, credit meter) with no purchase UI.
14. Inline upgrade prompts exactly where a limit is hit (web/desktop). Mobile shows a neutral locked-feature message with no purchase CTA. Read-only banners after a downgrade. Credit meter in the header.
15. Admin page (web only, admin only): users, plans, manual payment actions, grants/refunds, margin report, payments flagged `review`.
Adaptive layout:
- Desktop (macOS/Windows, wide web): three-panel workspace, sidebar navigation, keyboard shortcuts (Cmd/Ctrl+K command palette), system tray with global quick-capture shortcut, drag-and-drop file upload, native menus, native notifications.
- Phone (iOS/Android): bottom tab bar (Today, Workspaces, Calendar, Tasks, More); the workspace view becomes stacked navigation with Sources / Chat / Studio tabs; safe-area insets, keyboard-aware chat input, 44px minimum touch targets, pull-to-refresh, swipe gestures on flashcards, share-sheet "Import to Study OS" for PDFs and links (stretch goal), haptics where sensible.
- Tablet: two-panel layout.
- All platforms: dark/light follows OS, accessible (screen readers: VoiceOver/TalkBack, focus states, contrast, dynamic type/scaling), skeletons and progress states, never a blank screen, clear offline banner and sync indicator.

# PHASES (stop and summarize after each; wait for me to say "continue")
Phase 0: Scaffold: Cargo workspace, React+Vite app, Tauri 2 shell building for macOS and Windows, docker-compose (Postgres+pgvector, MinIO), migrations, config (plans.toml + billing_policy.toml loaders), health endpoint, CI (fmt, clippy, test, and build matrix for web + macOS + Windows). Create the `platform` adapter skeleton.
Phase 1: Auth with devices + rotating refresh sessions + email verification; sync-ready schema (sync columns, change_log + triggers) from the start; workspaces, source upload to object storage, ingestion worker (PDF + text), chunking, embeddings, status polling. Early native prototype: keychain token storage and deep-link round trip.
Phase 2: Hybrid retrieval, streaming chat with validated citations, workspace + chat UI (desktop-first but responsive).
Phase 3: Web search toggle + save-web-result-as-source.
Phase 4: Flashcards with append-only reviews + server-side SM-2 replay, study UI.
Phase 5: Quiz generation, attempts, results.
Phase 6: Reports (outline -> sections -> export).
Phase 7: Entitlements engine (incl. devices_max, storage_mb), credit ledger, usage_events with provider cost, monthly grant job, downgrade/read-only behavior, rate limits, PPTX/DOCX/URL ingestion, polish. Route ALL earlier features through the entitlements engine. Plans switchable manually via an admin endpoint (no payments yet).
Phase 8: Sync engine: protocol in crates/sync, /sync/pull, /sync/push, /sync/stream, mutation idempotency, local SQLite cache + outbox in src-tauri, offline reads, sync status UI, Devices screen, resync flow, change_log compaction. Convergence tests with simulated devices.
Phase 9: Tasks, quick-capture, break-down, events, Today view, settings, focus timer (all sync-native, offline-writable).
Phase 10: ICS feed + ICS import, then Google Calendar OAuth (native deep-link flow) + incremental sync, calendar UI.
Phase 11: Syllabus import with review screen, goals, deterministic planner with tests, plan preview/accept/replan, workload balancer.
Phase 12: Agentic assistant (tool calling, pending_actions confirmation across devices), reminders engine, push provider traits (APNs/FCM, desktop notifications), dashboard.
Phase 13: QRIS payments: PaymentProvider trait + `qris_gateway` implementation for my gateway, checkout + QR screen, idempotent verified webhooks, polling fallback and reconciliation jobs, period extension/upgrade rules, credit packs, discount/student codes, renewal reminders, pricing and billing pages (web/desktop), billing_policy enforcement, BYOK, admin payment tools and margin report. Use a gateway sandbox for tests.
Phase 14: Mobile: iOS and Android builds via Tauri mobile, phone/tablet adaptive UI pass, read-only plan screen with NO purchase UI, push notification plugins (APNs/FCM) with token registration, deep links/universal links/app links, secure storage, file picker and share-sheet import, local notifications, TestFlight and Play internal-testing builds. Write the mobile test checklist and the store-policy risk note in docs/RELEASE.md.
Phase 15: Release pipelines: macOS signing + notarization + universal binary, Windows code signing, Tauri updater with signed manifests, Android direct-APK option, store submission checklists (privacy labels, in-app account deletion, review notes), README and deployment notes (server Docker image).

# QUALITY BAR
- `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass at every phase; the web and desktop builds must compile in CI; mobile builds from Phase 14.
- Unit tests: chunking, RRF merge, citation validation, SM-2 replay determinism (same reviews in any arrival order -> same state), JSON-output parsing, planner (no overlap with busy time, respects daily max and study hours, deadline buffer, weak-topic weighting, timezone/DST-safe), ICS generation/parsing incl. recurrence, agent confirmation gating, entitlements (limit enforcement, plan/feature matrix generated from plans.toml, concurrent credit spend with no double spend, period rollover, downgrade read-only, device limits, BYOK bypassing credits), refresh-token rotation and reuse detection.
- Payment tests: webhook signature verification, idempotent duplicate webhooks, amount mismatch -> `review` (no activation), late payment after expiry, extension stacking vs. fresh start, prorated upgrade, polling fallback activates a payment when the webhook is missing, billing_policy hides purchase UI/endpoints on ios/android.
- Sync tests: two or three simulated devices making offline edits then syncing converge to the same state; idempotent push (replayed mutation_id); tombstone vs edit conflict; append-only merges; `410 resync_required` full-resync path; revoked device handling; no cross-tenant leakage through change_log.
- Integration tests: auth scoping, ingest -> chat flow, plan -> accept -> events, checkout -> verified webhook -> plan active on a second device, with mock LLM/embedder/calendar/payment/push providers via traits.
- No unwrap() in non-test code; typed errors mapped to HTTP status codes.
- Prompts live in dedicated versioned modules/files, not inline.
- Structured logging (tracing) with request IDs; log latency for retrieval, LLM calls, sync pull/push, sync jobs, and webhooks.

# OUT OF SCOPE FOR MVP
Apple In-App Purchase and Google Play Billing (the PaymentProvider trait must allow adding them later), auto-renewing/recurring billing, audio overviews, lecture recording/transcription, real-time collaboration/study groups, Mac App Store and Microsoft Store distribution, Linux builds, widgets/watch apps, camera/OCR scanning, on-device LLMs, offline chat/generation/uploads, multiple chat threads per workspace, non-Google OAuth login, campus LMS integrations, browser extension, Stripe/international payments.

Begin with Phase 0 now. Show the planned file tree first, then implement.
