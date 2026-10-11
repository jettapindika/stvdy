# Phase 1. Auth, devices, ingestion, native spike. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** an account created on one device, verified by email, usable on a second
device with rotating refresh sessions; a PDF (or pasted text) uploaded to a
workspace becomes chunks with embeddings and reports `ready`; the native shell
stores its tokens in the OS keychain and completes a `stvdy://` deep-link round
trip; the mobile capability spike answers the push/share/import questions
*before* Phase 14 depends on them.

**Exit gate (observable, from the roadmap):**
signup → verify → login on two "devices"; PDF upload → chunks + embeddings →
`ready`; iOS + Android device builds proving push-token registration and file
intake, results in `docs/ARCHITECTURE.md`.

---

## Global constraints inherited from the roadmap

- Server is the only source of truth. No client calls a provider directly.
- No `unwrap()`/`expect()`/`panic!` in non-test code (`clippy.toml` + workspace lints).
- All times `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
- Every query scoped to the authenticated `user_id`. No cross-tenant reads.
- Never log secrets or tokens. Refresh tokens stored as hashes only.
- Quality gate at the end of the phase: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test`, web build, desktop build.
- UI language English; Indonesian documents must ingest and chunk correctly
  (gate 6 is a Phase 2 gate, but the chunker is written here, so it carries the
  Indonesian sentence-boundary test now).

## Decisions this plan pins (so parallel work cannot diverge)

1. **SQL style: runtime queries only.** Phase 0 has an empty `.sqlx` cache and CI
   runs `cargo sqlx prepare --workspace --check`; every Phase 1 query is
   `sqlx::query` / `query_as::<_, T>` with an explicit `FromRow` struct and an
   explicit column list. No `query!` macros, no `SELECT *`. Rationale: sqlx 0.9
   macros cannot map `vector`/`tsvector` (roadmap § Risk register), and the
   offline cache then stays empty and honest.
2. **Limits are read from `AppConfig::plans`, never hardcoded.** Phase 1 needs
   five limits (`devices_max`, `workspaces`, `sources_total`, `max_file_mb`,
   `storage_mb`) before the entitlements engine exists (Phase 7). Phase 1 calls
   `crates/server/src/limits.rs` helpers that read `PlanConfig` directly and
   return the same `AppError::Entitlement` shape the engine will return, so
   Phase 7 swaps the call sites without changing a response body. The shim lives
   in the server, not in `crates/core/src/entitlements/`, so that directory is
   created exactly once, by Phase 7, with its own API and tests.
3. **Object storage has two backends behind one trait.** `StorageBackend::S3`
   (`aws-sdk-s3`; MinIO fork in dev, Cloudflare R2 in prod, A15) and
   `StorageBackend::Local` (a directory, `var/objects`). A15 killed the
   `minio/minio` images and Docker is not a build dependency of this repo's
   tests, so the local backend is what makes ingestion testable in CI and on a
   laptop. `STORAGE_BACKEND=local|s3`, default `local` in dev. Presigned URLs
   are `Storage::presign_get`; the local backend returns a signed, expiring
   `/sources/:id/file?exp=…&sig=…` URL handled by the server, so the *client
   contract is identical* on both backends.
4. **The bucket is created by the server at boot** (idempotent `create_bucket`),
   not by a compose `minio-init` sidecar: A15 removed the pullable `mc` image,
   and R2 needs the bucket created out-of-band anyway.
5. **PDF text extraction is `pdfium-render`** (roadmap Phase 1, A2), loaded from
   `PDFIUM_LIB_PATH` (default `var/pdfium/libpdfium.dylib` on macOS,
   `libpdfium.so` on Linux). If the path is unset the server still boots and a
   PDF ingest fails the *job* with a clear message; if the path is set but the
   library cannot be loaded, boot fails fast (a configured-but-broken extractor
   is worse than an absent one). DOCX/PPTX/URL ingestion is Phase 7; Phase 1
   accepts `pdf` and `text` and rejects the rest with `validation`.
6. **`Embedder` is `fastembed` `MultilingualE5Base`** (A2), 768 dims, with the
   `query: `/`passage: ` prefixes applied by the impl, never stored in the text.
   Boot asserts `EMBEDDING_DIM == Embedder::dim()` **and** that both equal the
   `chunks.embedding` column dimension read from `information_schema`.
7. **Access token carries no plan** (A22). Claims: `sub`, `did`, `sid`, `iat`,
   `exp`, `jti`. HS256 with `JWT_SECRET`. 15 min TTL.
