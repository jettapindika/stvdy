# Release

Filled by Phase 15, except the policy notes below which constrain earlier phases.

## Desktop distribution

Direct download: signed and notarized macOS universal build, signed Windows
installer. No Mac App Store, no Microsoft Store.

- macOS: `universal-apple-darwin` target, `codesign` + `notarytool`, hardened
  runtime, minimum macOS 11.0.
- Windows: `nsis` installer, `perMachine` install mode, code-signed.
- Updater: arrives with the release pipeline, because it needs a signing keypair
  and an HTTPS endpoint for signed manifests. It is deliberately not registered
  before then. The plugin panics at startup if its config is absent, and an
  updater that cannot check for updates is a fake capability.

## Mobile distribution

TestFlight / App Store for iOS, Play internal testing / Google Play for Android.
A direct-APK download from the website is an allowed alternative Android channel.
Mobile updates ship through the stores; the Tauri updater does not support them.

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
