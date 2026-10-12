# Handover — what the user needs to do

Written while the remaining phases execute. Everything that could be done without you has
been; this is the list of things that cannot be.

## Launch blockers found in the code, not in `ASSUMPTIONS.md`

These are **code gaps**, not credential gaps, and each one stops a store submission on its own.
They were found while writing the store checklists (Phase 15), and they are recorded here because
nothing in a phase's own file list owned them:

| # | What is missing | Why it blocks |
|---|---|---|
| **In-app account deletion** | **Shipped (A355).** `DELETE /account` and `GET /account/export` in `crates/server/src/account.rs`, the `purge_deleted_accounts` handler in `crates/server/src/jobs/handlers/purge_accounts.rs`, and the settings screen's data section with a type-to-confirm step | No longer a blocker. Apple requires in-app deletion of an account an app can create, and that is now `code` in `docs/RELEASE.md` (A2) |
| **The Play account-deletion URL** | The public **page** does not exist. The route behind it does: `DELETE /account` ships with the app | Google Play requires a public deletion URL for any app with account creation. The page is a deployment artefact rather than code, and it needs the production host — `docs/RELEASE.md` G2 is typed `user` for that reason |
| **The iOS build number** | ~~No build version in the config~~ — **fixed**: `bundle.iOS.bundleVersion` and `bundle.macOS.bundleVersion` are now `"1"`. Note the key is **per-platform**: `bundle.bundleVersion` is not in the CLI schema's `BundleConfig` at all (`app/node_modules/@tauri-apps/cli/config.schema.json`, `IosConfig`/`MacConfig` each define `bundleVersion` as "translates to CFBundleVersion"), so the top-level form would be rejected or silently ignored. Bumping the build number is now a one-line diff rather than a shell-version bump | A second submission without a higher build number is rejected as a duplicate |

The first two are the same feature, and **it is implemented in the working tree and being verified as this was written** — `crates/server/src/account.rs`, `crates/server/src/jobs/handlers/purge_accounts.rs`, `migrations/0037_account_purge.sql`, the two routes, the Settings Data section, and the retention/checklist documents corrected. It is not yet committed, and its acceptance commands were the first casualty of the disk filling up (a link that died with `errno=28`), so treat it as **unverified** until its own report lands. What will remain for it afterwards is not code: the **public Play deletion URL** needs the production host, and the Apple/Google review notes need your accounts.

The gap was a product-scope decision rather than a task: it needed the retention/purge machinery, a confirm step, and copy that says what is deleted. **A160 is the reason it went unowned** — Phase 7's plan re-tagged the `## Account` heading away from itself on the grounds that the work "belongs with the release prerequisites", and no phase's file list then picked it up, so it sat as a promise in `docs/API.md`, `docs/OPERATIONS.md`'s retention table and `docs/EULA.md` § 9 while nothing implemented it.

## One environment note worth acting on

**The machine is nearly out of disk, and it has already cost verification.** `/System/Volumes/Data` had **105 MB free at its worst** and sits around **7 GB free** now; the repository's `target/` is about **80 GB** and `src-tauri/target/` another **16 GB**. Four things died on it: Phase 13's test run with `No space left on device`, Phase 15's one local `docker build`, the Docker daemon itself (`docker info` fails; `~/Library/Containers/com.docker.docker` is 6.2 GB), and a `cargo test` link step with `ld: write() failed, errno=28`. A failed link is the disk, not the source — the signature is the library compiling clean while the test binary fails to link.

**What I have already reclaimed, and what is left:**

| Freed | How | Cost |
|---|---|---|
| 2.7 GB | `rm -rf target/release` | none — nothing in this session builds the release profile except the image build, which needs Docker |
| 1.2 GB | `rm -rf src-tauri/target/release` | none, same reason |
| 14 GB + 2 GB | `rm -rf target/debug/incremental src-tauri/target/debug/incremental` | rebuild time only; rustc regenerates it. **It regrows** — I have cleared it twice — so expect to do it again |

