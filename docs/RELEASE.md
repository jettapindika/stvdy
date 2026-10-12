# Release

**This is the normative release document** (A354). Every operational procedure lives either here
or in `docs/OPERATIONS.md`, and nowhere else:

- **This file** is what is distributed, under which policy, with which pinned components, and
  what must be true before a release is published — the release process, the store checklists and
  the launch checklist.
- **`docs/OPERATIONS.md`** is the day-two runbook: the production host checklist, the deploy
  sequence, backups and the restore drill, monitoring, the breach runbook and retention. It is
  the shorter document on purpose, because it is the one an operator opens during an incident.

Filled by Phase 15, except the policy notes below which constrain earlier phases.

## Desktop distribution

Direct download: signed and notarized macOS universal build, signed Windows
installer. No Mac App Store, no Microsoft Store.

- macOS: `universal-apple-darwin` target, `codesign` + `notarytool`, hardened
  runtime, minimum macOS 11.0, **and the notarization ticket stapled into both the `.app` and
  the `.dmg`** (A333) — an unstapled dmg fails on a machine with no network.
- Windows: `nsis` installer, `perMachine` install mode, code-signed, **SHA-256, with an RFC 3161
  timestamp** (A334). Without a certificate the installer builds unsigned and SmartScreen warns
  on every install; that is a recorded degraded state, not a defect.
- Updater: `tauri-plugin-updater` 2.13.3, **desktop only** — the plugin's own platform matrix is
  `ios: none, android: none`, so a mobile build that enabled the capability would fail to compile
  rather than expose an API that does nothing, and mobile updates ship through TestFlight and
  Play. The manifest endpoint and the minisign public key live in `tauri.conf.json`'s
  `plugins.updater`; the **private** key is a GitHub Actions secret used by `release.yml` only, and
  `scripts/verify-release-artifacts.sh` refuses to run if it is present in its environment (A332).
  Until the operator pastes their real public key, the committed placeholder means every signature
  fails to verify — a safe-but-useless updater, not a startup panic. The update is offered, never
  applied silently, and the panel renders only what the manifest carried (A336).

## Mobile distribution

TestFlight / App Store for iOS, Play internal testing / Google Play for Android.
A direct-APK download from the website is an allowed alternative Android channel.
Mobile updates ship through the stores; the Tauri updater does not support them.

**The bundle's own web assets must carry no purchase affordance**, and that is a
command rather than a promise: `scripts/check-mobile-no-purchase.sh` runs the
absence check against the assets a build packages, and
`scripts/check-mobile-no-purchase.sh path/to/artifact.apk` runs it against an
extracted store artifact. Both store checklists below cite it; what it does and
does not inspect is in § Store submission checklists.

## Store policy risk (read before Phase 13/14)

Billing is QRIS only, and **the iOS and Android apps do not sell anything**: no
checkout, no purchase buttons, no prices, and no links or instructions pointing
to a website purchase. Users buy on the web or the desktop app, and the plan then
syncs to their account on every device.

This is enforced in three places rather than by convention:

1. `config/billing_policy.toml`. `validate()` rejects a mobile entry that
   enables purchase UI, prices, or an upgrade CTA, so the file cannot be edited
   into a violation.
2. `GET /plans` omits prices on those platforms.
3. `POST /billing/checkout` is rejected for `platform in (ios, android)`.

**The risk to document and accept:** both Apple and Google require their own
in-app purchase for digital subscriptions in many regions. A QRIS-only mobile app
that unlocks paid features may be rejected, or required to add store billing. The
`PaymentProvider` trait is kept generic so store billing can be added later
without restructuring. Confirm the current rules for Indonesia before submission
rather than relying on this note.

### The confirmation step, and the date

This note is the one part of this document that **can expire**, so it is a step
with a date rather than a paragraph. Before each store submission:

| # | Step | Date | Result |
|---|---|---|---|
| 1 | Re-read Apple's App Review Guidelines § 3.1.1 and § 3.1.3 (In-App Purchase, and the reader/multiplatform exception) for the current Indonesian position | `<fill on the submission date>` | `<required / not required / exception applies>` |
| 2 | Re-read Google Play's Payments policy § "Payments" for Indonesia, and its current list of permitted alternative billing programmes | `<fill>` | `<required / not required>` |
| 3 | Confirm the *residual* risk this leaves: a reviewer who reaches the **web** checkout — which is a normal, correct part of the product on a desktop browser — could read it as an off-store purchase path being advertised to a mobile user | `<fill>` | `<accepted, with the evidence in step 4>` |
| 4 | Point at the evidence that answers step 3, and record its result | `<fill>` | `scripts/check-no-purchase-ui.mjs` and `scripts/check-mobile-no-purchase.sh`, both green on the submission commit |

