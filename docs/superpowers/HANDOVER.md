# Handover — what the user needs to do

Written while the remaining phases execute. Everything that could be done without you has
been; this is the list of things that cannot be.

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
| 7 (entitlements, credits, limits) | **Done.** The engine, the sweep, the ledger with a mutation-proved concurrency test, the period, the abuse controls, the ingestion formats, the tamper suite (doc-driven, 14 cases), admin, the SPA, the gates (one defect found: a 70×19 upgrade link, fixed and re-measured), the smoke run |
| 8 (sync engine) | Implementation complete and the convergence gate passing. A consumer-visible defect was found and fixed: a pull shipped only the *first* change of a row, so every later change arrived as `data: null` — which the client applies as a **delete**. Docs, gates and the smoke run are landing |
| 9 (tasks, events, Today, timer) | Implementation complete, including the offline exit gate. Four defects found and fixed, all of them "two write paths disagreeing" — a push delete that did not cascade, a restore that did not restore the cascade, a calendar-owned event writable through push, and a local apply that did not cascade. Gates measured; three `QuickCapture` defects found (Escape not closing the draft, a raw ISO date naming the wrong day in Jakarta, and 34px of overflow at 200% text) and being fixed |
| 10 (calendar) | Pure core done (the ICS feed, bounded recurrence expansion — the plan's bound was wrong and the crate's own behaviour proved it). Server landing |
| 11 (syllabus, planner) | Planner done (the purity sweep, the DST cases, and a determinism fixture that had to be strengthened before it could fail). Syllabus and the planner server landing |
| 12 (agent, reminders, dashboard) | Agent core done: the ten tools, the gate that cannot execute a write, and a tool-calling double so the injection test is not vacuous. Actions, reminders and the dashboard landing |
| 13 (payments) | Trait and pure math done (a real-HMAC mock, and the pricing rule in one function). Checkout, webhook, polling, periods and the margin report landing |
| 14 (mobile) | The bundle-absence proof, the read-only plan screen and the phone measurements landing. The native work (push receive, share extension, background scheduler) needs **your** accounts and a device |
| 15 (release) | The server image and the print assets landing. Signing, notarization, the updater and the store checklists need **your** certificates and a production host |

## Things found that you should know about

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