**What is left to take, and it is the big one:** `target/debug/deps` is **~78 GB** and `src-tauri/target/debug/deps` **~7 GB**. The only safe way to take them back is `cargo clean`, which forces a full rebuild.

**Note the two target directories — a `cargo clean` in the repo root does not touch the shell's.** `src-tauri` is its own workspace root (deliberately: `tauri_build::build()` must not run during `cargo clippy --workspace`), so its 16 GB survives a root `cargo clean` and needs its own:

```bash
cd /Users/mac/Documents/Coding/project/Stvdy && cargo clean
cd /Users/mac/Documents/Coding/project/Stvdy/src-tauri && cargo clean
```

**The order that costs only time:** stop every slice, then the two `cargo clean`s, then one `cargo test --workspace` — exit-checklist item 1 for every phase, and it took this session's whole afternoon to get it green once. Do a `cargo clean` while slices are building and every live invocation fails on the `target/` lock, which reads as a code failure rather than a disk one. Two more things outside the repo are yours to judge: `~/Library/Caches/com.spotify.client` (3.1 GB) and `~/.cache/huggingface` (5.7 GB, not this project's — the embedder uses `FASTEMBED_CACHE_DIR`).

## Standing blockers (from `docs/ASSUMPTIONS.md` § Blockers)

These need **you**, not code, and they block a public launch rather than a phase:

| # | What is needed | Why it cannot be automated |
|---|---|---|
| B1 | QRIS gateway identity + API docs, and a PJP to settle through | Phase 13 can implement against an env-driven internal contract (`QRIS_BASE_URL`, `QRIS_API_KEY`, `QRIS_CALLBACK_TOKEN`, `QRIS_SIGNATURE_SCHEME`), but no revenue is possible without the real gateway, and perorangan onboarding (KTP + NPWP + a bank account matching the business name) is lead-time work that must run in parallel with development |
| B2 | Tax/PPN treatment and store privacy-label text | Legal, not technical. Invoices must be correct and the UU PDP privacy notice must be true |
| B3 | SMTP host/user/password and a From address | Every AI action is gated on a verified email, so with the dev file sink no real account can be onboarded. The provider swap is config-only once the credentials exist |

## Credentials needed as the later phases land

| Phase | What |
|---|---|
| 10 (calendar) | A Google Cloud OAuth client for **Calendar** (id, secret, registered redirect URI). The same client serves sign-in-with-Google from Phase 1, but the calendar connection asks for `calendar.app.created` + `calendar.events.readonly` and may want its own client |
| 13 (payments) | The B1 values |
| 14 (mobile) | An Apple Developer account (APNs `.p8` key + key id + team id) and a Firebase project (`google-services.json` + an FCM v1 service account). The push-receive, share-extension and background-scheduler steps need a **physical device** to verify; they degrade to a simulator for everything except the push and background cases |
| 15 (release) | An Apple Developer ID certificate for notarization, a Windows code-signing certificate, and a **production host** (A45: the current VPS is a 1-month trial and must never be production). Also a Cloudflare R2 bucket + credentials for production object storage (A15) |
| 15 (release) | A decision on the updater signing key: where it lives and who holds it. A signed manifest whose key sits on a laptop means an update can be forged |

## The one capability nobody has verified, and it is the one that decides the mobile approach

**Every native probe in Phase 1's capability spike is recorded as NOT ATTEMPTED, and Phase 14 built on top of it anyway.** The roadmap's own Phase 1 text says this explicitly — *"do not defer to Phase 14 … Discover that in Phase 1, not after 13 phases"* — and the reason is that the answer can force a different plugin, a hand-written Swift/Kotlin shim, or a framework change that would have to be asked about before more is built on it.

The probes and their recorded state (`docs/ARCHITECTURE.md:344`, `docs/superpowers/plans/2026-10-11-phase-1-auth-devices-ingest.md:622`):

