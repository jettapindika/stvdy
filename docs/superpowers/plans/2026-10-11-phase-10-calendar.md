# Phase 10. Calendar: ICS feed, ICS import, Google two-way sync. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the roadmap
> disagree, the roadmap wins and this file is the bug.

**Goal:** the student's schedule is real. They subscribe their phone's calendar to
`/cal/:token.ics` and their tasks with due dates, their study blocks and their exams appear
in it. They paste their faculty's ICS URL and their classes land in the app as read-only
busy time, recurrence and timezones intact. They connect Google Calendar from the system
browser, and from then on their Google events appear here and their study blocks appear in
a dedicated "Study Plan" calendar — incrementally, so a poll costs one round trip and an
edit made on the phone shows up on the laptop.

**Architecture:** three pieces, in the order the roadmap fixes them, because each one is
what the next one needs:

- **Out.** A pure function in `crates/core` turns `events` rows into RFC 5545 text. The
  server's job is a token lookup and a `select`; the interesting part is that a feed is a
  *cache the client owns*, so a row's `UID` must be stable and its `SEQUENCE` must move
  when the row is edited. A feed that omits `SEQUENCE` is a feed whose edits are invisible
  forever (A230), which is why this is Task 1 and not a route detail.
- **In.** `icalendar` parses, `rrule` expands, `chrono-tz` resolves. The expansion is
  **bounded twice** (a horizon and a hard occurrence cap, with `limited()` checked — and the
  request asks for one more occurrence than it keeps, because `rrule` 0.14 sets `limited` when
  the collection merely *reached* the limit given), because an unbounded recurrence iteration
  is a server hang rather than a wrong answer — the roadmap's own risk register names it. The
  fetch goes through the *same* SSRF guard ingestion uses, as a second function on it rather
  than a second guard (A233), because a user-supplied ICS URL is exactly the primitive
  `--block-private-networks` exists for.
- **Two-way.** Google is the only place this phase writes to a remote system, and it does so
  behind a trait (`crates/core/src/traits/calendar.rs`), so the sync is testable without
  Google and the roadmap's "providers are config, not code" line holds. The consent flow is
  the **login flow's** state cipher, PKCE helper and deep-link return, extracted once and
  called twice (A235) — not a second OAuth. The scope pair is chosen so that the conflict
  policy is structural: we hold `calendar.app.created` (write only to calendars we made) and
  `calendar.events.readonly` (read the rest), so "Google wins for events we didn't create"
  is not a rule anyone has to remember (A234).

Phase 9 lands first and owns `events` CRUD and the four server-owned columns. This plan
**inherits** them and re-plans none of them; § "What Phase 9 hands over" states exactly
what arrives.

**Tech Stack:** Rust (core + server; two new crates), Postgres/sqlx, React + Vite +
TypeScript + Tailwind, TanStack Query, Tauri 2 (the calendar deep-link intent already
exists). No new frontend dependency.

**Exit gate (observable, from the roadmap):** `/cal/:token.ics` validates; an external ICS
is imported with recurrence; Google two-way sync is incremental and a 410 triggers a full
resync.

**Three roadmap gates are not optional here:**
- Gate 5, accessibility: this phase adds a screen and a grid. A calendar grid is the classic
  place to put state in colour and to make an event a `<div>` that a keyboard cannot reach.
  The check performed and its measured result are stated in Task 11, not claimed.
- Gate 6, Indonesian end to end: an Indonesian ICS feed (`TZID=Asia/Jakarta`, a
  `SUMMARY:UAS Jaringan Komputer`) must import with the right wall clock, and the feed must
  round-trip an Indonesian title without mangling it. A wrong tz resolution and a wrong
  folding rule both fail *silently* while English tests stay green.
- Gate 7, honesty: the calendar screen may not claim a connection is healthy while it holds
  an error, the "last synced" line is a timestamp from a row rather than a client guess, and
  the study blocks shown are the ones the planner actually wrote.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes.
  Prompts live in versioned modules, never inline. `tracing` structured logs with
  request ids; log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks — and
  for every Google and ICS fetch.
- **Server is the only source of truth.** Clients never call Google directly, never hold a
  Google token, and never see a refresh token. All heavy work is server-side.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings. **Calendar connect, ICS import and sync require connectivity
  and queue nothing** (`docs/SPEC.md` § Offline scope): the screen shows an offline state
  with a one-line reason rather than hiding the controls.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
  Local-date math only through `chrono-tz`, never a fixed offset. An ICS `TZID` is resolved
  through `chrono_tz::Tz`, never through the machine's zone.
- **Entitlements:** no limit is hardcoded; every limit is read from `config/plans.toml`
  through `crates/server/src/entitlements.rs`. Errors serialize to
  `{ code, message, min_plan?, limit?, used? }`.
