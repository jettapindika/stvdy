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

**The machine is nearly out of disk, and it has already cost work.** `/System/Volumes/Data` is 95%
full with ~22 GB free, and the repository's `target/debug` alone is **74 GB** (plus 2.7 GB of
`target/release`). Two builds have failed on it: Phase 13's test run died mid-run with
`No space left on device`, and Phase 15's one local `docker build` attempt died with the Docker
daemon, which no longer starts (`docker info` fails; `~/Library/Containers/com.docker.docker` is
6.2 GB).

Nothing on the critical path needs Docker except the server-image build, which CI is meant to be the
evidence for anyway. What it does need is enough space for `cargo test --workspace` and
`npm --prefix app test` to finish, so the remaining slices can verify.

If you want 40 GB back cheaply:

```bash
cd /Users/mac/Documents/Coding/project/Stvdy
cargo clean -p stvdy-server -p stvdy-core -p stvdy-sync -p stvdy-shell   # or just: cargo clean
```

`cargo clean` deletes all 76 GB under `target/` and the next build rebuilds it from scratch (a few
minutes for this workspace). That is the single largest reclaimable thing on the disk, and it costs
only time. **I have not run it**: a full rebuild competes with several live `cargo` invocations for
the same `target/` lock, and the machine's 128 GB `~/Documents` is somewhere I will not touch. Your
call — and if you would rather not, the failures above will show up again as flakes rather than as
errors.

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
| 11 (syllabus, planner) | **Done.** Two real client defects found by driving the wired process rather than by a unit test, and **four of the slice's own assertions were wrong about the product** — the strongest argument in the project for exercising the running app |
| 12 (agent, reminders, dashboard) | **Done.** One real **cross-tenant write** found by the slice's own injection test and fixed: a task or event could be filed into another account's workspace. The injection test's three-part anti-vacuity proof is the phase's best evidence |
| 13 (payments) | Implementation complete: the trait with a real-HMAC mock, the pure billing math, the gateway, the checkout, the webhook with two separately-tested idempotency defences, polling that shares one `apply_paid`, the periods, BYOK, and the margin report with its tripwires. **One real hole found**: the checkout screen never consulted `billing_policy` and the route was registered unconditionally, so a phone build had a purchase screen reachable by a typed URL — fixed |
| 14 (mobile) | The bundle-absence proof, the read-only plan screen and the phone measurements are done; **four** phone-layout defects found by measurement and fixed. A platform-keyed build with no purchase path is landing. The native work (push receive, share extension, background scheduler) needs **your** accounts and a device |
| 15 (release) | The release gate, the documents, the licensing artifacts and the store checklists are done; the verification script was exercised against a real signed fixture and found two bugs in itself. The server image is built in CI. Signing, notarization and the store submissions need **your** certificates and a production host |

## Things found that you should know about

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


## One thing worth knowing about the plan set

All sixteen phase plans now exist and are committed. Two of them were corrected against the
tree before execution rather than during it (Phase 6 needed five corrections, Phase 7 five
more), and two rounds of assumption-number collisions were resolved by renumbering before any
row was written. The reserved blocks are now: 6 → A119-A139, 7 → A140-A163, 8 → A164-A189,
9 → A190-A229, 10 → A230-A249, 11 → A250-A269, 12 → A270-A289, 13 → A290-A309, 14 → A310-A329,
15 → A330-A354.
