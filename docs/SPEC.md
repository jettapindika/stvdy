# Stvdy — Product & Technical Spec (source of record)

This file is the requirement document of record for the project. Every phase plan in `docs/superpowers/plans/` derives from it. Where a plan deviates, the deviation is recorded in `docs/ASSUMPTIONS.md`.

---

# ROLE
You are a senior Rust + full-stack engineer and design-systems implementer. Build an MVP of "Stvdy": an AI study app combining a NotebookLM-style source-grounded research workspace with a study planner (tasks, schedule, Google Calendar, exam-aware planning, an action-taking assistant), sold through 3 subscription plans paid via QRIS, shipped as native apps for macOS, Windows, iOS, and Android (plus a web build), where one account syncs everything across all devices. Work incrementally, keep the code compiling and tested at the end of every phase, and explain key decisions briefly. Don't ask me questions unless you're truly blocked; make reasonable assumptions and list them in docs/ASSUMPTIONS.md.

# PRODUCT
Users create workspaces (one per course), upload sources, and chat with them to get answers with citations to exact pages. The AI can optionally search the web. From selected sources users generate flashcards (spaced repetition), quizzes, and reports. The app also manages tasks and schedule: it imports syllabi, syncs with Google Calendar and ICS calendars, auto-plans study blocks backward from exam dates using quiz weaknesses and due flashcards, and has an assistant that can take actions (create tasks, schedule blocks) only after user confirmation. Access is governed by 3 plans (free, pro, power) with monthly AI credits.

The app runs on macOS, Windows, iOS, Android (and web). A user signs in with ONE account on any device and sees the same data everywhere: workspaces, sources, chat history, flashcards and review progress, quizzes, reports, tasks, events, settings, and subscription status. Changes made on one device (including offline edits to tasks, events, flashcard reviews, quiz attempts, study sessions, and settings) appear on the others.

Audience: Indonesian university students (18-24), many on mid-range Android phones, studying at night, on unreliable campus Wi-Fi, under exam pressure. The product must feel calm, fast, and honest.

Billing: users pay with QRIS through a payment gateway I already have (configured via env). There is NO Apple App Store / Google Play billing in this MVP. Subscriptions are prepaid periods, not auto-renewing.

Brand: the app is called **Stvdy** (pronounced "study"; the V stands in for the U in the wordmark). The in-app assistant is also called "Stvdy" (e.g. "Ask Stvdy"). Use "Stvdy" only as the brand name, never as a substitute for the word "study" in normal UI text. Voice: a sharp, kind senior student (kakak tingkat): short plain sentences, honest about uncertainty, never hype.

Differentiators (keep in mind in every design choice):
1. One workspace holds many sources with high limits; retrieval works across sources (no isolated notebooks).
2. Model-agnostic: LLM, embedding, search, and payment providers all sit behind traits.
3. Everything is exportable (Markdown/JSON, ICS). Never lock user data in.
4. Fast and streaming: LLM responses stream via SSE; ingestion and generation are async with visible progress. Native shell and local cache make the UI feel instant.
5. Planner and content feed each other (quiz weaknesses -> review blocks; syllabus -> calendar + tasks).
6. Honest by design: citations are validated; numbers shown to users are only ones the app can truly compute; AI limits are stated plainly.
7. Priced for Indonesian students (IDR), with margins protected by credits and cost tracking.
8. One account, every device: seamless sync, offline-tolerant, per-device session management.

# PLATFORMS AND ARCHITECTURE
- Server (Rust/Axum + Postgres) is the single source of truth. All heavy work (ingestion, embeddings, retrieval, LLM calls, planner, Google Calendar sync, billing) runs on the server. Clients never call LLM, search, or Google APIs directly (exception: BYOK keys are sent to the server per request, never stored on a device unencrypted).
- Clients: ONE shared UI codebase (React + Vite + TypeScript + Tailwind, SPA) packaged by Tauri 2 for macOS, Windows, iOS, and Android, and also deployed as a web app (used for admin, checkout, and as a fallback).
- Native shell (src-tauri, Rust) contains: the sync engine (crate `sync`), a local SQLite cache + outbox, secure token storage (OS keychain/keystore), notifications, deep links, file access, and platform plugins. Web build is online-first (no local DB; in-memory cache only).
- A small TypeScript `platform` adapter interface hides platform differences (secure storage, notifications, file picker/share target, deep links, open-external, updater, global shortcut, haptics, shortcut labels such as ⌘ vs Ctrl) with implementations for web, desktop, and mobile. UI code never calls Tauri APIs directly outside this adapter.
- Prototype the riskiest native pieces EARLY (keychain storage, deep-link OAuth callback, push notifications on iOS/Android, file picker/share-target on iOS) and record results in docs/ARCHITECTURE.md. If Tauri mobile lacks a required capability, write a thin Tauri plugin in Swift/Kotlin. Do not switch frameworks without asking me.
- Offline scope: native clients can READ all synced data offline and WRITE offline only for: tasks, events, flashcard reviews (including undo), quiz attempts, study sessions, settings. Chat, generation, web search, uploads/ingestion, checkout, and calendar connect require connectivity (show a clear offline state with a one-line reason; do not hide the controls).
- ALL runtime assets are bundled: fonts, icons, images, and the PDF viewer worker. No CDN scripts, stylesheets, fonts, or images at runtime (offline-first and strict CSP).
- Distribution: desktop = direct download (signed + notarized macOS universal build, signed Windows installer) with Tauri updater and signed update manifests. Mobile = TestFlight/App Store and Play internal testing/Google Play; an Android direct-APK download from my website is an allowed alternative. Mac App Store / Microsoft Store are out of scope.
- Purchase policy for mobile: because payment is QRIS only, the iOS and Android apps DO NOT sell anything: no checkout, no purchase buttons, no links or instructions pointing to a website purchase, no prices, no upgrade prompts. Users buy on the web or desktop app; the plan then syncs to their account on every device. On mobile, locked features show a neutral message ("Not available on your plan") with no call-to-action. Controlled by config/billing_policy.toml (default: purchase UI enabled on web/macos/windows, disabled on ios/android). Document the app-store review risk of this approach (stores may require their own billing for digital subscriptions in many regions) in docs/RELEASE.md, and keep the PaymentProvider trait extensible so store billing could be added later.

# DESIGN SYSTEM AND UI RULES (non-negotiable)
Visual reference: /design contains HTML exports from Google Stitch (mobile + desktop, light + dark). They are REFERENCE ONLY for layout and look. Re-implement everything as React components using semantic tokens. Do NOT copy: CDN scripts/fonts/images, hard-coded hex values, inline Tailwind configs, alert(), fixed-height hacks, fake data, or the placeholder name "Synapse AI". The app name is Stvdy.