Step 3 is the risk in its real shape, and it is worth stating precisely because
the mitigation is not a policy edit — it is the *absence of the affordance in the
shipped bytes*. A mobile build that contains no price, no checkout call, no
upgrade CTA and no link to a web checkout cannot advertise one to a reviewer,
whatever the website at large does.

**Where that evidence lives.** Three places, and the third is the one a reviewer's
question actually turns on:

1. `scripts/check-no-purchase-ui.mjs` — the check itself. It holds the purchase
   denylist (a readable rupiah price, `billing/checkout`, a `buy(`/`checkout(`/
   `purchase(` call), the rule that a scan of **nothing** is a failure rather
   than a pass, and a second assertion that `config/billing_policy.toml` still
   sets `allow_purchase_ui`, `show_prices` and
   `allow_purchase_cta_in_locked_messages` to `false` on `ios` and `android`.
   Its own tests are `scripts/check-no-purchase-ui.test.mjs` (nine cases,
   including the empty-bundle case and the strings the app legitimately ships).
2. **CI runs it on every commit**, in the `web` job, *after* `npm run build`
   (`.github/workflows/ci.yml`): a check that is only ever run by hand is the one
   that gets skipped. The same job runs `scripts/check-no-cdn.mjs`, so a build
   cannot satisfy one property by breaking the other.
3. `scripts/check-mobile-no-purchase.sh` — the **release-time** wrapper, run
   against the artifact rather than against `app/dist`. It runs the same check
   (it does not reimplement it) against the web assets a packaged build ships,
   and it reports how many files it inspected. It fails on a hit, on an empty
   directory, and — as exit code 2 — when it *could not check at all*, because
   "could not check" and "clean" are different answers.

The server is the third layer and is not in the bundle: `POST /billing/checkout`
is refused for `platform in (ios, android)` (A24, `crates/server/src/billing/mod.rs`),
and `GET /plans` omits prices where `show_prices` is false
(`crates/server/src/billing/routes.rs`).

## Store submission checklists

Per store, per artifact, with the evidence named (A351). **A row without a command
or an owner is a wish**, so every row below is one of:

- **code** — closed by this repository, and the command that proves it is in the row.
- **user** — a credential, an account, a legal review or a host decision. The user
  supplies it; no agent can. `docs/superpowers/HANDOVER.md` is the live list of what
  the user owes and is the authority for those rows, so this section names the row
  and does not restate it.
- **blocked** — waiting on something nobody has, with the workaround that is live
  until it lands.

**Two artifacts, not one.** The Play build and the direct-APK build carry the same
version string and the same `config/billing_policy.toml`, and they are signed by
**different keys** (A352). Run the checklists against each artifact you actually
upload, and record which key signed it.

