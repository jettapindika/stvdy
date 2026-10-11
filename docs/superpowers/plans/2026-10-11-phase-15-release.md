# Phase 15. Release. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`
> (the Phase 15 row and its exit gate, the **Launch prerequisites** risk-register row,
> § 1 Provider traits, § 8 Job kinds) and in `docs/SPEC.md` (§ PLATFORMS AND ARCHITECTURE
> "Distribution", § AUTH AND SECURITY "Account deletion", § QUALITY BAR, the Phase 15
> line). This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug. Every place this plan
> *does* deviate from a contract is listed in "Decisions this plan pins" with a number
> and a one-line justification.
>
> **`docs/RELEASE.md` is the normative release document.** This plan produces it. The
> plan file is the execution record; `docs/RELEASE.md` is what an operator reads at
> 03:00, and every fact below that an operator needs lives there too.

**Goal:** a signed, notarized macOS universal build and a signed Windows installer that
update themselves from signed manifests; a server container that runs the whole product
— a pinned Chromium, pdfium, the embedding model, the print assets, a document
converter — with no network dependency at render time; store submission checklists for
iOS and Android; and the launch prerequisites the roadmap files as A43/A44/A45 and
B1/B2/B3 written down as operations an operator can actually perform.

**Architecture:** this phase adds almost no product behaviour and a great deal of
*artifact* behaviour, so its architecture is custody and provenance rather than data
flow:

- **Two build systems, one version.** The desktop shell and the server are separate
  binaries with separate release cadences. The **shell** is versioned by
  `src-tauri/tauri.conf.json` (which already carries `0.1.0`) and `CARGO_PKG_VERSION`;
  the **server** is versioned by the workspace `Cargo.toml` and reports it from
  `/health`. A release tag names both, and `docs/RELEASE.md` says which of the two a
  hotfix may ship alone (the server, because a client cannot be recalled).
- **The trust chain runs from a key that is not in this repository.** A Tauri update is
  trusted because the manifest is signed by a minisign key whose public half is compiled
  into the app; the app is trusted because the OS verified its code signature; the code
  signature is trusted because a certificate authority issued it to the operator. Every
  link in that chain is either a file in this repo (the public key, the artifact paths,
  the verification script) or an operator secret that must never enter it (the private
  keys, the notarization credentials). This plan keeps that line sharp: **every task that
  needs a secret says so, names what degrades without it, and puts the secret in GitHub
  Actions secrets or the operator's keychain, never in a file this plan creates.**
- **The container is the only place the server's native dependencies exist.** Three of
  them (Chromium, pdfium, the ONNX embedder) are loaded at runtime from paths the code
  reads from the environment (`CHROME_PATH`, `PDFIUM_LIB_PATH`, `FASTEMBED_CACHE_DIR`),
  and each has a documented absent-binary policy (A51, A129/A130, A2). The image's job is
  to make all three present and **pinned**, and its verification is that the same
  acceptance test the laptop runs also runs **inside the image**.