8. **Refresh token is opaque**, 32 random bytes, base64url, stored only as
   SHA-256 hex in `auth_sessions.refresh_token_hash`, one row per rotation in a
   `family_id`. Reuse of a rotated token revokes the whole family (401).
9. **Rate limits in Phase 1 are `tower_governor` per-IP buckets on `/auth/*`**
   (A12: per-instance, documented as such). A43's DB-backed per-IP/per-domain
   signup limiter is a Phase 7 gate and is *not* built here; Phase 1 records the
   dependency instead of shipping a control that looks like one.
10. **`email_verified` gates spending, not login** (A43). Phase 1 has no spending
    endpoint, so it only records `email_verified`/`email_verified_at`; the
    `require_verified` extractor lands with the first spending route (Phase 2)
    and is asserted by the Phase 7 tamper test.
11. **Google login (A48) is Task 8**, after the local auth pair works, because it
    issues the *same* pair. `oauth2` 5.0.0 cannot verify an OIDC `id_token`
    (it has no JWT/JWKS support), so verification is `jsonwebtoken` +
    `jsonwebtoken`-decoded JWKS fetched from `jwks_uri` with a cached, key-id
    keyed set.
12. **Migrations stay append-only and `0017_sync_triggers.sql` stays last.** The
    only new table is `email_verification_tokens` (not `+sync`), so it goes in
    `0018_email_verification.sql` with no trigger and no sync columns.

---

## File structure (new this phase)

```
crates/core/src/
  traits/{mod.rs,embedder.rs,storage.rs,clock.rs,mailer.rs,testing.rs}
  chunk.rs                              # sentence-safe ~480/50 chunker
crates/core/tests/{traits.rs,chunk.rs}
crates/server/src/
  integrations/{mod.rs,storage.rs,embedder.rs,mailer.rs,clock.rs}
  limits.rs                             # Phase 1 plan-limit shim (Phase 7 replaces)
  auth/{mod.rs,routes.rs,tokens.rs,password.rs,email_verify.rs,extract.rs,google.rs}
  devices.rs  workspaces.rs  chunks.rs
  sources/{mod.rs,routes.rs,ingest.rs,parse.rs,status.rs}
  jobs/{mod.rs,worker.rs,handlers/{mod.rs,ingest.rs}}
  ratelimit.rs
crates/server/tests/{auth.rs,devices.rs,workspaces.rs,sources.rs,google.rs}
migrations/0018_email_verification.sql
app/src/api/{client.ts,errors.ts,queries/{auth.ts,workspaces.ts,sources.ts}}
app/src/features/auth/{SignInScreen.tsx,SignUpScreen.tsx,VerifyEmailScreen.tsx,useSession.ts}
app/src/lib/tokens.ts
src-tauri/src/deeplink.rs
docs/API.md  docs/ARCHITECTURE.md  docs/ASSUMPTIONS.md   # updated in place
```

---

## Task 1: `crates/core` provider traits + deterministic doubles

**Files:** `crates/core/src/traits/{mod.rs,embedder.rs,storage.rs,clock.rs,mailer.rs,testing.rs}`,
`crates/core/src/lib.rs`, `crates/core/Cargo.toml` (add `bytes`).

- [ ] **Step 1: Write the traits** exactly as the roadmap § Cross-Cutting Contracts
  § 1 specifies for `Embedder`, `Storage`, `Clock`, plus `Mailer`
  (`async fn send(&self, msg: Email) -> Result<(), MailError>`, `Email { to,
  subject, text, html: Option<String> }`). `Mailer` is a server-side capability
  like the others, so its trait lives here even though the roadmap's file list
  omits it; the omission is a file-list gap, not a design statement.
- [ ] **Step 2: Write the doubles** in `testing.rs`: `FixedClock`, `HashEmbedder`
  (deterministic, `dim = 768`, same text ⇒ same vector, different text ⇒
  different vector, unit norm), `InMemoryStorage` (with a working presign that
  returns a deterministic opaque string), `RecordingMailer` (captures messages,
  no IO).
- [ ] **Step 3: Test the invariants** in `crates/core/tests/traits.rs`:
  `HashEmbedder` is deterministic and dimension-correct; `InMemoryStorage` round
  trips put→get→presign_get→delete; `RecordingMailer` captures; `FixedClock`
  does not advance.

**Acceptance:** `cargo test -p stvdy-core` green; no IO, no web, no DB in `core`.