### Apple — App Store / TestFlight

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| A1 | **Privacy labels** — every data class in the EULA's inventory, each with its purpose and whether it is linked to identity | **user** | `docs/EULA.md` § 9 ("What we collect, why, and for how long") is the inventory; the labels are App Store Connect fields and the wording is B2's (`HANDOVER.md` B2) |
| A2 | **In-app account deletion**, reachable from inside the app, with the consequence stated *before* confirmation | **blocked** | **Not built** — the § Account deletion section above describes the intended behaviour, not the shipped one. `docs/SPEC.md` requires it, `docs/API.md` § Account tags `DELETE /account` as Phase 15, and `app/src/features/settings/SettingsScreen.tsx` renders it under "Not built yet". No `/account` route and no `purge_deleted_accounts` handler exist in `crates/`. Apple requires in-app deletion of an account an app can create; this blocks submission until it ships |
| A3 | **Review notes** — QRIS-only billing, that the app sells nothing, and where a reviewer creates an account and sees the read-only plan screen | **code** | The plan screen is `app/src/features/billing/MobilePlanScreen.tsx` (A322). The notes themselves are text the user submits |
| A4 | **The build sells nothing** — no price, no checkout, no upgrade CTA, no link to a web checkout | **code** | `scripts/check-mobile-no-purchase.sh <artifact.ipa>` (exit 0), and `node scripts/check-no-purchase-ui.mjs` on the same commit |
| A5 | **Build number** — `CFBundleVersion` bumps on every submission | **code** | Derived from `tauri.conf.json`'s `version` (currently `0.1.0`) into `src-tauri/gen/apple/project.yml` and the generated `Info.plist`. **`bundle.bundleVersion` is not set**, so bumping the shell version is currently the only way to bump it — set `bundleVersion` before the second submission, or the store will reject a duplicate build number |
| A6 | **Apple Developer Program membership + Developer ID certificate**, and the notarization credential | **user** | `docs/superpowers/HANDOVER.md` § Credentials (Phase 15); A333 |
| A7 | **TestFlight internal testing** on a physical device before review | **user** | Needs the account in A6 and a device; `HANDOVER.md` (Phase 14) records that the push and background cases need real hardware |

### Google — Play (internal testing, then production)

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| G1 | **Data Safety form** — the same EULA inventory mapped to Google's categories | **user** | `docs/EULA.md` § 9; the mapping and the wording are B2's (`HANDOVER.md` B2) |
| G2 | **Account-deletion URL** — a public web page where a user can request deletion without installing the app | **blocked** | The URL does not exist and the server route behind it does not either (see A2). Play requires the URL for any app with account creation |
| G3 | **Review notes** — QRIS-only, no purchase UI, how to reach the read-only plan screen | **code** | Same as A3 |
| G4 | **The build sells nothing** | **code** | `scripts/check-mobile-no-purchase.sh <artifact.apk>` (exit 0) |
| G5 | **Internal testing track before production** | **user** | Play Console; `HANDOVER.md` § Credentials (Phase 14) for the account |
| G6 | **Play App Signing key vs the direct-APK key** — different keys, recorded per artifact | **user** | A352. Confusing them is how an upgrade fails to install: the direct APK must be signed by the operator's key, and Play re-signs the Play build with its own |
| G7 | **Google Play Console account** | **user** | `docs/superpowers/HANDOVER.md` § Credentials (Phase 14) |

### What the absence check inspects, and what it does not

**What it does:** the text files a mobile build ships to the webview — `.html`,
`.js`, `.css`, `.json`, `.svg` — under `app/dist` and, for an archive target,
under every extracted member. It also re-asserts `config/billing_policy.toml`.

**What it does not, and no part of its output should be read as covering:** the
native layer (`lib*.so`, `classes.dex`, `Payload/*.app` binaries), the store
metadata and screenshots, the review notes, or whether the account-deletion
screen behaves as described. Rows A2, G2 and A1/G1 are exactly the ones it cannot
answer.

**It cannot be exercised against a real mobile artifact on this machine**, and
that is stated rather than implied: there is no Android SDK (`~/Library/Android/sdk`
is absent) and no signed iOS build, so no `.apk` or `.ipa` exists to point it at.
The CI-only path is `scripts/check-mobile-no-purchase.sh` with no argument, which
checks `app/dist` — the tree every mobile build packages. What *has* been
exercised is the script's failure modes: a hit (exit 1), an empty directory
(exit 1), an artifact with no web assets at all (exit 1), an uncheckable
argument (exit 2), and a clean tree (exit 0).

## Launch checklist

**Every row is `code`, `user` or `blocked`, and the checklist cannot be ticked by
an agent.** That is the point: it is the one place where "I did the reachable
part" and "the product can launch" are visibly different sentences. Rows 1-7 are
this phase's; rows 8-16 are not, and a plan that marked them done would be lying
about the only thing that matters at launch.