**Tech Stack:** Docker (multi-stage, `debian:bookworm-slim`), GitHub Actions, Tauri 2's
`tauri-plugin-updater` 2.13.3 + `createUpdaterArtifacts`, `codesign`/`notarytool`/
`stapler` (macOS, on the runner), NSIS via Tauri's own bundler plus `osslsigncode` for
verification, `minisign` (Tauri's signer), `rust-embed` for the served print assets,
Cloudflare R2 (`aws-sdk-s3`, unchanged), LibreOffice (`soffice --headless`). No new Rust
dependency in `crates/` except the `rust-embed` the asset route needs.

**Exit gate (observable, from the roadmap):** signed + notarized macOS universal, signed
Windows installer, an updater with signed manifests, store checklists, server Docker
image.

**The three roadmap gates, and which apply here.** This phase is mostly packaging, and
saying so is more honest than re-running gates on screens it did not touch:

- **Gate 5 (accessibility): applies to exactly one new screen** — the Settings → Updates
  panel (Task 2). Measured, in a real browser, on that screen only. Every other task is
  **no UI change** and says so.
- **Gate 6 (Indonesian end to end): does not apply.** This phase adds no prompt, no
  retrieval path, no tsvector config and no user-visible content path. It adds two
  strings to `en.json` and `id.json`; the existing `i18n.test.ts` key/placeholder parity
  check covers those, and that is the whole obligation. Claiming a fresh end-to-end
  Indonesian run here would be measuring Phase 2's code and calling it Phase 15's.
- **Gate 7 (honesty): applies to the same one screen**, because "an update is available"
  and "this build is signed" are claims about data. Clause (a) bites hardest: the panel
  must not show a version, a size or a download figure it did not receive from the
  manifest. Clauses (c)/(d) are not applicable (no composer, no generative action).

---

## This phase's real risk, and the defence

> **Three risks, in the order they can hurt, and none of them is caught by a test that
> runs on a developer's machine.**

**Risk 1 — a signed manifest whose key is on a laptop.** The updater's entire security
property is that an attacker cannot produce a manifest the app will accept. If the
minisign private key sits in a home directory, is copied into a CI secret with no
rotation, and is used from a developer's machine, then a phished laptop forges an update
that every installed app accepts and runs as the user. This is worse than having no
updater, because it *looks* like a control.

*The mechanical defence, in three parts.* (1) The private key and its password are GitHub
Actions secrets (`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`), and
Task 5's workflow is the **only** place the key is used; the verification script refuses
to run if `TAURI_SIGNING_PRIVATE_KEY` is set in its environment, so the key cannot drift
onto a developer machine by habit. (2) The public key is committed in
`src-tauri/tauri.conf.json`, and
`crates/server/tests/release.rs::the_committed_updater_public_key_verifies_a_signed_manifest`
signs a fixture manifest with a **test** keypair and asserts the verifier accepts it and
rejects a one-byte mutation and an unsigned manifest — so the verification path is
exercised in CI on every commit rather than only on release day. (3) `docs/RELEASE.md`
names the rotation procedure and the acceptable key storage, and the EULA (Task 12) makes
a forged update a breach rather than merely a bug. This is the phase's stated risk, and
it is the one item in the exit checklist that is a **process** gate.

**Risk 2 — an image that builds and runs here but not on the host.** This is the exact
failure Phase 6 named: `find_executable()` probes `/Applications/Google Chrome.app/…`
first, so on the developer's laptop the PDF pipeline works with the vendor's Chrome and
on the host — which has no Chrome — `AppState.renderer` is `None`. The symptom is not a
failed deploy; it is a **PDF nobody can download** (409 `conflict` on
`GET /cheatsheets/:id/pdf`), which a health check does not catch (A130). The same shape
applies to pdfium (`PDFIUM_LIB_PATH` unset ⇒ PDF ingestion fails at job time) and to the
embedder (weights fetched at first use ⇒ the first ingest on a fresh host either stalls or
fails when the host has no egress).

*The mechanical defence is that the laptop's acceptance test runs inside the image.*
Task 9's `image` CI job builds the Dockerfile, starts Postgres, boots the container, and
then runs, **inside the container**: (a) `stvdy-server --migrate-only`, (b) the health
probe against the live server, and (c) `cargo test -p stvdy-server --test pdf -- --ignored`
with `CHROME_PATH` pointing at the image's pinned Chromium — the same command
`.github/workflows/ci.yml`'s `pdf` job already runs on a bare runner. A second assertion
in that job reads the pinned version out of the running container
(`google-chrome --version`) and compares it to the value in `Dockerfile`'s
`ARG CHROMIUM_VERSION`, so an image that silently tracked `stable` fails the build. The
runtime test binaries are copied into the image on purpose (they are the *only* honest
way to test the image's native dependencies from inside it), and the image's
`assets/` directory is what makes the print hermetic — so `pdf.rs` passing in the
container also proves the assets are present, because that test asserts no raw `$`
delimiter reaches the PDF.

**Risk 3 — a launch gate that depends on the user, and so cannot be "done".** B1 (QRIS
gateway docs), B2 (tax/PPN + privacy text), B3 (SMTP credentials) and the A45 hosting
decision are not code and no task in this plan can close them. A plan that marked them
complete would be lying about the only thing that matters at launch.

*The mechanical defence is that they are checklist items with named owners, not tasks.*
Task 13 is the launch checklist, and every row is one of: **code** (closed by a task in
this plan, with the command that proves it), **user** (a credential, an account, a legal
review, a host decision — with the exact artifact to hand over and the date it was
requested), or **blocked** (B1/B2/B3, with the workaround that is live until it lands).
The checklist cannot be ticked by an agent, which is the point: it is the one place in the
plan where "I did the reachable part" and "the product can launch" are visibly different
sentences.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes. Prompts
  live in versioned modules, never inline. `tracing` structured logs with request ids;
  log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Server is the only source of truth.** Clients never call LLM, search, or Google APIs
  directly. Only BYOK keys travel client→server, per request, never stored unencrypted on
  device.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
  Local-date math only through `chrono-tz`.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings. Everything else requires connectivity and queues nothing.
- **No secret in this repository, ever.** Not a signing key, not a certificate, not a
  notarization credential, not a store password, not an R2 token. The committed artifacts
  are *public* keys, paths, checksums and procedure.
- **Prices (IDR) unchanged; no purchase UI on mobile.** `config/billing_policy.toml` is
  the authority; `BillingPolicy::validate` rejects a mobile entry that enables purchase
  UI, prices or an upgrade CTA.
- **Quality gate at every phase:** `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test`, web build,
  `node scripts/check-no-cdn.mjs`, the `src-tauri` trio. CI green before the phase is
  called done.
- **UI language English**, but Indonesian documents and queries must work end to end.

---

## What already exists (checked in this tree, not assumed)

Everything below was read in this tree at the time of writing. Where a file has a line
count or a line number, the fact was read from that line.

**The updater is deliberately absent, and the tree says why in three places.**

- `src-tauri/Cargo.toml:36-40` — a comment: `tauri-plugin-updater` is *not* a dependency,
  because "it needs a signing keypair and a release endpoint (both Phase 15), and
  registering it with placeholder config panics at startup".
- `src-tauri/capabilities/desktop.json:4` — "The updater capability arrives in Phase 15
  with its signing keypair and release endpoint."
- `src-tauri/tauri.conf.json:44` — `"createUpdaterArtifacts": false`.
- A35 in `docs/ASSUMPTIONS.md` records the same decision. **No `plugins.updater` block
  exists in `tauri.conf.json`** (the file is 58 lines; the only plugin key is
  `deep-link`, lines 52-57).

**The bundle config is half-built.**

- `src-tauri/tauri.conf.json:34-51` — `bundle.active: true`,
  `targets: ["app", "dmg", "nsis"]`, `createUpdaterArtifacts: false`,
  `macOS.minimumSystemVersion: "11.0"`, `windows.nsis.installMode: "perMachine"`.
  **Absent:** `macOS.hardenedRuntime` (schema default is `true`, so it is on but not
  pinned), `macOS.entitlements`, `macOS.signingIdentity`, `windows.certificateThumbprint`,
  `windows.digestAlgorithm`, `windows.timestampUrl`, and `bundleVersion` (the mobile build
  number). `version` is `"0.1.0"` (line 4) and is **not** wired to `CARGO_PKG_VERSION`.
- `src-tauri/tauri.conf.json:24` — the CSP already allows `https://api.stvdy.app` in
  `connect-src`, and nothing else remote. A25 in the schema (`updater`) is not present.

**CI has five jobs and no image, no signing, no release.**

- `.github/workflows/ci.yml` is 167 lines with jobs `lint`, `test`, `pdf`, `shell`, `web`,
  `desktop` (lines 6-167). The `pdf` job (lines 59-84) already runs
  `CHROME_PATH=/usr/bin/google-chrome cargo test -p stvdy-server --test pdf -- --ignored`
  on `ubuntu-24.04` after `test -x "$CHROME_PATH"`. The `desktop` job (lines 138-167)
  builds `--target universal-apple-darwin --no-bundle` and `--no-bundle` on
  `windows-2025`, with the comment "signing and notarization arrive in Phase 15".
  **There is no `release.yml`, no `Dockerfile`, and no `docker` job.**

**There is no `Dockerfile` anywhere in the repository.** `find . -iname 'Dockerfile*'`
(excluding `target/` and `node_modules/`) returns nothing. Phase 6 recorded this as this
phase's deliverable in three places: the roadmap's Phase 15 row ("server Docker image"),
`docs/RELEASE.md` § "The server image must bake a pinned Chromium (Phase 15
prerequisite)", and `docs/superpowers/plans/2026-10-11-phase-6-reports-cheatsheets.md:146`
("There is no server `Dockerfile` in this repository yet — Phase 15 owns it").

**The image's requirements are already written down, and the code they name exists.**

- `docs/RELEASE.md:66-112` is the normative statement: pin Chromium and set
  `CHROME_PATH`; a missing browser is **not** a crash (409 `conflict` on the PDF route,
  502 `provider_error` on a report PDF export); **pin the version**, because the
  acceptance test asserts a page count and a footer; the fonts the template names
  (`Inter`, `JetBrains Mono`, the KaTeX woff2 files) must be available locally because a
  render must not depend on a CDN; a `data-math-ready` failure fails the render rather
  than shipping a wrong document.
- `crates/server/src/render/chrome.rs:54-68` — `find_executable()` reads `CHROME_PATH`
  first, then probes four paths including `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.
- `crates/server/src/integrations/mod.rs:130-146` — the renderer is launched at boot;
  `None` is a supported state with a `tracing::warn!`.
- `crates/server/src/integrations/documents.rs:24-60` — `bind_pdfium(path)` binds once
  per process; unset `PDFIUM_LIB_PATH` logs "PDF ingestion is disabled"; a set-but-missing
  path is a hard boot failure (A51).
- `crates/server/src/config.rs:286-300` — `EMBEDDER` (default `fastembed`),
  `EMBEDDER_MODEL` (default `intfloat/multilingual-e5-base`), `EMBEDDING_DIM` (default
  768), `FASTEMBED_CACHE_DIR` (default `.fastembed_cache`, **a relative path**, i.e.
  relative to the working directory), `PDFIUM_LIB_PATH`.
- `Cargo.toml:100-114` — `fastembed` 7.1.1 with `hf-hub-rustls-tls` +
  `ort-download-binaries-rustls-tls` (the ONNX runtime is fetched at build time; the
  **model** is fetched at first use), and `pdfium-render` 0.9.4 with `pdfium_latest`
  and `thread_safe`, loaded dynamically from `PDFIUM_LIB_PATH`.
- `crates/server/tests/pdf.rs:22` — "The image sets `CHROME_PATH` to its pinned build";
  line 152/224/259 — the three sites that `panic!("no Chromium found; set CHROME_PATH")`.
- `crates/server/tests/pdfium.rs:30-42` — the test resolves the library from
  `PDFIUM_LIB_PATH` or `var/pdfium/libpdfium.dylib`, and **skips with a message** when
  neither exists, printing the exact download URL for
  `pdfium-binaries chromium/8086`. So `chromium/8086` is this repo's current pdfium pin.
- `crates/server/src/render/chrome.rs:96-101` — `allow_no_sandbox` is
  `cfg.env.is_dev()`: outside dev an unsandboxed render is refused. **Consequence for the
  image: the container must run Chromium with a working sandbox, or `APP_ENV` must not be
  `prod` — the plan pins the first.**

**The print assets are *referenced* as local and are not local.**

- `templates/cheatsheet.v1.html:48-51` — the template's own header comment says "the
  renderer rewrites ASSET_BASE (below) to a local mount". Line 61 says the same. **But
  the markup uses literal CDN URLs**: line 72 `https://cdn.jsdelivr.net/npm/katex@0.19.0/dist/katex.min.css`,
  lines 76-77 the two KaTeX scripts, lines 83-85 the Google Fonts stylesheet.
- `grep -rn "ASSET_BASE" --include=*.rs` finds **no code**: nothing rewrites it. The
  renderer (`crates/server/src/render/chrome.rs:245-256`) writes the HTML into the page
  with `document.write` and never touches the URLs, so a container with no egress renders
  every formula as literal `$…$` — which the `data-math-ready` guard turns into a failed
  job, i.e. a PDF nobody can download. This is a **gap this phase closes**, not a
  documentation task.
- `crates/core/src/render/mod.rs:37` — `CHEATSHEET_V1: &str = include_str!("../../../../templates/cheatsheet.v1.html")`,
  so the template is compiled into the binary and cannot drift from it.
- `templates/report.v1.html` exists and is compiled in at
  `crates/server/src/reports/render.rs:33`.
- `app/public/fonts/` holds six self-hosted woff2 files (Inter latin/latin-ext, JetBrains
  Mono latin/latin-ext, Plus Jakarta Sans latin/latin-ext) and `scripts/fetch-fonts.mjs`
  generates them. **They are the SPA's fonts; the template names `Inter` and
  `JetBrains Mono`, so two of the six are the ones the print needs.**

**Licensing and notices exist and are complete enough to ship, with one gap.**

- `LICENSE` (39 lines) already states all rights reserved, forbids modification and
  circumvention of entitlement/credit/rate-limit mechanisms, and states that the server
  is the sole authority. A44 asked for this and it is done.
- `THIRD_PARTY_NOTICES.md` (105 lines) lists bundled assets (IBM Plex + KaTeX) and direct
  Rust/JS dependencies with licences. **Two gaps:** it lists "IBM Plex Sans, IBM Plex
  Mono" as the bundled fonts, while `app/public/fonts/` actually ships Plus Jakarta Sans,
  Inter and JetBrains Mono (A105 changed the typefaces), and it does not list the assets
  this phase adds (a Chromium build, pdfium, LibreOffice, the ONNX runtime and the
  embedding model). A notices file that names the wrong fonts is a licence-compliance
  defect, not a typo.
- **There is no EULA / ToS / privacy policy file anywhere in the repo.** `docs/SPEC.md`
  § AUTH AND SECURITY says "Keep ToS/privacy placeholder pages"; `README.md` has no such
  section; `grep -ril "eula\|terms of service\|privacy policy" docs/` finds only
  discussion, never an artifact.

**The operations sections `docs/RELEASE.md` was created with do not exist yet.**

- `docs/RELEASE.md` is 112 lines and contains: Desktop distribution, Mobile distribution,
  Store policy risk, Privacy labels, Account deletion, the pinned-Chromium prerequisite,
  Tax and PPN, Versioning. **It has no backups, monitoring, breach runbook or retention
  section**, which is exactly what the roadmap's risk-register row and
  `docs/ASSUMPTIONS.md`'s "the operations sections in `docs/RELEASE.md`" refer to. This
  phase writes them.

**The assumptions and blockers this phase consumes.**

- `docs/ASSUMPTIONS.md` B1 (QRIS gateway + PJP identification, "Blocks public launch?
  **Yes. Hard.**"), B2 (tax/PPN + store privacy labels, "**Yes. Legal.**"), B3 (SMTP
  credentials, "**Yes. Operational.**"). The reading note under the table: "B1, B2 and B3
  all block a public launch … they must be started in parallel with development."
- A43 (free-tier abuse controls; code landed in Phase 7, `signup_attempts` is in
  `crates/server/tests/schema.rs:283-295`), A44 (licensing), A45 (**the VPS at
  `168.110.202.106` is a 1-month trial and is therefore STAGING ONLY — never
  production**; "do not put the QRIS webhook, Google OAuth redirect URI, Tauri updater
  manifests, or any user data behind it"), A47 (Supabase is never the production
  database), A15 (production object storage = **Cloudflare R2**; the client is unchanged,
  only the backend).
- The highest written row is **A131**. Phase 11's block is A227-A246; the rows are
  allocated per phase and this plan takes **A307** upward (the next free number after the
  A287-A306 block Phase 14 owns), because the reserved-block scheme collides with the
  numbered rows from A100 onward — the same reason Phases 9-11 ignored their reserved
  blocks and continued the sequence.

**Deployment facts the image must respect.**

- `crates/server/src/main.rs:20-30` — boot order: `AppConfig::from_env` → telemetry →
  `db::connect` → `db::migrate` (`sqlx::migrate!` on the compile-time
  `migrations/` directory) → `db::seed_plans` → `Providers::build` (storage reachable,
  bucket exists, pdfium binds, embedder dimension asserted against
  `information_schema`) → `requeue_orphans` → `refund_pending` for all four targets.
  **Migrations run on every boot, in-process, with no advisory lock of their own** — a
  second instance booting concurrently is a real race, which Task 6 handles with a
  `--migrate-only` mode.
- `crates/server/src/main.rs:71-75` — the API, the job worker and the scheduler are
  **three tasks in one process**. One container is therefore the whole server; scaling the
  API separately is a later concern and is documented, not built.
- `crates/server/src/routes/health.rs` — `/health` answers
  `{status, version, env, plans}` and touches the pool, so it is a real readiness probe
  and its `version` is `CARGO_PKG_VERSION`.
- `crates/server/src/routes/mod.rs:28-32` — `TAURI_ORIGINS` are built in;
  `WEB_ORIGIN` is required in prod for CORS.
- `.env.example` (129 lines) is the environment contract, and its `STORAGE_BACKEND` line
  already names the split: "`local` writes to a directory, `s3` talks to an S3-compatible
  endpoint (silo in dev, **Cloudflare R2 in prod**)".
- `docker-compose.yml` (91 lines) is **dev infrastructure only** — Postgres, `pgsty/silo`,
  `searxng` with a rendered config. It is not a production compose file and this plan does
  not pretend otherwise.

**Migrations taken:** `0023_report_generation.sql`, `0025_entitlements_limits.sql`,
`0027`-`0031` (Phases 8-11). `0017_sync_triggers.sql` must stay the last
trigger-attaching migration. **This phase adds no `+sync` table and no migration at all**,
so the rule is untouched.

**Missing (this phase's actual work):** `Dockerfile` and `.dockerignore`;
`src-tauri/tauri.conf.json`'s `plugins.updater`, `createUpdaterArtifacts: true`, the
macOS signing/entitlements keys and the Windows signing keys; `tauri-plugin-updater` in
`src-tauri/Cargo.toml` and its permission in `capabilities/desktop.json`; the updater
adapter in `app/src/platform/`; the asset rewrite in `crates/server/src/render/` and the
route that serves `assets/`; `assets/` itself (KaTeX, fonts) and the fetch script;
`.github/workflows/release.yml`; `scripts/verify-release-artifacts.sh`;
`scripts/release-image.sh`; `docs/EULA.md`; `docs/OPERATIONS.md`; the store checklists;
`README.md`'s deployment section; and the operations sections of `docs/RELEASE.md`.

---

## Decisions this plan pins (so parallel work cannot diverge)

1. **A307 — Production host: a Linux VPS with a lifetime that matches the product, in an
   Indonesian region, with a managed Postgres.** A45 forbids production on the
   `168.110.202.106` trial host and A47 forbids Supabase's free tier as the production
   database. The decision this plan pins is the *shape*, because the specific vendor is
   the user's purchase: **one region for the API, the database, the object store's
   bucket and SearXNG** (UU PDP data residency and the latency the Batam trial
   demonstrated), **managed Postgres 16 + pgvector** rather than a container volume, and
   **the container is deployed as one process** (API + worker + scheduler, per
   `main.rs`). The user names the vendor; `docs/OPERATIONS.md` carries the checklist that
   a candidate must satisfy before it is accepted.
2. **A308 — Cloudflare R2 is the production object store, and the client does not
   change.** A15 already decided this; what this plan pins is the configuration:
   `STORAGE_BACKEND=s3`, `S3_ENDPOINT=https://<account>.r2.cloudflarestorage.com`,
   `S3_REGION=auto`, `S3_FORCE_PATH_STYLE=false`, an R2 API token scoped to one bucket,
   and **no public bucket** — reads are presigned only (`PRESIGN_TTL_SECS`), because
   `GET /sources/:id/file` already hands out short-lived scoped URLs.
3. **A309 — The updater's private key never enters the repository and is used in exactly
   one workflow.** `TAURI_SIGNING_PRIVATE_KEY` + `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are
   Actions secrets; the public key is committed in `tauri.conf.json`'s
   `plugins.updater.pubkey`; `scripts/verify-release-artifacts.sh` **refuses to run** if
   the private key is present in its environment, so the key cannot leak onto a developer
   machine by habit. Rotation is documented, and rotating it requires a release, because
   installed apps verify against the compiled-in public key.
4. **A310 — The macOS build is a universal binary, signed with a Developer ID
   Application certificate, notarized with `notarytool`, and stapled.** Hardened runtime
   on, `minimumSystemVersion` 11.0 (already set), the two entitlements the shell actually
   needs and no others, and the notarization ticket stapled into the `.app` and the `.dmg`
   so a first launch with no network is not blocked. **The Apple Developer Program
   membership, the certificate and the notarytool credentials are the user's**; without
   them the build still produces an unsigned universal `.app`/`.dmg` that runs on the
   operator's machine, and `docs/RELEASE.md` says plainly that it must not be distributed.
5. **A311 — Windows ships an NSIS `perMachine` installer signed with an Authenticode
   certificate, SHA-256, with an RFC 3161 timestamp.** `installMode: "perMachine"` is
   already pinned in the config; the signature comes from a certificate thumbprint on the
   `windows-2025` runner (or Tauri's `signCommand` for a cloud signer). Without a
   certificate the installer builds unsigned; SmartScreen then warns on every install,
   and `docs/RELEASE.md` records that as a known degraded state rather than a defect.
6. **A312 — `createUpdaterArtifacts: true` and the `updater` capability arrive
   together, and mobile is excluded by construction.** The plugin's own platform matrix is
   `ios: none, android: none`, so a mobile build that enabled the capability would fail to
   compile rather than silently expose an API that does nothing. The capability goes in
   `capabilities/desktop.json` (the file already exists and already carries the comment
   saying the updater arrives here) rather than a fourth capability file — the permission
   is desktop-only, and one file that says so is better than two that agree.
7. **A313 — The update is offered, never automatic, and the panel shows only what the
   manifest said.** No silent download, no auto-restart, no "security update" wording
   unless the manifest is signed and verified. The panel renders the version string and
   the notes the manifest carried; it never computes a size or a percentage it did not
   receive. This is gate 7(a) applied to the one screen this phase adds.
8. **A314 — The container is `debian:bookworm-slim`, multi-stage, and the image carries
   the *test* binaries too.** Chromium is installed from a **pinned Debian package
   version** (`ARG CHROMIUM_VERSION`, no `apt-get upgrade`, no `chromium` meta-package
   that tracks `stable`) and `CHROME_PATH` names it. `debian:bookworm-slim` rather than
   `distroless` or Alpine because the three native dependencies (a glibc Chromium, a
   glibc pdfium build and LibreOffice) all exist there and none of them exists on musl;
   the security cost of a larger base is accepted and the mitigation is a pinned base
   digest plus a weekly rebuild, documented in `docs/OPERATIONS.md`. The release-profile
   `target/release/` directory is copied in so `cargo test --test pdf` can run inside the
   image (Risk 2's defence); the image is therefore larger than a minimal runtime, and
   that is a deliberate trade.
9. **A315 — Chromium runs with its sandbox, and the container is not privileged.** The
   renderer refuses an unsandboxed render outside dev
   (`crates/server/src/render/chrome.rs:96-101`), so the image must provide what the
   sandbox needs: a non-root user, a writable `/tmp` with the right mount options
   (documented as `--tmpfs /tmp` or a volume), and `--shm-size` guidance. Running as root
   with `--no-sandbox` is explicitly **not** the answer, and the image's `pdf` acceptance
   run is what proves the sandbox works there.
10. **A316 — The print assets are served by the server from an `assets/` directory, and
    the template's CDN URLs are rewritten to it.** `rust-embed` embeds `assets/` into the
    binary and a `GET /assets/{*path}` route serves it with explicit content types and
    `Cache-Control: public, max-age=31536000, immutable`; the renderer rewrites the
    template's `jsdelivr` and `fonts.googleapis.com` references to
    `<ASSETS_BASE_URL>/assets/…` before `document.write`, where `ASSETS_BASE_URL` is a new
    config value defaulting to `http://127.0.0.1:<BIND_ADDR port>`. The alternative —
    fetching the assets from the network at render time — is what the current template
    does and is exactly the failure `docs/RELEASE.md` describes. **Serving them over the
    server's own loopback origin is deliberate:** `document.write` gives the page no base
    URL, so a relative or `file://` reference would not resolve.
11. **A317 — The `data-math-ready` guard is the asset check, and it stays the gate.** The
    rewrite does not weaken it: a missing or 404 asset leaves
    `window.renderMathInElement` undefined, the template marks `unavailable`, and
    `render_within` fails the job with `RenderError::MathNotReady`. No new "did the
    assets load" assertion is added, because that would be a second check of the same
    fact and the first one already fails the render.
12. **A318 — pdfium is baked in at a pinned version with a verified checksum.** The pin
    is the `pdfium-binaries` release this repo's own test already names
    (`chromium/8086`, `crates/server/tests/pdfium.rs:41`), downloaded in the build stage,
    **checksum-verified**, installed at `/opt/pdfium/libpdfium.so`, with
    `PDFIUM_LIB_PATH` set. A51's policy is unchanged: a set-but-unloadable path fails
    boot. `libpdfium` is a glibc build, which is the second reason for A314's base image.
13. **A319 — The embedding model is pre-baked into the image and
    `FASTEMBED_CACHE_DIR` is absolute.** A2 says 1.06 GB fp32 and "pre-baked into the
    server image". The default in `config.rs:293` is the **relative** `.fastembed_cache`,
    which in a container means "relative to the working directory" — so a deployment that
    changed `WORKDIR` would silently re-download 1.06 GB at first ingest. The image sets
    an absolute `FASTEMBED_CACHE_DIR=/opt/models/fastembed` and the build stage
    **populates it with the model**, so first ingest never touches the network. The
    build-time population is verified by a check that the cache directory is non-empty and
    that the boot-time dimension assertion (`EMBEDDING_DIM` vs `chunks.embedding`, A2)
    still passes inside the image.
14. **A320 — LibreOffice is in the image, for the PPTX/DOCX preview, behind
    `SOFFICE_BIN`.** `docs/SPEC.md:99` and the Phase 15 line both name it, and A158
    defines the policy: an unset or unloadable `SOFFICE_BIN` leaves the source ingested
    with no preview rather than failing. The image installs `libreoffice-core` +
    `libreoffice-writer` + `libreoffice-impress` (not the whole suite) and sets
    `SOFFICE_BIN=/usr/bin/soffice`. **This is dormant until Phase 7's PPTX/DOCX/URL
    ingestion lands** (`grep -rn SOFFICE crates/` finds nothing today), so the image
    carries a capability the code does not yet use — stated here rather than discovered
    later.
15. **A321 — SearXNG is a separate service on the production host, not a service in the
    server image.** A3's tier 2 is a self-hosted compose service at `SEARXNG_URL`; A45
    names it among what the trial host is for. Bundling it into the app image would
    couple a Python service's upgrade cadence to the Rust server's, and the composite
    search already treats a dead tier as `unresponsive` rather than fatal. `SEARXNG_URL`
    points at the service's internal address; `docs/OPERATIONS.md` carries the deploy
    note and the `SEARXNG_SECRET` handling.
16. **A322 — The release workflow runs only after CI is green on the tagged commit, and
    it is the only place a signing secret is used.** `release.yml` is triggered by a
    `v*` tag, re-runs the same checks as `ci.yml` as a gate job, then builds: macOS
    universal (signed + notarized + stapled), Windows NSIS (signed), the updater
    manifest (`latest.json`) signed with minisign, and the Android APK for the direct
    channel. **Artifacts are uploaded to a GitHub Release and the manifest is published to
    the static HTTPS endpoint the app's `endpoints` names** — the endpoint is
    `https://updates.stvdy.app/{{target}}/{{arch}}/{{current_version}}` (or the
    operator's equivalent), a value the user supplies, because a manifest endpoint on a
    trial host is exactly what A45 forbids.
17. **A323 — `--migrate-only` is the deployment's migration step.** `main.rs` migrates on
    every boot with no lock, so two replicas starting together race. A new
    `--migrate-only` flag runs `connect` → `migrate` → `seed_plans` → exit, and the deploy
    procedure runs it once before the app container starts. `docs/OPERATIONS.md` carries
    the two commands. This is a **code** change, small and testable, and it is the one
    place this phase touches `main.rs`.
18. **A324 — Backups are Postgres PITR plus R2 versioning, with a restore drill that
    runs.** RPO 15 minutes / RTO 4 hours, both stated because "we have backups" is not a
    claim a restore can fail. `scripts/restore-drill.sh` restores the newest dump into a
    scratch database, runs `sqlx migrate info` and a row-count comparison, and prints
    `=== PASS`; it is run before launch and quarterly after, and its output is recorded in
    `docs/OPERATIONS.md`. **A backup that has never been restored is a hope**, so the
    drill is a documented, repeatable command and not a paragraph.
19. **A325 — Monitoring is four signals with thresholds, and one of them is money.**
    Liveness (`/health`), job backlog age (`jobs` where `status='queued'`), provider spend
    against `provider_cost_ceiling_usd` (A41 — the ceiling is enforced in code, but
    nobody is told when it trips), and 5xx rate. Alert routing is the operator's (a mail
    address and a chat webhook), and the alerting source is an external uptime check
    against `/health` plus a daily job-backlog query, because a server that is down
    cannot report that it is down.
20. **A326 — The breach runbook is a written sequence with the two facts a runbook needs
    to be useful: who decides, and how long we have.** UU PDP's notification window, the
    data inventory (what is actually stored, per table, from `docs/ARCHITECTURE.md` and
    the schema), the containment steps in order (revoke sessions, rotate
    `JWT_SECRET`/`ENCRYPTION_KEY_HEX`/provider keys, disable signups), the evidence
    preservation rule, and the notification template. Retention: `change_log` 90 days
    (A19), deleted accounts purged by `purge_deleted_accounts`, `var/mail` dev sink never
    used in prod, object storage lifecycle rules on R2.
21. **A327 — Licensing artifacts: the EULA/ToS is a new `docs/EULA.md`, and the notices
    file is corrected rather than replaced.** `LICENSE` (A44) already does the
    rights-reserved work; what is missing is the terms a *service* is offered under and a
    privacy notice, both of which are legal drafts the user must review. The plan produces
    `docs/EULA.md` with the clauses A44 names (no reverse engineering, no circumvention of
    entitlement/credit/rate-limit checks, tampering is a material breach and grounds for
    termination, the server is the sole authority) **marked as a draft requiring legal
    review**, and fixes `THIRD_PARTY_NOTICES.md`'s font list (Plus Jakarta Sans / Inter /
    JetBrains Mono, not IBM Plex) and adds the image's bundled components. No legal advice
    is offered and none is implied.
22. **A328 — Store submission checklists are per store, per artifact, with the evidence
    named.** Apple (privacy labels, in-app account deletion, review notes explaining
    QRIS-only and the absence of purchase UI, TestFlight build numbers) and Google (Data
    Safety form, deletion URL, review notes, Play internal testing then production). The
    **mobile no-purchase-UI evidence from Phase 14** is a checklist row whose proof is
    `scripts/check-mobile-no-purchase.sh` over the built mobile bundle plus the
    server-side refusal (`platform in (ios, android)` → 403). A checklist row without a
    command is a wish; every row here names one or names the user action it needs.
23. **A329 — The Android direct-APK channel is a signed release APK published to the
    website, and it is not the Play build.** The same version string, a different signing
    key (the APK key is the operator's; Play App Signing holds its own), and the same
    `billing_policy` — the direct APK must not contain purchase UI either, because the
    policy is a config compile-time read and not a store rule. `docs/RELEASE.md` records
    which key signs which artifact, because confusing them is how an upgrade fails to
    install.
24. **A330 — The server and the shell are versioned independently and the release tag
    names both.** `tauri.conf.json`'s `version` stays the shell's; the server's is
    `CARGO_PKG_VERSION` and is what `/health` reports. A server-only hotfix ships without
    a client release (clients are not recallable, and an update prompt for a fix that does
    not touch the client is noise). `docs/RELEASE.md` § Versioning carries the table.
25. **A331 — `docs/RELEASE.md` is normative and `docs/OPERATIONS.md` is its runbook
    appendix.** Every operational procedure lives in one of the two; the plan does not
    create a third. `docs/RELEASE.md` keeps the sections it already has (distribution,
    policy, the image prerequisite, tax, versioning) and gains the release process, the
    store checklists and the launch checklist; `docs/OPERATIONS.md` carries the
    day-two procedures (backups, restore drill, monitoring, breach runbook, retention,
    host checklist). Splitting them this way keeps the document an operator opens during
    an incident short.

---

## What needs the user, and what degrades without it

**A release plan that pretends a signing certificate can be generated is worse than one
that names the dependency.** Every row below is a hard prerequisite for one task, is
obtainable only by the user, and has a stated degraded path so the phase is not blocked
while it is outstanding.

| Prerequisite | Needed by | Who | What degrades without it |
|---|---|---|---|
| Apple Developer Program membership + a **Developer ID Application** certificate (`.p12`) and its password | Task 3 | **User** | `tauri build` produces a universal `.app`/`.dmg` that is **unsigned and unnotarized**. It runs on the machine that built it (Gatekeeper trusts a locally built app) and is blocked by Gatekeeper on every other Mac. **Not distributable.** `docs/RELEASE.md` says so in the same words. |
| Apple ID + an **app-specific password** (or an App Store Connect API key: issuer id, key id, `.p8`) for `notarytool` | Task 3 | **User** | Signing still works; **notarization does not**. The `.dmg` is signed but not stapled, and a user's first launch needs a network round-trip to Apple's notary service — and on a machine that is offline it fails with the "cannot be opened" dialog. |
| **Authenticode code-signing certificate** (or a cloud-signing service account) for Windows | Task 4 | **User** | The NSIS installer builds and installs, unsigned. **SmartScreen warns on every install** ("Windows protected your PC"), and the warning is per-publisher reputation, so it does not decay. Recorded as a degraded state, not a defect. |
| The **minisign keypair** (`tauri signer generate`) | Tasks 1, 5 | **User** (the plan can generate a *test* pair; the production pair is the operator's) | Without a real keypair the updater has no trusted public key. The plugin refuses to start with an absent `plugins.updater` config, so the honest state is **no updater at all** (which is where the tree is today) rather than an updater that trusts nothing. |
| The **HTTPS manifest endpoint** (`updates.stvdy.app` or equivalent) and the ability to publish to it | Tasks 5, 15 | **User** | The app can verify a manifest but has nowhere to fetch one. Task 5's workflow uploads to a GitHub Release as well, so the endpoint can be a redirect while the domain is settled. |
| **Apple App Store Connect** app record + review-notes review; **Google Play Console** account + Data Safety form | Task 14 | **User** | The checklists are written and the builds exist, but nothing can be submitted. This is the whole of B2's store half. |
| **Tax / PPN determination and privacy-label text** (B2) | Tasks 12, 14 | **User** (a tax professional and a lawyer) | Invoices carry no tax line items and the privacy notice is a marked draft. Both are launch blockers and neither is a code change. |
| **QRIS gateway identity, API docs and the PJP identification** (B1) | Task 13 (as a blocker row) | **User** | No revenue path exists. Phase 13 is implemented against an internal contract; the gateway's field mapping and signature scheme are the only open items. **Lead-time work, not code.** |
| **SMTP credentials and a From address** (B3) | Task 13 (as a blocker row) | **User** | No account can verify its email, and A43 makes verification a precondition for spending, so **no user can use the product**. The dev sink (`var/mail`) is not a production path. |
| **A production host** (A307) and the domain(s) | Tasks 6, 9, 15 | **User** (the purchase) | Everything in this plan can be built, tested and published to a release page; nothing can be *deployed*. A45 forbids the trial VPS, and putting the updater manifest or the QRIS webhook behind it is the specific mistake the row names. |
| **Cloudflare R2** account, bucket and API token (A15/A308) | Task 6 | **User** | `STORAGE_BACKEND=local` works on a single host's disk and is not durable or shareable; `docs/RELEASE.md` records the R2 configuration as the production value. |

---

## File structure (new this phase)

```
Dockerfile                                   # NEW — multi-stage, the pinned image
.dockerignore                                # NEW — keep target/ and node_modules/ out of the build context
assets/README.md                             # NEW — what is vendored, from where, at which version
assets/katex/0.19.0/…                        # NEW — katex.min.css, katex.min.js, auto-render.min.js, fonts/*.woff2
assets/fonts/inter-*.woff2                   # NEW — the two families the print template names
scripts/fetch-print-assets.sh                # NEW — downloads + checksums the vendored assets
scripts/verify-release-artifacts.sh          # NEW — the machine-readable release gate
scripts/release-image.sh                     # NEW — build, run and probe the image locally
scripts/restore-drill.sh                     # NEW — the backup restore that runs
scripts/check-mobile-no-purchase.sh          # NEW — greps the mobile bundle for purchase UI
crates/server/src/assets.rs                  # NEW — rust-embed + the /assets route
crates/server/src/routes/mod.rs              # + .merge(assets::routes()) and /assets in the public group
crates/server/src/render/chrome.rs           # + the asset-base rewrite before document.write
crates/server/src/config.rs                  # + assets_base_url
crates/server/src/main.rs                    # + --migrate-only
crates/server/src/lib.rs                     # + pub mod assets;
crates/server/tests/assets.rs                # NEW — the rewrite and the route
crates/server/tests/release.rs               # NEW — manifest signature, config invariants
src-tauri/tauri.conf.json                    # + plugins.updater, createUpdaterArtifacts, macOS/windows signing keys
src-tauri/Cargo.toml                         # + tauri-plugin-updater
src-tauri/capabilities/desktop.json          # + updater:default
src-tauri/src/lib.rs                         # + the updater plugin registration (desktop cfg)
app/src/platform/types.ts                    # + UpdateStatus / checkForUpdate / installUpdate
app/src/platform/desktop.ts                  # the Tauri updater implementation
app/src/platform/web.ts, mobile.ts           # explicit unsupported
app/src/features/settings/UpdatesPanel.tsx   # NEW — the one new screen
app/src/features/settings/UpdatesPanel.test.tsx  # NEW
app/src/i18n/en.json, id.json                # + the updater strings, both languages
app/src/features/settings/SettingsScreen.tsx # + the Updates section
.github/workflows/release.yml                # NEW — tag → signed artifacts + manifest
.github/workflows/ci.yml                     # + the image job
docs/RELEASE.md                              # the normative document: release process, checklists, ops sections
docs/OPERATIONS.md                           # NEW — backups, restore drill, monitoring, breach, retention, host checklist
docs/EULA.md                                 # NEW — draft terms + privacy notice, marked for review
docs/ARCHITECTURE.md                         # + the image's contents and the asset-serving decision
docs/ASSUMPTIONS.md                          # + A307-A331, and B1/B2/B3 status
docs/THREAT_MODEL.md                         # + the update channel as an asset and its row in § 5/§ 6
THIRD_PARTY_NOTICES.md                       # corrected fonts + the image's components
README.md                                    # + deployment, release and mobile sections
```

---

## Task 1: The updater plugin, the signed manifest, and the capability boundary

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`,
  `src-tauri/capabilities/desktop.json`, `src-tauri/src/lib.rs`
- Create: `crates/server/tests/release.rs`

**Interfaces:**
- Consumes: A35 (the plugin arrives here), A309, A312.
- Produces: `plugins.updater` in the config with `endpoints` and `pubkey`;
  `createUpdaterArtifacts: true`; `updater:default` in the desktop capability; the
  plugin registered under `#[cfg(desktop)]`.

**The secrets, and who owns them.** The public key is committed. The private key and its
password are **GitHub Actions secrets only** (`TAURI_SIGNING_PRIVATE_KEY`,
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) and are never written to a file in this repository,
never echoed, and never needed to *run* anything. The plan generates a **test** keypair in
`var/` (gitignored) to prove the verification path in CI; the operator generates the
production pair with `cargo tauri signer generate -w ~/.tauri/stvdy.key` on their own
machine and pastes only the public half into the config. **Degraded without it:** the
plugin panics at startup with an absent `plugins.updater` config, so the honest fallback
is not "an updater that trusts nothing" — it is no updater, which is where the tree is
today.

- [ ] **Step 1: Write the failing test**

`crates/server/tests/release.rs` is a plain (non-`sqlx::test`) suite, because none of these
facts need a database. Its first case is the one that makes the trust chain testable:

```rust
/// A signed manifest verifies, a mutated one does not, and an unsigned one does not.
///
/// This is the verification half of the update channel, exercised on every commit
/// instead of on release day. The fixture keypair is generated in the test and is
/// NOT the production key: what is asserted is the *procedure* (minisign over the
/// manifest bytes, against the public key the config commits), and the production
/// key's presence is asserted separately by `the_committed_public_key_is_present`.
#[test]
fn a_signed_manifest_verifies_and_a_mutated_one_does_not() { … }
```

The test shells out to the same `minisign` binary Tauri uses (or, where it is absent,
uses the `minisign-verify` crate's API — whichever the implementation picks, the
assertion is the same three cases: **valid → Ok, one flipped byte → Err, no signature →
Err**).

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p stvdy-server --test release
```

Expected: fails to compile, because `release.rs` names nothing that exists yet, and the
config assertions below have no `plugins.updater` to read.

- [ ] **Step 3: The config, with the two keys the schema requires**

`src-tauri/tauri.conf.json` gains, in `bundle`:

```json
"createUpdaterArtifacts": true,
"macOS": {
  "minimumSystemVersion": "11.0",
  "hardenedRuntime": true,
  "entitlements": "entitlements.plist"
},
"windows": {
  "nsis": { "installMode": "perMachine" },
  "digestAlgorithm": "sha256",
  "timestampUrl": "http://timestamp.digicert.com"
}
```

and a `plugins` entry:

```json
"plugins": {
  "deep-link": { … unchanged … },
  "updater": {
    "endpoints": ["https://updates.stvdy.app/{{target}}/{{arch}}/{{current_version}}"],
    "pubkey": "<the operator's minisign public key>",
    "windows": { "installMode": "passive" }
  }
}
```

`endpoints` and `pubkey` are the two fields the plugin's deserializer requires; an absent
one is the startup panic A35 records. **`signingIdentity` is deliberately not set in the
committed file** — the workflow passes `APPLE_SIGNING_IDENTITY` from a secret, so a
contributor's build is unsigned rather than failing on a missing identity.

- [ ] **Step 4: The dependency and the capability**

`src-tauri/Cargo.toml`:

```toml
# Desktop-only: the plugin declares `ios: none, android: none`, and mobile updates
# ship through TestFlight/Play (A35, A312). Registered here rather than in Phase 0
# because it panics at startup without a signing keypair and a release endpoint.
[target.'cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))'.dependencies]
tauri-plugin-updater = "2.13.3"
```

`capabilities/desktop.json` gains `"updater:default"` in `permissions` and its
`description` loses the "arrives in Phase 15" clause, because it has arrived. The
capability's `platforms` is already `["macOS", "windows", "linux"]`, which is the plugin's
own platform matrix — a mobile build cannot see the permission.

- [ ] **Step 5: Register it**

`src-tauri/src/lib.rs` builds the plugin conditionally:

```rust
// The updater is desktop-only, and it is the one plugin whose absence from mobile is
// enforced by the plugin rather than by us: its own platform matrix is
// `ios: none, android: none`, so this block is cfg'd to match (A312).
#[cfg(desktop)]
let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
```

- [ ] **Step 6: The config invariants, as tests**

The rest of `crates/server/tests/release.rs` asserts the committed facts that are easy to
undo by hand, each one a bug the release workflow would otherwise discover late:

```rust
#[test]
fn the_bundle_creates_updater_artifacts_and_names_a_public_key() { … }
#[test]
fn the_updater_is_desktop_only() { … }            // the permission is not in default.json or mobile.json
#[test]
fn the_macos_bundle_pins_hardened_runtime_and_the_minimum_version() { … }
#[test]
fn the_windows_installer_is_per_machine_and_sha256_timestamped() { … }
#[test]
fn the_version_is_the_workspace_version() { … }   // tauri.conf.json version == Cargo.toml [workspace.package] version
```

The last one is the version-drift guard A330 needs: two files that must agree and a test
that says so beats a release that ships 0.1.0 twice.

- [ ] **Step 7: Run the tests**

```bash
cargo test -p stvdy-server --test release
cargo test --manifest-path src-tauri/Cargo.toml
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 2: The updater UI — one screen, and it says only what the manifest said

**Files:**
- Modify: `app/src/platform/types.ts`, `app/src/platform/desktop.ts`,
  `app/src/platform/web.ts`, `app/src/platform/mobile.ts`,
  `app/src/features/settings/SettingsScreen.tsx`, `app/src/i18n/en.json`,
  `app/src/i18n/id.json`
- Create: `app/src/features/settings/UpdatesPanel.tsx`,
  `app/src/features/settings/UpdatesPanel.test.tsx`

**Interfaces:**
- Consumes: `Platform` (`app/src/platform/types.ts`), the existing `SettingsScreen`
  sections.
- Produces: `Platform.checkForUpdate(): Promise<UpdateStatus>`,
  `Platform.installUpdate(onProgress): Promise<void>`, where

```ts
export type UpdateStatus =
  | { kind: 'unsupported' }                       // web and mobile
  | { kind: 'up-to-date'; current: string }
  | { kind: 'available'; current: string; version: string; notes: string | null }
  | { kind: 'error'; message: string }            // a failed check, never a silent no-op
```

- [ ] **Step 1: Write the failing test**

`UpdatesPanel.test.tsx` asserts the three states a user can be in and the honesty
constraint that makes the panel safe:

- `unsupported` renders nothing on web and mobile (the component returns `null`, and the
  Settings section is absent rather than disabled — a disabled "Check for updates" on a
  phone is a promise the platform cannot keep).
- `available` renders the version and the notes **exactly as the manifest carried them**,
  and renders **no** percentage, no size and no "security update" label (A313).
- `error` renders a message and a retry, and does **not** render "you are up to date".
  A failed check that reads as success is the one bug this panel can have that a user
  cannot detect.

- [ ] **Step 2: Run it and watch it fail**

```bash
npm --prefix app test -- UpdatesPanel
```

- [ ] **Step 3: The adapter, three ways**

`types.ts` declares the two methods. `desktop.ts` implements them against
`@tauri-apps/plugin-updater`'s `check()` / `downloadAndInstall()` with a progress
callback; it must **not** call the plugin on a build that lacks the capability (the
capability is desktop-only, so the import is already inside the desktop adapter, which is
the only module allowed to touch Tauri APIs — `docs/SPEC.md` § PLATFORMS AND
ARCHITECTURE). `web.ts` and `mobile.ts` return `{ kind: 'unsupported' }`. The mobile
adapter's comment says why in one line: mobile updates ship through TestFlight and Play,
and the Tauri updater does not support them.

- [ ] **Step 4: The panel**

`UpdatesPanel.tsx` is a `LockedFeature`-free, ordinary Settings section: the current
version (from `Platform`), a `Check for updates` button, the three states, and an install
action that shows the adapter's progress. Every string goes through `t()` in both
`en.json` and `id.json` (A104's contract; `i18n.test.ts` fails on a key present in one
file only, and on a placeholder mismatch between them).

- [ ] **Step 5: Run the web tests and the build**

```bash
npm --prefix app test
npm --prefix app run build
```

**Gates:** 5 — **applies, measured on this screen**: keyboard reach for both buttons with
the visible `outline-ring` focus style, contrast in both themes for the three states,
44px targets at 390px, no state by colour alone (each state carries its own word), and
200% text scale with no clipping. Recorded in `docs/ARCHITECTURE.md` with the numbers.
6 — no UI change (the two new strings are covered by the existing parity test). 7 —
**applies**: (a) no number reaches the screen that the manifest did not carry; (e) the
install action is labelled as an update, not as a "fix" or a "security patch".

---

## Task 3: macOS — the universal binary, signing, notarization, and the staple

**Files:**
- Create: `src-tauri/entitlements.plist`, `scripts/notarize-macos.sh`
- Modify: `.github/workflows/release.yml` (created in Task 5; this task adds the macOS
  job), `docs/RELEASE.md`

**Interfaces:**
- Consumes: A310, the `macos-15` runner, `tauri build --target universal-apple-darwin`.
- Produces: a `.dmg` and a `.app` that pass `codesign --verify --deep --strict`,
  `spctl -a -vvv`, and `xcrun stapler validate`.

**The secrets, and who owns them.** `APPLE_CERTIFICATE` (base64 `.p12`),
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, and for notarization either
`APPLE_ID` + `APPLE_PASSWORD` (an app-specific password) + `APPLE_TEAM_ID`, or
`APPLE_API_KEY` + `APPLE_API_ISSUER` + `APPLE_API_KEY_ID`. All are Actions secrets.
**Degraded without them:** `tauri build` produces an unsigned universal bundle that runs
only on the machine that built it. `docs/RELEASE.md` states that in those words and the
release workflow **fails the macOS job** when `APPLE_CERTIFICATE` is absent, so an
unsigned artifact is never published by accident.

- [ ] **Step 1: Write the entitlement file, and keep it minimal**

`src-tauri/entitlements.plist` carries exactly what the shell needs and nothing else. The
hardened runtime blocks JIT and unsigned executable memory, so an entitlement added "just
in case" is a hole that survives review:

```xml
<!-- The hardened runtime is on (A310). These two are the ones the shell needs:
     the webview's JavaScript engine requires the JIT allowance on macOS, and the
     app writes its SQLite cache and outbox under the app data directory. Anything
     else here is a hole that a reviewer would have to find. -->
<key>com.apple.security.cs.allow-jit</key><true/>
<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
```

`com.apple.security.cs.disable-library-validation` is **not** added: it exists for
loading unsigned plugins, and the shell loads none. If a future plugin needs it, that is
a decision with a reason, not a default.

- [ ] **Step 2: The notarization script**

`scripts/notarize-macos.sh` takes the `.dmg` path, submits it with `notarytool submit
--wait`, and **staples the ticket to both the `.dmg` and the `.app` inside it** before
re-packing. Two details that are easy to get wrong and are the reason this is a script
rather than three workflow lines:

- `--wait` returns exit 0 for a submission that was *rejected*; the script parses
  `status: Accepted` out of the JSON output and fails otherwise, printing the log id.
- A stapled `.dmg` is not enough on its own: Gatekeeper checks the `.app` it contains, so
  the ticket is stapled to the app before the dmg is rebuilt. A user who copies the app
  out of the dmg onto a second machine then launches it offline gets a working app.

- [ ] **Step 3: The workflow job**

`.github/workflows/release.yml`'s `macos` job, on `macos-15`:

```bash
# The identity comes from a secret so a contributor's local build is unsigned
# rather than failing on a missing identity (Task 1 step 3).
export APPLE_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY"
./app/node_modules/.bin/tauri build --target universal-apple-darwin \
  --config '{"bundle":{"macOS":{"entitlements":"entitlements.plist"}}}'
bash scripts/notarize-macos.sh "src-tauri/target/universal-apple-darwin/release/bundle/dmg/"*.dmg
```

then asserts, before uploading:

```bash
codesign --verify --deep --strict --verbose=2 "$APP"          # the signature
spctl -a -vvv -t install "$APP"                               # Gatekeeper accepts it
xcrun stapler validate "$DMG"                                 # the ticket is stapled
lipo -archs "$APP/Contents/MacOS/Stvdy" | grep -q 'x86_64 arm64'  # it is universal
```

- [ ] **Step 4: Record what was measured**

`docs/RELEASE.md` § Desktop distribution gains the exact commands and the runner image
they ran on, so the claim "signed and notarized" is a transcript rather than an adjective.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 4: Windows — the NSIS installer, signed and timestamped

**Files:**
- Modify: `.github/workflows/release.yml`, `docs/RELEASE.md`

**Interfaces:**
- Consumes: A311, the `windows-2025` runner, the `nsis` bundle target already in
  `tauri.conf.json`.
- Produces: a signed `Stvdy_<version>_x64-setup.exe` that passes
  `osslsigncode verify`.

**The secret, and who owns it.** A `.pfx` (base64) plus its password, or a cloud-signing
service's credentials, in Actions secrets. Tauri reads
`WINDOWS_CERTIFICATE`/`WINDOWS_CERTIFICATE_PASSWORD`, or `bundle.windows.signCommand` for
a cloud signer. **Degraded without it:** the installer builds and installs unsigned, and
SmartScreen warns on every install until publisher reputation accrues — which an unsigned
binary never accrues.

- [ ] **Step 1: The job**

On `windows-2025`, `tauri build --bundles nsis` with the certificate environment set, then
verification:

```powershell
osslsigncode verify -in "src-tauri\target\release\bundle\nsis\Stvdy_*_x64-setup.exe"
```

The verification asserts three things and fails on any: the signature verifies, the
**digest algorithm is SHA-256** (a SHA-1 signature is refused by modern Windows and would
be discovered by a user, not by us), and a timestamp is present (without one, the
signature stops validating when the certificate expires, and every installed copy becomes
unverifiable).

- [ ] **Step 2: Record the degraded path**

`docs/RELEASE.md` gains one sentence and one command: without a certificate the job is
skipped with a warning, the artifact is published unsigned, and that is a **known degraded
state** with the SmartScreen consequence named. A release with an unsigned installer is
not a release failure; a release that did not say so is.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 5: `.github/workflows/release.yml` — the pipeline, and the manifest

**Files:**
- Create: `.github/workflows/release.yml`
- Modify: `docs/RELEASE.md` (§ Versioning, § Release process)

**Interfaces:**
- Consumes: Tasks 1, 3, 4; A322, A330; the `v*` tag.
- Produces: a GitHub Release with the macOS dmg, the Windows installer, the Android APK,
  the updater artifacts (`*.app.tar.gz`, `*.nsis.zip`, and their `.sig` files) and
  `latest.json`.

- [ ] **Step 1: The gate job, first**

The workflow's first job re-runs `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test --workspace` and the web build on the tagged commit. A release pipeline that
trusts a green CI badge from a branch is a pipeline that can ship a commit CI never saw,
because tags are not commits.

- [ ] **Step 2: The build matrix**

Three jobs, each producing its own artifacts: `macos` (Task 3, universal, signed,
notarized, stapled), `windows` (Task 4, NSIS, signed), `android` (the release APK for the
direct channel, A329). Each uploads its artifacts and its `.sig` files. `createUpdaterArtifacts:
true` makes Tauri emit the `.sig` next to each updater archive; a job that produced the
archive and not the signature is a failure, and the workflow asserts the `.sig` exists
rather than trusting the bundler's exit code.

- [ ] **Step 3: The manifest**

`latest.json` is generated from the artifacts, with one entry per
`{{target}}/{{arch}}` pair, each carrying `signature` (the contents of the `.sig`) and
`url` (the release asset URL). It is **signed with the same minisign key** and published
to the static endpoint `plugins.updater.endpoints` names (A322). The manifest is the
artifact whose integrity the whole channel rests on, so the workflow verifies its own
output: it downloads the published manifest back over HTTPS, verifies the signature
against the committed public key, and fails if that round trip does not hold.

- [ ] **Step 4: The version, in one place**

A step asserts `Cargo.toml [workspace.package] version`, `src-tauri/tauri.conf.json
version`, and the tag (without its `v`) agree, and fails with the three values printed.
A330's rule — the server and the shell are versioned independently — means the *tag* names
the shell, and a server-only hotfix is tagged `server-v…` and skips the client jobs. The
table in `docs/RELEASE.md` § Versioning is what makes that a procedure rather than a
guess.

- [ ] **Step 5: Run the artifact verifier**

The last step of every job runs `scripts/verify-release-artifacts.sh` (Task 11) against
what was just produced, in the mode that matches the artifact. **This is the step that
makes the exit gate mechanical rather than a claim**: a job that produced an unsigned
installer or an unstapled dmg fails here, before the artifact is published.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 6: `Dockerfile` — the pinned image

**Files:**
- Create: `Dockerfile`, `.dockerignore`
- Modify: `docs/ARCHITECTURE.md` (§ Container image), `docs/RELEASE.md`

**Interfaces:**
- Consumes: A314-A320, the config keys `CHROME_PATH` / `PDFIUM_LIB_PATH` /
  `FASTEMBED_CACHE_DIR` / `SOFFICE_BIN`, `docs/RELEASE.md:66-112`.
- Produces: an image whose `/usr/bin/google-chrome --version` equals
  `ARG CHROMIUM_VERSION`, whose `/opt/pdfium/libpdfium.so` exists, whose
  `/opt/models/fastembed` is populated, and whose `/usr/bin/soffice` runs.

- [ ] **Step 1: The build stage**

```dockerfile
# syntax=docker/dockerfile:1
# The server image. The three native dependencies the code loads at runtime are
# pinned here and nowhere else: Chromium (CHROME_PATH, A129), pdfium
# (PDFIUM_LIB_PATH, A51) and the embedding model (FASTEMBED_CACHE_DIR, A2).
# Each has a documented absent-binary policy, and the failure mode of getting
# one wrong is a feature that quietly stops working on deploy rather than a
# failed start (docs/RELEASE.md).
FROM rust:1.94.1-bookworm AS build
WORKDIR /src
# The dependency layer, so a source-only change does not re-download the crate graph.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY prompts ./prompts
COPY templates ./templates
COPY config ./config
COPY .sqlx ./.sqlx
ENV SQLX_OFFLINE=true
RUN cargo build --release --workspace --locked
```

The whole `target/release/` directory is what the runtime stage copies (A314), so the
acceptance test binaries come with it. `--locked` so the image builds the lockfile that
CI tested.

- [ ] **Step 2: The runtime stage, with the pin as an argument**

```dockerfile
FROM debian:bookworm-slim AS runtime
# Pinned, not `chromium` and not `stable`. The renderer's output is the document a
# student prints and the acceptance test asserts a page count and a footer, so an
# unpinned browser makes the test's result depend on when the image was built
# (docs/RELEASE.md).
ARG CHROMIUM_VERSION=141.0.7390.54-1~deb12u1
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl fonts-liberation \
      chromium=${CHROMIUM_VERSION} chromium-common=${CHROMIUM_VERSION} \
      libreoffice-core libreoffice-writer libreoffice-impress \
    && rm -rf /var/lib/apt/lists/*
ENV CHROME_PATH=/usr/bin/chromium
```

**The package name and the version string are read from `apt-cache policy chromium` in the
Debian bookworm archive at implementation time, and the exact value is pinned in the file
and asserted by Task 9's job.** A plan that guessed a version string would be a plan whose
image does not build.

- [ ] **Step 3: pdfium, with a verified checksum**

```dockerfile
ARG PDFIUM_RELEASE=chromium/8086
# The release this repo's own test names (crates/server/tests/pdfium.rs:41). The
# checksum is committed here rather than fetched-and-trusted: an unpinned download
# from a release URL is a supply-chain hole in the one dependency that parses
# attacker-supplied files.
ARG PDFIUM_SHA256=<sha256 of pdfium-linux-x64.tgz for that release>
RUN curl -sSL -o /tmp/pdfium.tgz \
      "https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE}/pdfium-linux-x64.tgz" \
    && echo "${PDFIUM_SHA256}  /tmp/pdfium.tgz" | sha256sum -c - \
    && mkdir -p /opt/pdfium && tar xz -C /opt/pdfium --strip-components=1 -f /tmp/pdfium.tgz \
    && rm /tmp/pdfium.tgz
ENV PDFIUM_LIB_PATH=/opt/pdfium/lib/libpdfium.so
```

- [ ] **Step 4: The embedding model, pre-baked, at an absolute path**

The build stage runs a small fetch using the same code path the server uses, so the cache
layout cannot differ between the image and the running process:

```dockerfile
# `FASTEMBED_CACHE_DIR` is RELATIVE in config.rs (`.fastembed_cache`), which in a
# container means "relative to the working directory" — so a deployment that changed
# WORKDIR would silently re-download 1.06 GB at first ingest, on a host that may have
# no egress (A319). The image sets an absolute path and populates it here, so the
# first ingest never touches the network.
ENV FASTEMBED_CACHE_DIR=/opt/models/fastembed
RUN cargo run --release -p stvdy-server --fetch-embedder-model
```

`--fetch-embedder-model` is a second small flag alongside A323's `--migrate-only`: it
builds the embedder (which downloads into `FASTEMBED_CACHE_DIR`) and exits. It is the
honest way to populate the cache, because it uses `fastembed`'s own layout rather than a
hand-copied directory that a version bump would invalidate.

- [ ] **Step 5: The vendored print assets, and the fonts**

```dockerfile
# The template names Inter and JetBrains Mono and loads KaTeX from a CDN. The
# renderer rewrites both to this directory (Task 7), because a render must not depend
# on a third-party CDN being reachable (docs/RELEASE.md). scripts/fetch-print-assets.sh
# pins the versions and their checksums.
COPY assets /opt/assets
ENV ASSETS_DIR=/opt/assets
```

`assets/` is embedded with `rust-embed` at compile time (Task 7), so `ASSETS_DIR` is the
path the **build** reads; the runtime copy exists for the operator's inspection and for
the checksum record. One of the two is redundant, and the plan keeps both because a
release engineer needs to see what shipped — the redundancy is stated here so it is not
mistaken for a mistake.

- [ ] **Step 6: The runtime user, the sandbox, and the entrypoint**

```dockerfile
# A non-root user, because Chromium's sandbox is what the renderer requires outside
# dev (crates/server/src/render/chrome.rs:96-101) and running as root with
# --no-sandbox is explicitly not the answer (A315). The UID is fixed so a mounted
# volume has predictable ownership.
RUN useradd --create-home --uid 10001 --shell /usr/sbin/nologin stvdy
COPY --from=build /src/target/release/stvdy-server /usr/local/bin/stvdy-server
COPY --from=build /src/target/release/ /opt/tests/          # the acceptance binaries (A314)
USER stvdy
WORKDIR /app
ENV APP_ENV=prod BIND_ADDR=0.0.0.0:8080 SOFFICE_BIN=/usr/bin/soffice
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/stvdy-server"]
```

`--migrate-only` (A323) is the same entrypoint with the flag. **The container needs a
`--shm-size` and a writable `/tmp`** for Chromium; both are recorded in
`docs/OPERATIONS.md` with the exact `docker run` / compose stanza, because a missing
`/dev/shm` is the classic "Chromium crashes with a memory error that looks like a code
bug" failure.

- [ ] **Step 7: `.dockerignore`**

`target/`, `node_modules/`, `app/dist/`, `.git/`, `var/`, `.env*`, `supabase/` and the
plan documents. Without it the build context is gigabytes and a stale `var/` leaks into
the image.

- [ ] **Step 8: `docs/ARCHITECTURE.md` gains the image's contents**

A new `## Container image` section, one table: what is installed, at which version, why,
and which environment variable names it. This is the section the task's brief asks for, and
it is the reference for the next person who adds a native dependency.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 7: Serve the print assets from the server, and rewrite the template

**Files:**
- Create: `crates/server/src/assets.rs`, `crates/server/tests/assets.rs`,
  `assets/README.md`, `scripts/fetch-print-assets.sh`
- Modify: `crates/server/src/lib.rs`, `crates/server/src/routes/mod.rs`,
  `crates/server/src/render/chrome.rs`, `crates/server/src/config.rs`,
  `templates/cheatsheet.v1.html`, `docs/ARCHITECTURE.md`

**Interfaces:**
- Consumes: A316, A317; the template's `ASSET_BASE` comment and its literal CDN URLs.
- Produces: `GET /assets/{*path}`; `chrome::rewrite_asset_base(html, base) -> String`;
  `AppConfig.assets_base_url`.

- [ ] **Step 1: Write the failing test**

`crates/server/tests/assets.rs`:

```rust
/// The rewrite points every CDN reference at the server's own origin.
///
/// The defect this catches is the one the tree has today: the template's comment
/// says the renderer rewrites ASSET_BASE and no code does, so a container with no
/// egress renders every formula as literal `$…$` — which the data-math-ready guard
/// turns into a failed job, i.e. a PDF nobody can download (docs/RELEASE.md).
#[test]
fn the_template_has_no_remote_asset_reference_after_the_rewrite() { … }

/// A rewritten URL resolves to bytes through the route, with the content type the
/// browser needs. A 200 with the wrong MIME type is a stylesheet that does not apply.
#[sqlx::test(migrations = "../../migrations")]
async fn the_asset_route_serves_katex_and_the_fonts(pool: PgPool) { … }
```

The first test asserts the post-rewrite HTML contains **no** `https://cdn.jsdelivr.net`
and **no** `https://fonts.googleapis.com`, and that it does contain the base URL for
`katex.min.css`, `katex.min.js`, `auto-render.min.js` and at least one woff2.

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p stvdy-server --test assets
```

Expected: fails, because neither the route nor the rewrite exists.

- [ ] **Step 3: The vendored assets and the fetch script**

`scripts/fetch-print-assets.sh` downloads, for a pinned version, the four KaTeX files the
template references (`katex.min.css`, `katex.min.js`, `contrib/auto-render.min.js`, and
the `fonts/*.woff2` the CSS names) plus the Inter and JetBrains Mono woff2 files, into
`assets/`, verifying a SHA-256 per file against a committed list. It is the same shape as
`scripts/fetch-fonts.mjs` (which already does this for the SPA) and it is the reason the
assets are in the repo rather than fetched at build time: a build that reaches the network
for a font is a build that fails on the day jsdelivr is slow, and the failure would be a
missing glyph rather than an error.

`assets/README.md` records each file, its version, its source URL and its checksum.
`THIRD_PARTY_NOTICES.md` gains the KaTeX and font entries it does not have yet (Task 12).

- [ ] **Step 4: The route**

`crates/server/src/assets.rs` uses `rust-embed` over `assets/` and serves
`GET /assets/{*path}` with a match on the extension for the content type
(`text/css`, `application/javascript`, `font/woff2`) and
`Cache-Control: public, max-age=31536000, immutable` (the path carries a version, so
immutable is correct and a stale stylesheet is impossible). It is registered in the
**public** group in `routes/mod.rs`, because the browser that requests it is Chromium with
no credentials — and because the assets are public files with no user data. `routes/mod.rs`'s
public group is where `Exempt` routes live, so `crates/server/tests/route_sweep.rs` gets a
row for the new path (Task 11 covers the sweep).

- [ ] **Step 5: The rewrite, and where it goes**

`crates/server/src/render/chrome.rs` gains

```rust
/// Points the template's CDN references at the server's own origin.
///
/// The template documents this rewrite and nothing implemented it (the file's own
/// comment, `templates/cheatsheet.v1.html:48`). The base is the server's loopback
/// origin because `document.write` gives the page no base URL: a relative reference
/// would not resolve, and `file://` would need a scheme the browser refuses for a
/// written document (A316).
pub fn rewrite_asset_base(html: &str, base: &str) -> String { … }
```

applied inside `render_once` **before** the `document.write`, and unit-tested on the string
alone (no browser needed) in `crates/server/tests/assets.rs`. The CDN URL forms it
replaces are the four the template carries; a test asserts the count of replacements equals
the count of matches in the source, so a template that gains a fifth remote reference fails
the test rather than silently loading it.

- [ ] **Step 6: The config value**

`AppConfig` gains `assets_base_url: String`, from `ASSETS_BASE_URL`, defaulting to
`http://127.0.0.1:{port of BIND_ADDR}`. **The default is what makes a single-container
deployment work with no configuration**, and the override is what makes a deployment
where Chromium cannot reach the server's advertised address work at all. `docs/RELEASE.md`
records the variable and the two cases.

- [ ] **Step 7: Keep the guard as the gate**

`data-math-ready` is unchanged (A317). The render still fails when the assets do not
load, and `crates/server/tests/pdf.rs`'s "no raw `$` delimiters" assertion is what proves
it inside the image (Task 9). No second assertion is added.

**Gates:** 5 — no UI change. 6 — no UI change (the template's copy is already
Indonesian-capable and this task changes URLs, not text). 7 — no UI change.

---

## Task 8: `--migrate-only` and `--fetch-embedder-model`

**Files:**
- Modify: `crates/server/src/main.rs`, `crates/server/tests/release.rs`,
  `docs/OPERATIONS.md`

**Interfaces:**
- Consumes: A319, A323; `main.rs`'s existing boot order.
- Produces: `stvdy-server --migrate-only` (connect → migrate → seed → exit) and
  `stvdy-server --fetch-embedder-model` (build the embedder, which populates the cache,
  then exit).

- [ ] **Step 1: Write the failing test**

A process-level test is the honest shape here, and `std::process::Command` against
`env!("CARGO_BIN_EXE_stvdy-server")` is how `crates/server/tests/release.rs` runs it:

```rust
/// `--migrate-only` migrates and exits without binding a port.
///
/// The race it prevents is real: main.rs migrates on every boot with no lock, so two
/// replicas starting together both run `sqlx::migrate!` against one database (A323).
#[sqlx::test(migrations = "../../migrations")]
async fn migrate_only_exits_zero_without_binding(pool: PgPool) { … }
```

The assertion that it did **not** bind is a port probe after the process exits, because
"it exited 0" is also true of a process that bound and then crashed.

- [ ] **Step 2: The flags**

`main.rs` parses `std::env::args()` before anything else, and the two modes are early
returns:

```rust
// Two modes that exist for the deployment, not for a developer. Both are parsed
// before the pool is opened so a typo in a flag is not discovered after a
// migration has already run.
match std::env::args().nth(1).as_deref() {
    Some("--migrate-only") => return migrate_only().await,
    Some("--fetch-embedder-model") => return fetch_embedder_model().await,
    Some(other) => anyhow::bail!("unknown argument {other}"),
    None => {}
}
```

`fetch_embedder_model` builds `Providers` far enough to construct the embedder and drops
it; the download is the side effect. It does **not** require a database, so the build
stage can run it before Postgres exists — which is what Task 6's Step 4 depends on.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p stvdy-server --test release
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 9: The image in CI — build it, boot it, and run the acceptance test inside it

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `scripts/release-image.sh`
- Modify: `docs/OPERATIONS.md`

**Interfaces:**
- Consumes: Risk 2's defence, A314, the existing `pdf` job's command.
- Produces: a `docker` job that builds the image, starts Postgres, and runs the PDF
  acceptance test **inside** the container.

- [ ] **Step 1: The job, and what it proves**

```yaml
  image:
    name: server image (pinned Chromium, pdfium, model)
    runs-on: ubuntu-24.04
    timeout-minutes: 45            # a hang here is the failure mode, not a slow build
    services:
      postgres: { image: pgvector/pgvector:pg16, … }   # the same service the `test` job uses
    steps:
      - uses: actions/checkout@v5
      - uses: docker/setup-buildx-action@v3
      - run: docker build -t stvdy-server:ci .
      - name: The pin is the pin
        run: |
          # An image that silently tracked `stable` would pass every other check.
          built=$(docker run --rm --entrypoint /usr/bin/chromium stvdy-server:ci --version)
          declared=$(grep -oP '^ARG CHROMIUM_VERSION=\K.*' Dockerfile)
          test "$built" = "Chromium $declared" || { echo "built $built, declared $declared"; exit 1; }
      - name: Migrate, then boot
        run: |
          docker run --rm --network host -e DATABASE_URL=… stvdy-server:ci --migrate-only
          docker run -d --name stvdy --network host --shm-size=1g \
            -e DATABASE_URL=… -e APP_ENV=prod -e JWT_SECRET=… -e ENCRYPTION_KEY_HEX=… \
            -e STORAGE_BACKEND=local -e STORAGE_LOCAL_DIR=/tmp/objects \
            stvdy-server:ci
          for i in $(seq 1 60); do curl -fsS localhost:8080/health && break; sleep 1; done
          curl -fsS localhost:8080/health | grep -q '"status":"ok"'
      - name: The PDF pipeline runs here, not only on a laptop
        # The same command the `pdf` job runs on a bare runner, but against the
        # image's own Chromium, pdfium and assets. This is Risk 2's defence: a
        # laptop-only pass is exactly the failure Phase 6 named.
        run: |
          docker exec stvdy sh -c \
            'cd /opt/tests && CHROME_PATH=/usr/bin/chromium \
               ./deps/pdf-$(ls deps | grep -oP "pdf-\K[0-9a-f]+" | head -1) --ignored'
```

The last step's exact binary name is resolved at implementation time (cargo's test binary
names carry a hash); `scripts/release-image.sh` is the local, readable form of the same
sequence so a developer can run it on their machine.

- [ ] **Step 2: `scripts/release-image.sh`**

Builds the image, runs the three steps above against a local Postgres, prints the pinned
versions it found, and ends with `=== PASS`. It is the command `docs/OPERATIONS.md` tells
an operator to run **on the production host** before deploying a new image — because
"the image works in CI" and "the image works on this host" are different claims, and the
second one is the one that ships.

- [ ] **Step 3: Record the measured output**

The phase summary quotes the job's output, per roadmap verification gate 3.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 10: `scripts/verify-release-artifacts.sh` — the machine-readable release gate

**Files:**
- Create: `scripts/verify-release-artifacts.sh`
- Modify: `docs/RELEASE.md`

**Interfaces:**
- Consumes: Tasks 3, 4, 5.
- Produces: a script that takes a directory of built artifacts and a mode
  (`--macos` / `--windows` / `--manifest` / `--all`), verifies what is verifiable, prints
  what it checked, and exits non-zero on any failure.

- [ ] **Step 1: The refusals, first**

```bash
# The private key is used by exactly one workflow (A309). This script must never
# have it: a verification tool that can sign is a signing tool, and a signing tool
# on a developer's machine is how the key leaks.
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "refusing to run with TAURI_SIGNING_PRIVATE_KEY set: verification must not sign" >&2
  exit 2
fi
```

- [ ] **Step 2: The checks**

- **macOS:** `codesign --verify --deep --strict`, `spctl -a -vvv -t install`,
  `xcrun stapler validate`, `lipo -archs` contains both architectures.
- **Windows:** `osslsigncode verify`, and the digest algorithm is SHA-256, and a timestamp
  is present.
- **Manifest:** the committed `pubkey` verifies the published `latest.json`, and **a
  one-byte mutation of it does not** — the negative case is what makes the positive one
  meaningful.
- **Version:** the tag, `Cargo.toml` and `tauri.conf.json` agree (A330).
- **Presence:** every artifact the manifest names exists at the URL it names (a manifest
  pointing at a 404 is a channel that reports "up to date" forever).

Each check prints `ok <what>` or `FAIL <what> <observed>`; the script is quiet on success
and specific on failure, because the person reading it is mid-release.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 11: Docs — `docs/RELEASE.md` becomes normative, `docs/ARCHITECTURE.md` gets the image

**Files:**
- Modify: `docs/RELEASE.md`, `docs/ARCHITECTURE.md`, `docs/ASSUMPTIONS.md`,
  `docs/THREAT_MODEL.md`, `crates/server/tests/route_sweep.rs`

**Interfaces:**
- Consumes: A331; the roadmap's docs obligation (verification gate 4).
- Produces: the normative release document; the image section; A307-A331; the update
  channel in the threat model; the `/assets` route row.

- [ ] **Step 1: `docs/RELEASE.md`, restructured but not rewritten**

It keeps every section it has (Desktop distribution, Mobile distribution, Store policy
risk, Privacy labels, Account deletion, the pinned-Chromium prerequisite, Tax and PPN,
Versioning) and gains: **Release process** (the tag → gate → build → verify → publish
sequence, with the commands), **Store submission checklists** (Task 14), **Launch
checklist** (Task 13), **Server deployment** (the image, `--migrate-only`, the environment,
the reverse proxy, R2), and a pointer to `docs/OPERATIONS.md` for day-two procedures
(A331). The versioning section gains A330's two-binary table.

- [ ] **Step 2: `docs/ARCHITECTURE.md`**

The image's contents (Task 6 Step 8) and a short `### The print assets are served, not
fetched` subsection recording A316's reason — `document.write` gives the page no base URL
— because that is the kind of fact a future reader would otherwise "simplify" back into a
CDN reference.

- [ ] **Step 3: `docs/ASSUMPTIONS.md`**

A307-A331 as rows, in the house format (area | decision | rationale), appended after A131.
B1/B2/B3's status cells are updated with the workaround that is live (the dev mail sink,
the internal payment contract, the draft legal text) — **their "Blocks public launch?"
values stay `Yes`**, because none of them has been resolved by this phase.

- [ ] **Step 4: `docs/THREAT_MODEL.md`**

The update channel is an asset that did not exist before this phase, so § 2 (Assets) gains
it, § 5 gains its row (*"Forge an update manifest" → **Fails**, enforced by the minisign
signature verified against the compiled-in public key*), and § 6 gains the residual risk
(*R7 — the update key is a single point of trust; a compromised key forges an update for
every installed copy; the remedy is key custody and rotation, and there is no client-side
defence because the client is the thing being replaced*). § 9's verification table gains
the row: *update manifest signature verified and a mutation rejected* | 15 |
`crates/server/tests/release.rs`.

- [ ] **Step 5: The route sweep**

`/assets/{*path}` gets its `Exempt` row in `crates/server/tests/route_sweep.rs`'s table
with its one-line reason ("a public file with no user data; the requester is Chromium with
no credentials"). The sweep fails on an unclassified route and on a stale row, so this is
not optional.

- [ ] **Step 6: Run the sweeps**

```bash
cargo test -p stvdy-server --test route_sweep
cargo test -p stvdy-server --test release
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 12: Licensing artifacts — the EULA draft and a corrected notices file

**Files:**
- Create: `docs/EULA.md`
- Modify: `THIRD_PARTY_NOTICES.md`, `LICENSE`, `docs/RELEASE.md`, `docs/ASSUMPTIONS.md`

**Interfaces:**
- Consumes: A44, A327, B2.
- Produces: a terms/privacy draft, a notices file that names the fonts it actually ships,
  and the image's components listed.

- [ ] **Step 1: The EULA draft**

`docs/EULA.md` carries the clauses A44 names, and **opens with a draft banner** stating it
requires legal review before publication, because this plan is not legal advice and a
terms document nobody reviewed is worse than none: (1) no reverse engineering; (2) no
circumvention of entitlement, credit or rate-limit checks; (3) tampering is a material
breach and grounds for termination; (4) the server is the sole authority on entitlements;
(5) prepaid periods, no auto-renewal, remaining prepaid time forfeited on deletion; (6) the
privacy notice's data inventory (account email, uploaded material, chat content, calendar
events, device identifiers for push, payment records) and the retention terms. The
privacy half is where B2's legal input lands.

- [ ] **Step 2: The notices file, corrected**

`THIRD_PARTY_NOTICES.md`'s bundled-assets table says IBM Plex Sans / IBM Plex Mono, and
`app/public/fonts/` ships Plus Jakarta Sans, Inter and JetBrains Mono (A105 changed the
typefaces). That is a compliance defect: an attribution for fonts the product does not
ship, and no attribution for the three it does. The table is rewritten from the directory
listing, and it gains the image's components: Chromium (BSD-3-Clause plus its own
third-party notices), pdfium (BSD-3-Clause), LibreOffice (MPL-2.0), the ONNX Runtime (MIT)
and the `intfloat/multilingual-e5-base` model (MIT).

- [ ] **Step 3: One sentence in `LICENSE` and one in `docs/RELEASE.md`**

`LICENSE` gains a pointer: the hosted service is offered under `docs/EULA.md`. `docs/RELEASE.md`
gains a row in the launch checklist naming B2 as the blocker for both the tax line items
and the privacy text.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change (no user-facing copy).

---

## Task 13: `docs/OPERATIONS.md` and the launch checklist

**Files:**
- Create: `docs/OPERATIONS.md`
- Modify: `docs/RELEASE.md`, `docs/ASSUMPTIONS.md`, `.env.example`

**Interfaces:**
- Consumes: A307, A308, A321, A324-A326; B1/B2/B3.
- Produces: the day-two runbook and the one place the launch decision is made.

- [ ] **Step 1: The production host checklist (A307)**

A section an operator reads before buying anything: region (Indonesia, for UU PDP
residency and the latency the Batam trial demonstrated), managed Postgres 16 + pgvector
with the extensions the migrations create, a lifetime that matches the product (**A45's
trial host is named here as disqualified**), egress, a static IP or DNS, the ability to
run one container with `--shm-size` and a writable `/tmp` (A315), and TLS termination.
Each row has a pass condition, so "is this host suitable" has an answer.

- [ ] **Step 2: Deployment, step by step**

The exact sequence: pull the image, run `--migrate-only` once, start the container with
the environment from `.env.example`'s production subset, point `WEB_ORIGIN` at the web
build's origin, set `STORAGE_BACKEND=s3` with the R2 values (A308), set `SEARXNG_URL` at
the separate service (A321), verify `/health`, and run `scripts/release-image.sh` on the
host. Then the reverse proxy's rules: TLS, the SSE route needs buffering off (the
heartbeat exists precisely because proxies close idle streams), and the `GET /assets/`
route is public.

- [ ] **Step 3: Backups and the restore drill (A324)**

RPO 15 minutes, RTO 4 hours. Postgres: managed PITR if the host offers it, otherwise a
`pg_dump` on a timer to R2 with a 30-day retention and a daily verification that the dump
is non-empty and restorable. R2: bucket versioning plus a lifecycle rule for the
`reports/` and `cheatsheets/` prefixes. **`scripts/restore-drill.sh`** restores the newest
dump into a scratch database, runs `sqlx migrate info`, compares row counts for the
`+sync` tables, and prints `=== PASS`. Its output goes in this section, dated, because a
backup that has never been restored is a hope.

- [ ] **Step 4: Monitoring and alerting (A325)**

Four signals with thresholds and an owner: `/health` from an external uptime check
(60 s interval, alert after two failures); the oldest `queued` job's age (alert above 15
minutes — a stuck worker is invisible otherwise); provider spend against
`provider_cost_ceiling_usd` (alert at 80% — the ceiling is enforced in code (A41) but
nobody is told when it trips); and the 5xx rate over five minutes. Alert routing is a
placeholder the user fills in, and the section says so.

- [ ] **Step 5: The breach runbook (A326)**

Numbered, in the order to do them: contain (rotate `JWT_SECRET` and
`ENCRYPTION_KEY_HEX`, revoke all sessions, disable signups), preserve (snapshot the
database and the logs before rotating anything else), assess (which tables, which users,
from the data inventory), notify (UU PDP's window; the template is here), and review. The
two facts a runbook needs — who decides, and how long we have — are stated as blanks the
user fills, because inventing them would be worse than leaving them visible.

- [ ] **Step 6: Retention (A326)**

`change_log` 90 days (A19), deleted accounts purged by `purge_deleted_accounts`, R2
lifecycle rules, `var/mail` never used in production, and the log retention the operator
chooses. One table, one row per data class, with the mechanism that enforces it — a
retention policy with no mechanism is a sentence.

- [ ] **Step 7: The launch checklist**

In `docs/RELEASE.md`, a table whose every row is **code**, **user** or **blocked**:

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| 1 | Every phase's exit gate green on the release commit | code | `ci.yml` run URL |
| 2 | macOS signed + notarized + stapled | code | `scripts/verify-release-artifacts.sh --macos` |
| 3 | Windows installer signed | code | `--windows` |
| 4 | Updater manifest signed and published | code | `--manifest` |
| 5 | Image built and the PDF test passes inside it | code | `--all` + the `image` job |
| 6 | Store checklists complete | code | Task 14 |
| 7 | Mobile bundle has no purchase UI | code | `scripts/check-mobile-no-purchase.sh` |
| 8 | EULA/privacy reviewed and published | **user** | legal review |
| 9 | Tax/PPN treatment settled, invoices carry the line items | **user** | B2 |
| 10 | SMTP credentials set; a real verification email arrives | **user** | B3 |
| 11 | QRIS gateway integrated against its real docs | **blocked** | B1 |
| 12 | Production host provisioned and `/health` green there | **user** | A307, A45 |
| 13 | R2 bucket live and a presigned URL works | **user** | A15/A308 |
| 14 | Restore drill run against the production backup | code + user | `scripts/restore-drill.sh` output |
| 15 | Uptime check and alert routing live | **user** | A325 |
| 16 | Breach runbook's two blanks filled in | **user** | A326 |

**The checklist is the phase's honest boundary.** Rows 1-7 are this plan's; rows 8-16 are
not, and a plan that marked them done would be lying about the only thing that matters at
launch (Risk 3).

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Task 14: Store submission checklists and the no-purchase-UI evidence

**Files:**
- Create: `scripts/check-mobile-no-purchase.sh`
- Modify: `docs/RELEASE.md` (§ Store submission checklists, § Mobile distribution)

**Interfaces:**
- Consumes: A24, A328, Phase 14's builds; the existing enforcement in
  `config/billing_policy.toml`, `BillingPolicy::validate`, `GET /plans` and
  `POST /billing/checkout`.
- Produces: two per-store checklists and the command that proves the mobile claim.

- [ ] **Step 1: The evidence script**

`scripts/check-mobile-no-purchase.sh` greps the built mobile bundle
(`app/dist` as packaged into the iOS/Android artifact) for purchase UI: the price
strings from `config/plans.toml` (`Rp 10.000`, `Rp 100.000`, `Rp 25.000`, `Rp 250.000`),
the checkout route path, the upgrade CTA keys, and `billing.checkout`. It fails on a hit,
and it **also fails when it finds zero files to check**, because a scan of an empty
directory passes every grep and proves nothing.

- [ ] **Step 2: The Apple checklist**

Privacy labels (each data class from the EULA's inventory, with its purpose and whether it
is linked to identity), in-app account deletion (a row pointing at the screen and at
`DELETE /account`'s behaviour: deletes or anonymizes, revokes sessions, cancels pending
jobs, forfeits remaining prepaid time **stated before confirmation**), review notes
explaining QRIS-only billing, that the app sells nothing, and where a reviewer can create
an account and see the read-only plan screen. Plus the build-number rule (mobile build
numbers bump per submission, `macOS.bundleVersion`'s iOS sibling).

- [ ] **Step 3: The Google checklist**

Data Safety form (the same inventory, mapped to Google's categories), the account-deletion
URL, review notes, the internal-testing track before production, and the Play signing key
vs the direct-APK key (A329).

- [ ] **Step 4: The policy-risk note, kept current**

`docs/RELEASE.md`'s existing "Store policy risk" section already names the risk (both
stores may require their own IAP for digital subscriptions, and a QRIS-only app that
unlocks paid features may be rejected). It gains the confirmation step: **check the
current Indonesian rules before submission**, with the date, because this note is the one
part of the release document that can expire.

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change (checklist text, not user
copy).

---

## Task 15: README and deployment notes

**Files:**
- Modify: `README.md`, `docs/RELEASE.md`, `justfile`

**Interfaces:**
- Consumes: Tasks 6, 9, 13; the existing README structure.
- Produces: a README whose "Run it" and "Ship it" halves are both real.

- [ ] **Step 1: The README's deployment section**

The existing README documents local development thoroughly (it is 200+ lines of quickstart,
Supabase, quality gates and layout). It gains a `## Deploying` section that is a **pointer
and a summary**, not a second copy of `docs/OPERATIONS.md`: the image, the
`--migrate-only` step, the environment's production subset, and a link. Two documents that
both explain deployment drift within a month; one that points at the other does not.

- [ ] **Step 2: The release section**

How to cut a release: bump the version, tag, watch the workflow, run the verifier, publish.
Three sentences and a link to `docs/RELEASE.md` § Release process.

- [ ] **Step 3: The mobile section**

How to build for TestFlight and Play, and where the checklists live. This is the section
Phase 14's work needs to be reproducible, and it names the two build commands
(`tauri ios build` / `tauri android build`) that the roadmap's verification gate 2 lists.

- [ ] **Step 4: The justfile**

`just image` (build the image), `just image-check` (`scripts/release-image.sh`),
`just verify-release <dir>` (`scripts/verify-release-artifacts.sh`),
`just restore-drill` — the four commands an operator runs most, aliased so the documented
command and the typed command are the same string.

- [ ] **Step 5: Run the whole quality gate**

```bash
just check
cargo test --manifest-path src-tauri/Cargo.toml
node scripts/check-no-cdn.mjs
```

**Gates:** 5 — no UI change. 6 — no UI change. 7 — no UI change.

---

## Phase 15 exit checklist

- [ ] `src-tauri/tauri.conf.json` carries `plugins.updater` with `endpoints` and `pubkey`,
      `createUpdaterArtifacts: true`, `macOS.hardenedRuntime` + `entitlements`, and
      `windows.digestAlgorithm: "sha256"` + a `timestampUrl`; `tauri-plugin-updater` is a
      desktop-only dependency and `updater:default` is in `capabilities/desktop.json` and
      **not** in `default.json` or `mobile.json`.
- [ ] A signed manifest verifies against the committed public key and a one-byte mutation
      of it does not; an unsigned manifest does not (`crates/server/tests/release.rs`).
- [ ] The Settings → Updates panel offers the update, never applies one automatically, and
      shows only the version and notes the manifest carried.
- [ ] `tauri build --target universal-apple-darwin` produces a binary whose `lipo -archs`
      lists both architectures; `codesign --verify --deep --strict`, `spctl -a -vvv` and
      `xcrun stapler validate` all pass; the notarization is `Accepted`.
- [ ] The NSIS installer is signed, SHA-256, and timestamped, and `osslsigncode verify`
      says so.
- [ ] `Dockerfile` installs a **pinned** Chromium (asserted equal to the running
      container's `--version`), pdfium at a verified checksum, the embedding model at an
      absolute `FASTEMBED_CACHE_DIR`, and LibreOffice behind `SOFFICE_BIN`.
- [ ] The `pdf` acceptance test runs **inside** the image and passes; `/health` answers
      `ok` from the running container; `--migrate-only` migrates and exits without binding.
- [ ] The container runs as a non-root user with Chromium's sandbox enabled.
- [ ] The template's CDN references are rewritten to the server's `/assets/` route, the
      route serves KaTeX and the fonts with the right content types, and a rewrite that
      left a remote reference fails a test.
- [ ] `scripts/verify-release-artifacts.sh` refuses to run with the signing private key in
      its environment, and its checks pass on the release artifacts.
- [ ] `docs/EULA.md` exists as a draft marked for legal review; `THIRD_PARTY_NOTICES.md`
      names the fonts the product actually ships and the image's components.
- [ ] `docs/OPERATIONS.md` covers the host checklist, deployment, backups with a restore
      drill, monitoring with thresholds, the breach runbook and retention.
- [ ] Both store checklists exist, `scripts/check-mobile-no-purchase.sh` passes against the
      built mobile bundle **and fails on an empty directory**, and the policy-risk note
      carries its confirmation step.
- [ ] `README.md` has Deploying, Release and Mobile sections that point at the normative
      documents rather than restating them.
- [ ] `docs/RELEASE.md` carries the release process, the store checklists, the launch
      checklist and the pointer to `docs/OPERATIONS.md`.
- [ ] `docs/{ARCHITECTURE,ASSUMPTIONS,THREAT_MODEL}.md` and the route sweep are updated in
      the same phase.
- [ ] The full quality gate is green.

### Exit checklist table (gate | command | result)

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --all -- --check` + `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` | pending |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` + the `src-tauri` manifest | pending |
| Tests | `cargo test --workspace` + `cargo test --manifest-path src-tauri/Cargo.toml` | pending |
| Web | `npm --prefix app test` + `npm --prefix app run build` | pending |
| Assets | `node scripts/check-no-cdn.mjs` | pending |
| Image | `docker build -t stvdy-server:ci .` + `bash scripts/release-image.sh` | pending |
| PDF in the image | the `image` job's in-container `--test pdf -- --ignored` | pending |
| Release artifacts | `bash scripts/verify-release-artifacts.sh --all <dir>` | pending |
| Mobile policy | `bash scripts/check-mobile-no-purchase.sh <bundle>` | pending |
| Roadmap gate 5 | the Settings → Updates panel, measured in a browser, recorded in `docs/ARCHITECTURE.md` | pending |
| Roadmap gate 6 | **no UI change** (two i18n strings; the parity test covers them) | n/a |
| Roadmap gate 7 | the Settings → Updates panel: no uncomputed number, no unverifiable claim | pending |

---

## Execution notes (to be written as built)

*(Placeholder removed at execution time. Three things are known in advance and are named
so the executor does not treat them as surprises:)*

1. **The exact Chromium package version and the pdfium checksum are read from the archive
   and the release at implementation time, not guessed here.** A plan that pinned a
   version string it invented would be a plan whose image does not build.
2. **The cargo test binary's hashed name inside the image** (`deps/pdf-<hash>`) is
   resolved by the job rather than hard-coded; `scripts/release-image.sh` is the readable
   form.
3. **`tauri-plugin-updater`'s exact manifest JSON shape** is read from the plugin's source
   at implementation time, because the signature is over the manifest bytes and getting
   that wrong produces a channel that reports "up to date" forever rather than an error.

---

## Self-Review

- **Spec coverage:** the roadmap's Phase 15 row asks for macOS signing + notarization +
  universal binary (Task 3), Windows code signing (Task 4), a Tauri updater with signed
  manifests (Tasks 1, 2, 5, 10), an Android direct-APK channel (Task 5, A329), store
  submission checklists (Task 14), README + deployment notes + server Docker image
  (Tasks 6-9, 15). The Launch-prerequisites risk-register row asks for licensing artifacts
  (Task 12), abuse controls (Phase 7's, recorded in Task 13's checklist), backups,
  monitoring, a breach runbook and retention (Task 13), the hosting decision (A307, Task
  13) and the R2 production storage (A308, Task 6/13). `docs/SPEC.md`'s Phase 15 line adds
  LibreOffice in the image (Task 6, A320). Every one is a task above.
- **The three things this plan refuses to pretend:** that a certificate can be generated
  (the prerequisite table names the user, the artifact and the degraded path); that an
  image that works on a laptop works on a host (the acceptance test runs inside the
  image); that a launch gate can be closed by code (the checklist's three kinds).
- **Placeholders:** none in the plan. Three values are deliberately *deferred to
  implementation* (the Chromium version string, the pdfium checksum, the test binary's
  hashed name) and each says so, because inventing them here would be a plausible-looking
  plan that does not build. `docs/EULA.md` is a marked draft, which is the honest state of
  a legal document this plan cannot finish.
- **The bug this plan exists to avoid:** the tree says the renderer rewrites `ASSET_BASE`
  and **no code does** (`templates/cheatsheet.v1.html:48` vs `grep ASSET_BASE --include=*.rs`
  finding nothing). Task 7 closes it, and its test fails if a remote reference survives the
  rewrite — which is the same class of silent failure the `data-math-ready` guard was added
  for.
- **Type consistency:** `UpdateStatus` is defined once in `app/src/platform/types.ts`;
  `assets_base_url` once in `AppConfig`; `rewrite_asset_base` once in `render::chrome`.
  No new trait, no new job kind (the roadmap § 8 list is untouched), no migration, and
  `0017_sync_triggers.sql` stays the last trigger-attaching migration because this phase
  adds no `+sync` table.
- **Open items carried forward, not closed:** B1 (QRIS gateway — Phase 13's
  implementation is against an internal contract; the field mapping and signature scheme
  remain), B2 (tax/PPN and the privacy text — Task 12 drafts it, a professional reviews
  it), B3 (SMTP credentials — the dev sink is live and is not a production path). All
  three keep `Blocks public launch? = Yes` in `docs/ASSUMPTIONS.md`.