- **Quality gate:** `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
  `npm --prefix app test`, `npm --prefix app run build`,
  `node scripts/check-no-cdn.mjs`, `cargo sqlx prepare --workspace --check`.
- **UI language English**, but Indonesian documents, queries and **calendar payloads** must
  work end to end.

### Values this phase charges and refuses (copied from `config/plans.toml`)

| Action | Cost | Read from |
|---|---|---|
| ICS feed read | `0` | — (a `select`; it is the student's own data) |
| ICS import / poll | `0` | — (no model, no credit) |
| Google connect, sync | `0` | — (no model, no credit) |
| app-owned event pushed to Google | `0` | — |

| Feature | free | pro | power |
|---|---|---|---|
| `google_calendar` | `false` → 403 `plan_required` | `true` | `true` |
| `google_accounts_max` | `0` | `1` | `unlimited` |
| ICS feed + ICS import | **available** | available | available |

The last row is the spec's exact wording ("free … `google_calendar=false` (**ICS feed +
import only**)"), so the two ICS paths are deliberately *not* behind `Feature::GoogleCalendar`
and must not be.

---

## What already exists (checked in this tree, not assumed)

**The schema is complete for the feed token and for `events`. The connection row needs
columns, not a table.**

- `migrations/0003_users.sql:22` — `ics_feed_token text unique, -- revocable secret for the
  ICS feed`. **The feed token column already exists**; `POST /calendar/ics-token/regenerate`
  writes it and the feed route reads it. No migration is needed for the token.
- `migrations/0013_calendar_events.sql` — `calendar_connections (id, user_id, provider,
  account_email, refresh_token_enc bytea, sync_token text, ics_url text, last_synced_at)`
  with `index on (user_id, provider)`. **No `created_at`, no `updated_at`, no status/error,
  no conditional-GET validators, no study-calendar id** — see A239/A237.
- `migrations/0013_calendar_events.sql` — `events (id, user_id, workspace_id, kind, title,
  starts_at, ends_at, external_id, etag, connection_id references calendar_connections on
  delete set null, source default 'app', task_id references tasks on delete set null,
  created_at, updated_at, deleted_at)`, `index on (user_id, starts_at)`, `index on
  (task_id)`, and `unique index on (connection_id, external_id) where external_id is not
  null`. That unique index is this phase's upsert key (A231).
- `migrations/0017_sync_triggers.sql` — `events` is **already** in the `id_tables` array, so
  this phase adds no `+sync` table, appends nothing to that array, and **does not touch
  that file**. Its header comment stays true: it is still the last trigger-attaching
  migration.
- `crates/server/tests/schema.rs` — `NON_SYNC_TABLES` already contains
  `calendar_connections` (`:51`) and the `documented` list already contains it (`:310`), so
  a migration that only adds columns to it needs no new row there.

**The OAuth return path is built, and it is the calendar's.**

- `crates/server/src/auth/google.rs` — the whole pattern, with line-level facts:
  - `StatePayload { verifier, nonce, exp, device_name, device_platform, app_version }` and
    `encrypt_state`/`decrypt_state` over `Aes256Gcm` with a 12-byte random nonce and
    URL-safe base64. **The `state` is encrypted, not signed**, and the module's own header
    says why: it carries the PKCE verifier, and a readable `state` would hand the verifier
    to the party PKCE exists to distrust.
  - `pkce_pair() -> (String, String)` (32 random bytes, base64url verifier, SHA-256
    challenge), `random_token()`, `hash()` (SHA-256 hex).
  - `STATE_TTL = 600s`, `TICKET_TTL_MINUTES = 5`, `GOOGLE_AUTH_ENDPOINT`,
    `GOOGLE_TOKEN_ENDPOINT`, `GOOGLE_JWKS_URI`.
  - `start()` builds the authorization URL with `url::Url::parse_with_params` and `callback()`
    exchanges the code with `reqwest`; it ends at
    `Ok(format!("stvdy://auth/callback?ticket={ticket}"))`, i.e. **a single-use ticket, never
    a token** (A48/A54).
  - **It does not call the `oauth2` crate.** `oauth2 5.0.0` is pinned
    (`crates/server/Cargo.toml:58`, `Cargo.toml` workspace deps) and the roadmap's crate-list
    deviation records why, but the login flow is `url` + `reqwest` + `jsonwebtoken`. This
    plan therefore reuses the *pattern* and the *cipher*, and does not claim a crate call
    that does not exist.
- `src-tauri/src/deeplink.rs:109` — `"oauth/calendar" => DeepLinkIntent::Calendar(Calendar { params })`,
  with `classifies_a_calendar_return` (`:322`) and the serialized-shape assertion (`:450`)
  already green. `app/src/platform/types.ts:41` carries
  `{ action: 'calendar'; payload: { params } }`. **The deep-link return for the calendar is
  already built and tested**; this phase consumes it.
- `crates/server/src/config.rs:178` — `GoogleLoginConfig { client_id, client_secret, redirect_uri }`,
  parsed from `GOOGLE_LOGIN_CLIENT_ID` / `_SECRET` / `_REDIRECT_URI` (`:383-393`), and
  `AppConfig.encryption_key_hex` (`:37`) validated as 32 bytes of hex (`:481`).

**The SSRF guard exists, and `ChainFetcher` is the wrong entry point for an ICS body.**

- `crates/server/src/integrations/fetch.rs` — `is_public_ip` (`:24`), `parse_target` (`:147`,
  scheme + host + embedded-credential refusal), `resolve_public` (`:181`, returns the public
  addresses so the caller can *pin* one), `ReqwestFetcher` (`:223`, per-request client with
  `.resolve(&host, pinned)`, `MAX_REDIRECTS = 5`, every hop re-validated through
  `parse_target`), and `ChainFetcher` (`:539`).
- **`ChainFetcher::fetch` ends in `extract_readable`** (`:335`), which runs readability and
  falls back to a tag stripper. An ICS body is not HTML, so that path would hand back a
  mangled document. `ChainFetcher` also tries Exa `/contents` and the `moli` CLI first,
  which is wrong for a feed. See A233.
- `crates/server/tests/ssrf.rs` — the guard's suite: every private and reserved range
  (`:23`), the IPv4-mapped IPv6 case, the chain's scheme refusal (`:135`), the resolver's
  refusal of a literal private host (`:182`), and `is_citable_url` (`:212`).

**The job machinery, the entitlements layer, and the route table.**

- `crates/server/src/jobs/worker.rs:84` — `dispatch` matches on `job.kind`; the kinds today
  are `ingest_source`, `generate_flashcards`, `generate_quiz`, `generate_report_section`,
  `generate_cheatsheet`, `monthly_grant`, `reset_counters`, `render_cheatsheet_pdf`, and an
  unknown kind is `Failure::permanent`. **`sync_calendar` is not dispatched today.**
- `crates/server/src/jobs/schedule.rs` — the scheduler as a **producer of rows**: a 900s
  ticker, a bounded `SCHEDULE_BATCH = 500`, a `dedupe_key` of
  `"<kind>:<user_id>:<local YYYY-MM>"`, and a `not exists` guard so a second pass covers the
  rest rather than re-examining. This phase's poll is the same shape (A238).
- `crates/server/src/jobs/mod.rs:24` — `MAX_ATTEMPTS = 5`, `POLL_INTERVAL = 5s`,
  `Failure::transient`/`permanent`, `ClaimedJob::user_id()` (a payload without one is
  **permanent**).
- `crates/server/src/entitlements.rs` — `snapshot`, `require`, `reserve_in`, `refusal`,
  `effective_plan`. `Feature::GoogleCalendar` already exists
  (`crates/core/src/entitlements/feature.rs:30`, in `ALL` at `:70`), and `check.rs:158` maps
  it to `cfg.google_calendar`. `google_accounts_max` already exists in the matrix
  (`config/plans.toml:47/80/109` = `0/1/unlimited`, `crates/core/src/config/plans.rs:291`)
  and is already in the entitlement *view* (`crates/server/src/entitlements.rs:665`) — but
  **no `Feature` variant counts it and nothing enforces it** (A245).
- `crates/server/src/routes/mod.rs` — `public_routes` (health + Google start/callback), the
  `auth_limiter` bucket on the credential-taking routes, and one `.merge(<module>::routes())`
  per feature module, so a new module registers itself.
- `crates/server/tests/route_sweep.rs` — the table is `read_only_rows()` / `swept_rows()` /
  `exempt_rows()`, and `Class::Exempt` is defined as "**no authenticated user at all: a
  health check, a credentials route, an OAuth redirect, or an HMAC-signed object download**".
  The feed route and the calendar callback are exactly that.
- `crates/sync/src/proto/entities.rs:21` — `Entity::Event`, table `events`, dependency rank
  `10`, and `PayloadShape::PatchWithBase` (Phase 8's A176). `crates/server/src/sync/apply.rs`
  has `rule_for(entity)` and `merge_patch`, so the field-level merge for `events` the
  roadmap's conflict table promises **already exists**.
- `crates/server/src/testing.rs` — `TestState::new(pool)`, `with_llm`, `with_search`,
  `with_clock`, `with_embedder`; the seam a fake calendar API and a fake fetcher join.

**The SPA's calendar is a placeholder with a route and a nav entry.**

- `app/src/router.tsx:84` — `path: 'calendar'` → `NotBuiltRoute` with
  `routes.calendar.what`; `app/src/components/layout/Sidebar.tsx:26` links to `/calendar`.
- `app/src/i18n/locales/en.json:77` — `routes.calendar.what` = "a week and month view of
  classes, deadlines, and the study blocks the planner proposes, plus Google Calendar and
  ICS connections". `id.json` must carry the same key.
- `app/src/api/queries/*.ts` — the TanStack Query pattern; `app/src/components/ui/LockedFeature.tsx`
  is the plan-gated shell; `app/src/features/a11y.test.tsx` is where a screen joins the axe gate.
- `app/src/features/calendar/` **does not exist.**

**The two crates this phase needs are not pinned anywhere.**

- `Cargo.toml` `[workspace.dependencies]` and `crates/core/Cargo.toml` /
  `crates/server/Cargo.toml`: `chrono-tz 0.10.4` **is** pinned; **`icalendar` and `rrule`
  are in neither file.** They appear only as a crate-list deviation in
  `docs/ASSUMPTIONS.md` ("`ical` for parsing ICS → `icalendar` 0.17.14 for both parse and
  generate; `rrule` 0.14 for recurrence"). Verified by reading both manifests, not assumed.
  Task 1 pins them.

---

## What Phase 9 hands over (inherited; do not re-plan)

Phase 9 (`docs/superpowers/plans/2026-10-11-phase-9-tasks-events-today-timer.md`) lands
first. This phase inherits, and adds nothing to:

- **`crates/server/src/events.rs`** — `POST /events`, `PATCH /events/:id`,
  `DELETE /events/:id`, `GET /events?from=&to=`, with `kind in (class, exam, study_block,
  deadline, external)` and `ends_at >= starts_at`.
- **Phase 9's A198** — `events.source`, `external_id`, `etag` and `connection_id` are
  **server-owned**: a push that sets any of them (other than `source = 'app'` with the
  other three null) is `read_only_entity`, and the same through REST is 400 `validation`.
  A `source != 'app'` row is **not editable at all** — `PATCH` and `DELETE` are 403
  `forbidden` with a message naming the calendar, because a 404 would read as "deleted".
- **Phase 9's A199** — a deadline is an `event` with `kind = 'deadline'` and
  `ends_at = starts_at`, there is no `all_day` column, and **date-only serialisation is this
  phase's ICS feed's problem**. A248 is where that lands.
- **`migrations/0028_tasks_events_settings.sql`** (Phase 9's) and the `tasks.position`,
  `user_settings` quiet-hours/language/theme columns.
- **The route-sweep rows** for the four events routes, and the tamper rows that assert the
  four columns are refused on both write paths.
- **`platform.sync.write(entity, id, payload)`** (Phase 8) as the SPA's single write path.
  The calendar screen's event list is **read-only**, so it does not use it; the connections
  panel talks to REST, because connecting a calendar is not a synced entity.

**What this phase therefore does not do:** no `events` CRUD, no `events` migration, no
change to `sync/apply.rs`'s rule for `Event`, and no change to `0017_sync_triggers.sql`.

---

## Decisions this plan pins (so parallel work cannot diverge)

1. **A230 — the feed's `UID` is `<events.id>@stvdy.app` and its `SEQUENCE` is derived from
   `(created_at, updated_at)`, not from a clock reading.** `sequence(created_at, updated_at)
   = if updated_at > created_at { (updated_at - created_at).num_seconds().max(1) } else { 0 }`,
   and the `VEVENT` also carries `DTSTAMP` and `LAST-MODIFIED` from `updated_at`. Two
   renders of an unchanged row are byte-identical, and an edit strictly raises `SEQUENCE`
   (0 for a new event, ≥1 after any edit, monotone thereafter).
   *Rationale:* a feed is a cache the client owns. A client that caches on `UID` alone and
   finds no `SEQUENCE` has no signal that the event changed, so an edited title or a moved
   time is invisible forever — the failure is silent, permanent and per-client.
   *Stated limitation:* two edits inside the same second produce the same `SEQUENCE` and the
   same `LAST-MODIFIED`, because both are second-granular. Within one second there is no
   observable difference to lose; a sub-second edit history would need a counter column, and
   a counter on a synced table is a second write path for a value nobody reads.
2. **A231 — an occurrence, not a series, is a row.** `events.external_id` holds the
   *occurrence* key: Google's own opaque event id verbatim, and for ICS
   `"<UID>|<recurrence-id in UTC>"`, with `"<UID>"` alone for a non-recurring one. The
   existing `unique index on (connection_id, external_id) where external_id is not null` is
   then the upsert key, and a re-poll is idempotent.
   *Rationale:* `events` has no `rrule` column and the roadmap's Phase 11 planner reads
   `events` rows as busy time. Storing the series and expanding on read would need a second
   recurrence engine in the planner, in the SPA's week grid and in the ICS feed, all three
   of which would drift.
3. **A232 — expansion is bounded twice, and truncation is recorded.** `MAX_OCCURRENCES = 500`
   and `HORIZON_DAYS = 180`. The set asks for **one more than it keeps**
   (`all((MAX_OCCURRENCES + 1) as u16)`), keeps the first `MAX_OCCURRENCES`, and reports
   `truncated` when more were available. A truncated expansion writes
   `calendar_connections.error = 'recurrence_truncated'` — the **server** reads the
   `Expansion.truncated` flag, never `occurrences.len()`, or the same inference bug reappears
   one layer up — and keeps every occurrence it read.
   *Rationale:* `all()` without a limit on a rule with no `UNTIL` never returns, which is a
   hang rather than a wrong answer — the roadmap's risk register names it in the planner row.
   The horizon is what Phase 11 needs; the count is what makes even an `FREQ=SECONDLY` rule
   finite *inside* the horizon.
   **Measured against `rrule` 0.14.0, and it is why the "ask for exactly `MAX_OCCURRENCES`
   and read `limited`" form is wrong:** `collect_with_error` ends with
   `was_limited || limit == list.len()`, so `all(500)` on a rule with exactly 500 occurrences
   reports `limited == true`. That is a rule with no more occurrences to give recorded as
   truncated — a connection that looks broken while it is fine, which is the opposite of what
   the honesty gate asks for. Asking for one more than are kept makes a natural end report
   `false`, and the two cases that prove it have the **same occurrence count and opposite
   answers**: `FREQ=DAILY;COUNT=500` over a window wider than the horizon is not truncated,
   `FREQ=DAILY;COUNT=501` over the same window is.
4. **A233 — the ICS fetch is a second *function* on the SSRF guard, not a second guard.**
   `crates/server/src/integrations/fetch.rs` gains
   `pub async fn fetch_guarded_text(url: &str, accept: &str) -> Result<GuardedText, FetchError>`
   returning `GuardedText { final_url, body, etag: Option<String>, last_modified: Option<String> }`,
   built from `parse_target` + `resolve_public` + the same pinned-address client and redirect
   re-check as `ReqwestFetcher`, and **skipping readability**.
   *Rationale:* `ChainFetcher::fetch` runs `extract_readable`, which is right for a web page
   and destroys an ICS body; and its Exa/moli tiers are wrong for a feed. The guard is what
   must not be duplicated, so the guard is shared and the *body handling* differs.
   *Consequence, stated:* a route test cannot point the real fetcher at a local stub — the
   guard is precisely what refuses `127.0.0.1`. So the importer takes a fetcher seam, the
   route tests inject a fake, and the refusal is proven in `crates/server/tests/ssrf.rs`
   against `fetch_guarded_text` directly. A test that pointed at a local server and passed
   would be proving the guard *absent*.
5. **A234 — the calendar is a second Google OAuth client with a two-scope pair, chosen so
   the conflict policy is structural.** `GOOGLE_CALENDAR_CLIENT_ID` / `_SECRET` /
   `_REDIRECT_URI`, scope = `https://www.googleapis.com/auth/calendar.app.created` **plus**
   `https://www.googleapis.com/auth/calendar.events.readonly`, with `access_type=offline`
   and `prompt=consent`.
   *Rationale:* `calendar.app.created` can create and manage only calendars this app made —
   which is exactly "our study blocks go to a dedicated 'Study Plan' calendar" — and
   `calendar.events.readonly` can read everything else and change nothing. So "Google wins
   for events we didn't create" is enforced by the token, not by remembering a rule, and a
   bug in our merge logic cannot rewrite a user's own event. Login's client is a different
   client with different scopes and a different redirect URI; sharing one client would mean
   the login screen asking for calendar write access.
6. **A235 — the refresh token is encrypted with the login flow's own cipher, extracted once.**
   `encrypt_state`/`decrypt_state`/`cipher`/`pkce_pair`/`random_token`/`hash` move to
   `crates/server/src/auth/oauth_state.rs` (public within the crate) and `auth::google`
   calls them. The calendar's `state` carries `{verifier, nonce, exp, user_id, return_path}`
   and is encrypted for the same reason: it holds a PKCE verifier.
   `calendar_connections.refresh_token_enc` is `aes-gcm` under `ENCRYPTION_KEY_HEX`, written
   once at connect, never logged, never serialised to a client.
   *Rationale:* a second copy of an AES-GCM `state` codec is a second place to get the nonce
   handling wrong, and the login flow's version has the tests. The roadmap's crate-list
   deviation already accepts one `aes-gcm`; this keeps it one *use*, too.
7. **A236 — `sync_token` is the only sync cursor, and the mode is recorded.**
   `calendar_connections.sync_token` holds Google's `nextSyncToken`; a response without one
   is a bug we log. `calendar_connections.last_sync_was_full boolean not null default true`
   records which mode the last pass ran in, and a `410 Gone` clears `sync_token` so the
   **next** pass is a full listing.
   *Rationale:* "an incremental sync that silently stops being incremental" is this phase's
   third named risk, and it is invisible: every poll succeeds, the events stop changing, and
   nothing errors. Recording the mode makes it a value a test can assert — pass 2 must have
   sent the stored token, and the pass after a 410 must be full and the one after that
   incremental again.
8. **A237 — the "Study Plan" calendar is found-or-created once, at connect, and its id is
   stored.** `calendar_connections.study_calendar_id text`; the calendar is named exactly
   `Study Plan`; an app-owned event the user pushes goes there.
   *Rationale:* the spec's reason is "so they're easy to hide or delete" — a user who hides
   the calendar loses only the mirrored copy, never the `events` row. Finding-or-creating at
   connect (rather than lazily at first push) means a connect that cannot create one fails
   while the user is watching, instead of silently dropping the first study block.
9. **A238 — the poll is a row with a windowed dedupe key, and `POST /calendar/sync` is the
   same row.** `crates/server/src/jobs/schedule.rs` gains a calendar pass:
   for each live connection with `last_synced_at is null or last_synced_at < now() -
   CALENDAR_POLL_INTERVAL` (`900s`, env `CALENDAR_POLL_INTERVAL_SECS`), enqueue
   `sync_calendar` with `dedupe_key = 'sync_calendar:<connection_id>:<floor(now/INTERVAL)>'`
   and a `not exists` guard. `POST /calendar/sync` enqueues the same kind with
   `dedupe_key = 'sync_calendar:<connection_id>:manual:<floor(now/60)>'` and answers
   `202 { "queued": n }`.
   *Rationale:* the scheduler is already "a producer of rows, not a second worker", and a
   restart or a second instance must be a no-op rather than a double sync. The per-minute
   manual key bounds a user's own retry loop without a second rate limiter. Running the sync
   inline in the request would hold a request open across several Google round trips.
10. **A239 — the ICS connection uses a conditional GET, and a `304` advances only
    `last_synced_at`.** `calendar_connections.ics_etag` / `ics_last_modified` hold the
    validators; the fetch sends `If-None-Match` / `If-Modified-Since`.
    *Rationale:* an academic calendar is polled every 15 minutes and changes a few times a
    term. The expensive half is not the download: an unconditional re-upsert **touches every
    row**, and `events` is `+sync`, so every touched row writes a `change_log` entry and
    every device pulls the whole calendar every 15 minutes. The conditional GET is what
    keeps the poll cheap in the part that actually costs.
11. **A240 — the read-only rule for an external event lives in the routes and the push
    validator, never in a trigger.** The ICS importer and the Google sync write
    `source`, `external_id`, `etag` and `connection_id` through plain SQL, so they are
    unaffected by the rule that refuses a client the same columns (A198).
    *Rationale:* a trigger that refused the write would refuse the sync's own write, and the
    only way to keep the two apart would be a session variable — a second mechanism for a
    distinction the two paths already make structurally. `crates/server/tests/calendar.rs`
    asserts the non-obvious half: a row the sync wrote is refused by `PATCH`, and the sync
    can still write it.
12. **A241 — the feed carries app-owned rows only.** Included: `source = 'app'`,
    `deleted_at is null`, and `kind in (class, exam, study_block, deadline)` — the spec's
    "tasks with due dates, study blocks, and exams" — plus a task with a `due_at` rendered as
    a deadline event. Excluded: `kind = 'external'` and any `source != 'app'`.
    *Rationale:* including the Google/ICS rows would echo the user's own calendar back to
    their calendar client, where every event would appear twice — once from Google and once
    from us — with different `UID`s and no way to tell them apart.
13. **A242 — the feed token is stored in plaintext, and that is a threat-model row rather
    than a silent choice.** 32 random bytes, URL-safe base64, in
    `users.ics_feed_token` (unique), written by `POST /calendar/ics-token/regenerate` and
    readable again by the settings screen ("copy 'your calendar feed' link with Regenerate",
    `docs/SPEC.md` § Screens item 13).
    *Rationale:* a hash (the rule for refresh tokens and login tickets, A53) cannot be shown
    again, and the spec's screen requires the link to be available at any time. So the honest
    position is that the token is a **capability URL**: a database leak hands over every feed.
    What bounds it: the feed is read-only and contains only the student's own app-owned
    events (no sources, no chunks, no chat, no account data), it is revoked and replaced in
    one click, and `docs/THREAT_MODEL.md` § 2 gains the asset row and § 6 a residual risk.
14. **A243 — `GET /cal/:token.ics` is `Exempt` in the route sweep and sits in the auth rate
    bucket.** `Class::Exempt`'s own definition covers it ("no authenticated user at all"),
    and the route is an unauthenticated database lookup, so it is rate-limited at the same
    30/min/IP as the credential routes. It logs the resolved `user_id` and the byte length,
    **never the token**.
    *Rationale:* an unauthenticated lookup keyed by a secret is a brute-force target, and a
    secret in a `tracing` field is a secret in every log aggregator the deployment has.
15. **A244 — the calendar screen reads Phase 9's `GET /events?from=&to=` (web) or the sync
    cache (native), and the grid is a table.** The week and month views are pure functions of
    the rows in the window, unit-tested against a fixed clock and a fixed `Asia/Jakarta` zone.
    *Rationale:* this phase adds no events read route, because Phase 9's window query is
    exactly it. And a grid of absolutely-positioned `<div>`s is a grid a keyboard cannot
    enter and a screen reader cannot read; a table with one row per day and a text equivalent
    per event is what gate 5 and the spec's "charts have text equivalents" rule require.
16. **A245 — the Google account cap is a counted feature through the engine.**
    `Feature::GoogleAccount` joins `crates/core/src/entitlements/feature.rs` (and `ALL`, so
    the matrix test walks it), `check.rs` maps it to
    `counted(f, cfg.google_accounts_max, u.google_accounts, plan)`, and `UsageSnapshot` gains
    `google_accounts: u32`, counted in `entitlements::snapshot` as the live
    `calendar_connections` rows with `provider = 'google'` and a non-null
    `refresh_token_enc`.
    *Rationale:* `google_accounts_max` already exists in the matrix and in the entitlement
    view and is enforced **nowhere** — a limit that is configured but not enforced is worse
    than none, because it looks like a control (the roadmap says exactly this about the cost
    ceiling). Reading it in the connect route instead would be the second source of truth
    `route_sweep.rs::no_file_outside_the_entitlements_layer_reads_a_plan_limit` fails on.
17. **A246 — an ICS URL is validated when it is saved *and* guarded when it is fetched.**
    `POST /calendar/ics` runs `fetch::parse_target` on the body's URL before inserting the
    connection and answers 400 `validation` with a readable reason if it is not
    `http`/`https` or carries credentials. The fetch path guards again (A233).
    *Rationale:* the two checks do different jobs. Save-time validation is for the user — an
    immediate "that is not a URL we can fetch" beats a connection that silently never syncs.
    Fetch-time guarding is the protection, because DNS can change between the two.
18. **A247 — a partial import keeps what it read.** If expansion truncates, or a `VEVENT`
    fails to parse, the importer stores every event it did parse, records the reason in
    `calendar_connections.error`, and the job **succeeds**. The error is cleared by the next
    clean pass.
    *Rationale:* one malformed recurring rule in a faculty calendar must not mean the student
    gets nothing, and a job that failed would retry the same broken feed five times and
    record the same error. The screen shows the error beside the connection, which is gate 7
    (honesty): the app does not claim a connection is healthy while it holds an error.
19. **A248 — a deadline is an all-day `VEVENT` in the user's IANA zone.** `DTSTART;VALUE=DATE:`
    and `DTEND;VALUE=DATE:` set to the local date and the next local date; every other kind
    is a timed event with `DTSTART`/`DTEND` in UTC.
    *Rationale:* this is A199's handover. A deadline is `ends_at = starts_at`, and a
    zero-length timed event is either rejected or drawn as a point by most clients. A date-only
    event is what "due Friday" means. The zone comes from `users.timezone` through
    `chrono-tz`, so a Jakarta student's Friday is not a Thursday.
20. **A249 — `GET /calendar/connections` is an addition beyond the spec's route list.** It
    returns `{ connections: [{ id, provider, account_email, status, error, last_synced_at,
    study_calendar_id }] }` — **never** a token of any kind.
    *Rationale:* the spec lists connect/callback/ics/delete/sync/regenerate/feed and no list,
    so a connection made in one page load is invisible in the next and the settings screen
    cannot show its status — the same reasoning as A81 (`GET /workspaces/:id/decks`) and A125.
    `account_email` is what lets a student tell two accounts apart, and it is the one Google
    fact worth storing.

---

## File structure (new this phase)

```
migrations/0030_calendar.sql                # calendar_connections: created_at, updated_at,
                                            # status, error, ics_etag, ics_last_modified,
                                            # study_calendar_id, last_sync_was_full,
                                            # watch_channel_id, watch_expires_at
Cargo.toml                                  # + icalendar 0.17.14, rrule 0.14
crates/core/Cargo.toml                      # + icalendar, rrule
crates/core/src/traits/calendar.rs          # NEW — CalendarApi, CalendarEvent, CalendarError
crates/core/src/traits/mod.rs               # + pub mod calendar;
crates/core/src/calendar/mod.rs             # NEW
crates/core/src/calendar/feed.rs            # NEW — ICS out (pure)
crates/core/src/calendar/expand.rs          # NEW — parse + bounded recurrence (pure)
crates/core/src/calendar/policy.rs          # NEW — the conflict decision (pure)
crates/core/src/lib.rs                      # + pub mod calendar;
crates/core/tests/calendar.rs               # NEW
crates/core/src/entitlements/feature.rs     # + GoogleAccount
crates/core/src/entitlements/usage.rs       # + UsageSnapshot.google_accounts
crates/core/src/entitlements/check.rs       # + the counted arm
crates/server/Cargo.toml                    # + icalendar, rrule
crates/server/src/calendar/mod.rs           # NEW — routes()
crates/server/src/calendar/ics_out.rs       # NEW — GET /cal/:token.ics, regenerate
crates/server/src/calendar/ics_in.rs        # NEW — POST /calendar/ics, DELETE connections
crates/server/src/calendar/oauth.rs         # NEW — connect + callback
crates/server/src/calendar/google.rs        # NEW — the CalendarApi impl
crates/server/src/calendar/sync.rs          # NEW — one pass over one connection
crates/server/src/auth/oauth_state.rs       # NEW — extracted from auth/google.rs
crates/server/src/auth/google.rs            # EDIT — call the shared module
crates/server/src/integrations/fetch.rs     # EDIT — fetch_guarded_text
crates/server/src/integrations/mod.rs       # EDIT — Providers.calendar
crates/server/src/jobs/handlers/calendar.rs # NEW — sync_calendar
crates/server/src/jobs/handlers/mod.rs      # + pub mod calendar;
crates/server/src/jobs/worker.rs            # + "sync_calendar" => ...
crates/server/src/jobs/schedule.rs          # EDIT — the calendar pass
crates/server/src/config.rs                 # + GoogleCalendarConfig, CALENDAR_* env
crates/server/src/routes/mod.rs             # + .merge(calendar::routes()), the two public
crates/server/src/lib.rs                    # + pub mod calendar;
crates/server/src/entitlements.rs           # + google_accounts in the snapshot
crates/server/src/testing.rs                # + a fake CalendarApi and a fake fetcher
crates/server/tests/calendar.rs             # NEW
crates/server/tests/route_sweep.rs          # EDIT — the new rows
crates/server/tests/ssrf.rs                 # EDIT — the raw-fetch refusal
app/src/api/queries/calendar.ts             # NEW
app/src/features/calendar/calendar.ts       # NEW — the window maths (pure)
app/src/features/calendar/CalendarScreen.tsx    # NEW
app/src/features/calendar/WeekGrid.tsx      # NEW
app/src/features/calendar/MonthGrid.tsx     # NEW
app/src/features/calendar/ConnectionsPanel.tsx  # NEW
app/src/features/auth/DeepLinkHandler.tsx   # EDIT — the calendar branch
app/src/router.tsx                          # EDIT — /calendar
app/src/i18n/locales/en.json                # + the calendar keys
app/src/i18n/locales/id.json                # + the same keys
docs/{API,SYNC,ARCHITECTURE,ASSUMPTIONS,THREAT_MODEL}.md
scripts/smoke-phase10.sh                    # NEW
```

---

## Task 1: Pin the two crates, and the ICS feed as a pure function

**Files:**
- Modify: `Cargo.toml`, `crates/core/Cargo.toml`
- Create: `crates/core/src/calendar/mod.rs`, `crates/core/src/calendar/feed.rs`
- Modify: `crates/core/src/lib.rs`
- Create: `crates/core/tests/calendar.rs`

- [ ] **Step 1: Write the failing test**

`crates/core/tests/calendar.rs`:

```rust
//! The ICS feed and the bounded expansion. Both are pure, so both are tested here
//! without a database, and both fail silently in production when wrong: a feed a
//! client refuses to re-read, and an expansion that never returns.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeZone, Utc};
use stvdy_core::calendar::feed::{FeedEvent, FeedKind, render_feed};

fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
}

fn event(id: &str, created: DateTime<Utc>, updated: DateTime<Utc>) -> FeedEvent {
    FeedEvent {
        id: id.parse().unwrap(),
        kind: FeedKind::StudyBlock,
        title: "Review: jaringan komputer".to_string(),
        starts_at: at(2026, 10, 16, 2),
        ends_at: at(2026, 10, 16, 3),
        created_at: created,
        updated_at: updated,
        timezone: "Asia/Jakarta".parse().unwrap(),
    }
}

/// The two properties a client depends on and cannot recover from: the UID is
/// stable across renders, and an edit moves the SEQUENCE.
#[test]
fn a_feed_round_trips_and_the_sequence_moves_when_the_event_is_edited() {
    let id = "01920000-0000-7000-8000-000000000001";
    let created = at(2026, 10, 12, 0);
    let fresh = render_feed(&[event(id, created, created)]);
    let rendered_twice = render_feed(&[event(id, created, created)]);

    // Byte-identical: no clock reading, no map iteration order, no hash seed.
    assert_eq!(fresh, rendered_twice, "two renders of one row must be identical");

    // It parses as a calendar, and the UID is derived from the row id.
    // `read_components` gives the `Vec<Component>` directly; `Component::find_prop`
    // is the crate's own lookup, so this does not depend on the `properties` field
    // being public (verified against the 0.17.14 docs, not assumed).
    let components = icalendar::parser::read_components(&fresh).unwrap();
    assert_eq!(components.len(), 1);
    let uid = components[0].find_prop("UID").unwrap().to_string();
    assert!(uid.contains(&format!("{id}@stvdy.app")), "{uid}");

    // A new event is SEQUENCE 0, and an edit is strictly greater. Without this a
    // client that cached the feed never sees the edit.
    let seq = |ics: &str| -> i64 {
        icalendar::parser::read_components(ics).unwrap()[0]
            .find_prop("SEQUENCE")
            .unwrap()
            .to_string()
            .trim_start_matches("SEQUENCE:")
            .trim()
            .parse()
            .unwrap()
    };
    assert_eq!(seq(&fresh), 0);
    let edited = render_feed(&[event(id, created, at(2026, 10, 12, 0) + chrono::Duration::seconds(120))]);
    assert_eq!(seq(&edited), 120);
    let edited_again = render_feed(&[event(id, created, at(2026, 10, 12, 1))]);
    assert!(seq(&edited_again) > seq(&edited), "SEQUENCE must be monotone");
}

/// A deadline is date-only (A248): a zero-length timed event is drawn as a point
/// or dropped by most clients, and "due Friday" is a date.
#[test]
fn a_deadline_is_an_all_day_event_in_the_users_zone() {
    let created = at(2026, 10, 12, 0);
    let mut e = event("01920000-0000-7000-8000-000000000002", created, created);
    e.kind = FeedKind::Deadline;
    // 2026-10-15T17:00Z is 2026-10-16T00:00+07:00 — the 16th in Jakarta, the 15th in UTC.
    e.starts_at = at(2026, 10, 15, 17);
    e.ends_at = e.starts_at;
    let ics = render_feed(&[e]);
    assert!(ics.contains("DTSTART;VALUE=DATE:20261016"), "{ics}");
    assert!(ics.contains("DTEND;VALUE=DATE:20261017"), "{ics}");
}
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p stvdy-core --test calendar
```
Expected: `error[E0432]: unresolved import stvdy_core::calendar` — the module does not exist
and neither crate is pinned.

- [ ] **Step 3: Pin the crates**

In `Cargo.toml` `[workspace.dependencies]`, next to `chrono-tz`:

```toml
# --- calendar (Phase 10) ---
# Pinned for the same reason `headless_chrome` is: the feed is a document a client
# caches and the parser is what reads someone else's, so a minor upgrade must be a
# deliberate change with the acceptance test re-run.
#
# `features = ["parser"]` is REQUIRED and is NOT a default feature (verified in the
# 0.17.14 docs): without it `icalendar::parser::read_components` does not exist, so
# both the feed's round-trip test and the whole ICS import fail to compile. The
# builder half (generation) needs no feature.
icalendar = { version = "0.17.14", features = ["parser"] }
# The recurrence engine, pinned explicitly rather than used through `icalendar`'s
# re-export: the crate re-exports only a *subset* of `rrule`'s types, so relying on
# it would tie the expansion API to `icalendar`'s release cadence and could silently
# hand back a different `rrule` version than the one this plan's bound is written
# against. `all(limit) -> RRuleResult { dates, limited }` is the pair A232 depends on.
rrule = "0.14"
```

and in `crates/core/Cargo.toml`:

```toml
icalendar.workspace = true
rrule.workspace = true
```

- [ ] **Step 4: Write the module**

`crates/core/src/calendar/mod.rs`:

```rust
//! Calendar: the ICS feed, the bounded expansion of an imported feed, and the
//! conflict decision. Pure — no web, no database, no clock. The server reads rows
//! and calls in; the rules live here so they have tests that do not need a server.

pub mod feed;
```

`crates/core/src/calendar/feed.rs`:

```rust
pub enum FeedKind { Class, Exam, StudyBlock, Deadline }

pub struct FeedEvent {
    pub id: uuid::Uuid,
    pub kind: FeedKind,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub timezone: chrono_tz::Tz,
}

/// `SEQUENCE` (A230). 0 for a row that has never been edited, and the whole
/// seconds between creation and the last edit after that, floored at 1 so the
/// first edit is always an increase.
pub fn sequence(created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> i64;

/// `PRODID`, `CALSCALE:GREGORIAN`, one `VEVENT` per row, `METHOD:PUBLISH`.
/// Deterministic: no clock, no randomness, no map iteration order.
///
/// Infallible, because `icalendar`'s builder is: `Calendar::new().push(Event::new()
/// .summary(..).starts(..).ends(..).append_property(Property::new("UID", ..)))` and
/// `Display` produces the text. A deadline goes through the crate's own all-day path
/// (`Event::all_day(NaiveDate)`), which is A248 expressed as the API rather than as a
/// hand-written `VALUE=DATE` property.
pub fn render_feed(events: &[FeedEvent]) -> String;

/// RFC 5545 § 3.1: fold at 75 octets on a character boundary, with a leading
/// space on the continuation. `icalendar` folds on write, but a title with a
/// multi-byte character at the boundary is the case that mangles, so the test
/// covers it rather than trusting the crate's default.
pub fn fold_line(line: &str) -> String;
```

- [ ] **Step 5: Run the tests**

```
cargo test -p stvdy-core --test calendar
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```
feat(core): the ICS feed, with a stable UID and a SEQUENCE that moves
```

---

## Task 2: Bounded recurrence expansion — the phase's real risk

**Files:**
- Create: `crates/core/src/calendar/expand.rs`
- Modify: `crates/core/src/calendar/mod.rs`
- Modify: `crates/core/tests/calendar.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/core/tests/calendar.rs`:

```rust
/// An unbounded recurrence iteration is a hang, not a wrong answer. The cap and
/// the horizon are the two bounds, and `limited()` is the belt: it says which one
/// fired.
#[test]
fn expansion_is_bounded_and_reports_truncation() {
    // No UNTIL, no COUNT: `all()` without a limit never returns.
    let endless = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:x@f\r\n\
                   DTSTART;TZID=Asia/Jakarta:20260101T090000\r\nDTEND;TZID=Asia/Jakarta:20260101T100000\r\n\
                   RRULE:FREQ=SECONDLY\r\nSUMMARY:Bogus\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let out = expand_feed(endless, Window::days_from(at(2026, 1, 1, 0), 180)).unwrap();
    assert_eq!(out.occurrences.len(), MAX_OCCURRENCES, "the cap must bind");
    assert!(out.truncated, "a rule the cap cut must say so");

    // A weekly class over eight weeks ends on its own: the horizon binds nothing
    // and the cap does not fire.
    let weekly = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:w@f\r\n\
                  DTSTART;TZID=Asia/Jakarta:20260105T080000\r\nDTEND;TZID=Asia/Jakarta:20260105T100000\r\n\
                  RRULE:FREQ=WEEKLY;COUNT=8\r\nSUMMARY:Kuliah\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let out = expand_feed(weekly, Window::days_from(at(2026, 1, 1, 0), 180)).unwrap();
    assert_eq!(out.occurrences.len(), 8);
    assert!(!out.truncated, "a rule that ended is not a truncation");
}

/// A `TZID` is resolved through `chrono-tz`, never through the machine's zone and
/// never through a fixed offset: a Jakarta class is 07:00 UTC and a New York class
/// is the same wall clock on both sides of a DST transition.
#[test]
fn a_tzid_event_keeps_its_wall_clock_across_a_dst_boundary() {
    let jakarta = "…DTSTART;TZID=Asia/Jakarta:20260105T080000…";
    let out = expand_feed(jakarta, Window::days_from(at(2026, 1, 1, 0), 2)).unwrap();
    assert_eq!(out.occurrences[0].starts_at, at(2026, 1, 5, 1)); // 08:00+07:00

    // America/New_York: 2026-03-06 is EST (-05), 2026-03-13 is EDT (-04).
    let ny = "…DTSTART;TZID=America/New_York:20260306T090000…RRULE:FREQ=WEEKLY;COUNT=2…";
    let out = expand_feed(ny, Window::days_from(at(2026, 3, 1, 0), 30)).unwrap();
    assert_eq!(out.occurrences[0].starts_at, at(2026, 3, 6, 14)); // 09:00 EST
    assert_eq!(out.occurrences[1].starts_at, at(2026, 3, 13, 13)); // 09:00 EDT
}

/// A UTC `DTSTART` with a `Z` is not a floating time and must not be re-zoned.
#[test]
fn a_utc_event_is_taken_as_utc() { /* … */ }