`docs/superpowers/HANDOVER.md` is the live list of what the user owes; the rows
below name it rather than repeating it, so the two cannot drift.

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| 1 | Every phase's exit gate green on the release commit | **code** | the `ci.yml` run URL for the release commit |
| 2 | macOS signed + notarized + stapled | **code** | `scripts/verify-release-artifacts.sh --macos` |
| 3 | Windows installer signed | **code** | `scripts/verify-release-artifacts.sh --windows` |
| 4 | Updater manifest signed and published | **code** | `scripts/verify-release-artifacts.sh --manifest` |
| 5 | Image built and the PDF test passes inside it | **code** | `scripts/release-image.sh` (builds, boots against Postgres, runs the PDF acceptance test inside the container, compares the running Chromium to the pin) + the `image` job in `ci.yml`. `verify-release-artifacts.sh --all` is **not** this row: `--all` is version + macos + windows + manifest |
| 6 | Store checklists complete | **code** | § Store submission checklists above |
| 7 | Mobile bundle has no purchase UI | **code** | `scripts/check-mobile-no-purchase.sh` (exit 0, with the file count in its output) |
| 8 | EULA/privacy reviewed and published | **user** | `docs/EULA.md` is a marked draft; legal review, `HANDOVER.md` B2 |
| 9 | Tax/PPN treatment settled, invoices carry the line items | **user** | B2 (`HANDOVER.md`); invoices currently carry no tax lines |
| 10 | SMTP credentials set; a real verification email arrives | **user** | B3 (`HANDOVER.md`) |
| 11 | QRIS gateway integrated against its real docs | **blocked** | B1 (`HANDOVER.md`); Phase 13 is implemented against an env-driven internal contract |
| 12 | Production host provisioned and `/health` green there | **user** | A330, A45 (`HANDOVER.md`); the Batam VPS is a 1-month trial and is staging only |
| 13 | R2 bucket live and a presigned URL works | **user** | A15/A331 (`HANDOVER.md`) |
| 14 | Restore drill run against the production backup | **user** | `scripts/restore-drill.sh` exists and runs; running it against **production** needs the host in row 12, so the production claim is the user's |
| 15 | Uptime check and alert routing live | **user** | A348; `docs/OPERATIONS.md` carries the four signals, the routing is the operator's |
| 16 | Breach runbook's two blanks filled in | **user** | A349; "who decides" and "how long we have" are blanks on purpose |

**Two rows this slice added, because they are store blockers and were not in the
plan's table:** A2/G2 (in-app account deletion and the deletion URL — specified,
unbuilt) and A5 (the mobile build number has no `bundleVersion` to bump). Both are
named here rather than fixed, and neither is this slice's code to write.

## Privacy labels

Store privacy disclosures must be confirmed before launch. Known data the app
handles: account email, uploaded study material, chat content, calendar events,
device identifiers for push, and payment records. B2 in `docs/ASSUMPTIONS.md`.

## Account deletion

In-app `Delete account`, also required by app store rules. It deletes or
anonymizes the user's data, revokes all sessions, and cancels pending jobs. There
are no recurring charges to cancel; remaining prepaid time is forfeited, which
must be stated in the UI before confirmation.

`Export my data` produces JSON + Markdown + ICS.

## Release process

The sequence, with the commands. **A release is a tag, not a branch**: tags are not commits, and a
pipeline that trusts a green badge from a branch can ship a commit CI never saw (A345).