## Task 2: `crates/core/src/chunk.rs`. Sentence-safe chunker

**Files:** `crates/core/src/chunk.rs`, `crates/core/tests/chunk.rs`.

- [ ] **Step 1: Pin the config.** `ChunkConfig { target_tokens: 480,
  max_tokens: 512, overlap_tokens: 50 }` (A2: the model's `max_length` is 512).
  `chunk(text, cfg, count: &dyn Fn(&str) -> usize) -> Vec<String>` where `count`
  is injected so the server can pass the real tokenizer and tests pass a
  deterministic counter.
- [ ] **Step 2: Sentence-safe packing.** Split on Unicode sentence boundaries
  (`unicode_segmentation::UnicodeSegmentation::split_sentence_bounds`), pack
  sentences greedily until adding the next would exceed `target_tokens`, carry
  `overlap_tokens` of trailing context into the next chunk, and split a single
  over-long sentence on word boundaries so nothing exceeds `max_tokens`. Never
  emit an empty or whitespace-only chunk; never split mid-word.
- [ ] **Step 3: Tests.** English fixture; an **Indonesian** fixture (the
  roadmap's language gate depends on this chunker, and Indonesian has different
  sentence punctuation conventions); overlap present; no chunk exceeds
  `max_tokens`; concatenation covers every input sentence; empty input ⇒ no
  chunks; determinism (two runs, identical output).

**Acceptance:** `cargo test -p stvdy-core` green including the Indonesian case.

## Task 3: Migration `0018` + dev storage/infra wiring

**Files:** `migrations/0018_email_verification.sql`, `docker-compose.yml`,
`.env.example`, `justfile` (only if a recipe changes).

- [ ] **Step 1: Write the migration.** `email_verification_tokens(id uuid pk
  default gen_random_uuid(), user_id uuid not null references users on delete
  cascade, token_hash text not null unique, expires_at timestamptz not null,
  used_at timestamptz, created_at timestamptz default now())`, index on
  `(user_id, expires_at)`. Not `+sync`: a client must never see or replay a
  verification token.
- [ ] **Step 2: Fix the compose storage service** to a pullable image (A15:
  `minio/minio` and `minio/mc` 404). Use `pgsty/silo` for the server, drop the
  `minio-init` sidecar (the server creates the bucket at boot, decision 4), and
  update the comment to name A15 and the reason.
- [ ] **Step 3: Extend `.env.example`** with `STORAGE_BACKEND`, `STORAGE_LOCAL_DIR`,
  `OBJECTS_PUBLIC_BASE_URL`, `PRESIGN_TTL_SECS`, `PDFIUM_LIB_PATH`,
  `MAIL_DIR`, `ACCESS_TOKEN_TTL_MIN`, `REFRESH_TOKEN_TTL_DAYS`,
  `VERIFY_TOKEN_TTL_HOURS`, `GOOGLE_LOGIN_{CLIENT_ID,CLIENT_SECRET,REDIRECT_URI}`,
  `FASTEMBED_CACHE_DIR`. Mirror into `.env` for the local run.
- [ ] **Step 4: Apply and verify** `sqlx migrate run` on the local Postgres, then
  `\d email_verification_tokens`.

**Acceptance:** migration applies on a fresh database; compose no longer
references a nonexistent image.

## Task 4: Server foundations. config, state, error, integrations

**Files:** `crates/server/src/config.rs`, `state.rs`, `error.rs`,
`integrations/{mod.rs,storage.rs,embedder.rs,mailer.rs,clock.rs}`,
`crates/server/Cargo.toml`.

- [ ] **Step 1: Config.** Add typed sections to `AppConfig` (`storage`, `auth`,
  `mailer`, `embedder`, `pdfium_lib_path`, `google`), all with the same
  `from_env_at` override mechanism so tests never mutate the process env
  (Phase 0 execution note 11). `Mailer` selection: `SMTP_URL` set ⇒ SMTP via
  `lettre`, else the dev sink writing `var/mail/<ts>-<to>.eml`.
- [ ] **Step 2: Error mapping.** Extend `AppError::Entitlement` with
  `limit: Option<i64>`/`used: Option<i64>` and map `limit_reached` → **422**
  (roadmap § 5), keeping `insufficient_credits` → 402 and the rest → 403. Add
  `AppError::EmailNotVerified` → 403 `email_not_verified` for Phase 2 to use.
- [ ] **Step 3: State.** `AppState { pool, config, storage: Arc<dyn Storage>,
  embedder: Arc<dyn Embedder>, mailer: Arc<dyn Mailer>, clock: Arc<dyn Clock>,
  jobs: jobs::Queue }`.
- [ ] **Step 4: `integrations::storage`.** S3 (`aws-sdk-s3`, path-style for
  MinIO, presign GET/PUT with TTL) and local (`var/objects/<sha256[0..2]>/<key>`,
  `presign_get` → HMAC-signed URL verified by the file route). Boot asserts the
  bucket exists (S3: `create_bucket` if absent).
- [ ] **Step 5: `integrations::embedder`.** fastembed `MultilingualE5Base` with
  the A2 prefixes; dim assertion against `EMBEDDING_DIM` **and** the column
  dimension read from `information_schema.columns` (fail fast, A2).
- [ ] **Step 6: `integrations::mailer`.** `lettre` SMTP impl + dev sink impl.
- [ ] **Step 7: `integrations::clock`.** `SystemClock`.

**Acceptance:** `cargo clippy -p stvdy-server --all-targets -- -D warnings` clean;
a config test asserts the new fields load from overrides and that a prod boot
with a missing `PDFIUM_LIB_PATH` is still allowed (lazy, decision 5).

## Task 5: Auth. password, tokens, routes, verification mail

**Files:** `crates/server/src/auth/*`, `crates/server/src/ratelimit.rs`,
`crates/server/tests/auth.rs`.

- [ ] **Step 1: `password.rs`.** argon2id, parameters from a single const, hash +
  verify; a verify against a *dummy* hash when the user does not exist, so login
  timing does not reveal account existence.
- [ ] **Step 2: `tokens.rs`.** `issue_pair(pool, user_id, device_id) ->
  TokenPair { access, refresh, expires_at }`; `rotate(pool, refresh_plain, device_id)
  -> TokenPair`; `verify_access(token) -> AccessClaims`. Rotation revokes the
  presented row and inserts a new one in the same family; presenting a
  **revoked** row revokes every row in that family and returns
  `AppError::Unauthorized`. `logout` revokes one family, `logout-all` revokes
  every family for the user.
- [ ] **Step 3: `email_verify.rs`.** Issue a token (32 random bytes, base64url,
  stored SHA-256), mail a link `{WEB_ORIGIN}/verify?token=…`, and consume it
  idempotently (`used_at` set once, `expires_at` enforced, single-use).
- [ ] **Step 4: `extract.rs`.** `AuthUser` extractor (bearer → claims → user row,
  rejecting a revoked device), `VerifiedUser` (adds the `email_verified` check;
  used by Phase 2's spending routes).
- [ ] **Step 5: `routes.rs`.** `POST /auth/signup` (email, password, device name/
  platform; creates user + device + session; mails verification),
  `POST /auth/login`, `POST /auth/refresh`, `POST /auth/logout`,
  `POST /auth/logout-all`, `POST /auth/verify-email`. All under a
  `tower_governor` per-IP bucket (decision 9).
- [ ] **Step 6: `tests/auth.rs`.** signup → verification mail captured (dev sink)
  → `verify-email` → `email_verified` true and `email_verified_at` set; login
  returns a pair; refresh rotates and the **old** refresh is rejected and kills
  the family (the reused token's *sibling* is also dead); logout-all kills every
  family; wrong password is 401 with the same body shape as an unknown email;
  a second device is a new session; `devices_max` reached ⇒ 422 `limit_reached`
  with `min_plan`.

**Acceptance:** `cargo test -p stvdy-server --test auth` green; no token value
appears in any log line (assert by grepping the test's captured logs).

## Task 6: Devices + workspaces

**Files:** `crates/server/src/devices.rs`, `crates/server/src/workspaces.rs`,
`crates/server/tests/{devices.rs,workspaces.rs}`.

- [ ] **Step 1: Devices.** `GET /devices` (name, platform, last_seen_at, this
  device flagged), `PATCH /devices/:id` (rename), `DELETE /devices/:id` (revoke:
  `revoked_at` + revoke its sessions), `PUT /devices/:id/push-token`
  (`push_tokens` upsert).
- [ ] **Step 2: Workspaces.** CRUD scoped by `user_id`, tombstone delete
  (`deleted_at`), `workspaces` limit from the plan config (decision 2).
- [ ] **Step 3: Tests.** A revoked device's access token is 401 on the next
  request; renaming another user's device is 404 (never 403: do not leak
  existence); a workspace list never contains another user's row; the
  workspace limit trips at 2 on free with `limit_reached`.

**Acceptance:** `cargo test -p stvdy-server` green.

## Task 7: Sources, storage, jobs worker, ingestion

**Files:** `crates/server/src/sources/*`, `crates/server/src/chunks.rs`,
`crates/server/src/jobs/{mod.rs,worker.rs,handlers/*}`, `crates/server/tests/sources.rs`.

- [ ] **Step 1: Job queue + worker.** `jobs::Queue::enqueue(pool, kind, payload,
  dedupe_key?)` + `Notify`; worker claims with `for update skip locked`, marks
  `running`, runs the handler, `done` on success, `attempts += 1` + exponential
  `run_at` on failure (→ `failed` past the cap), and recovers `running` rows
  orphaned by a restart (boot sweep, since a crash mid-job would otherwise park
  the row forever).
- [ ] **Step 2: `POST /workspaces/:id/sources`** (multipart: `kind`, `file` |
  `text`, `title?`). Validate: workspace ownership; kind ∈ {pdf, text};
  extension/mime for `pdf`; `max_file_mb`; `storage_mb` quota (`sum(file_size)`
  over live sources); `sources_total`; then `Storage::put` and insert the
  `sources` row (`status='pending'`), enqueue `ingest_source`.
- [ ] **Step 3: `ingest_source` handler.** `status='processing'` → fetch bytes →
  extract text with page numbers (`parse.rs`: pdfium for PDF, identity for text)
  → `chunk()` with the real tokenizer count → batch `embed()` (32) → insert
  `chunks` rows, copying the vector from an existing row with the same
  `(embedder, content_hash)` **within the same user** (migration 0007's index is
  deliberately non-unique; the lookup is per-user) → `status='ready'`, or
  `status='failed'` with the readable `error`.
- [ ] **Step 4: Reads.** `GET /workspaces/:id/sources`,
  `GET /sources/:id/status` (status + `chunk_count` + `error`),
  `DELETE /sources/:id` (tombstone + storage delete + chunks cascade),
  `GET /sources/:id/file` (short-lived presigned URL),
  `GET /chunks/:id` (owner-scoped chunk text + page).
- [ ] **Step 5: Tests.** text source → `ready` with N chunks and 768-dim
  vectors; the same content uploaded twice reuses embeddings (assert by
  comparing the stored vector of the duplicate chunk to the original) and does
  not violate any constraint; a 2 MB file on a 20 MB-per-file free plan is 400;
  a file that pushes past `storage_mb` is 422; another user's source id is 404
  on every read; a corrupt PDF yields `failed` with a readable error and the job
  is not retried forever.
- [ ] **Step 6: Smoke the real path** with `cargo run -p stvdy-server` against
  the local Postgres: signup → verify → create workspace → upload a real PDF →
  poll status to `ready` → `GET /chunks/:id`.

**Acceptance:** the Step 6 smoke output is quoted in the phase summary.

## Task 8: Sign-in with Google (A48)

**Files:** `crates/server/src/auth/google.rs`, `crates/server/tests/google.rs`,
`docs/API.md`.

- [ ] **Step 1: `GET /auth/google/start`** → 302 to Google with `state` (random,
  stored in a short-lived signed cookie or a `state` table row keyed by hash) and
  PKCE `S256`; `GET /auth/google/callback` verifies `state`, exchanges the code,
  verifies the `id_token` (issuer `https://accounts.google.com`, audience = our
  client id, `exp`/`iat`, `nonce`, `email_verified`), then links by `google_sub`
  → verified email → create.
- [ ] **Step 2: Security rules as tests.** A password account is **never**
  silently taken over by a Google login with the same address (link requires a
  signed-in session or a confirmation step); a Google-only account cannot set a
  password before verifying its email; a token with an edited plan claim changes
  nothing downstream.
- [ ] **Step 3: Issue the same pair** via `auth::tokens::issue_pair`, so
  everything downstream (devices, refresh, logout) is unchanged.

**Acceptance:** unit tests cover state mismatch, `email_verified: false`, wrong
audience, and the takeover refusal. A live round trip needs real Google
credentials (B-blocker class, env-only) and is reported as such, not faked.

## Task 9: Native shell. deep-link round trip + keychain token storage

**Files:** `src-tauri/src/deeplink.rs`, `src-tauri/src/lib.rs`,
`src-tauri/src/commands.rs`, `app/src/platform/*`.

- [ ] **Step 1: Deep-link round trip (macOS).** Emit the URL to the frontend on
  both cold start and a running instance (`single-instance` re-emit is already
  registered), and expose `on_open_url` through the `platform` adapter so the
  SPA never touches a Tauri API directly (Phase 0's boundary test enforces this).
- [ ] **Step 2: Token storage.** `secret_set/get/delete` already exist; add the
  SPA-side `tokens.ts` that uses them on desktop/mobile and `localStorage` on web
  with the documented caveat that the web build has no keychain.
- [ ] **Step 3: Prove it.** Run the debug shell, open `stvdy://auth/callback?…`,
  observe the event and the stored secret; record the exact command and output in
  `docs/ARCHITECTURE.md` § Native prototypes.
- [ ] **Step 4: iOS simulator build** (`tauri ios build`) if the toolchain is
  present; record the result either way.

## Task 10: SPA. auth screens, workspaces, upload

**Files:** `app/src/api/*`, `app/src/features/auth/*`, `app/src/lib/tokens.ts`,
`app/src/features/workspaces/*`, `app/src/router.tsx`.

- [ ] **Step 1: API client.** `fetch` wrapper: bearer access token, automatic
  single-flight refresh on 401 with one retry, typed error mapping to the
  documented `{code,message,min_plan?}` shape, and SSE-ready scaffolding (Phase 2).
- [ ] **Step 2: Auth screens** (sign up, sign in, verify email, signed-in
  session bootstrap) wired to the real endpoints; the verification link route
  reads `?token=`.
- [ ] **Step 3: Workspaces + sources.** List/create/rename/delete, workspace
  detail with the source list, a PDF/text upload with progress, and status
  polling until `ready`/`failed` with the readable error.
- [ ] **Step 4: Accessibility gate** (roadmap gate 5): keyboard-only reach with a
  visible focus ring, contrast on the default dark **and** light themes, ≥44px
  targets at phone width, no state by colour alone, 200% text scale without
  clipping. State the check and the result in the phase summary.

## Task 11: Docs and the phase gate

- [ ] **Step 1: `docs/API.md`** — the real Phase 1 request/response shapes for
  auth, devices, workspaces, sources, chunks, Google login.
- [ ] **Step 2: `docs/ARCHITECTURE.md`** — § Native prototypes Phase 1 results
  (pass *and* fail), § Provider traits table rows that Phase 1 fills in.
- [ ] **Step 3: `docs/ASSUMPTIONS.md`** — record the Phase 1 decisions that are
  not already rows (local storage backend for dev, pdfium load policy, the
  Phase 1 limit-reading shim that Phase 7 replaces, `email_verification_tokens`).
- [ ] **Step 4: `docs/SYNC.md`** — only if a protocol/conflict rule changed
  (Phase 1 should not change one; state that explicitly if so).
- [ ] **Step 5: Run the gate.** `cargo fmt --all -- --check`;
  `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo test --workspace`; `npm --prefix app test`;
  `npm --prefix app run build`; `cargo fmt/clippy/test` for `src-tauri`;
  `cargo sqlx prepare --workspace --check` with `SQLX_OFFLINE=true`.
- [ ] **Step 6: Phase summary**, including the mobile capability spike results
  and anything still blocked on the user.

---

## Phase 1 exit checklist

- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo test --workspace` green (Phase 0's 62 + this phase's).
- [ ] `npm --prefix app test` and `npm --prefix app run build` green.
- [ ] `src-tauri` fmt/clippy/test green; keychain round trip still passes.
- [ ] Fresh DB: `sqlx migrate run` applies `0001`–`0018`, **34** sync triggers
  still exist, and `email_verification_tokens` exists with no trigger.
- [ ] `cargo sqlx prepare --workspace --check` exits 0 with `SQLX_OFFLINE=true`.
- [ ] Signup → verify → login on two devices, with the second device visible in
  `GET /devices`.
- [ ] Refresh rotation: a replayed refresh token kills its family (test + curl).
- [ ] PDF upload → `ready` with chunks and 768-dim embeddings (smoke output
  quoted).
- [ ] Same content twice reuses embeddings; no cross-tenant read anywhere.
- [ ] Deep-link round trip recorded in `docs/ARCHITECTURE.md` (macOS; iOS
  simulator result recorded either way).
- [ ] Mobile capability spike: every probe in the roadmap's table recorded as
  pass/fail, with what is still blocked on the user named explicitly.
- [ ] Accessibility gate result stated, not asserted.
- [ ] Phase summary posted; wait for "continue".

## Self-Review

- **Spec coverage.** SPEC `docs/SPEC.md:597` (Phase 1) is fully covered:
  auth + devices + rotating refresh + email verification, workspaces, source
  upload to object storage, ingestion worker (PDF + text), chunking, embeddings,
  status polling, keychain + deep-link prototype. The mobile spike is the
  roadmap's addition and is covered by Task 9/11 with an explicit blocked path.
  Google login (A48) is covered by Task 8.
- **Deliberate scope boundaries.** DOCX/PPTX/URL ingestion, the entitlements
  engine, the DB-backed A43 signup limiter, `POST /sync/pull|push|stream`, chat,
  and the updater are *not* here; each is a later phase's contract. Phase 1
  leaves a call site or a doc note where it would otherwise be tempting to
  half-build them.
- **Risk.** fastembed's model (1.06 GB) and `ort`'s prebuilt runtime download are
  the two network-dependent pieces; both are cache-able and both are reported,
  not faked, if unavailable. pdfium's dylib is the Docker packaging risk the
  roadmap already flags; the local backend (decision 3) keeps ingestion testable
  without it.
- **Type consistency.** `Storage`, `Embedder`, `Clock`, `Mailer`, `TokenPair`,
  `AccessClaims`, `AppError::Entitlement`, `Queue` are defined once and
  referenced by name in every later task and phase.

## Execution notes (Phase 1, as built)

Deviations and discoveries from running the plan. The plan text above is left as
written so the next phase can see what changed and why.

1. **`pdfium-render` binds once per process, so the handle is bound at boot.**
   `Pdfium::bind_to_library` returns `PdfiumLibraryBindingsAlreadyInitialized`
   on a second call, because the bindings live in a process-wide slot. A
   per-document bind therefore works for the first PDF and fails for every one
   after it. The Phase 1 smoke run caught it (the second uploaded PDF came back
   `failed: PdfiumLibraryBindingsAlreadyInitialized`), and the fix moved the
   failure from first use to boot: `integrations::documents::init` binds once and
   `handle()` shares the result. A52/A51 record the policy.
2. **The `jobs.dedupe_key` index is partial, so the `ON CONFLICT` target must
   repeat the predicate.** `on conflict (dedupe_key) do update` fails with
   "there is no unique or exclusion constraint matching the ON CONFLICT
   specification"; the working form is `on conflict (dedupe_key) where dedupe_key
   is not null`. A test would not have caught this (the enqueue path is
   exercised only at runtime), which is why it was found by the smoke run.
3. **`sum(bigint)` is `numeric` in Postgres and sqlx will not decode it into
   `i64`.** The storage-quota query needs an explicit `::bigint` cast. Caught by
   the sources suite on the first upload.
4. **Axum's default body limit is 2 MB.** A multipart upload over it is rejected
   as a *parse* error, not as a plan limit, which would report a 20 MB plan as
   "invalid multipart". The sources router raises the limit to the largest plan's
   `max_file_mb` plus framing, so the per-plan check is what produces the precise
   error.
5. **`lettre`'s `tokio1-rustls` feature is not sufficient on its own.** Without a
   crypto provider and a certificate store the build fails with "feature `rustls`
   also requires ...". `tokio1-rustls-tls` is the feature that pulls both.
6. **`secrecy` 0.10 renamed `expose()` to `expose_secret()`**, and
   `argon2` 0.6 moved `SaltString` into the `phc` crate with a different
   `hash_password` signature (it now generates its own salt). `crates/core`'s
   doubles and `crates/server/src/auth/password.rs` reflect the 0.6 API.
7. **The auth extractor must check `auth_sessions.revoked_at`.** The first
   version joined the family without that predicate, so `logout-all` and
   refresh-token reuse revocation returned 200 on the very next request. The auth
   suite caught it (`logout_all_revokes_every_session_of_the_account`); the
   clause is now load-bearing and commented as such.
8. **A retryable/permanent failure split was necessary, not cosmetic.** A missing
   pdfium library fails identically on every attempt, so retrying it five times
   left the source `processing` for half a minute and blocked the queue behind
   it. `jobs::Failure` now carries the flag and a permanent failure ends the job
   immediately (A55).
9. **The dev mail sink is what makes the auth suite possible.** `RecordingMailer`
   (a double) captures the message, so a test reads the verification link from
   memory. The file sink (`var/mail/*.eml`) is what the smoke script reads, and
   both exist because B3 keeps real SMTP credentials an operational blocker.
10. **The local storage backend needed a signed URL shape that is honest about
    itself.** The download route is `/objects/:id?exp=&sig=` rather than
    overloading `/sources/:id/file`, because the latter is the authenticated JSON
    endpoint that hands out a URL: two different things must not share a path in
    a log. The signature covers `source_id:exp`, not the host, so a deployment
    behind a different origin can rewrite the host without invalidating it.
11. **`#[sqlx::test]` cases run in parallel threads, so an `AppError::Internal`
    reaches a test as a bare 500.** `testing::TestState::new` installs a
    best-effort tracing subscriber with `with_test_writer()` for that reason; it
    is what turned the `sum(...)::bigint` failure into a readable cause instead of
    a mystery.
12. **The workspace now has runtime SQL only.** Every Phase 1 query is
    `sqlx::query`/`query_as` with an explicit column list, so `.sqlx` stays empty
    and `cargo sqlx prepare --workspace --check` passes with `SQLX_OFFLINE=true`.
    That is the consequence of decision 1, and it is why the CI lint job can keep
    compiling without a database.

### Phase 1 verification (as executed)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **136 passing**, 0 failing |
| `npm --prefix app test` | **85 passing** (15 files) |
| `npm --prefix app run build` | clean (399 kB / 124 kB gzip) |
| `src-tauri` fmt + clippy + test | clean; **12 passing** (10 deep-link, 2 keychain) |
| `cargo sqlx prepare --workspace --check` (`SQLX_OFFLINE=true`) | exits 0, `.sqlx` empty |
| Fresh DB `sqlx migrate run` | 18 migrations apply; **34** sync triggers; no trigger on the two new tables |
| End-to-end smoke (`scripts/smoke-phase1.sh`) | PASS: signup, verification mail, second device, workspace, text source and a **real PDF** (cupsfilter) both `ready` with chunks, `GET /chunks/:id`, signed download byte-identical to the upload, tampered signature 403, cross-account 404, refresh rotation, reuse revocation |
| Native shell | `stvdy://` round trip **PASS** on macOS for a running instance and cold start; iOS simulator **BLOCKED** (missing simulator runtime); keychain webview-to-Rust hop **PARTIAL** |

### Mobile capability spike (roadmap Phase 1, do not defer to 14)

| Probe | Result |
|---|---|
| Deep-link round trip, macOS, running instance | **PASS** (`action="auth_callback" stage="event"`) |
| Deep-link round trip, macOS, cold start | **PASS** (`action="auth_callback" stage="launch"`) |
| Foreign scheme / unknown route ignored, no credential logged | **PASS** |
| Keychain adapter -> command -> OS keychain | **PARTIAL**: both hops verified separately; the live webview IPC hop was not observable from the CLI |
| iOS simulator build | **BLOCKED**: no simulator runtime installed on this machine |
| APNs / FCM token registration | **NOT ATTEMPTED**: needs a device build, which needs the simulator or a physical device |
| iOS document intake / share sheet | **NOT ATTEMPTED**: same prerequisite |

The three `NOT ATTEMPTED` probes are the roadmap's "Phase 14 prerequisite"
category and are recorded as such rather than assumed. They need either a
simulator runtime or a physical device to answer, which is a user-side
prerequisite, not a code change.

### Phase 1 exit checklist

- [x] `cargo fmt --all -- --check` clean.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] `cargo test --workspace` green (136).
- [x] `npm --prefix app test` and `npm --prefix app run build` green.
- [x] `src-tauri` fmt/clippy/test green; keychain round trip still passes.
- [x] Fresh DB: `sqlx migrate run` applies `0001`-`0019`, **34** sync triggers
  still exist, and neither new table has one.
- [x] `cargo sqlx prepare --workspace --check` exits 0 with `SQLX_OFFLINE=true`.
- [x] Signup -> verify -> login on two devices, with both visible in
  `GET /devices`.
- [x] Refresh rotation: a replayed refresh token kills its family (suite + smoke).
- [x] PDF upload -> `ready` with chunks and 768-dim embeddings (smoke output
  quoted in the phase summary).
- [x] Same content twice reuses embeddings; no cross-tenant read anywhere.
- [x] Deep-link round trip recorded in `docs/ARCHITECTURE.md`.
- [x] Mobile capability spike: every probe recorded, and the three that need a
  device are named as such.
- [x] Accessibility gate result stated, not asserted (SPA report: keyboard-only
  walk with measured focus rings, 44px targets, contrast, 200% text scale).
- [ ] Phase summary posted; wait for "continue".