/// A malformed component is skipped and named, not fatal (A247).
#[test]
fn a_malformed_vevent_is_reported_and_the_rest_survive() { /* … */ }
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p stvdy-core --test calendar expansion
```
Expected: `cannot find function expand_feed`.

- [ ] **Step 3: Write the module**

Add `pub mod expand;` to `crates/core/src/calendar/mod.rs` (the module is declared here, not
in Task 1, so Task 1's tree compiles on its own), then write
`crates/core/src/calendar/expand.rs`:

```rust
/// The two bounds (A232). The count is what makes an `FREQ=SECONDLY` rule finite
/// even inside the horizon; the horizon is what Phase 11's planner needs.
pub const MAX_OCCURRENCES: usize = 500;
pub const HORIZON_DAYS: i64 = 180;

pub struct Window { pub from: DateTime<Utc>, pub to: DateTime<Utc> }
impl Window { pub fn days_from(from: DateTime<Utc>, days: i64) -> Self; }

pub struct Occurrence {
    /// The series UID and, for a recurring series, the occurrence's start in UTC.
    pub uid: String,
    pub recurrence_id: Option<DateTime<Utc>>,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

pub struct Expansion { pub occurrences: Vec<Occurrence>, pub truncated: bool, pub skipped: Vec<String> }

pub enum ExpandError {
    NotACalendar(String),
    TooLarge(usize),
}

/// Parses and expands. **Never calls `all_unchecked()`**, and asks for one more
/// occurrence than it keeps, so `limited` is a real answer rather than a tautology.
/// The API, verified against rrule 0.14.0 by measurement rather than assumed:
/// `RRuleSet::all(limit: u16) -> RRuleResult { dates: Vec<DateTime<Tz>>, limited: bool }`,
/// where `limited == true` means the cap cut the rule. `all_unchecked()` is the unbounded
/// call this function must not contain, and the `u16` is why `MAX_OCCURRENCES` is capped at
/// 65535 by the crate rather than by this plan.
pub fn expand_feed(body: &str, window: Window) -> Result<Expansion, ExpandError>;

/// Resolves a `TZID` or a floating time. A `TZID` the tz database does not know
/// is a skipped component, not a fallback to UTC: silently treating a Jakarta
/// class as UTC moves it seven hours and looks like a scheduling bug.
///
/// `rrule::Tz` is a wrapper around `chrono_tz::Tz` (verified in 0.14's docs), so a
/// parsed `TZID` converts in and the occurrence comes back in that zone — this is
/// what makes "resolve timezones with `chrono-tz`" true rather than nominal, and
/// why the expansion is done in the event's zone and converted to UTC only at the
/// boundary. Expanding in UTC and adding an offset is the mistake that breaks at a
/// DST transition.
fn resolve_tz(raw: &str) -> Option<chrono_tz::Tz>;
```

The body-size guard (`TooLarge`) exists because a hostile or broken feed can be
hundreds of megabytes and the parse is in a worker.

**The `limited` check is the belt, and the cap is the braces — but only if you ask for one
more than you keep.** `dates.len() == MAX_OCCURRENCES` does not mean the cap fired: a rule with
exactly 500 remaining occurrences ends there naturally and is not truncated. So neither the
length comparison *nor* `limited` on a request for exactly `MAX_OCCURRENCES` distinguishes the
two, because `collect_with_error` ends with `was_limited || limit == list.len()` and therefore
sets `limited` when the collection merely *reached* the limit. Ask for `MAX_OCCURRENCES + 1`,
keep the first `MAX_OCCURRENCES`, and `limited` then means what it says. A232 records
`error = 'recurrence_truncated'` from that flag and never from a length comparison.

- [ ] **Step 4: Run the tests**

```
cargo test -p stvdy-core --test calendar
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```
feat(core): bounded recurrence expansion, and the hang it prevents
```

---

## Task 3: `migrations/0030_calendar.sql` — the connection's state

**Files:**
- Create: `migrations/0030_calendar.sql`
- Modify: `crates/server/tests/schema.rs`

- [ ] **Step 1: Confirm the number and the trigger rule**

```
ls migrations/
```
Expected: `0028_tasks_events_settings.sql` is the highest committed number, and Phase 9 adds
`0029_tasks_cascade_restore.sql` for the parent-restore marker (a defect its offline gate
found: restoring a parent left its cascade-tombstoned children deleted forever). Confirm the
highest number present and use the next one; if it is not `0030`, use the next free number and
adjust this task's filename and every reference to it in this plan.

Confirm `migrations/0017_sync_triggers.sql` is **not** in this task's diff, and that no new
table is `+sync`.

- [ ] **Step 2: Write the migration**

```sql
-- Calendar connections. `calendar_connections` and `events` come from 0013; this adds
-- the state a poll and a two-way sync need. No new table, so 0017 is untouched: `events`
-- is already in its trigger array, and `calendar_connections` is deliberately not a
-- synced entity (devices never hold a Google token, docs/SPEC.md § Calendar integration).

alter table calendar_connections
  add column created_at timestamptz not null default now(),
  add column updated_at timestamptz not null default now(),
  -- `connected | error | revoked`. `error` is not a failure of the row: the events the
  -- connection did import stay, and the screen shows the reason beside it (A247).
  add column status text not null default 'connected',
  add column error text,
  -- The ICS conditional GET's validators (A239). Without them every 15-minute poll
  -- re-upserts every row, and because `events` is +sync that writes a change_log entry
  -- per row per poll and every device pulls the whole calendar every 15 minutes.
  add column ics_etag text,
  add column ics_last_modified text,
  -- The dedicated "Study Plan" calendar (A237), found or created at connect.
  add column study_calendar_id text,
  -- Which mode the last pass ran in (A236): "an incremental sync that silently stopped
  -- being incremental" is otherwise invisible, because every poll still succeeds.
  add column last_sync_was_full boolean not null default true,
  -- Reserved for the watch channels the spec allows as an optimisation. Polling is the
  -- fallback the spec names and the one this phase implements; the columns exist so
  -- adding a channel is not a migration in a later phase.
  add column watch_channel_id text,
  add column watch_expires_at timestamptz;

-- The poll's query is "live connections whose last_synced_at is older than the
-- interval", so the index has to carry the watermark.
create index on calendar_connections (provider, last_synced_at);

-- Two Google accounts for one user are two rows, which the existing
-- `index on (user_id, provider)` already allows. The cap is `google_accounts_max`
-- through the entitlements engine (A245), not a unique index: a plan can change, and a
-- constraint that refuses the row would be a second source of truth for what a plan grants.
```

- [ ] **Step 3: Apply it and check the assertions**

Add to `crates/server/tests/schema.rs`:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn calendar_connections_carries_the_poll_and_sync_state(pool: PgPool) {
    // The columns the poll, the conditional GET and the two-way sync need, asserted by
    // name so a migration that silently omits one fails here rather than at first use.
    for column in [
        "created_at", "updated_at", "status", "error", "ics_etag", "ics_last_modified",
        "study_calendar_id", "last_sync_was_full", "watch_channel_id", "watch_expires_at",
    ] { /* information_schema.columns, assert present */ }
}

#[sqlx::test(migrations = "../../migrations")]
async fn calendar_connections_is_still_not_synced(pool: PgPool) {
    // A device must never receive a Google token or a sync cursor through the change log.
    // Asserts zero triggers on the table and that it is in NON_SYNC_TABLES.
}
```

```
cargo test -p stvdy-server --test schema
```

- [ ] **Step 4: Commit**

```
feat(db): the calendar connection's state, and the poll watermark
```

---

## Task 4: The ICS feed out, and its revocable token

**Files:**
- Create: `crates/server/src/calendar/mod.rs`, `crates/server/src/calendar/ics_out.rs`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`
- Modify: `crates/server/tests/route_sweep.rs`
- Create: `crates/server/tests/calendar.rs`

- [ ] **Step 1: Write the failing test**

`crates/server/tests/calendar.rs`:

```rust
//! The calendar's four surfaces: the feed out, the ICS import, the consent flow and
//! the two-way sync. Split by surface, not by module, because each has a failure the
//! others cannot produce: a feed a client refuses, a private host reached, a token in a
//! deep link, a sync that stopped being incremental.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
use common::{json, post_json, signed_up};

/// The feed validates, carries the caller's own rows, and a regenerated token retires
/// the old one. The last assertion is the one that matters: a revoked token that still
/// works is not revoked.
#[sqlx::test(migrations = "../../migrations")]
async fn the_feed_validates_and_a_regenerated_token_stops_working(pool: PgPool) {
    let app = signed_up(pool).await;
    // one exam, one study block, one task with a due date, one Google event
    // …

    let first = app.get_unauthenticated("/cal/{token}.ics").await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        first.headers()[CONTENT_TYPE],
        "text/calendar; charset=utf-8"
    );
    assert_eq!(first.headers()["cache-control"], "no-store");
    assert_eq!(first.headers()["x-robots-tag"], "noindex");
    let body = first.text();
    assert!(body.contains("BEGIN:VCALENDAR"));
    // The exam, the study block and the task's due date are present.
    assert!(body.contains("UAS Jaringan Komputer"));
    // The Google event is not: echoing it back would duplicate it in the user's client.
    assert!(!body.contains("Rapat himpunan"));

    // Another user's token resolves to that user's rows and never this one's.
    // …

    // Regenerate retires the old token.
    let rotated = app.post("/calendar/ics-token/regenerate", json!({})).await;
    let new_token = rotated["ics_feed_token"].as_str().unwrap();
    assert_ne!(new_token, old_token);
    assert_eq!(app.get_unauthenticated(&format!("/cal/{old_token}.ics")).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(app.get_unauthenticated(&format!("/cal/{new_token}.ics")).await.status(), StatusCode::OK);

    // A token that is not a token is 404, not 500: the lookup is an index hit or a miss.
    assert_eq!(app.get_unauthenticated("/cal/nope.ics").await.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p stvdy-server --test calendar
```
Expected: `404` for the feed route (it does not exist) and `cannot find function routes`.

- [ ] **Step 3: Write the module**

`crates/server/src/calendar/mod.rs`:

```rust
//! Calendar: the ICS feed out, the ICS import, the Google consent flow and the
//! two-way sync. `routes()` returns the authenticated half; the feed and the
//! callback are registered in `routes::mod` because they carry no bearer token.

pub mod google;
pub mod ics_in;
pub mod ics_out;
pub mod oauth;
pub mod sync;

pub fn routes() -> Router<AppState>;
/// The two routes with no authenticated user: the feed (token-authenticated) and
/// the Google callback (a browser redirect). Separate so `routes::mod` can place
/// them in `public_routes` deliberately rather than by merging everything.
pub fn public_routes() -> Router<AppState>;
```

`crates/server/src/calendar/ics_out.rs`:

```rust
/// `GET /cal/:token.ics`. No bearer token: the path token is the credential (A242).
/// Logs `user_id` and `bytes`, never the token (A243).
pub async fn feed(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Response>;

/// `POST /calendar/ics-token/regenerate`. Authenticated. Writes 32 fresh random
/// bytes, URL-safe base64, and returns `{ "ics_feed_token": "..." }` once.
pub async fn regenerate_token(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>>;

/// The rows the feed carries (A241): `source = 'app'`, not tombstoned, and
/// `kind in (class, exam, study_block, deadline)`, plus one synthetic deadline per task
/// with a `due_at`. Read in one statement with an explicit column list (A27).
async fn feed_rows(pool: &PgPool, user_id: Uuid, timezone: chrono_tz::Tz) -> AppResult<Vec<FeedEvent>>;
```

Register in `crates/server/src/routes/mod.rs`: `feed` in `public_routes` (beside
`/health`), `regenerate_token` and the rest of `calendar::routes()` in `authenticated`.
The feed route goes in the **auth limiter** bucket too — it is an unauthenticated database
lookup (A243).

- [ ] **Step 4: Add the route-sweep rows**

In `crates/server/tests/route_sweep.rs`, `exempt_rows()` gains:

```rust
exempt("GET", "/cal/{token}.ics", "token-authenticated; no bearer user, like a signed object URL"),
```
and `read_only_rows()` gains:

```rust
ro("POST", "/calendar/ics-token/regenerate", "rotating the feed token grants nothing and spends nothing"),
```

- [ ] **Step 5: Run the tests**

```
cargo test -p stvdy-server --test calendar
cargo test -p stvdy-server --test route_sweep
```

- [ ] **Step 6: Commit**

```
feat(server): the ICS feed route and its revocable token
```

---

## Task 5: The guarded raw fetch, and the ICS import

**Files:**
- Modify: `crates/server/src/integrations/fetch.rs`
- Create: `crates/server/src/calendar/ics_in.rs`
- Modify: `crates/server/src/calendar/mod.rs`, `crates/server/src/testing.rs`
- Modify: `crates/server/tests/ssrf.rs`
- Modify: `crates/server/tests/calendar.rs`

- [ ] **Step 1: Write the failing tests**

`crates/server/tests/ssrf.rs` gains:

```rust
/// The raw text fetch shares the guard with the page fetcher (A233). A second
/// implementation of the guard is the failure this asserts against.
#[tokio::test]
async fn the_guarded_text_fetch_refuses_a_private_host_and_a_non_http_scheme() {
    use stvdy_server::integrations::fetch::fetch_guarded_text;
    for url in ["http://127.0.0.1/feed.ics", "http://169.254.169.254/feed.ics",
                "http://[::ffff:169.254.169.254]/feed.ics", "file:///etc/passwd",
                "https://user:pw@example.com/feed.ics"] {
        assert!(matches!(fetch_guarded_text(url, "text/calendar").await,
                         Err(FetchError::Refused(_) | FetchError::Invalid(_))), "{url}");
    }
}
```

`crates/server/tests/calendar.rs` gains:

```rust
/// An ICS URL is imported with its recurrence, and a second poll sends the validator
/// so an unchanged feed touches nothing. The second half is the one that matters: an
/// unconditional re-upsert of a synced table writes one change_log row per event per
/// poll, and every device pulls the whole calendar every 15 minutes.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ics_url_is_imported_with_its_recurrence_and_a_poll_sends_the_validator(pool: PgPool) {
    let app = signed_up_with_fake_calendar(pool).await;
    // The fake fetcher returns a feed with a weekly class (COUNT=8) and a single exam,
    // plus `ETag: "v1"`.

    let created = app.post("/calendar/ics", json!({ "url": "https://faculty.example/academic.ics" })).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = created["connection"]["id"].as_str().unwrap();

    // Eight occurrences plus the exam, each its own row keyed by the occurrence (A231).
    assert_eq!(app.get(&format!("/events?from=2026-01-01&to=2026-06-30")).await.count(), 9);
    // Read-only in the UI and on both write paths: the row is not the client's.
    assert_eq!(app.patch(&format!("/events/{class_id}"), json!({ "title": "x" })).await.status(), StatusCode::FORBIDDEN);

    // The second fetch carries If-None-Match and a 304 changes nothing.
    app.trigger_sync(id).await;
    let calls = app.fake_calendar_calls().await;
    assert_eq!(calls[1].if_none_match.as_deref(), Some("\"v1\""));
    assert_eq!(calls[1].stored_events_delta, 0, "a 304 must not touch a row");
    assert_eq!(app.get(&format!("/calendar/connections")).await["connections"][0]["last_synced_at"].is_string(), true);
}

/// A private host is refused at save time with a readable reason, and at fetch time by
/// the guard (A246).
#[sqlx::test(migrations = "../../migrations")]
async fn an_ics_url_that_is_not_fetchable_is_refused_when_it_is_saved(pool: PgPool) {
    let app = signed_up(pool).await;
    let refused = app.post("/calendar/ics", json!({ "url": "http://169.254.169.254/feed.ics" })).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(refused["code"], "validation");
}

/// A truncated expansion keeps what it read and records the reason (A247, A232).
#[sqlx::test(migrations = "../../migrations")]
async fn a_truncated_recurrence_imports_what_it_read_and_records_the_error(pool: PgPool) { /* … */ }
```

- [ ] **Step 2: Run them and watch them fail**

```
cargo test -p stvdy-server --test ssrf guarded_text
cargo test -p stvdy-server --test calendar ics
```
Expected: `cannot find function fetch_guarded_text`, and `404` for `/calendar/ics`.

- [ ] **Step 3: Add the raw fetch**

In `crates/server/src/integrations/fetch.rs`:

```rust
pub struct GuardedText {
    pub final_url: String,
    pub body: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A user-supplied URL fetched as text, through the **same** guard as `ReqwestFetcher`
/// and without readability (A233): an ICS body is not HTML, and `extract_readable`
/// would hand back a tag-stripped document. `If-None-Match`/`If-Modified-Since` are
/// sent when the caller has validators, and a `304` is `Ok` with an empty body.
pub async fn fetch_guarded_text(
    url: &str,
    accept: &str,
    validators: Option<(&str, &str)>,
) -> Result<GuardedText, FetchError>;
```

It reuses `parse_target` + `resolve_public` + the pinned-address client + the redirect
re-check, extracted from `ReqwestFetcher::fetch` into a shared
`async fn guarded_request(url: &str, accept: &str, validators: Option<(&str, &str)>, raw: bool)`
so there is one redirect loop and not two.

- [ ] **Step 4: Write the importer**

`crates/server/src/calendar/ics_in.rs`:

```rust
/// The fetch seam (A233): the route tests inject a fake, because the real guard is
/// exactly what refuses a local stub, so a test that pointed at one would be proving
/// the guard absent.
#[async_trait]
pub trait IcsFetch: Send + Sync {
    async fn get(&self, url: &str, validators: Option<(&str, &str)>)
        -> Result<GuardedText, FetchError>;
}

/// `POST /calendar/ics`. Validates the URL (A246), inserts a `provider = 'ics'`
/// connection, and enqueues the first `sync_calendar` (A238).
pub async fn add(State(state): State<AppState>, user: AuthUser, Json(body): Json<AddIcsRequest>)
    -> AppResult<(StatusCode, Json<Value>)>;

/// `GET /calendar/connections` (A249). Never returns a token of any kind.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>>;

/// `DELETE /calendar/connections/:id`. Tombstones nothing: the connection row is deleted
/// and its events are deleted with it, because an external row has no meaning without
/// the connection that produced it. The user's own rows are untouched.
pub async fn remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>)
    -> AppResult<StatusCode>;
```

and in `crates/server/src/calendar/sync.rs` the ICS half:

```rust
/// One pass over one ICS connection: conditional GET, parse, bounded expand, upsert on
/// `(connection_id, external_id)`. A `304` advances `last_synced_at` only (A239).
pub async fn sync_ics(state: &AppState, connection: &ConnectionRow) -> AppResult<SyncOutcome>;

/// The upsert. `on conflict (connection_id, external_id) where external_id is not null
/// do update` — the partial index's predicate must be repeated, as the roadmap's
/// `dedupe_key` note records for the same reason.
async fn upsert_occurrence(tx: &mut PgConnection, connection: &ConnectionRow, o: &Occurrence)
    -> AppResult<()>;
```

- [ ] **Step 5: Run the tests**

```
cargo test -p stvdy-server --test ssrf
cargo test -p stvdy-server --test calendar
```

- [ ] **Step 6: Commit**

```
feat(server): ICS import through the same SSRF guard, with a conditional GET
```

---

## Task 6: The consent flow, on the login flow's state cipher

**Files:**
- Create: `crates/server/src/auth/oauth_state.rs`
- Modify: `crates/server/src/auth/google.rs`, `crates/server/src/auth/mod.rs`
- Create: `crates/server/src/calendar/oauth.rs`
- Modify: `crates/server/src/config.rs`, `crates/server/src/routes/mod.rs`
- Modify: `crates/server/tests/route_sweep.rs`, `crates/server/tests/calendar.rs`

- [ ] **Step 1: Write the failing test**

```rust
/// The consent URL asks for offline access, uses PKCE, and carries an encrypted state;
/// the callback returns a deep link with a status and **never a token** (A234, A235).
#[sqlx::test(migrations = "../../migrations")]
async fn the_consent_url_asks_for_offline_access_and_the_callback_never_returns_a_token(pool: PgPool) {
    let app = signed_up_with_google_calendar(pool).await;

    let started = app.get("/calendar/google/connect").await;
    let url = url::Url::parse(started["url"].as_str().unwrap()).unwrap();
    let q: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(q["access_type"], "offline");
    assert_eq!(q["prompt"], "consent");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(
        q["scope"],
        "https://www.googleapis.com/auth/calendar.app.created \
         https://www.googleapis.com/auth/calendar.events.readonly"
    );
    // The state is opaque: it holds the PKCE verifier, so it must not be readable.
    assert!(!q["state"].contains("verifier"));

    // The callback exchanges, stores an encrypted refresh token, and redirects.
    let redirect = app.get(&format!("/calendar/google/callback?code=abc&state={}", q["state"])).await;
    assert!(redirect.location().unwrap().starts_with("stvdy://oauth/calendar?status=connected"));
    let raw: Vec<u8> = /* select refresh_token_enc */;
    assert!(!String::from_utf8_lossy(&raw).contains("1//refresh"), "the token must be ciphertext");

    // A free account is refused before any of it (A245, `google_accounts_max = 0`).
    let free = signed_up_free(pool).await;
    let refused = free.get("/calendar/google/connect").await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused["code"], "plan_required");