### 1. Land everything, and let CI be green on the commit

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
just check-shell                    # src-tauri is outside the workspace, so this is separate
npm --prefix app test
npm --prefix app run build
node scripts/check-no-cdn.mjs
node scripts/check-no-purchase-ui.mjs
node scripts/check-no-purchase-ui.test.mjs
bash scripts/check-mobile-no-purchase.sh
```

`check-shell` depends on `check-web` on purpose: `generate_context!()` panics at compile time when
`app/dist` is missing, and `dist/` is gitignored.

### 2. Bump the version, in the two places that must agree

`Cargo.toml`'s `[workspace.package] version` and `src-tauri/tauri.conf.json`'s `version` (A353 —
see § Versioning for which of the two a hotfix may ship alone). `crates/server/tests/release.rs`
asserts the two are equal, so a bump that misses one fails the suite rather than the release.

### 3. Tag, and push the tag

```bash
git tag v0.2.0            # the shell's version; `server-v…` for a server-only hotfix
git push origin v0.2.0
```

`.github/workflows/release.yml` triggers on `v*`. Its **first job** re-runs the step-1 checks on
the tagged commit; a failure there stops the release before anything is built. Then three build
jobs (macOS universal, Windows NSIS, the Android APK for the direct channel), each uploading its
artifacts **and its `.sig` files** — a job that produced an archive and not its signature is a
failure, and the workflow asserts the `.sig` exists rather than trusting the bundler's exit code.

### 4. The manifest, and the round trip

`latest.json` is generated with one entry per `{{target}}/{{arch}}` pair, each carrying the
`signature` (the contents of the `.sig`) and the artifact's `url`, and it is signed with the same
minisign key. The workflow then **downloads the published manifest back over HTTPS and verifies
its signature against the committed public key**, failing if that round trip does not hold — the
manifest is the artifact the whole channel rests on.

### 5. Verify the artifacts before publishing

The last step of every job runs the gate against what it just produced:

```bash
scripts/verify-release-artifacts.sh --macos   --dir <bundle dir>
scripts/verify-release-artifacts.sh --windows --dir <bundle dir>
scripts/verify-release-artifacts.sh --manifest --manifest-dir <dist dir>
scripts/verify-release-artifacts.sh --version --tag v0.2.0
```

**This is the step that makes the exit gate mechanical rather than a claim**: a job that produced
an unsigned installer or an unstapled dmg fails here, before the artifact is published. Every mode
is fail-closed — a check that cannot read what it needs is a `FAIL`, never a skip — and the script
**refuses to run at all (exit 2) when `TAURI_SIGNING_PRIVATE_KEY` or its password is in its
environment**, because a verification tool that can sign cannot establish the one property it
exists to establish (A332).

> **The consequence for the workflow, stated so it is not discovered by a failing release:** the
> signing secret must be scoped to the signing **step**, never to the job. A job-wide secret is how
> the key ends up in the environment of every later step — which is exactly the drift A332 forbids,
> and the verifier refuses rather than tolerating it.

### 6. Publish, and record

Attach the artifacts to the GitHub Release, publish `latest.json` and its `.sig` to the static
endpoint `plugins.updater.endpoints` names, and record the run URL in the launch checklist
(`docs/RELEASE.md` § Launch checklist). Then deploy the image — § Server deployment below.

### What degrades without a certificate, and what that costs

Every row is the user's to obtain; `docs/superpowers/HANDOVER.md` is the live list, and this
document does not restate it.

| Missing | What still builds | What it costs |
|---|---|---|
| Apple Developer ID certificate | a universal `.app`/`.dmg`, unsigned | Runs **only** on the machine that built it (Gatekeeper trusts a locally built app) and is blocked on every other Mac. **Not distributable** |
| notarytool credentials | a signed but unstapled dmg | A first launch needs a network round trip to Apple; offline, it fails with the "cannot be opened" dialog |
| Windows Authenticode certificate | the NSIS installer, unsigned | SmartScreen warns on every install. The warning is per-publisher reputation, so it does not decay |
| a real minisign keypair | everything, with a placeholder `pubkey` | Every signature fails to verify — a safe-but-useless updater. The plugin requires `plugins.updater` to exist, so the honest alternative is no updater at all |
| a production host | every artifact | Nothing can be *deployed*. A45 forbids the trial VPS, and putting the manifest endpoint or the QRIS webhook behind it is the specific mistake that row names |

## Server deployment

The image is the deployment unit: API, job worker and scheduler are three tasks in one process
(`crates/server/src/main.rs`), so one container is the whole server. `docs/OPERATIONS.md` is the
step-by-step version, and this section is the summary of what a release has to be true of.

**The environment.** `.env.example` is the contract. Production sets `APP_ENV=prod` (which requires
`JWT_SECRET` of at least 32 characters and a `WEB_ORIGIN`), `DATABASE_URL`, `ENCRYPTION_KEY_HEX`,
`STORAGE_BACKEND=s3` with the R2 values (`S3_ENDPOINT`, `S3_BUCKET`, `S3_REGION=auto`,
`S3_FORCE_PATH_STYLE=false`; A331), `SEARXNG_URL` pointing at the **separate** search service
(A344), and the image's own defaults for `CHROME_PATH`, `PDFIUM_LIB_PATH`, `FASTEMBED_CACHE_DIR`,
`SOFFICE_BIN` and `ASSETS_DIR` (A337-A343).

**The migration is a step, not a side effect.** `--migrate-only` runs to completion and exits
without binding a port; it runs **once, before any replica starts** (A346). That is a deployment
contract rather than a convenience: `main.rs` migrates on every boot with no lock of its own, so
two replicas starting together race, and the one that loses serves traffic against a schema it has
not verified.

```bash
docker run --rm --network host -e DATABASE_URL="$DATABASE_URL" stvdy-server:<tag> --migrate-only
docker run -d --name stvdy --shm-size=1g --tmpfs /tmp <env…> -p 8080:8080 stvdy-server:<tag>
```

`--shm-size` and a writable `/tmp` are both required by Chromium's sandbox (A338).

**The reverse proxy.** TLS termination, `WEB_ORIGIN` pointed at the web build's origin, **buffering
off on the SSE route** (the heartbeat exists precisely because proxies close idle streams), and
`GET /assets/` public — the requester is headless Chromium printing a document, and every file is a
vendored upstream release with no user data in it (A339).

**Object storage.** R2, presigned reads only, no public bucket: `GET /sources/:id/file` already
hands out short-lived scoped URLs, and a public bucket would make the presigning decorative (A331).

**Before deploying, run the image check on the host.** "Works in CI" and "works on this host" are
different claims, and the second is the one that ships:

```bash
IMAGE=stvdy-server:<tag> DATABASE_URL="$DATABASE_URL" bash scripts/release-image.sh
```

It compares the running Chromium to the `Dockerfile`'s pin, checks pdfium, LibreOffice and a
non-empty model cache, runs `--migrate-only`, boots the server, probes `/health` and the four
`/assets/` paths, and then runs the PDF acceptance test **inside the container** — the same test
CI's `image` job runs, against the image's own browser. A pass proves the print assets resolved,
because a missing KaTeX leaves every formula as literal LaTeX.

## The server image bakes a pinned Chromium

**There is now a `Dockerfile`** (Phase 15, Task 6), and it is the authority for what the image
contains; `docs/ARCHITECTURE.md` § Container image is the table, and `docs/OPERATIONS.md` is
how to run it. Phase 6 recorded the requirement before the image existed, and it is unchanged:

- **The image bakes a pinned Chromium and sets `CHROME_PATH` to it.**
  `crates/server/src/render/chrome.rs::find_executable` reads `CHROME_PATH` first and
  only then probes the usual install locations, so a deployment that names its build
  gets exactly that build. Without `CHROME_PATH` the server falls back to a probe
  list that is meant for a developer's laptop, and a runner or image that has no
  Chrome at all leaves `AppState.renderer` as `None`.
- **A missing browser is not a crash.** Without a renderer the server still starts,
  still generates cheatsheets (the blocks and the HTML are stored, and the document
  is `ready`), and answers 409 `conflict` on `GET /cheatsheets/:id/pdf` and 502
  `provider_error` on a report's `format=pdf` export. So the symptom of a missing
  Chromium in the image is **a PDF nobody can download**, not a failed deploy — which
  is exactly why it needs to be a documented image requirement rather than something
  a health check catches (A130).
- **The image bakes headless LibreOffice and sets `SOFFICE_BIN` to it.** Phase 7's
  DOCX and PPTX sources get a PDF rendition through
  `soffice --headless --convert-to pdf` (`crates/server/src/sources/parse.rs::preview_pdf`),
  stored under `sources.preview_key` and served by `GET /sources/:id/preview`, so the
  client opens one media type for every source kind. This follows A51's absent-binary
  policy: when `SOFFICE_BIN` is unset the source **still ingests** — the text and the
  slide numbers come out of the OOXML itself, with no converter — the preview is absent
  and one `tracing::info!` is logged; when it is set but the conversion fails, the source
  still ingests and a `tracing::warn!` is logged, so an unloadable converter is louder
  than an absent one (A158). A missing LibreOffice is therefore not a crash and not a
  failed ingest: it is a source with no preview, and the remedy is the image.
- **The version is pinned, and the pin is asserted.** `ARG CHROMIUM_VERSION` names a
  specific Debian package version (never the `chromium` meta-package and never `stable`),
  because the renderer's output is the document a student prints and the acceptance test
  asserts a specific page count and a footer read back from the renderer. CI's `image` job
  compares the version the running image reports (`chromium --version`) to that `ARG`, so a
  base-image change that silently moves Chromium **fails the build** rather than changing
  every PDF.
- **CI runs the acceptance test, and runs it in the image too.** `.github/workflows/ci.yml`
  has two jobs: `pdf` runs
  `CHROME_PATH=/usr/bin/google-chrome cargo test -p stvdy-server --test pdf -- --ignored`
  on `ubuntu-24.04` (failing before the test if that path is not executable), and `image`
  builds the Dockerfile, boots it against Postgres and runs the same test **inside the
  container** against the image's own Chromium, pdfium and assets. The test is `#[ignore]`d
  by default so a laptop without a browser still passes `cargo test --workspace`; the
  second job is what keeps that from making it a test nobody runs. `scripts/release-image.sh`
  is the readable form of the same sequence, and it is what an operator runs on the
  production host before deploying — "works in CI" and "works on this host" are different
  claims.
