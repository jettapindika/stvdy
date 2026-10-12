# Handover — what the user needs to do

Written while the remaining phases execute. Everything that could be done without you has
been; this is the list of things that cannot be.

## Launch blockers found in the code, not in `ASSUMPTIONS.md`

These are **code gaps**, not credential gaps, and each one stops a store submission on its own.
They were found while writing the store checklists (Phase 15), and they are recorded here because
nothing in a phase's own file list owned them:| # | What is missing | Why it blocks |
|---|---|---|
| **In-app account deletion** | No route exists: no `/account` handler in `crates/server/src/routes/mod.rs`, and `purge_deleted_accounts` appears only as a **comment** in `migrations/0015_jobs.sql:11` with no dispatching handler. `SettingsScreen.tsx:37` lists it under "Not built yet" | Apple requires in-app deletion of an account an app can create. Phase 7's plan re-tagged `## Account` away from itself (A160) and deferred it to "the release prerequisites", where nothing picked it up |
| **The Play account-deletion URL** | The public URL and the server route behind it both do not exist | Google Play requires a public deletion URL for any app with account creation |
| **The iOS build number** | `src-tauri/tauri.conf.json` sets no `bundle.bundleVersion`, so `CFBundleVersion` is derived from the shell version (`0.1.0`). Bumping the shell version is currently the only way to bump the build number | A second submission without a higher build number is rejected as a duplicate |

The first two are the same feature and it is a product-scope decision rather than a task: they need
the retention/purge machinery, a confirm step, and copy that says what is deleted. Say the word and
I will implement it — it is not large, but it is not mine to invent either.

## One environment note worth acting on

**The machine is out of disk, and it is now blocking verification rather than merely threatening it.** `/System/Volumes/Data` has **13 GB free of 460 GB (97% full)** and the repository's `target/` is **89 GB** (87 GB of it `target/debug`, 2.7 GB `target/release`). Three things have already died on it: Phase 13's test run with `No space left on device`, Phase 15's one local `docker build`, and the Docker daemon itself, which no longer starts (`docker info` fails; `~/Library/Containers/com.docker.docker` is 6.2 GB).

Nothing on the critical path needs Docker except the server-image build, which CI is meant to be the evidence for anyway. What it does need is enough room for `cargo test --workspace` and `npm --prefix app test` to finish, and at 13 GB that is no longer comfortable.

**The cheapest 87 GB on this machine:**

```bash
cd /Users/mac/Documents/Coding/project/Stvdy
cargo clean          # removes all 89 GB under target/; the next build rebuilds in a few minutes
```

I have not run it, and the reason is a real tradeoff rather than caution: every `cargo` invocation in flight holds the `target/` lock, so a clean now would make several live slices fail with a lock error, and a full rebuild then competes with them for the same lock. **If you are reading this while I am still running, the right order is: let the remaining slices finish, then `cargo clean`, then one `cargo test --workspace`.** If you would rather have the space immediately, stop me first (`cargo clean` while I am mid-run will look like a spurious test failure, not like a disk problem).

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

## Verified green so far

Every phase's exit gate is recorded in its own plan file under "Execution notes", and each
phase's working notes are in `.superpowers/sdd/<plan>/progress.md` (git-ignored scratch, on
disk only). The short version:

| Phase | Status |
|---|---|
| 0–5 | Done before this session; verified green again here |
| 6 (reports, cheatsheet, PDF) | **Done.** Implementation, a review round with one Critical fix (an SVG attribute-value injection that reached a printed PDF), the three roadmap gates measured in a real browser (four defects found and fixed), the smoke run passing |
| 7 (entitlements, credits, limits) | **Done.** The engine, the sweep, the ledger with a mutation-proved concurrency test, the period, the abuse controls, the ingestion formats, the tamper suite (doc-driven), admin, the SPA, the gates (one defect found: a 70×19 upgrade link), the smoke run |
| 8 (sync engine) | **Done.** A consumer-visible defect was found and fixed: a pull shipped only the *first* change of a row, so every later change arrived as `data: null` — which the client applies as a **delete**. Gates measured; one WCAG Level A failure found (the document's language attribute never followed the UI language) and fixed |
| 9 (tasks, events, Today, timer) | **Done.** Four defects found and fixed, all "two write paths disagreeing" — a push delete that did not cascade, a restore that did not restore the cascade, a calendar-owned event writable through push, and a local apply that did not cascade. Three `QuickCapture` defects found by measurement and fixed |
| 10 (calendar) | **Done.** Two plan-level errors corrected (the `rrule` bound, and the migration numbering); two more 200%-text layout defects found by the gate and fixed; a two-clock defect in the poll diagnosed from the rows and fixed |
| 11 (syllabus, planner) | **Done.** Two real client defects found by driving the wired process rather than by a unit test, and **four of the slice's own assertions were wrong about the product** — the strongest argument in the project for exercising the running app. **And its exit-gate suite had never been run**: `cargo test -p stvdy-server --test syllabus` was written, committed, left out of the exit checklist's result column, and found red on committed code (14 passed / 5 failed) when the workspace gate was finally run. Four of the five were the same mistaken premise — the test author read `item.index` as a position in the *sorted* review list, while the contract (`crates/core/src/generation/syllabus.rs:109-113`) defines it as the position in the **reply**, across the module's field order; the product was right in every case and the assertions were corrected with the assumption named. The fifth was the test harness's `drain_jobs`, which cannot assert an empty queue for a deliberately failing retryable job. Now **19 passed / 0 failed** (`9f1d6ae`) |
| 12 (agent, reminders, dashboard) | **Done.** One real **cross-tenant write** found by the slice's own injection test and fixed: a task or event could be filed into another account's workspace. The injection test's three-part anti-vacuity proof is the phase's best evidence |
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
