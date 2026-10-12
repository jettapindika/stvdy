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
| 6 (reports, cheatsheet, PDF) | Done: implementation, review round with one Critical fix (an SVG attribute-value injection that reached a printed PDF), the three roadmap gates measured in a real browser, the smoke run passing |
| 7 (entitlements, credits, limits) | Done: the engine, the sweep, the ledger with a mutation-proved concurrency test, the period, the abuse controls, the ingestion formats, the tamper suite, admin, the SPA, the gates, the smoke run |
| 8 (sync engine) | In progress: the protocol, the three routes, the conflict table and the compaction job are done; the client engine and the convergence suite are landing |
| 9 (tasks, events, Today, timer) | In progress: the prompt modules and the pure rules are done; the server modules are landing |
| 10 (calendar) | In progress: the pure ICS feed and bounded recurrence expansion |
| 11–15 | Planned in full (see `docs/superpowers/plans/`), executing in order |

## One thing worth knowing about the plan set

All sixteen phase plans now exist and are committed. Two of them were corrected against the
tree before execution rather than during it (Phase 6 needed five corrections, Phase 7 five
more), and two rounds of assumption-number collisions were resolved by renumbering before any
row was written. The reserved blocks are now: 6 → A119-A139, 7 → A140-A163, 8 → A164-A189,
9 → A190-A229, 10 → A230-A249, 11 → A250-A269, 12 → A270-A289, 13 → A290-A309, 14 → A310-A329,
15 → A330-A354.