- **The image runs as a non-root user with Chromium's sandbox enabled**, and needs
  `--shm-size` plus a writable `/tmp`. Running as root with `--no-sandbox` is explicitly not
  the answer (A338); the `docker run` stanza is in `docs/OPERATIONS.md`.

The fonts the template names (`Inter`, `JetBrains Mono`, and the KaTeX woff2 files) are
**vendored in the repository and served by the server**, not fetched at render time. The
template used to link them from a CDN while its own comment claimed the renderer rewrote
`ASSET_BASE` — and no code did, so a sealed container rendered every formula as literal `$…$`.
`GET /assets/{*path}` serves them (`crates/server/src/assets.rs`, embedded with `rust-embed`)
and `render::chrome::rewrite_asset_base` points the template's references at it before the
page is written, using `ASSETS_BASE_URL` (default `http://127.0.0.1:<BIND_ADDR port>`; A339).
The `data-math-ready` guard is unchanged and is still the gate: an unreachable asset leaves
`renderMathInElement` undefined, the template marks `unavailable`, and the render fails rather
than shipping a document full of LaTeX source (A340).


## Tax and PPN

Unconfirmed (B2). Invoices currently carry no tax line items. Tax treatment and
PPN handling must be settled before launch.

## Versioning

`CARGO_PKG_VERSION` for the server and shell, the same value in
`tauri.conf.json`. Mobile build numbers are bumped per store submission.