    // The second account on pro hits the cap of 1.
    // …

    // The connections list never carries a token (A249).
    let list = app.get("/calendar/connections").await;
    assert!(!list.to_string().contains("refresh"));
}
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p stvdy-server --test calendar consent
```
Expected: `404` for `/calendar/google/connect`.

- [ ] **Step 3: Extract the shared OAuth state module**

Move `cipher`, `encrypt_state`, `decrypt_state`, `pkce_pair`, `random_token`, `hash` out of
`auth/google.rs` into `crates/server/src/auth/oauth_state.rs`, generalising the payload to
carry a `purpose`:

```rust
/// What an encrypted `state` carries. `purpose` is what keeps a login state from being
/// replayed as a calendar state: the two flows share a codec, not a trust domain.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatePayload {
    pub purpose: Purpose,       // Login | Calendar
    pub verifier: String,
    pub nonce: String,
    pub exp: i64,
    pub user_id: Option<Uuid>,  // Calendar only: the flow is started by a signed-in user
    pub device_name: String,
    pub device_platform: String,
    pub app_version: Option<String>,
}

pub fn encrypt_state(state: &AppState, payload: &StatePayload) -> AppResult<String>;
pub fn decrypt_state(state: &AppState, expected: Purpose, encoded: &str) -> AppResult<StatePayload>;
pub fn pkce_pair() -> (String, String);
pub fn random_token() -> String;
pub fn hash(value: &str) -> String;
```

`auth/google.rs` is edited to call these with `Purpose::Login`. Its tests keep passing
unchanged; `decrypt_state` refusing a mismatched `purpose` gets its own test.

- [ ] **Step 4: Write the consent flow**

`crates/server/src/config.rs` gains, parsed exactly like `google_login`:

```rust
pub struct GoogleCalendarConfig { pub client_id: String, pub client_secret: SecretString, pub redirect_uri: String }
// GOOGLE_CALENDAR_CLIENT_ID / _SECRET / _REDIRECT_URI. `None` when unset, and the
// connect route answers 404 in that case — the same rule `auth::google::start` uses, so
// a half-configured client never produces a redirect that fails confusingly at Google.
```

`crates/server/src/calendar/oauth.rs`:

```rust
/// `GET /calendar/google/connect`. Requires `Feature::GoogleAccount` through
/// `entitlements::require` (A245), then returns `{ "url": "..." }`. The SPA opens it with
/// `platform.openExternal`, i.e. the system browser, never an embedded webview.
pub async fn connect(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>>;

/// `GET /calendar/google/callback`. Public. Verifies the state, exchanges the code,
/// encrypts and stores the refresh token, finds or creates "Study Plan" (A237), and
/// redirects to `stvdy://oauth/calendar?status=connected|denied|error&reason=...`.
/// **No token of any kind travels in the deep link** (A235/A48).
pub async fn callback(State(state): State<AppState>, Query(q): Query<CallbackQuery>) -> AppResult<Redirect>;
```

Register `connect` in `authenticated`, `callback` in `public_routes`; add the sweep rows
(`connect` is `Swept` with feature `GoogleAccount`, refusal `403 plan_required`; `callback`
is `Exempt`).

- [ ] **Step 5: Run the tests**

```
cargo test -p stvdy-server --test calendar
cargo test -p stvdy-server --test auth
cargo test -p stvdy-server --test route_sweep
```

- [ ] **Step 6: Commit**

```
feat(server): the calendar consent flow, on the login flow's state cipher
```

---

## Task 7: Two-way sync — incremental, with a 410 fallback

**Files:**
- Create: `crates/core/src/traits/calendar.rs`, `crates/core/src/calendar/policy.rs`
- Modify: `crates/core/src/traits/mod.rs`, `crates/core/src/calendar/mod.rs`
- Create: `crates/server/src/calendar/google.rs`
- Modify: `crates/server/src/calendar/sync.rs`, `crates/server/src/integrations/mod.rs`,
  `crates/server/src/state.rs`, `crates/server/src/testing.rs`
- Modify: `crates/core/tests/calendar.rs`, `crates/server/tests/calendar.rs`

- [ ] **Step 1: Write the failing tests**

`crates/core/tests/calendar.rs` gains:

```rust
/// The conflict policy as a function, so it has a test rather than a comment (A234).
#[test]
fn google_wins_for_an_event_we_did_not_create_and_we_win_for_a_study_block() {
    // source = 'google': the remote is authoritative for every field.
    assert_eq!(resolve(Origin::Google, local_dirty(), remote_changed()), Winner::Remote);
    // source = 'app' mirrored to Google: the local edit is the user's, and it is pushed.
    assert_eq!(resolve(Origin::App, local_dirty(), remote_changed()), Winner::Local);
    // A remote tombstone for a Google event is honoured; for an app event it is ignored
    // and the local row is re-pushed, because Google cannot delete what it cannot write.
    assert_eq!(resolve(Origin::Google, local_clean(), remote_deleted()), Winner::Remote);
    assert_eq!(resolve(Origin::App, local_clean(), remote_deleted()), Winner::Local);
}
```

`crates/server/tests/calendar.rs` gains:

```rust
/// The two properties the roadmap's exit gate names: the sync is incremental, and a 410
/// is a full resync. `last_sync_was_full` is what makes "silently stopped being
/// incremental" assertable at all (A236).
#[sqlx::test(migrations = "../../migrations")]
async fn an_incremental_sync_sends_the_stored_token_and_a_410_resyncs_fully(pool: PgPool) {
    let app = signed_up_with_google_calendar_connected(pool).await;

    // Pass 1: no token, so a full listing. It returns `nextSyncToken: "t1"`.
    app.run_sync(conn_id).await;
    let calls = app.fake_calendar_calls().await;
    assert_eq!(calls[0].sync_token, None);
    assert_eq!(app.connection(conn_id).await["last_sync_was_full"], true);

    // Pass 2 sends t1 and stores t2. This is the assertion that fails the moment the
    // sync stops sending a token while still "succeeding" every poll.
    app.run_sync(conn_id).await;
    let calls = app.fake_calendar_calls().await;
    assert_eq!(calls[1].sync_token.as_deref(), Some("t1"));
    assert_eq!(app.connection(conn_id).await["last_sync_was_full"], false);

    // A 410 clears the token, so the next pass is full again and the one after that
    // incremental.
    app.fake_calendar_returns_gone().await;
    app.run_sync(conn_id).await;
    assert_eq!(app.connection(conn_id).await["sync_token"], Value::Null);
    app.run_sync(conn_id).await;
    let calls = app.fake_calendar_calls().await;
    assert_eq!(calls[3].sync_token, None, "the pass after a 410 is full");
    assert_eq!(app.connection(conn_id).await["last_sync_was_full"], true);
    app.run_sync(conn_id).await;
    assert_eq!(app.fake_calendar_calls().await[4].sync_token.as_deref(), Some("t3"));
}
```

- [ ] **Step 2: Run them and watch them fail**

```
cargo test -p stvdy-core --test calendar policy
cargo test -p stvdy-server --test calendar incremental
```
Expected: `cannot find function resolve`, and `cannot find function run_sync`.

- [ ] **Step 3: The trait, and the pure policy**

`crates/core/src/traits/calendar.rs`:

```rust
#[async_trait]
pub trait CalendarApi: Send + Sync {
    /// A listing. `sync_token: None` is a full listing; `Some` is incremental. A token
    /// Google has expired is `CalendarError::Gone`, which the caller answers with a full
    /// resync rather than a retry (A236).
    async fn list_events(&self, calendar_id: &str, sync_token: Option<&str>)
        -> Result<EventPage, CalendarError>;
    async fn list_calendars(&self) -> Result<Vec<CalendarMeta>, CalendarError>;
    async fn create_calendar(&self, summary: &str) -> Result<CalendarMeta, CalendarError>;
    /// `insert`/`patch` into a calendar this app created (`calendar.app.created`).
    async fn upsert_event(&self, calendar_id: &str, event: &RemoteEvent) -> Result<RemoteEvent, CalendarError>;
    async fn refresh_access_token(&self, refresh_token: &str) -> Result<AccessToken, CalendarError>;
}

pub struct EventPage { pub events: Vec<RemoteEvent>, pub next_sync_token: Option<String> }
pub struct RemoteEvent { pub id: String, pub etag: String, pub status: String, pub summary: String,
                         pub start: Option<DateTime<Utc>>, pub end: Option<DateTime<Utc>>, pub recurring_event_id: Option<String> }
pub enum CalendarError { Gone, Unauthorized(String), Transport(String), Provider(String) }
```

`crates/core/src/calendar/policy.rs`:

```rust
pub enum Origin { App, Google }
pub enum Winner { Local, Remote, Both }
/// The roadmap's conflict table, as a function (A234): Google wins for an event we did
/// not create; our own study block wins and is pushed.
pub fn resolve(origin: Origin, local: LocalState, remote: RemoteState) -> Winner;
```

- [ ] **Step 4: The Google implementation and the sync pass**

`crates/server/src/calendar/google.rs` — `GoogleCalendarApi` implementing `CalendarApi` over
`reqwest` + the refresh-token exchange at `GOOGLE_TOKEN_ENDPOINT`. It is `Option` on
`AppState` (`AppState.calendar: Option<Arc<dyn CalendarApi>>`), `None` when
`GOOGLE_CALENDAR_CLIENT_ID` is unset, mirroring `llm`/`search`/`renderer`.

`crates/server/src/calendar/sync.rs`:

```rust
pub struct SyncOutcome { pub inserted: u32, pub updated: u32, pub deleted: u32, pub was_full: bool }

/// One pass over one Google connection. Never retries a `410`: it clears `sync_token`
/// and returns `was_full: false`, so the next pass is full (A236). Records
/// `last_sync_was_full` in the same transaction as the events it wrote.
pub async fn sync_google(state: &AppState, connection: &ConnectionRow) -> AppResult<SyncOutcome>;

/// A remote event we own (`source = 'app'`, mirrored) is pushed; a remote-only one is
/// adopted read-only as `source = 'google'` (A234).
async fn reconcile(tx: &mut PgConnection, connection: &ConnectionRow, page: EventPage) -> AppResult<SyncOutcome>;
```

- [ ] **Step 5: Run the tests**

```
cargo test -p stvdy-core --test calendar
cargo test -p stvdy-server --test calendar
```

- [ ] **Step 6: Commit**

```
feat(server): Google two-way sync, incremental, with a 410 fallback
```

---

## Task 8: The poll as a row, and the manual sync

**Files:**
- Create: `crates/server/src/jobs/handlers/calendar.rs`
- Modify: `crates/server/src/jobs/handlers/mod.rs`, `crates/server/src/jobs/worker.rs`,
  `crates/server/src/jobs/schedule.rs`, `crates/server/src/calendar/mod.rs`
- Modify: `crates/server/tests/calendar.rs`, `crates/server/tests/route_sweep.rs`

- [ ] **Step 1: Write the failing test**

```rust
/// The poll is a row with a windowed dedupe key (A238): a restart, a second instance or
/// a double tick is a no-op rather than a double sync. And the manual sync is the same
/// row, so a user holding the button cannot open a sync per press.
#[sqlx::test(migrations = "../../migrations")]
async fn the_poll_job_is_dedupe_guarded_and_a_manual_sync_is_rate_limited(pool: PgPool) {
    let app = signed_up_with_google_calendar_connected(pool).await;

    // Two passes inside one window enqueue one job.
    app.run_scheduler_pass().await;
    app.run_scheduler_pass().await;
    assert_eq!(app.queued_jobs("sync_calendar").await, 1);

    // A connection synced inside the interval is not re-enqueued.
    app.mark_synced_now(conn_id).await;
    app.run_scheduler_pass().await;
    assert_eq!(app.queued_jobs("sync_calendar").await, 1);

    // The handler updates the watermark and re-enqueues nothing itself.
    app.run_one_job().await;
    assert!(app.connection(conn_id).await["last_synced_at"].is_string());

    // The manual route enqueues the same kind and answers 202.
    let manual = app.post("/calendar/sync", json!({})).await;
    assert_eq!(manual.status(), StatusCode::ACCEPTED);
    assert_eq!(manual["queued"], 1);
    assert_eq!(app.post("/calendar/sync", json!({})).await["queued"], 0, "one per minute per connection");

    // A free account cannot sync a Google connection it cannot have.
    // …

    // An unknown kind is still a permanent failure, not a panic.
}
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p stvdy-server --test calendar poll
```
Expected: `cannot find function run_scheduler_pass`.

- [ ] **Step 3: Write the handler and the scheduler pass**

`crates/server/src/jobs/handlers/calendar.rs`:

```rust
/// `sync_calendar`: one connection, one pass. A Google connection that lost its refresh
/// token marks itself `revoked` and **succeeds** (retrying a revoked grant five times
/// cannot un-revoke it); a transport failure is transient.
pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure>;
```

`jobs/worker.rs::dispatch` gains `"sync_calendar" => crate::jobs::handlers::calendar::run(state, job).await,`.

`jobs/schedule.rs` gains `pub async fn calendar_pass(state: &AppState) -> AppResult<u64>` and
calls it from the existing ticker beside `pass`. It enqueues
`sync_calendar` with `dedupe_key = 'sync_calendar:<connection_id>:<floor(now/INTERVAL)>'`
and the same `not exists` guard the boundary pass uses.

`POST /calendar/sync` in `crates/server/src/calendar/sync.rs`:

```rust
/// `POST /calendar/sync`. Enqueues the same kind the poll uses, with a per-minute
/// dedupe key, and answers `202 { "queued": n }`. Rate-limited by the auth limiter.
pub async fn manual_sync(State(state): State<AppState>, user: AuthUser) -> AppResult<(StatusCode, Json<Value>)>;
```

Sweep rows: `POST /calendar/sync` is `Swept` (feature `GoogleCalendar`, 403 `plan_required`);
`POST /calendar/ics` and `DELETE /calendar/connections/{id}` are `ReadOnly` (an ICS
connection is available on free, and deleting one is the remedy a refusal names).

- [ ] **Step 4: Run the tests**

```
cargo test -p stvdy-server --test calendar
cargo test -p stvdy-server --test route_sweep
cargo test -p stvdy-server --test jobs
```

- [ ] **Step 5: Commit**

```
feat(server): the calendar poll, as rows the existing worker runs
```

---

## Task 9: The read-only rule on both write paths, for the rows the sync writes

**Files:**
- Modify: `crates/server/tests/calendar.rs`

- [ ] **Step 1: Write the failing test**

```rust
/// Phase 9's A198 refuses a *client* the four calendar-owned columns. This asserts the
/// other half, which is the one that can silently break: the sync's own write is not
/// affected by that rule, because the rule lives in the routes and the push validator
/// and not in a trigger (A240).
#[sqlx::test(migrations = "../../migrations")]
async fn a_calendar_sourced_event_is_read_only_to_a_client_and_writable_by_the_sync(pool: PgPool) {
    let app = signed_up_with_google_calendar_connected(pool).await;
    let synced = app.sync_one_remote_event("g-1", "Rapat himpunan").await;

    // The client cannot edit it, delete it, or push it.
    assert_eq!(app.patch(&format!("/events/{synced}"), json!({ "title": "x" })).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(app.delete(&format!("/events/{synced}")).await.status(), StatusCode::FORBIDDEN);
    let pushed = app.push(json!({ "entity": "Event", "op": "upsert", "id": synced, "payload": { "title": "x" } })).await;
    assert_eq!(pushed["results"][0]["code"], "read_only_entity");

    // And the sync can move it, because the same event changed in Google.
    app.fake_calendar_returns_changed("g-1", "Rapat himpunan (baru)").await;
    app.run_sync(conn_id).await;
    assert_eq!(app.get(&format!("/events/{synced}")).await["title"], "Rapat himpunan (baru)");
    assert_eq!(app.get(&format!("/events/{synced}")).await["source"], "google");
}
```

- [ ] **Step 2: Run it and watch it fail**

```
cargo test -p stvdy-server --test calendar read_only
```
Expected: fails on the last block — the update path either refuses the sync's own write or
fails to update the row.

- [ ] **Step 3: Make it pass**

If the sync's write is refused, the rule has leaked into a trigger or into a shared
validator. Move it back to the route and the push validator. No production change is
expected here; the test is the guard, and the failure it catches is a later refactor that
"tidies" the check into the wrong layer.

- [ ] **Step 4: Run the tests**

```
cargo test -p stvdy-server --test calendar
cargo test -p stvdy-server --test events
cargo test -p stvdy-server --test tamper
```

- [ ] **Step 5: Commit**

```
test(server): the sync writes the columns the routes refuse, and the rule stays in the routes
```

---

## Task 10: The SPA — the week and month views, and the connections panel

**Files:**
- Create: `app/src/api/queries/calendar.ts`, `app/src/features/calendar/calendar.ts`,
  `app/src/features/calendar/CalendarScreen.tsx`, `WeekGrid.tsx`, `MonthGrid.tsx`,
  `ConnectionsPanel.tsx`
- Modify: `app/src/router.tsx`, `app/src/features/auth/DeepLinkHandler.tsx`,
  `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json`,
  `app/src/features/a11y.test.tsx`

- [ ] **Step 1: Write the failing tests**

`app/src/features/calendar/calendar.test.ts`:

```ts
// The window maths is a pure function of a fixed clock and an explicit zone, because a
// week boundary computed in the machine's zone is the defect that is invisible in
// Jakarta and wrong in London.
describe('the calendar window', () => {
  it('is the local week, Monday first, in the user zone', () => { /* … */ })
  it('covers every day of the local month, including the DST-short one', () => { /* … */ })
  it('places a 23:30+07:00 event on the right local day', () => { /* … */ })
})

describe('an external event', () => {
  it('renders a lock and no edit affordance', () => { /* … */ })
  it('is announced as read-only to a screen reader', () => { /* … */ })
})
```

`app/src/features/a11y.test.tsx` gains `CalendarScreen`.

- [ ] **Step 2: Run them and watch them fail**

```
npm --prefix app test -- calendar
```
Expected: `Failed to resolve import "@/features/calendar/calendar"`.

- [ ] **Step 3: Write the screen**

`app/src/features/calendar/calendar.ts` — pure: `localWeek(now: Date, zone: string): Day[]`,
`localMonth(now: Date, zone: string): Day[]`, `placeEvents(events: Event[], days: Day[], zone: string)`.
Every zone comes from `Intl.DateTimeFormat` with an explicit `timeZone`, never the machine's.

`app/src/features/calendar/CalendarScreen.tsx` — the week/month toggle, the grid, and the
connections panel. The grid is a **table** (A244): one row per day, a caption naming the
week, and each event a cell with its time as text plus a lock icon and an
`aria-label` for an external row. Native reads the sync cache; web reads
`GET /events?from=&to=`.

`app/src/features/calendar/ConnectionsPanel.tsx` — `GET /calendar/connections`,
"Connect Google Calendar" through `platform.openExternal(connect.url)` (the **system
browser**, never an embedded webview), the "your calendar feed" link with **Copy** and
**Regenerate**, the "Add an ICS URL" field, and a per-connection status line that shows the
error when there is one (gate 7: the panel must not claim a connection is healthy while it
holds an error). On free, "Connect Google Calendar" is a `LockedFeature` with
`google_calendar`'s minimum plan.

`app/src/features/auth/DeepLinkHandler.tsx` gains the `calendar` branch: `status=connected`
refreshes the connections query and toasts; `denied`/`error` shows the reason.

- [ ] **Step 4: Run the tests and the build**

```
npm --prefix app test
npm --prefix app run build
node scripts/check-no-cdn.mjs
```

- [ ] **Step 5: Commit**

```
feat(app): the week and month calendar, and the connections panel
```

---

## Task 11: Docs, the three gates, and the smoke script

**Files:**
- Modify: `docs/API.md`, `docs/SYNC.md`, `docs/ARCHITECTURE.md`,
  `docs/ASSUMPTIONS.md`, `docs/THREAT_MODEL.md`
- Create: `scripts/smoke-phase10.sh`

- [ ] **Step 1: `docs/API.md`**

Replace `## Calendar (Phase 10)`'s placeholder with the real section: the eight routes, the
`ConnectionView` shape (A249), the feed's `Content-Type`/`Cache-Control`/`X-Robots-Tag`
(A241/A242), the token-rotation rule, the 400 `validation` for an unfetchable ICS URL
(A246), the 202 + `{queued}` of `POST /calendar/sync` (A238), the 410-inside-the-sync note
(never surfaced as an HTTP status to the client, because the client is not the one holding
the token), and one paragraph naming the two-scope consent (A234).

- [ ] **Step 2: `docs/SYNC.md`**

Add a row to the conflict table for calendar-sourced events: `events` whose `source != 'app'`
are **server-owned for the calendar columns** and read-only to a client on both write paths,
while `title`/`starts_at`/`ends_at` on an app-owned event keep the existing LWW +
field-merge rule. Note that the sync writes through plain SQL and is deliberately not
subject to the client rule (A240), and that the ICS/Google rows carry no new `+sync` table,
so `0017_sync_triggers.sql` is unchanged.

- [ ] **Step 3: `docs/ARCHITECTURE.md`**

Add `## Calendar (Phase 10)`: the three pieces and why the split is where it is (the feed is
a pure function of rows; the import is parse + a bounded expand + one upsert keyed on the
occurrence; the two-way sync is a trait implementation behind a seam); the bound and the
hang it prevents (A232); the guard shared with ingestion and why `ChainFetcher` is not the
entry point for a feed (A233); the conditional GET and the `change_log` churn it avoids
(A239); and the scope pair that makes the conflict policy structural (A234).

- [ ] **Step 4: `docs/ASSUMPTIONS.md`**

Append A230–A249 as rows in the pinned-decisions table. Add to "Schema deviations" one row
for `0030_calendar.sql`'s nine columns on `calendar_connections`. Note under "Crate-list
deviations" that `icalendar`/`rrule` are now **pinned in `Cargo.toml`**, not only recorded
here — the deviation row was aspirational until this phase — and that `icalendar` needs its
non-default `parser` feature for the import half.

- [ ] **Step 5: `docs/THREAT_MODEL.md`**

- § 2 Assets: add `ICS feed tokens` — "read the user's own calendar feed; a capability URL",
  living in `users.ics_feed_token` (plaintext, because the settings screen must be able to
  show the link again, A242).
- § 5: add "Reach a private address through an ICS URL" → **Fails.** `fetch_guarded_text`
  shares `parse_target`/`resolve_public` with ingestion, and every redirect hop is
  re-checked (A233).
- § 6: add **R7 — the feed token is a capability URL.** A leaked token reads the student's
  app-owned events (never sources, chunks, chat or account data), and it is revoked and
  replaced in one click. *Revisit if:* the feed ever carries anything beyond the user's own
  app-owned events.
- § 9: add the row `ICS feed cannot be brute-forced or indexed` → Phase 10 →
  `crates/server/tests/calendar.rs` (rate-limited, `no-store`, `noindex`, and a regenerated
  token 404s the old one).

- [ ] **Step 6: Roadmap gate 5 — accessibility, on the calendar screen**

Measured in a real browser, both `prefers-color-scheme` values, at 390x844 and 1365x900, on
the week view, the month view, the connections panel and the event detail:

- **(a) keyboard only** — `Tab` reaches the week/month toggle, every event cell in order,
  the "Connect Google Calendar" button, the feed link's Copy and Regenerate, and the ICS
  field. `Escape` closes the event detail and returns focus to the cell that opened it.
- **(b) contrast** — computed WCAG ratios for the day heading, the event title, the time
  label, the lock icon, the connection status line and the error line, dark **and** light,
  against 4.5 (3.0 for large text). The grid's tinted event chips are the pair most likely
  to fail; they are checked, not assumed.
- **(c) 44px targets** — every event cell and both icon buttons at phone width. The month
  view's cells are the tight case and are measured.
- **(d) no state by colour alone** — an external event carries a lock **and** an
  `aria-label` saying read-only; a connection with an error says the reason in words; the
  "today" cell says "Today".
- **(e) 200% text scale at 390px** — the week grid reflows to one day per row with no
  clipping, no overlap and no horizontal page overflow. A seven-column grid is exactly the
  shape that refuses to shrink, so the reflow is the finding to record either way.

- [ ] **Step 7: Roadmap gate 6 — Indonesian end to end**

- **Server.** `an_indonesian_ics_feed_imports_with_the_right_wall_clock`: a feed whose
  `SUMMARY` is `UAS Jaringan Komputer` and whose `DTSTART;TZID=Asia/Jakarta:20260610T080000`
  imports to `2026-06-10T01:00:00Z`, and the `title` stored is the Indonesian string
  unmangled (the folding test at a multi-byte boundary is the one that fails silently).
- **Server.** `the_feed_renders_an_indonesian_title_and_the_date_only_deadline_in_jakarta`:
  a deadline at `2026-06-09T17:00Z` is `DTSTART;VALUE=DATE:20260610`.
- **UI.** Driven in the browser: an Indonesian event title renders in the grid with no
  clipping at 200% scale, and a `2026-06-10T01:00Z` class appears on the 10th in
  `Asia/Jakarta` while a UTC implementation would put it on the 9th.
- The silent failures this catches, named: a `TZID` resolved through the machine's zone
  (a class seven hours out), an unfolded long line (a mangled title), and a date-only
  deadline computed in UTC (a Friday deadline landing on Thursday).

- [ ] **Step 8: Roadmap gate 7 — honesty**

- **(a)** No number on the screen is invented: the week and month views show rows from the
  cache or from `GET /events`, and "last synced" is `calendar_connections.last_synced_at`
  rendered, not a client-side guess.
- **(b)** Each number traces: the event count is the rows in the window, "N connections" is
  the rows in `GET /calendar/connections`, and the truncated-recurrence notice is the
  connection's stored `error` rather than a computed estimate.
- **(c)** `honesty.composerHint` is not applicable to this screen (no composer), and the plan
  says so rather than leaving the box unticked.
- **(d)** No action on this screen is generative and none spends credits; the panel says so
  in one line rather than showing a `0` cost, because a cost badge that is always zero is
  noise.
- **(e)** Nothing on this screen is an AI proposal. The "Study Plan" blocks it shows were
  written by the planner (Phase 11) and are ordinary events; a Phase 11 proposal is labelled
  there, not here.

- [ ] **Step 9: The smoke script**

`scripts/smoke-phase10.sh` — signup, copy the feed token, fetch `/cal/<token>.ics` and
assert `text/calendar` and one `VEVENT` for a created event, regenerate and assert the old
token 404s, add an ICS URL pointing at a local file served by the script's own stub (with
`ICS_ALLOW_PRIVATE=1` set **only in the script's env**, and the script printing that it did
so), assert the recurrence imported, connect Google against the fake (`CALENDAR_FAKE=1`),
run a manual sync and assert the second pass sent the stored `syncToken`, then force a 410
and assert the next pass was full. Follows `scripts/smoke-phase5.sh`'s shape: it reads the
one key it needs from `.env` (not `source .env`, which Phase 5's notes record as a hazard),
and ends with `=== PASS`.

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
| smoke | `bash scripts/smoke-phase10.sh` |

- [ ] **Step 11: Commit**

```
docs(phase 10): the calendar contract, and the three gates
```

---

## Phase 10 exit checklist

| # | Gate | Command | Result |
|---|---|---|---|
| 1 | `/cal/:token.ics` validates and carries the caller's own app-owned rows | `cargo test -p stvdy-server --test calendar feed` | |
| 2 | A regenerated feed token retires the old one | the same test, assertion 3 | |
| 3 | The feed is `text/calendar`, `no-store`, `noindex` | the same test, assertions 2 | |
| 4 | An external ICS is imported with recurrence and timezones | `cargo test -p stvdy-server --test calendar ics` | |
| 5 | A second poll sends the validator and a 304 touches nothing | the same test, assertion 2 | |
| 6 | A private host is refused at save and at fetch | `cargo test -p stvdy-server --test ssrf` + `--test calendar refused` | |
| 7 | Expansion is bounded and reports truncation | `cargo test -p stvdy-core --test calendar expansion` | |
| 8 | Google two-way sync is incremental | `cargo test -p stvdy-server --test calendar incremental` | |
| 9 | A 410 triggers a full resync, and the pass after it is incremental again | the same test, assertion 3 | |
| 10 | A calendar-sourced event is read-only to a client on both write paths | `cargo test -p stvdy-server --test calendar read_only` | |
| 11 | The sync can still write the columns the routes refuse | the same test, assertion 4 | |
| 12 | The consent URL is offline + PKCE + the two scopes; the callback returns no token | `cargo test -p stvdy-server --test calendar consent` | |
| 13 | A free account is 403 `plan_required` on connect, and the account cap binds on pro | the same test, assertions 3–4 | |
| 14 | The poll is dedupe-guarded and the manual sync is bounded | `cargo test -p stvdy-server --test calendar poll` | |
| 15 | Every new route is classified | `cargo test -p stvdy-server --test route_sweep` | |
| 16 | `0017` is still the last trigger migration and `calendar_connections` is not synced | `cargo test -p stvdy-server --test schema` | |
| 17 | The week/month windows are unit-tested against a fixed clock and zone | `npm --prefix app test -- calendar` | |
| 18 | Roadmap gate 5 (accessibility) measured and recorded | Task 11 Step 6 | |
| 19 | Roadmap gate 6 (Indonesian) measured and recorded | Task 11 Step 7 | |
| 20 | Roadmap gate 7 (honesty) stated and recorded | Task 11 Step 8 | |
| 21 | `docs/{API,SYNC,ARCHITECTURE,ASSUMPTIONS,THREAT_MODEL}.md` updated in the same commits | — | |
| 22 | The full quality gate is green | Task 11 Step 10 | |

---

## Execution notes

To be filled in as the phase is built, in the shape Phases 5, 6 and 9 use: what the plan did
not anticipate, each with the file that proved it; the gate measurements' actual numbers; the
verification table with real counts; and a `NOT VERIFIED HERE` line naming what the phase
could not exercise. At minimum that line will name: **the live Google Calendar API** (the
tests drive a fake, and the real endpoint needs a verified OAuth client that does not exist
in this repo), **the real Google `410`** (the fake returns it on demand; the real one arrives
after a long idle period), **watch channels** (the columns exist, the channel does not), and
**the native iOS and Android calendar screens** (no simulator run in this phase).

---

## Self-Review

**Spec coverage, item by item.** `docs/SPEC.md` § Calendar integration:

- "Google Calendar stays SERVER-SIDE: one connection per account works on every device" →
  Tasks 6, 7, 8; the connection row is not synced (Task 3's schema assertion), and the SPA
  never holds a token (A235/A249).
- "OAuth2 with offline access, minimal scopes, PKCE + state parameter; refresh tokens
  encrypted (aes-gcm)" → Task 6 (`access_type=offline`, the two-scope pair A234, `S256`, the
  encrypted state and the aes-gcm `refresh_token_enc`).
- "Incremental sync via syncToken; on 410 do a full resync" → Task 7, with `last_sync_was_full`
  making "incremental" an asserted value rather than an assumption (A236).
- "Optionally register watch channels (webhook) with polling via jobs as fallback" → the
  columns exist (Task 3); **polling is the implemented path** (A238), and the optional half
  is recorded as not built rather than implied.
- "Maintain our own `events` rows with external_id + etag" → Task 5's and Task 7's upsert on
  `(connection_id, external_id)`, with `etag` stored for the conditional logic.
- "Conflict policy: Google wins for events we didn't create" → Task 7's `policy.rs` plus the
  scope pair that makes it structural (A234).
- "our study blocks go to a dedicated 'Study Plan' calendar so they're easy to hide or
  delete" → Task 6's find-or-create and `study_calendar_id` (A237).
- "Google/ICS events are read-only in the UI (lock icon)" → Task 10's lock + `aria-label`,
  and Task 9's assertion on both write paths.
- "Native OAuth flow: open the system browser (never an embedded webview), return via deep
  link … validate state and PKCE" → Task 6 uses `platform.openExternal`, the existing
  `stvdy://oauth/calendar` classifier (`src-tauri/src/deeplink.rs:109`) and the shared
  `decrypt_state` with a `purpose` check.
- "ICS feed: `GET /cal/:token.ics` outputs tasks with due dates, study blocks, and exams.
  Token is per-user, revocable, regenerable" → Task 4 (A241/A242/A248).
- "ICS import/subscribe: user adds an external ICS URL … poll periodically; treat events as
  busy time. Support recurring events (rrule) and timezones correctly" → Tasks 2, 5, 8;
  "busy time" is what `kind = 'external'` means to Phase 11's planner, and this plan says so
  rather than adding a column.
- `docs/SPEC.md` § Routes, Calendar → the eight routes; the addition (`GET /calendar/connections`)
  is A249, and `DELETE /calendar/connections/:id` is the spec's own disconnect.
- `docs/SPEC.md` § Screens item 13, "Calendar connections (Google status, add ICS URL, copy
  'your calendar feed' link with Regenerate)" → Task 10's `ConnectionsPanel`.

**Placeholders:** none. The choices the spec and the roadmap leave open are pinned rather
than waved at: the feed's UID and SEQUENCE (A230), the occurrence key (A231), the two bounds
and the `limited()` check (A232), the fetch seam and why it is a second function on the guard
(A233), the scope pair (A234), the cipher's home (A235), the sync cursor and the recorded
mode (A236), the Study Plan calendar (A237), the poll as a row (A238), the conditional GET
(A239), where the read-only rule lives (A240), what the feed excludes (A241), the token's
storage and its threat-model row (A242), the feed's classification and rate bucket (A243),
the grid as a table (A244), the account cap through the engine (A245), the two validation
points (A246), partial-import behaviour (A247), the all-day deadline (A248), and the one
route added beyond the spec (A249).

**Type consistency:** `FeedEvent`, `FeedKind`, `Occurrence`, `Expansion`, `Window`,
`ExpandError`, `GuardedText`, `IcsFetch`, `CalendarApi`, `EventPage`, `RemoteEvent`,
`CalendarError`, `Origin`, `Winner`, `SyncOutcome`, `ConnectionRow`, `StatePayload`,
`Purpose`, `AddIcsRequest`, `GoogleCalendarConfig` are each defined once and referenced by
name from the server and the SPA. `Feature::GoogleCalendar`, `Feature::GoogleAccount`,
`EntitlementError`, `UsageSnapshot`, `AppError`, `Failure`, `ClaimedJob`, `Entity::Event`,
`merge_patch`, `FetchedPage`, `FetchError`, `parse_target`, `resolve_public`,
`platform.openExternal`, `DeepLinkIntent::Calendar` and `LockedFeature` are reused, never
redefined.

**The phase's real risk, and its mechanical defence.** The risk is that **the calendar is
the first phase whose failures are all invisible from a laptop with a working network.**
Three of them, each with a defence that fails loudly:

- **The unbounded recurrence iteration is a hang, not a bug** — the roadmap's risk register
  names it in the planner row and it applies verbatim here. A single faculty feed with
  `RRULE:FREQ=SECONDLY` and no `UNTIL` takes the job worker down for every user. The defence
  is `all(MAX_OCCURRENCES)` **plus** the `limited()` check (never an inference from the
  count) **plus** the horizon, and Task 2's first test is an endless rule that must return.
- **The feed token is a capability URL** — a token in a share sheet, a screenshot or a
  referrer is a token someone else has. The defence is `Cache-Control: no-store` +
  `X-Robots-Tag: noindex`, the token never entering a log line (A243), one-click revocation
  with the old token asserted dead (Task 4), a feed that carries nothing but the user's own
  app-owned events (A241), and a threat-model row that says so out loud instead of leaving it
  to be discovered (A242).
- **An incremental sync that silently stops being incremental** — every poll succeeds, the
  events stop changing, and nothing errors. The defence is `last_sync_was_full` recorded per
  pass and a test that asserts pass 2 sent the stored token, that a `410` produced exactly
  one full pass, and that the pass after it was incremental again (Task 7).

Two more, closed the same way: a second OAuth implementation (A235 — the cipher is extracted,
so there is one) and a second SSRF guard (A233 — the guard is shared and the refusal is
tested against `fetch_guarded_text` itself, in `ssrf.rs`, because a route test cannot point
the real guard at a local stub).

**What this plan does not do, deliberately:** it does not re-plan `events` CRUD or the four
server-owned columns (Phase 9's, stated in "What Phase 9 hands over"); it does not add a
`+sync` table or touch `0017_sync_triggers.sql`; it does not implement watch channels (the
columns exist, the spec calls them optional and names polling as the fallback); it does not
build the planner or its busy-time model (Phase 11); it does not add a `GET /events` route
(Phase 9's); and it does not add a credits charge anywhere, because nothing in this phase
calls a model.

**Open items:**
- B1 (QRIS gateway docs) is still needed at Phase 13, not before.
- A real Google OAuth client with the two calendar scopes must exist before this phase can be
  verified against Google. Until then the fake is what the tests drive, and the execution
  notes must say so under `NOT VERIFIED HERE` rather than implying a live run.
- `rrule`'s `all(limit)` takes a `u16`, so `MAX_OCCURRENCES` is capped at 65535 by the crate
  rather than by this plan; the 500 here is a product choice, and the horizon is the bound
  that makes it rarely matter.