## Tokens (CSS variables, light/dark; wire into the Tailwind theme; no raw hex in components)
role              | light                          | dark
bg                | #FAF8FF                        | #0B0F19
surface           | #FFFFFF                        | #111827
surface-raised    | #F2F3FF                        | #1A2335
border            | #DDE0F0                        | #263246
text              | #131B2E                        | #F1F5F9
text-muted        | #464555                        | #A3B1C6
text-subtle       | #6B6A7C                        | #8493AA
primary (fill)    | #4F46E5 (white label)          | #4F46E5 (white label)
primary-text/icon | #3525CD                        | #A5B4FC
primary-tint      | #E2DFFF                        | #1E1B4B
source (citations)| text #006591, tint #C9E6FF     | text #7DD3FC, tint #0C2A3F
success           | text #005338, tint #D5F5E8     | text #4EDEA3, tint #0B2A1F
warning           | text #92400E, tint #FEF3C7     | text #FBBF24, tint #3A2A08
danger            | text #BA1A1A, tint #FFDAD6     | text #FDA4AF, tint #3F1420
Semantic usage: indigo = actions/brand/selection; sky = sources and citations ONLY; emerald = done/mastered; amber = due soon/warning; rose = Again/overdue/destructive/error. Course colors: 6 categorical hues (indigo, sky, emerald, amber, rose, violet) used as a 4px marker + tinted chip + initial/icon, never as the only signal. Add a CI test that computes contrast for every documented text/background token pair (>= 4.5:1 text, >= 3:1 large text and UI icons/borders) and fails the build on a miss.

## Type, shape, motion
- Fonts bundled locally: Plus Jakarta Sans (headings 600/700), Inter (UI/body), JetBrains Mono (page refs, timestamps, hotkeys only).
- Scale (px/line-height): Display 32/40, H1 24/32, H2 20/28, H3 16/24, Body 16/24 on mobile and 14/22 on desktop, Small 13/18, Caption 12/16. NOTHING below 12px. Mobile inputs are 16px (prevents iOS zoom). Layouts must survive 200% text scaling and Indonesian strings (~25% longer than English).
- 4pt spacing grid. Radius: 8 controls, 12 cards, 16 sheets/modals, 999 chips. 1px borders in both themes; shadows only for floating layers.
- Motion 120/200/300 ms ease-out. Respect prefers-reduced-motion (card flip becomes a crossfade). No looping animations except loading and active-recording states.
- One outline icon set (Material Symbols Rounded outlined subset or an equivalent bundled SVG set), 20px (24px in tab bars). Use an icon, not an emoji, for the streak.

## Accessibility (WCAG 2.2 AA)
Focus ring on every interactive element (2px primary ring + 2px offset, both themes; never remove outlines without replacing them); tap targets >= 44px on mobile and >= 32px with 8px gaps on desktop; never color-only meaning (add icon/text/pattern); labels on all inputs; aria-live for toasts and streaming answers; correct heading hierarchy; accessible names for icon buttons; charts and heatmaps have text equivalents; pinch-zoom is allowed (no `user-scalable=no`, no `maximum-scale`); keyboard-navigable everywhere on desktop. Add automated a11y checks (axe) to CI for the component gallery and key screens.