| Probe | State | What it decides |
|---|---|---|
| APNs token registration (iOS) | **NOT ATTEMPTED** | Whether the pinned community plugin (`tauri-plugin-notifications`, A310) is enough or a custom Swift plugin is needed |
| FCM token registration (Android) | **NOT ATTEMPTED** | The Kotlin equivalent, plus whether the Gradle plugin and `google-services.json` wiring is right |
| Foreground push receipt + tap-to-open, including cold start | **NOT ATTEMPTED** | Tap-to-open is the only guaranteed JS surface (A316), so this is the whole of the reliable push UX |
| iOS "Open in…" document intake | **NOT ATTEMPTED** | Whether `RunEvent::Opened` delivers on a **cold** start under the scene lifecycle, which the capability matrix flags as unverified |
| iOS share-sheet row | **NOT ATTEMPTED** | Whether the Share Extension + App Group path (A312) works, and whether the entitlement is obtainable |
| Android `content://` share/import | **NOT ATTEMPTED** | Whether a concrete `mimeType` in `fileAssociations` delivers a URI the fs plugin can read |

**What this means for you.** The code for all of it is written and its pure halves are asserted, but **none of the six has ever run on a device**. The Phase 0 finding already on record is the warning shot: the official `tauri-plugin-notification` **provably cannot receive remote push** (its iOS delegate skips `UNPushNotificationTrigger` and its Android manifest registers no FCM service), which is exactly the kind of discovery the spike existed to make early. So the honest state is: **the mobile native work is unverified, and a failure here is a design change rather than a bug fix.** Do the spike before building anything further on it — an Apple Developer account and one physical device answer all six rows, and a simulator answers four of them.

## Verified green so far

Every phase's exit gate is recorded in its own plan file, and each phase's working notes are in
`.superpowers/sdd/<plan>/progress.md` (git-ignored scratch, on disk only). **One exception worth
knowing before you trust the table below:** Phase 4's plan has no `## Execution notes` section at
all, so its gates were never recorded anywhere — its tests are covered by the workspace run, and
its two roadmap gates (5 and 6) have no measurement in any document. Phase 0's accessibility box
and Phase 4's are the same gap: two surfaces whose accessibility has never been measured, and the
honest state is "unknown" rather than "fine". The short version:

| Phase | Status |
|---|---|
| 0–5 | Done before this session; verified green again here |
| 6 (reports, cheatsheet, PDF) | **Done.** Implementation, a review round with one Critical fix (an SVG attribute-value injection that reached a printed PDF), the three roadmap gates measured in a real browser (four defects found and fixed), the smoke run passing |
| 7 (entitlements, credits, limits) | **Done.** The engine, the sweep, the ledger with a mutation-proved concurrency test, the period, the abuse controls, the ingestion formats, the tamper suite (doc-driven), admin, the SPA, the gates (one defect found: a 70×19 upgrade link), the smoke run |
| 8 (sync engine) | **Done.** A consumer-visible defect was found and fixed: a pull shipped only the *first* change of a row, so every later change arrived as `data: null` — which the client applies as a **delete**. Gates measured; one WCAG Level A failure found (the document's language attribute never followed the UI language) and fixed |
| 9 (tasks, events, Today, timer) | **Done.** Four defects found and fixed, all "two write paths disagreeing" — a push delete that did not cascade, a restore that did not restore the cascade, a calendar-owned event writable through push, and a local apply that did not cascade. Three `QuickCapture` defects found by measurement and fixed |
| 10 (calendar) | **Done.** Two plan-level errors corrected (the `rrule` bound, and the migration numbering); two more 200%-text layout defects found by the gate and fixed; a two-clock defect in the poll diagnosed from the rows and fixed |
| 11 (syllabus, planner) | **Done.** Two real client defects found by driving the wired process rather than by a unit test, and **four of the slice's own assertions were wrong about the product** — the strongest argument in the project for exercising the running app. **And its exit-gate suite had never been run**: `cargo test -p stvdy-server --test syllabus` was written, committed, left out of the exit checklist's result column, and found red on committed code (14 passed / 5 failed) when the workspace gate was finally run. Four of the five were the same mistaken premise — the test author read `item.index` as a position in the *sorted* review list, while the contract (`crates/core/src/generation/syllabus.rs:109-113`) defines it as the position in the **reply**, across the module's field order; the product was right in every case and the assertions were corrected with the assumption named. The fifth was the test harness's `drain_jobs`, which cannot assert an empty queue for a deliberately failing retryable job. Now **19 passed / 0 failed** (`9f1d6ae`) |
| 12 (agent, reminders, dashboard) | **Done.** One real **cross-tenant write** found by the slice's own injection test and fixed: a task or event could be filed into another account's workspace. The injection test's three-part anti-vacuity proof is the phase's best evidence. **And one defect that cost money, found only by running the suite:** a turn refused for a spent balance set its `charged` flag *before* returning the refusal without recording why, so the next call in the same turn read that flag as "already paid" and **ran its second tool call for free** — a turn that could not afford `agent_turn` still proposed. The first diagnosis (a `limit_reached` refusal falling through a one-string match) was real but partial, and fixing only the string would have left the free call live |
| 13 (payments) | **Done.** The trait with a real-HMAC mock, the pure billing math, the gateway behind one mapping file, the checkout, the webhook with two separately-tested idempotency defences, polling that shares one `apply_paid`, the periods, BYOK, the margin report with its tripwires, the Jakarta money gate, and the smoke run green. **One real hole found and fixed**: the checkout screen never consulted `billing_policy` and the route was registered unconditionally, so a phone build had a purchase screen reachable by a typed URL. **Two live product defects found by the green run itself**: `POST /billing/checkout` was a 500 for every caller (an INT4 amount decoded as i64), and a paying student kept free limits because `users.plan_id` was never written on an upgrade |
| 14 (mobile) | **Done, except the native work.** The bundle-absence proof is now a build-time exclusion rather than a promise: `TAURI_ENV_PLATFORM` keys the store build, every build stamps its own `stvdy-build-target`, and the check refuses anything but `store` — measured, **0** occurrences of `billing/checkout` in the 764,625-byte store chunk against **5** in the 778,765-byte web one, failing closed on an empty directory, an absent stamp and a `full` stamp. Gate 5 now runs at **both** viewports for the first time (13 surfaces, 80 combinations at 390×844 and 80 at 1365×900); six phone-layout defects and three desktop findings came out of it. The native work (push receive, share extension, background scheduler) needs **your** accounts and a device |
| 15 (release) | The release gate, the documents, the licensing artifacts and the store checklists are done; the verification script was exercised against a real signed fixture and found two bugs in itself. The server image is built in CI. Signing, notarization and the store submissions need **your** certificates and a production host |

## Things found that you should know about

- **A live SSRF guard that did DNS work instead of refusing, and the test that proved it was red on committed code.** `guarded_get` read the request host with `Url::host_str()`, which **keeps an IPv6 literal's brackets** (`[::ffff:169.254.169.254]`). `resolve_public` then tried `parse::<IpAddr>()` on that string, failed on the brackets, and fell through to a resolver — so the fetch-time guard, the one whose own doc comment calls itself "the thing that must not be duplicated", did network work where it had to refuse, and answered with a `Provider("resolving …")` error that reads like an outage. `check_fetchable_literal` had already been fixed for exactly this reason and says the suite caught it; the fix landed in the courtesy check and not in the guard. `crates/server/tests/ssrf.rs::the_guarded_text_fetch_refuses_a_private_host_and_a_non_http_scheme` asserted the property correctly, needs no network and no stub, and was **7 passed / 1 failed on committed code** — a test nobody ran, not a test that was missing. Fixed (`d23232c`), now 8/0.
- **A gap in the build's own safety net, found by hand:** the root workspace excludes
  `src-tauri` and `src-tauri` is its own workspace root, so `[workspace.lints]` in the root
  **never reached it** — the project's "no `unwrap`/`expect` in non-test code" rule was
  unenforced in the one crate that holds a keychain and a SQLite cache. Fixed: the shell declares
  the lints, and the one real violation (an `expect` on the event loop's result) now logs and
  exits non-zero, so a supervisor sees a failure rather than a process that vanished.
- **A defect in the roadmap itself, found by Phase 13's plan:** the margin report's attribution
  was specified as a join on `users.plan_id`, which is wrong in the one month the report exists
  for (it holds only the *current* plan, so a converting user re-attributes their own earlier
  free-tier cost). Corrected in the roadmap, with a test that seeds a June-free/July-paid user.
- **A defect in Phase 10's plan, found by its own implementation:** `rrule` 0.14 reports
  `limited == true` when the collection merely *reached* the limit, so the planned bound would
  have marked a rule with exactly 500 occurrences as truncated — a connection that looks broken
  while it is fine. Corrected, with two cases that have the same occurrence count and opposite
  answers.
- **Three defects in the app's `QuickCapture`**, all found by measuring rather than looking:
  Escape does not close the draft or return focus, the Due line renders a raw UTC instant (so a
  task due 06:00 in Jakarta names the wrong day), and the ISO token pushes the page 34px wide at
  200% text on a phone.
- **A WCAG Level A failure in the SPA:** `index.html` is a static `lang="en"` and nothing updates
  it, so a screen reader pronounces the entire Indonesian UI with an English voice. Found by
  Phase 8's gate; being fixed.
- **Every piece of copy the server generates for a student is English-only, in an Indonesian product — and one of them is a notification the student gets at 09:00.** The spec's line 93 requires "English and Bahasa Indonesia from day one … no hard-coded strings" for the SPA, and the SPA meets it. The server does not, and nothing checks it:
  - `crates/server/src/auth/email_verify.rs:83` — subject `"Verify your Stvdy email"`, body `"Confirm this address to finish setting up your account."` This is the email that gates every AI action (A43), sent to a student who may have chosen Indonesian.
  - `crates/server/src/jobs/handlers/renewal.rs:338-367` — `copy()` builds `"Your {plan} plan {when}"` and a two-sentence body, and this is the text that arrives **as a push notification and as the in-app feed row** (the comment says so: "the text the student sees in the app and the text that arrives as a push are the same sentence from the same row").
  - `crates/server/src/billing/apply.rs:530-548` — the receipt's subject and body. **And this one has a second defect that is not a judgement call:** the activated/extended arm writes `period_end.to_rfc3339()` into the student's email, so what arrives is `Your pro plan is active until 2026-11-01T17:00:00+00:00` — a **raw RFC3339 instant in UTC**, for a product whose own Jakarta gate (`crates/server/tests/billing_jakarta.rs`) exists to assert that every student-visible date renders through `chrono_tz` in the reader's zone. For a Jakarta student that instant is **2026-11-02 00:00 WIB**, so the email names the wrong day by the same mechanism already found twice in this project (the `QuickCapture` Due line, and `plan.periodEnd`, which the Jakarta test asserts correctly). The plan name is `plan.as_str()`, so the email also says "pro" rather than "Pro".

  **The mechanism, and why the tree cannot express the fix as written.** The server does send a key — `renewal::PLAN_ENDING_KEY = "reminder.planEnding"` goes into the `send_push` payload beside the rendered sentence — and the intent is stated in the code: "so the SPA's feed renders the same reminder in either locale from one key rather than re-deriving the copy from the numbers". But `app/src/i18n/locales/en.json` has **no `reminder` block at all** (only `reminders.local.*`, which is the client-computed task/event copy), so that key resolves to itself, and the client's own guard is what hides it: `useLocalNotifications.ts:90`'s `feedKey` returns `null` when `subject_id` is `null`, and the renewal row's `subject_id` **is** `null` by construction (`renewal.rs:257`), so the row is skipped before any copy is looked up. **A renewal reminder therefore renders nowhere on the client, and the rendered English sentence the server built is what the student actually receives.**

  **Also worth knowing:** `user_settings.language` exists (`migrations/0028_tasks_events_settings.sql:30`, `check (language in ('en','id'))`) and is per-account, so the lookup the fix needs is already modelled. What is missing is a copy table in two languages and the read. **I have not implemented it**: it is a product decision as much as a task (is an English receipt acceptable? a receipt is a financial document, and the answer may differ from the answer for a verification email), and it is new scope rather than an unexecuted phase. Recorded here with the evidence so the decision is yours.
- **Gate 5's desktop leg had never run, for any phase, and nobody could tell.** The shared walk's
  steps name the phone tab bar's labels (`nav.work` = "Work") while the desktop sidebar reads
  `nav.workspaces` = "Workspaces", so `VIEWPORT_WIDTH=1365` died on `FAIL: no nav a labelled
  "Work"` after the first surface. Every phase's desktop-width accessibility evidence was either
  taken by a private probe or absent. Fixed in `bc7eed8`; the first real run found the sidebar's
  nav items at **215×40** (under 44px in height on every surface, both themes) and a `minmax(200px,
  1fr)` grid floor that does not grow with the text.
- **A class of layout defect that only one measurement caught, and it is the one that matters
  most.** The dashboard scrolled sideways at 390px **at 100% text** (423 > 390, 15 elements past
  the edge) — the first defect in the whole series visible at *normal* size, and a different
  mechanism from the rem-doubling one that every other finding belonged to: a `truncate`'s
  `white-space: nowrap` min-content propagating up through a flex column until an *ancestor*
  reports the overflow, where the element that is too wide reports none of its own. `min-width: 0`
  on the root is **inert in the cross axis** (measured: still 423/390); `w-full` is what makes the
  width definite. Both scales are now part of the pass condition.
- **The absence gate was asserting the wrong thing about the wrong artifact, and the fix changed
  the remedy.** Phase 13's slice first reported that `scripts/check-no-purchase-ui.mjs` "inspects
  the web build while calling it the mobile build". That was wrong: `frontendDist` is
  `../app/dist` with no per-platform override, so those bytes **are** what a phone loads and the
  check was reading the right artifact and reporting a real violation. What was missing was a
  **build-time exclusion** — the route was chosen at run time, which does not remove bytes. Now
  `TAURI_ENV_PLATFORM` keys the store build so the phone bundle lands where `frontendDist` points,
  every build stamps its own `stvdy-build-target`, and the check refuses anything but `store`:
  **0** occurrences of `billing/checkout` in the 764,625-byte store chunk against **5** in the
  778,765-byte web one, and it fails closed on an empty directory, an absent stamp and a `full`
  stamp as well.
- **A screen that rendered five of its twelve field labels as raw locale keys**, caught by the
  Indonesian browser render and by nothing else: the card resolved labels from the tool schema's
  argument names, and neither the i18n contract test (which marks every key under a prefix as
  used) nor the card's own test (which asserts the *value*) could see a missing label.
- **A locale key that could not be rendered at all:** `reminder.planEnding` has no entry in either
  locale file and no renderer in the SPA — the renewal writes `subject_id = null` and
  `feedKey` returns `null` for such a row. Found by trying to assert the Indonesian renewal
  reminder, and recorded as a contract the tree does not implement rather than papered over.

## Things worth knowing about the plan set

All sixteen phase plans now exist and are committed. Two of them were corrected against the
tree before execution rather than during it (Phase 6 needed five corrections, Phase 7 five
more), and two rounds of assumption-number collisions were resolved by renumbering before any
row was written. The reserved blocks are now: 6 → A119-A139, 7 → A140-A163, 8 → A164-A189,
9 → A190-A229, 10 → A230-A249, 11 → A250-A269, 12 → A270-A289, 13 → A290-A309, 14 → A310-A329,
15 → A330-A354.