**The server and the shell are versioned independently, and the tag names which one ships**
(A353). They are separate binaries with separate release cadences, and clients are not
recallable — so a server-only fix must be able to ship alone, and an update prompt for a fix
that does not touch the client is noise.

| Artifact | Versioned by | Reported by | Shipped by |
|---|---|---|---|
| The **shell** (desktop app) | `src-tauri/tauri.conf.json` `version`, plus `CARGO_PKG_VERSION` of `src-tauri` | the bundle's own metadata; the Settings → Updates panel shows what the manifest said | a `v*` tag, through the macOS/Windows jobs |
| The **server** | the workspace `Cargo.toml`'s `[workspace.package] version` | `GET /health`'s `version` field | the image, from the same repository at its own cadence |
| **Mobile** | the same shell version, plus a build number bumped per submission | the store | TestFlight / Play — **not** the Tauri updater |

So a release tag names the shell, and a server-only hotfix is tagged `server-v…` and skips the
client jobs. The workflow asserts the three values agree and fails with all three printed, and
`scripts/verify-release-artifacts.sh --version` is the same check on demand — it accepts a
`server-v…` tag against the workspace version **alone**, because the shell is not part of that
release. `crates/server/tests/release.rs::the_version_is_the_workspace_version` binds
`tauri.conf.json`'s version to the workspace version, so a bump that misses one fails the suite
rather than the release.

**What a hotfix may ship alone: the server.** The client cannot be recalled, and a client release
for a server-only fix asks every installed copy to download and reinstall for no change. The
reverse is not true: a client fix needs a release, and an installed copy gets it only by accepting
the update the manifest offers.

## Day-two operations

Backups and the restore drill, monitoring and alerting, the breach runbook, retention windows,
the production host checklist and the step-by-step deploy sequence are all in
**`docs/OPERATIONS.md`** (A354). They are there rather than here because this document is the
release's, and that one is the operator's — during an incident the shorter document is the one
that gets read.

Three things in it are launch prerequisites rather than nice-to-haves, and the launch checklist
rows name them: the restore drill (row 14), the uptime check and alert routing (row 15), and the
breach runbook's two blanks (row 16).