## Honesty rules for UI copy and data
- NEVER show or claim: "zero hallucination", "100% grounded", "verified", match percentages, confidence scores, latency numbers, predicted grades, retention projections, "cognitive"/"neuro" scores, or fake references/DOIs.
- Show only numbers the app computes: cards due, accuracy, topic coverage, minutes planned, streak, credits left.
- "Prep progress" for an exam shows its true components: topic coverage (share of the exam's topics/sources that have at least one review or quiz attempt), quiz accuracy (last 14 days, or "no quizzes yet"), and cards due. No composite "readiness" or grade prediction.
- Under the chat composer, always show: "AI can make mistakes. Check the citations."
- The grounding toggle is labeled "Answer only from my sources" (tooltip: "Stvdy uses only your selected sources and cites them. It can still make mistakes, so check the cited page.").
- AI suggestions are labeled "Suggested ..." and nothing is applied until the user accepts. Never say "Applied" for a pending proposal.
- Use the single assistant name "Stvdy" everywhere (no "Copilot", "Synthesis Engine", etc.). Do not show a model/vendor name to users (Power users get a plain "Model" dropdown in Settings).
- Plain words, sentence case, no sci-fi jargon. Examples: "Show answer" (not "Reveal Grounded Answer"), "Review 14 cards" (not "Start Smart Queue Session"), "Due for review", "Add source", "Rebalance my week", "Build my plan", "Today's workload".

## UX rules
- One primary action per screen; progressive disclosure; skeletons instead of blank screens; every list/page has designed empty, loading, error, and offline states.
- Undo (toast, 5s) instead of confirmation dialogs for reversible actions; confirm only destructive actions.
- Flashcards: the grade buttons appear only AFTER the answer is revealed (and keys 1-4 are ignored before reveal). Each grade button shows the card's computed next interval. Cards auto-size with internal scroll (never clip). After grading, show an Undo toast. Hints are inline, never alert().
- Generative actions show their credit cost before running ("Make flashcards - 4 credits").
- Locked features: web/desktop = lock + "Available on Pro/Power" + Upgrade button; iOS/Android = lock + "Not available on your plan" with no price, no upgrade button, no external link (driven by billing_policy.toml).
- Platform shortcuts are shown by platform (⌘K on macOS, Ctrl+K elsewhere). Desktop native windows leave a 36px top drag region (macOS traffic-light inset on the left, Windows controls inset on the right).
- Streaks are gentle: no guilt copy, no loss-aversion pressure.

## i18n
English and Bahasa Indonesia from day one (i18next + react-i18next, ICU plurals, no hard-coded strings, locale files en.json and id.json, pseudo-locale test for 30% longer strings). Default from OS language; user can change it in Settings (synced). Dates/times via Intl with the user's IANA timezone (24-hour clock by default in Indonesian), IDR via Intl (e.g. "Rp 10.000"). Source content may be in either language; answers follow the user's question language.
- Store all times in UTC (`timestamptz`) plus the user's IANA timezone; the default timezone is Asia/Jakarta.

# TECH STACK (fixed)
- Backend: Rust (stable), Axum, Tokio, sqlx (Postgres), serde, reqwest, tracing, thiserror/anyhow, tower-http, argon2, JWT access tokens + rotating refresh tokens.
- Database: Postgres 16 with pgvector and tsvector (hybrid search). Object storage (S3-compatible, e.g. MinIO in dev) for original files and generated PDF previews; clients get short-lived signed URLs.
- Parsing: pdfium-render or lopdf for PDF (keep page numbers and text positions); zip/xml parsing for PPTX and DOCX. For PPTX/DOCX, the worker also renders a PDF preview (headless LibreOffice in the server image) so the viewer is uniform. Store text positions (page + bounding boxes) per chunk so a citation can highlight the exact passage.
- Embeddings: trait `Embedder` with impls: (a) `fastembed` local multilingual model, (b) configurable HTTP API. Choose via env. Embeddings and chunks stay server-side.
- LLM: trait `LlmProvider` with streaming AND tool/function calling. Implement one provider first (base URL + API key + model via env). Support a per-request override key (BYOK).
- Web search: trait `SearchProvider`, one impl first (Tavily or Brave via env).
- Calendar: `oauth2` (Google OAuth, offline access, PKCE), `icalendar` (generate ICS), `ical` (parse ICS), `chrono` + `chrono-tz`, `rrule`. Encrypt secrets at rest with `aes-gcm` using a key from env.
- Payments: trait `PaymentProvider` with ONE implementation now: `qris_gateway`, for my existing gateway. GATEWAY = <FILL IN: gateway name and link to its API docs>. Implement against that gateway's documented API (create QRIS invoice, status query, webhook/callback signature verification). Everything gateway-specific (base URL, keys, callback token/signature scheme, fee rates) comes from env/config. Keep the trait generic so other providers (including store billing) can be added later.
- Push: APNs (iOS) and FCM (Android) via a `PushProvider` trait; desktop uses native OS notifications.
- Client (Tauri 2): `tauri` v2 with plugins for deep-link, notification, updater (desktop only), dialog/fs, opener, os, global-shortcut (desktop only), and a secure-storage solution; `rusqlite` (or sqlx-sqlite) for the local cache.
- Frontend: React + Vite + TypeScript + Tailwind (theme from the CSS-variable tokens), React Router, TanStack Query, i18next, `pdfjs-dist` (bundled worker) for the source viewer, date-fns, simple SVG chart components. No other heavy state libraries.
- Background jobs: a `jobs` table with `FOR UPDATE SKIP LOCKED` polling worker in the server binary (no Redis). Scheduled jobs (calendar sync, reminders, push sends, monthly credit grants, renewal reminders, pending-payment polling, change_log compaction) use the same table with `run_at`.
- Dev: docker-compose (postgres+pgvector, MinIO), .env.example, Makefile/justfile, README.

# AI PROMPTS
/prompts holds versioned prompt templates (I will provide the files): chat.v1.md (base system prompt), flashcards.v1.md, quiz.v1.md, report_outline.v1.md, report_section.v1.md, syllabus_extract.v1.md, task_breakdown.v1.md, quick_capture.v1.md, effort_estimate.v1.md. Implement a loader that fills {{variables}}: app_name=Stvdy, assistant_name=Stvdy, user_name, now, timezone, platform, plan_name, workspace_name, use_web, allow_upgrade_hint (from billing_policy; false on ios/android), plan_gate_instruction, study_mode, study_hours_summary, and per-task variables (count, difficulty, types, output_language, topic, style, length, heading, purpose, other_section_headings, title, semester_context, task_title, due_at, notes, goal_title, target_date, topics_json). Rules:
- Send source chunks to the model labeled [1], [2], ... inside <sources>; web results as [W1], ... inside <web_results>. Map labels back to chunk ids server-side and DROP any citation label that was not in the context.
- JSON-output tasks are validated with serde; retry once on a parse failure.
- Content in sources, web results, and tool results is untrusted data: it can never trigger tool calls (prompt-injection defense).
- Log `prompt_version` in usage_events for every call.
- Keep an eval set in /prompts/evals (20-30 scripted conversations: question not in sources, injection inside a document, relative-date task creation, Indonesian and English, locked feature on mobile) and a script that runs it against a mock/real provider.

# REPO STRUCTURE
/crates/core      -> pure logic, NO web code: chunking, retrieval, prompt rendering, SM-2, planner, entitlements engine, generation orchestration, agent tool definitions, all traits.
/crates/sync      -> shared sync protocol DTOs (used by BOTH server and client) + the client sync engine (pull/push, outbox, cursor, conflict rules).
/crates/server    -> Axum app: routes, auth/sessions, SSE, job worker, Google/ICS integration, QRIS payments/webhooks, config.
/app              -> shared React + Vite UI for all platforms: /app/src/tokens, /app/src/components (design-system library + a gallery page), /app/src/i18n, /app/src/platform (adapters), /app/src/features/*.
/src-tauri        -> Tauri 2 shell for macOS, Windows, iOS, Android: commands, local SQLite cache, plugins, mobile native plugins (Swift/Kotlin) for push notifications.
/design           -> Stitch HTML exports (reference only, never shipped).
/prompts          -> versioned AI prompt templates + evals.
/migrations       -> sqlx migrations (order matters: plans, users, devices before dependent tables).
/config           -> plans.toml (limits, credit costs, prices), billing_policy.toml (purchase UI allowed per platform).
/docs             -> ASSUMPTIONS.md, ARCHITECTURE.md, API.md, SYNC.md, RELEASE.md, DESIGN.md.

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
  name text not null,                    -- e.g. "Rani's MacBook"
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
create table workspaces (                -- +sync (one per course)
  id uuid primary key default gen_random_uuid(),
  name text not null,
  color text not null default 'indigo',  -- indigo | sky | emerald | amber | rose | violet
  created_at timestamptz default now()
);

create table sources (                   -- +sync (metadata only; chunks stay on server)
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references workspaces on delete cascade,
  kind text not null,              -- pdf | pptx | docx | url | text | web | syllabus
  title text not null,
  url text,
  file_key text,                   -- object storage key of the original file
  preview_key text,                -- object storage key of the PDF rendition used by the viewer
  file_size bigint,
  mime text,
  page_count int,
  status text not null default 'pending', -- pending | processing | ready | failed
  progress int not null default 0,        -- 0-100 while processing
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
  bbox jsonb,                      -- optional list of rectangles on the page, for citation highlighting
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
  starred boolean not null default false,
  flagged boolean not null default false,
  ease real not null default 2.5,
  interval_days int not null default 0,
  due_at timestamptz not null default now()
);

create table flashcard_reviews (         -- +sync, APPEND-ONLY (a review may be deleted only as an undo, see sync rules), client-generated id
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
  outline jsonb,                   -- sections with source_labels and a "gaps" flag
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
  topics jsonb not null default '[]',    -- list of topic names the exam covers
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
  language text not null default 'en',   -- en | id
  theme text not null default 'system',  -- system | light | dark
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
  prompt_version text,
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
- Accept PDF, PPTX, DOCX, pasted text, and URL (fetch + readability-style cleanup). Extract text with page/slide numbers and text positions. Store the original file in object storage (counts against the plan's storage quota); for PPTX/DOCX also render and store a PDF preview.
- Chunk ~500 tokens with ~50 overlap, avoid splitting mid-sentence. Skip re-embedding chunks whose content_hash already exists. Embed in batches. Update source.status and progress (0-100), store readable errors. Detect scanned PDFs with no text and fail with a clear message ("This PDF looks scanned. Text recognition isn't available yet.").
- Clients open the preview/original via a short-lived signed URL, and resolve a citation to its chunk text, page, and highlight rectangles via `GET /chunks/:id`.

## Retrieval and chat
- Hybrid retrieval: vector top-k + keyword (tsvector) top-k merged with reciprocal rank fusion, optionally limited to user-selected sources.
- tsvector uses the Postgres `simple` config and embeddings are multilingual, so Indonesian source content and Indonesian queries retrieve correctly without a language-specific stemmer.
- Prompting follows /prompts/chat.v1.md: chunks labeled by number, citations required, VALIDATE citations after generation (drop labels not in context). If the sources don't contain the answer, say so and offer "Search the web" (if enabled) or "Answer from general knowledge" (clearly labeled, no citations).
- Stream tokens over SSE; send citations as a final structured event; persist messages (they then sync to the user's other devices). The user can stop a stream.
- Per-chat toggles: "Answer only from my sources" (default on) and "Search the web". When web is on: run SearchProvider, clean/truncate results, include as separate context with [W#] citations. Provide "save to workspace" to turn a web result into a source (kind=web).
- UI behavior: citation chips [1] open a popover (desktop) or bottom sheet (mobile) with source title, page, snippet, and "Open page"; clicking opens the source in the viewer scrolled to the page with the passage highlighted and labeled "Cited". Under each answer show "Based on N sources" and at most 3 follow-up actions with credit costs.

## Flashcards
- Generate from selected sources (prompt: /prompts/flashcards.v1.md). Strict JSON validated with serde; retry once on parse failure; each card links to its source chunk. Users can edit, star, and flag cards.
- Reviews are APPEND-ONLY events (`flashcard_reviews`). The server derives each card's ease, interval_days, and due_at by replaying that card's reviews in `reviewed_at` order with SM-2 (deterministic, unit-tested). Clients compute the next-interval preview for each grade button from the same SM-2 function (shared in crates/core, compiled for the client).
- Undo: a review can be undone for 5 minutes after it was made. If not yet pushed, remove it from the outbox; if already pushed, push a delete for that review (allowed ONLY for flashcard_reviews within 5 minutes, same device or user). The server then recomputes the card state.
- Review UI: grade buttons appear only after "Show answer"; end-of-session summary (cards reviewed, accuracy by rating, next review time); "all caught up" state.

## Quiz
- MCQ and short answer, configurable count and difficulty (prompt: /prompts/quiz.v1.md), strict JSON, explanation + linked chunk per question. Record attempts (append-only); return score and weak-topic summary grouped by source/page. Results offer "Review these cards" and "Suggest review blocks" (creates a Suggested plan, never auto-applied).

## Reports
- Step 1: editable outline (/prompts/report_outline.v1.md); sections whose topics are not supported by the sources are flagged `gaps: true` ("Not covered in your sources"). Step 2: generate each section separately with citations (/prompts/report_section.v1.md). Store Markdown; export .md first (PDF/DOCX later, gated by plan).

# CORE BEHAVIOR: STUDY OS

## Tasks
- CRUD with subtasks, priority, due date, estimate, workspace link. Quick-capture endpoint accepting free text ("finish crypto assignment friday") parsed by the LLM into a task draft (/prompts/quick_capture.v1.md); relative dates resolved in the user's timezone.
- "Break down": LLM proposes subtasks with time estimates (/prompts/task_breakdown.v1.md); the user accepts/edits/dismisses before saving.

## Calendar integration
- Google Calendar stays SERVER-SIDE: one connection per account works on every device. OAuth2 with offline access, minimal scopes, PKCE + state parameter; refresh tokens encrypted (aes-gcm). Incremental sync via syncToken; on 410 do a full resync. Optionally register watch channels (webhook) with polling via jobs as fallback. Maintain our own `events` rows with external_id + etag. Conflict policy: Google wins for events we didn't create; our study blocks go to a dedicated "Study Plan" calendar so they're easy to hide or delete. Google/ICS events are read-only in the UI (lock icon).
- Native OAuth flow: open the system browser (never an embedded webview), return via deep link / universal link / app link, validate state and PKCE.
- ICS feed: `GET /cal/:token.ics` outputs tasks with due dates, study blocks, and exams. Token is per-user, revocable, regenerable.
- ICS import/subscribe: user adds an external ICS URL (e.g. academic calendar); poll periodically; treat events as busy time. Support recurring events (rrule) and timezones correctly.

## Syllabus import
- User uploads a syllabus/course schedule (source kind=syllabus). The LLM extracts structured JSON (/prompts/syllabus_extract.v1.md): class meetings (recurrence), assignment deadlines, exam dates, each with evidence and confidence. Show a REVIEW screen where the user edits/accepts items; only then create events, tasks, and goals.

## Planner (deterministic, in crates/core, heavily unit-tested)
- Input: goals (exam dates, topics), sources/topics per goal, quiz weakness data, due flashcards, existing events (busy time), tasks with estimates, user_settings.
- Algorithm: work backward from each goal date; compute remaining effort per topic (weight weak topics higher); place study blocks into free slots within study hours, respecting max minutes/day, block length, breaks, and never overlapping busy time; spread evenly with spaced-repetition-style revisits before the exam; prefer earlier slots for higher priority; leave a buffer before deadlines.
- The LLM may only help estimate effort (/prompts/effort_estimate.v1.md) and break down tasks. Slot placement is plain Rust: deterministic and fast. The planner runs on the server; results sync to all devices as events.
- Output a SUGGESTED plan the user previews, tweaks, and accepts (then write events + optionally push to Google). Replan endpoint reschedules unfinished blocks when things change.
- Prep progress for a goal (`GET /goals/:id/progress`): topic coverage %, quiz accuracy % (last 14 days or null), cards due, weak topics. No composite score and no grade prediction.
- Workload meter: planned minutes today vs the user's daily limit.
- Workload balancer ("Rebalance my week", Power): detect overloaded days/weeks and propose moves or splits. Weak-topic auto review blocks (Power).

## Agentic assistant
- Chat gets tool calling. Tools: `create_task`, `update_task`, `list_tasks`, `create_event`, `find_free_slots`, `schedule_study_block`, `create_flashcards`, `create_quiz`, `get_due_flashcards`, `get_upcoming_deadlines`.
- READ tools execute immediately. WRITE tools never execute directly: they create a `pending_actions` row and stream a confirmation card to the UI (human-in-the-loop) with Confirm / Edit / Dismiss and the note "Nothing is added until you confirm." Expire pending actions after a timeout. Pending actions are visible and confirmable from any signed-in device.
- Treat retrieved document text and web content as untrusted data: it must never trigger tool calls on its own.

## Today, Study hub, focus timer, reminders, dashboard
- Today (home): "Due now" card ("14 flashcards due" -> "Review 14 cards", 1 tap), today's timeline (blocks, classes, deadlines, tasks), deadlines this week, quick capture, focus timer, "Continue where you left off", gentle streak. New-user variant: 3-step checklist (create workspace, add first source, make first flashcards). Streak: a day counts if the user reviews at least one card, answers a quiz, or logs a study session (in the user's timezone).
- Study hub: decks across workspaces with due badges, recent quizzes with scores, and a single "Review all due" action.
- Pomodoro-style focus timer logging to study_sessions (optionally tied to a task). Timer state survives app restarts.
- Reminders via server jobs: push notifications (APNs/FCM) to mobile devices, native notifications on desktop, in-app, email, Telegram/Discord optional (Power); for upcoming events, due tasks, due flashcards. Respect timezone and quiet hours. Avoid duplicates across devices (deliver to the most recently active device, or dedupe by reminder id). Clients may also schedule local notifications from synced events/tasks so reminders work offline. Use a pre-permission screen explaining why before the OS notification prompt.
- Dashboard: time per course, review activity, quiz mastery per topic, weak-topic list that can propose review blocks. Charts have text equivalents; no invented metrics.

# MULTI-DEVICE ACCOUNT AND SYNC (document in docs/SYNC.md)

## Auth across devices
- Login returns a short-lived access token (e.g. 15 min) and a rotating refresh token bound to a `device`. Store the refresh token only in the OS keychain/keystore on native clients. Refresh-token reuse detection: if a rotated token is reused, revoke the whole token family and require login (show "You were signed out from another device").
- Devices screen: list devices (name, platform, last seen), rename, sign out a device remotely (revoke), sign out all. Revoked devices get 401 and wipe their local cache on next contact. Enforce `devices_max` per plan (error includes the option to revoke an old device).

## Sync protocol (types in crates/sync, shared by server and client)
- Pull: `GET /sync/pull?cursor=<seq>&limit=<n>` returns changes for the user since `seq` as `[{seq, entity, id, op, data?}]` plus `next_cursor` and `has_more`. Initial sync starts at cursor 0 and pages through in dependency-safe order. If the cursor is older than retained history, return `410 resync_required` and the client does a full resync.
- Push: `POST /sync/push` with `device_id` and a list of mutations `{mutation_id (uuid), entity, op, id, payload, client_ts}` from the client's outbox. The server validates each mutation against entitlements and ownership, applies it in a transaction, records it in `mutation_log` (IDEMPOTENT: replaying a mutation_id returns the stored result), and returns per-mutation results (applied / rejected with reason / conflict).
- Realtime hint: `GET /sync/stream` (SSE) emits `{seq}` when the user's change_log advances so other online devices pull immediately. Mobile apps in the background rely on push notifications and pull on foreground.
- Clients write locally first (SQLite + outbox), update the UI immediately, then push; retry with exponential backoff; show sync status (Synced / Syncing / Offline / Needs attention, with "Last synced ..." on tap).
- Compaction: a job trims change_log older than the retention window (default 90 days) but keeps tombstones at least that long; document the resync fallback.

## Conflict rules
- Append-only entities (flashcard_reviews, quiz_attempts, study_sessions, messages): just insert; ids are client-generated, duplicates ignored. Exception: a flashcard review may be deleted as an undo (see Flashcards).
- Flashcard scheduling state is server-derived from reviews (no conflicts possible).
- Mutable entities (tasks, events, goals, decks, flashcard text/star/flag, reports, workspaces, settings): record-level last-write-wins using server receive time, with field-level merge for tasks and events when changed fields don't overlap. A delete (tombstone) beats a concurrent edit unless the user explicitly restores. Report/long-text edits on two devices: keep the latest and store the losing version as a recoverable copy.
- Server-owned entities (sources/ingestion status, subscription, credits, planner output) are read-only for clients except via their dedicated endpoints.
- Document these rules and test them.

## What is cached locally on native clients
Synced entities above (not chunks/embeddings), plus downloaded previews/originals on demand (user can "make available offline" per source). Provide "Reset local data and resync" in settings. Optional encrypted local DB (SQLCipher) behind a config flag.

# SUBSCRIPTIONS, PLANS, ENTITLEMENTS

## Plans (config/plans.toml, loaded at startup; defaults below, tune after measuring real cost)
Prices (IDR): free=0; pro monthly=10000, yearly=100000; power monthly=25000, yearly=250000. Optional student-verified promo prices configurable per plan. Account sync works on ALL plans; only device count and storage differ. Plan names in the UI are plain: Free, Pro, Power.

free:  workspaces=2, sources_total=20, max_file_mb=20, storage_mb=200, devices_max=2, credits_per_month=60, chat_msgs_per_day=20, web_searches_per_day=3, ai_flashcards_per_month=30, quizzes_per_month=2, reports_per_month=1, export_formats=[md], task_ai_breakdown=false, quick_capture=false, google_calendar=false (ICS feed + import only), syllabus_imports_total=1, planner="manual", agent_actions=false, reminder_channels=[web,push], model="cheap default", byok=false

pro:   workspaces=unlimited, sources_total=150, max_file_mb=50, storage_mb=2000, devices_max=5, credits_per_month=500, web_searches_per_day=30, export_formats=[md,pdf,docx], task_ai_breakdown=true, quick_capture=true, google_calendar=true (1 account, two-way), syllabus_imports=unlimited, planner="auto", agent_actions=true, reminder_channels=[web,push,email], model="cheap default", byok=false

power: everything in pro plus sources_total=500, max_file_mb=100, storage_mb=10000, devices_max=10, credits_per_month=2000, web_searches_per_day=100, multiple Google accounts, planner="auto+balancer+weakness_blocks", reminder_channels add telegram/discord, advanced analytics + CSV/JSON export, better models selectable, byok=true (user supplies their own LLM API key, encrypted at rest; BYOK usage consumes no credits and has no credit limit)

## Credit costs (config, tunable)
chat_message=1, web_sear, flashcard_batch(10)=4, quiz(10 questions)=6, report_outline=3, report_section=6, task_breakdown=2, syllabus_import=8, plan_generation=2, agent_turn=2. Long-document operations scale with input tokens (simple multiplier). The UI shows the cost before the action runs.

## Entitlements engine (crates/core)
- `Entitlements` built from plan + subscription status. One function `check(user, Feature) -> Result<(), EntitlementError>` used by EVERY handler (including sync push mutations); no limits hardcoded anywhere else. Structured errors (`plan_required`, `limit_reached`, `insufficient_credits`) include the minimum plan needed so the UI can show a precise locked state (on mobile builds: no purchase CTA, per billing_policy).
- Entitlements are per ACCOUNT, never per device: paying on the web or desktop unlocks every device as soon as the payment is verified.
- Credit accounting: monthly grant job at period start (no rollover by default, configurable). Spend inside a DB transaction against `credit_ledger` (no negative balance, safe under concurrency); spend monthly bucket first, then purchased. Purchased credit-pack credits never expire.
- Daily/monthly counters (chat msgs, web searches, quizzes, reports) via usage_events or a counters table; reset by timezone-aware jobs.
- Downgrade/expiry: never delete user data. After a 3-day grace period (configurable), over-limit workspaces/sources become read-only (viewable, exportable); features above the plan are locked with clear messaging; extra devices beyond the new limit are flagged read-only until the user revokes some.
- Margin safety: log provider_cost_usd per action in usage_events; admin margin report (revenue minus provider cost minus gateway fees, per plan and per user); a hard monthly provider-cost ceiling per user per plan (config) that falls back to the cheap model or pauses expensive actions with a clear message.

## Payments: QRIS via my gateway (trait `PaymentProvider`: create_invoice, get_status, parse_and_verify_webhook, refund_note)
- QRIS is a one-time payment, so billing is PREPAID PERIODS only: 30 days (monthly) or 365 days (yearly). No auto-renew, no stored payment methods. Every renewal or credit pack is a new QRIS invoice.
- Checkout flow:
  1. `POST /billing/checkout` {plan_id, period} or {credit_pack}, optional discount_code. Server computes the amount (plan price, discount, student promo), creates a `payments` row (status=pending, unique `order_id`, `expires_at`, default 15 minutes, configurable) and calls the gateway to create a QRIS invoice.
  2. Return the QR payload/image URL and expiry. The client renders the QR with a countdown (and an "open e-wallet" hint if the gateway provides one), then waits via `GET /billing/payments/:id` polling and/or the sync SSE hint.
  3. Gateway webhook marks the payment paid. The handler verifies the signature/callback token, checks that order_id exists and belongs to a pending payment, and that the paid amount EQUALS the expected `amount_idr` and currency is IDR; then, in ONE transaction and idempotently (`webhook_events` unique key): marks paid, activates/extends the subscription, grants monthly credits (or adds purchased credits), records the gateway fee, and notifies the user (push/in-app: "Pro is active").
  4. Fallback: a `poll_payment` job queries the gateway's status API for pending payments until expiry, in case the webhook is delayed or lost. A reconciliation job compares recent gateway transactions with our `payments` table and flags mismatches.
- Edge cases to handle and test: duplicate webhooks, amount mismatch or over/under-payment (mark `review`, never activate automatically, alert admin), payment confirmed after invoice expiry (default: honor it if the gateway confirms it was paid; configurable), expired/failed invoices, user paying twice for the same period, webhook arriving before the client polls.
- Period rules: paying while a subscription is active EXTENDS it starting at `current_period_end`; paying after expiry starts at payment time. Upgrade (pro -> power) switches the plan immediately and charges the prorated price difference for the remaining days (configurable); downgrade takes effect at period end.
- Renewal reminders at 7, 3, and 1 day before expiry and on expiry day (push, email, in-app on web/desktop; on mobile builds the reminder only states the plan is ending, with no purchase link, per billing_policy). 3-day grace after expiry, then downgrade to free behavior (data kept).
- Support: monthly/yearly toggle (default yearly at checkout, since fixed per-transaction costs hurt at low prices), one-time credit packs, discount codes, student discount (auto-apply when `.ac.id` email is verified via email link). Admin tools: manually mark a payment paid or refunded, extend a subscription, grant credits. Refunds are processed outside the app via the gateway; record them in `payments`. Require verified email before checkout.
- Never trust client-reported payment status. Never store card or wallet data. Gateway secrets only in env. Restrict the webhook endpoint (signature verification, plus IP allowlist if the gateway offers one), and rate-limit checkout.
- config/billing_policy.toml: per platform {web, macos, windows, ios, android} flags: `allow_purchase_ui`, `show_prices`, `allow_purchase_cta_in_locked_messages`. Defaults: true for web/macos/windows; false for ios/android. The UI reads this on every platform; mobile builds must contain no checkout screen, no purchase buttons, and no links or text directing users to buy elsewhere.

# AUTH AND SECURITY
- Email+password signup/login, argon2, email verification, per-device sessions. Every query scoped to the authenticated user (no cross-tenant access), including sync pull/push and signed file URLs. Validate and sanitize all inputs; restrict file types and sizes per plan; never log secrets or tokens; encrypt OAuth refresh tokens and BYOK keys; rate-limit auth, sync, checkout, and agent endpoints. Request minimal Google scopes and document that Google app verification is required beyond test users.
- Tauri hardening: strict CSP with no remote origins for scripts, styles, fonts, or images; minimal capabilities/permissions per window; no remote code loading; validate every IPC command input; tokens only in keychain/keystore; validate deep-link payloads (state/PKCE); HTTPS only. Add a test that the production build makes no requests to third-party CDNs.
- Account deletion: in-app `Delete account` (also required by app store rules) that deletes or anonymizes all user data, revokes sessions, and cancels pending jobs. There are no recurring charges to cancel; document what happens to remaining paid time. Also `Export my data` (JSON + Markdown + ICS).
- Keep ToS/privacy placeholder pages and note that tax/PPN handling and store privacy-label disclosures must be confirmed before launch.

# API (document in docs/API.md)
Auth: POST /auth/signup, /auth/login, /auth/refresh, /auth/logout, /auth/logout-all, /auth/verify-email, /auth/forgot-password
Devices: GET /devices, PATCH /devices/:id, DELETE /devices/:id (revoke), PUT /devices/:id/push-token
Sync: GET /sync/pull, POST /sync/push, GET /sync/stream (SSE)
Home: GET /today (due counts, today's timeline, deadlines this week, last workspace, streak, onboarding checklist), GET /study/overview (decks with due counts, recent quizzes)
Workspaces: CRUD /workspaces
Sources: POST /workspaces/:id/sources (multipart or JSON), GET list, DELETE, GET /sources/:id/status, GET /sources/:id/file and /preview (signed URLs), GET /chunks/:id (text, page, highlight rects)
Chat: GET /workspaces/:id/messages, POST /workspaces/:id/chat (SSE; body: message, source_ids?, use_web, sources_only), POST /chat/:id/stop
Flashcards: POST /workspaces/:id/decks, GET /decks/:id, GET /decks/:id/due, PATCH /flashcards/:id (text/star/flag), POST /flashcards/:id/review (writes a flashcard_reviews row), DELETE /flashcard-reviews/:id (undo, 5-minute window)
Quiz: POST /workspaces/:id/quizzes, GET /quizzes/:id, POST /quizzes/:id/attempts, GET /quizzes/:id/results
Reports: POST /workspaces/:id/reports/outline, POST /reports/:id/generate, PATCH /reports/:id, GET /reports/:id/export?format=
Tasks: CRUD /tasks, POST /tasks/quick-capture, POST /tasks/:id/breakdown
Events: CRUD /events, GET /events?from=&to=
Goals/Planner: CRUD /goals, GET /goals/:id/progress, POST /goals/:id/plan (suggestion), POST /plans/accept, POST /plan/replan
Syllabus: POST /workspaces/:id/syllabus, GET /syllabus/:id/draft, POST /syllabus/:id/confirm
Calendar: GET /calendar/google/connect, GET /calendar/google/callback, POST /calendar/ics, DELETE /calendar/connections/:id, POST /calendar/sync, POST /calendar/ics-token/regenerate, GET /cal/:token.ics
Assistant actions: GET /actions/pending, POST /actions/:id/confirm, POST /actions/:id/reject
Study: POST /sessions, GET /dashboard
Settings: GET/PUT /settings, PUT/DELETE /settings/api-key (BYOK, power only)
Account: DELETE /account, GET /account/export
Billing: GET /plans, GET /billing/me (plan, period end, credits, usage; works on all platforms), POST /billing/checkout (blocked by policy on ios/android), GET /billing/payments/:id, GET /billing/invoices, POST /webhooks/qris
Admin: users, plan overrides, manual payment mark/refund, manual credit grants, margin report

# FRONTEND (one adaptive UI for all platforms)

## Information architecture
- Phone (iOS/Android) bottom tab bar, 5 tabs: Today, Workspaces, Study, Planner, More. Tasks live inside Planner as a segmented control (Plan | Tasks). Flashcards and quizzes live in Study and in each workspace's Studio. "More" holds Settings, Devices and sync, Plan and usage, Help.
- Desktop/web sidebar (248px, collapsible to a 64px icon rail below 1200px): Today, Workspaces (course list nested), Study, Planner (Calendar | Tasks), and Settings at the bottom. Top bar: ⌘K/Ctrl+K search and command palette, sync indicator, credit meter, avatar menu.
- Tablet: two-pane layouts.

## Screens
1. Auth: sign in, sign up, verify email, forgot password; first run (language, timezone, optional notification pre-permission); "signed out from another device".
2. Today: described above; phone + desktop.
3. Workspaces list: course cards (color marker, source count, due cards, next exam).
4. Workspace view. Desktop: Sources rail (280px, collapsible) | Chat | right panel with tabs Viewer and Studio (resizable). Phone: top segmented control Chat | Sources | Studio; the reader opens full-screen from a source or citation.
   - Sources: add source (upload, link, paste text), per-source status chips (Ready, Processing %, Failed with Retry, Read-only), select-all/checkboxes, storage meter, empty drop zone.
   - Chat: message anatomy as described (citation chips, "Based on N sources", follow-up actions with costs, streaming with Stop, not-in-sources variant, web answer variant with [W#] chips, error with Retry, low-credits banner, agent confirmation card). Composer: auto-growing input, "@" source mention, attach, scope chip ("4 of 4 sources"), toggles "Answer only from my sources" and "Search the web", Send; hint text "AI can make mistakes. Check the citations."
   - Viewer (pdf.js): tabs per open source, page navigation, zoom, find, citation highlight labeled "Cited", text selection toolbar (Ask about this, Make flashcard, Copy).
   - Studio: Flashcards (decks with due counts, create flow with cost), Quiz (recent scores, new quiz), Report (new report); generating items show progress.
5. Add source and processing flow with error states (file too large, unsupported type, scanned PDF, storage full, source limit reached with the web/desktop vs mobile variants).
6. Study hub and flashcard review (mobile immersive, desktop centered with a collapsible side panel): hotkeys Space (show answer), 1-4 (grade, after reveal only), Z (undo), E (edit); computed interval labels; Undo toast; end-of-session summary; all-caught-up state; deck list; real "last 14 days" and "cards by status" charts with text equivalents.
7. Quiz: setup sheet (sources, count, difficulty, type, cost), question screen (large option rows with idle/selected/correct/wrong using icon + text), per-answer feedback with explanation and source chip, results with weak topics, "Review these cards", "Suggest review blocks", review mistakes.
8. Report: outline editor (reorder, rename, delete, gap warnings), generation progress per section, document view with citations, export menu (PDF/Word locked per plan with the right locked variant).
9. Planner: Plan/Calendar and Tasks. Phone: week strip with counts, "Prep progress" card (coverage / quiz accuracy / cards due), "Today's workload" bar, timeline (classes muted/read-only, study blocks, deadlines, tasks), Suggested-plan card with Accept / Edit / Dismiss, add sheet with quick capture and structured fields using native pickers. Desktop: left column (exams with prep progress, due for review, tasks), right a real time grid 07:00-22:00 with blocks positioned by start/duration, current-time line, all-day row, correct overlap layout, Day/Week/Month toggle, drag to move/resize with 15-minute snapping, keyboard navigation, right-side detail panel, Suggested-plan mode with dashed blocks and Accept all / Accept selected / Dismiss, "Rebalance my week" (Power; locked variants), "Add exam" modal (course, name, date/time, topics or sources picker; "Build my plan"), link to study-hours settings.
10. Tasks: list and board views, subtasks, priority, due date, estimate, "Break down" suggestions with Accept/Edit/Dismiss, quick capture.
11. Syllabus review screen.
12. Dashboard (inside Study or More): time per course, review activity, weak topics.
13. Settings: Account; Study preferences (timezone, study hours, daily limit, session and break length); Notifications (channels, quiet hours); Calendar connections (Google status, add ICS URL, copy "your calendar feed" link with Regenerate); Devices and sync (list, sign out, sign out all, reset local data and resync); Data (export, delete account); Language (English / Bahasa Indonesia); Appearance (System / Light / Dark); BYOK key (Power, web/desktop only).
14. Plan and usage. Phone: read-only (plan, renewal date, credit meter, usage by feature), NO prices or purchase UI. Web/desktop: pricing page (3 plan cards, yearly default, student discount banner, comparison table, FAQ), billing page (current plan, renewal date, credits, payment history, buy credits), QRIS checkout (order summary, large QR, amount, 15:00 countdown, status states: waiting, paid, expired -> new QR, amount mismatch -> "under review", failed, offline).
15. System states: offline, syncing, sync error, 404, generic error, maintenance; desktop command palette (⌘K/Ctrl+K) searching sources, decks, tasks, and actions.
16. Admin (web only, admin only): users, plans, manual payment actions, grants/refunds, margin report, payments flagged `review`.
Every screen needs designed loading (skeleton), empty, error, and offline states, in light and dark, and must pass the design-system rules above.

## Adaptive layout
- Desktop: multi-panel layouts, keyboard shortcuts, system tray quick-capture popover, drag-and-drop file upload, native menus and notifications.
- Phone: stacked navigation, bottom sheets with grabber, safe-area insets, keyboard-aware chat input, 44px targets, pull-to-refresh, optional swipe gestures on flashcards with a one-time hint, share-sheet "Import to Stvdy" for PDFs and links (stretch goal), haptics where sensible. No FAB overlapping the composer or tab bar.

# PHASES (stop and summarize after each; wait for me to say "continue")
Phase 0: Scaffold: Cargo workspace, React+Vite app, Tauri 2 shell building for macOS and Windows, docker-compose (Postgres+pgvector, MinIO), migrations, config loaders (plans.toml, billing_policy.toml), health endpoint, CI (fmt, clippy, test, web + macOS + Windows build matrix, contrast test, axe checks). Design system: tokens as CSS variables + Tailwind theme, bundled fonts/icons, the base component library with a gallery page (all components, all states, light + dark) built to match /design, i18n skeleton (en, id), `platform` adapter skeleton, app shell with the navigation above routing to placeholders.
Phase 1: Auth with devices + rotating refresh sessions + email verification; sync-ready schema (sync columns, change_log + triggers) from the start; workspaces (with colors), source upload to object storage, ingestion worker (PDF + text) with progress, chunking with positions, embeddings, status polling; Workspaces list and Sources UI. Early native prototype: keychain token storage and deep-link round trip.
Phase 2: Prompt loader + /prompts, hybrid retrieval, streaming chat with validated citations, Chat UI (citation chips, popover/sheet, toggles, not-in-sources and error variants), Viewer with pdf.js and citation highlighting (`GET /chunks/:id`). Run the first prompt evals.
Phase 3: Web search toggle + [W#] citations + save-web-result-as-source.
Phase 4: Flashcards: generation, append-only reviews, server-side SM-2 replay, shared client SM-2 for interval previews, review UI (reveal-then-grade, undo, summaries), Study hub.
Phase 5: Quiz generation, attempts, results, weak topics.
Phase 6: Reports (outline with gap flags -> sections -> export).
Phase 7: Entitlements engine (incl. devices_max, storage_mb), credit ledger, credit cost previews in the UI, usage_events with provider cost and prompt_version, monthly grant job, downgrade/read-only behavior, locked-state components (web/desktop and mobile variants), rate limits, PPTX/DOCX/URL ingestion with PDF previews, polish. Route ALL earlier features through the entitlements engine. Plans switchable manually via an admin endpoint (no payments yet).
Phase 8: Sync engine: protocol in crates/sync, /sync/pull, /sync/push, /sync/stream, mutation idempotency, local SQLite cache + outbox in src-tauri, offline reads, offline writes for the allowed entities, sync status UI, Devices screen, resync flow, change_log compaction. Convergence tests with simulated devices.
Phase 9: Tasks, quick-capture, break-down, events, Today screen, settings (incl. language and theme), focus timer, streak (all sync-native, offline-writable).
Phase 10: ICS feed + ICS import, then Google Calendar OAuth (native deep-link flow) + incremental sync, calendar UI with the real time grid.
Phase 11: Syllabus import with review screen, goals with topics, `/goals/:id/progress`, deterministic planner with tests, Suggested-plan flow (accept/replan), workload meter, workload balancer.
Phase 12: Agentic assistant (tool calling, pending_actions confirmation across devices), reminders engine, push provider traits (APNs/FCM, desktop notifications), dashboard.
Phase 13: QRIS payments: PaymentProvider trait + `qris_gateway` implementation for my gateway, checkout + QR screen, idempotent verified webhooks, polling fallback and reconciliation, period extension/upgrade rules, credit packs, discount/student codes, renewal reminders, pricing and billing pages (web/desktop), billing_policy enforcement, BYOK, admin payment tools and margin report. Use a gateway sandbox for tests.
Phase 14: Mobile: iOS and Android builds via Tauri mobile, phone/tablet adaptive pass, read-only Plan and usage screen with NO purchase UI, push notification plugins (APNs/FCM) with token registration and pre-permission screen, deep links/universal links/app links, secure storage, file picker and share-sheet import, local notifications, TestFlight and Play internal-testing builds. Write the mobile test checklist and the store-policy risk note in docs/RELEASE.md.
Phase 15: Release pipelines: macOS signing + notarization + universal binary, Windows code signing, Tauri updater with signed manifests, Android direct-APK option, store submission checklists (privacy labels, in-app account deletion, review notes), README and deployment notes (server Docker image including LibreOffice for previews).

# QUALITY BAR
- `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass at every phase; the web and desktop builds must compile in CI; mobile builds from Phase 14.
- Unit tests: chunking, RRF merge, citation validation, SM-2 replay determinism (same reviews in any arrival order -> same state) including undo, JSON-output parsing, prompt rendering (all variables filled, no leftover {{}}), planner (no overlap with busy time, respects daily max and study hours, deadline buffer, weak-topic weighting, timezone/DST-safe), prep-progress calculation, ICS generation/parsing incl. recurrence, agent confirmation gating, entitlements (limit enforcement, plan/feature matrix generated from plans.toml, concurrent credit spend with no double spend, period rollover, downgrade read-only, device limits, BYOK bypassing credits), refresh-token rotation and reuse detection.
- Payment tests: webhook signature verification, idempotent duplicate webhooks, amount mismatch -> `review` (no activation), late payment after expiry, extension stacking vs. fresh start, prorated upgrade, polling fallback activates a payment when the webhook is missing, billing_policy hides purchase UI/endpoints on ios/android.
- Sync tests: two or three simulated devices making offline edits then syncing converge to the same state; idempotent push (replayed mutation_id); tombstone vs edit conflict; append-only merges; undo of a pushed review; `410 resync_required` full-resync path; revoked device handling; no cross-tenant leakage through change_log.
- UI tests: token contrast test, axe a11y checks, pseudo-locale (30% longer strings) screenshot checks for key screens, reduced-motion behavior, keyboard-only flows (flashcard review, chat, planner), no third-party network requests in production builds, grade buttons hidden/disabled before reveal.
- AI evals (/prompts/evals): grounding (answers cite only provided labels), "not in sources" behavior, prompt-injection inside a document is ignored, relative-date task creation in the user's timezone, Indonesian and English, locked feature on mobile with no upgrade wording. Run on every prompt change.
- Integration tests: auth scoping, ingest -> chat flow, plan -> accept -> events, checkout -> verified webhook -> plan active on a second device, with mock LLM/embedder/calendar/payment/push providers via traits.
- No unwrap() in non-test code; typed errors mapped to HTTP status codes. Prompts live in /prompts, not inline.
- Structured logging (tracing) with request IDs; log latency for retrieval, LLM calls, sync pull/push, sync jobs, and webhooks.

# OUT OF SCOPE FOR MVP
Apple In-App Purchase and Google Play Billing (the PaymentProvider trait must allow adding them later), auto-renewing billing, audio overviews/TTS/"Listen", Viva Voce, 3D explorers, Anki export, Google Drive sync, camera/OCR scanning, predicted grades and any "cognitive/retention forecast" metrics, lecture recording/transcription, real-time collaboration/study groups, leaderboards or social features, Mac App Store and Microsoft Store distribution, Linux builds, widgets/watch apps, on-device LLMs, offline chat/generation/uploads, multiple chat threads per workspace, non-Google OAuth login, campus LMS integrations, browser extension, Stripe/international payments.

Begin with Phase 0 now. Show the planned file tree first, then implement.
