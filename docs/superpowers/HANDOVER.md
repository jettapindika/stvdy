**Nothing yet.** All nine upstream slices (0-5) were verified green before this session
started: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (410 passed), `npm --prefix app test` (240 passed),
`npm --prefix app run build`, `node scripts/check-no-cdn.mjs`, the `src-tauri` trio
(17 passed), and `bash scripts/smoke-phase5.sh`.

Sessions 6-15 are being executed now. This file is appended to as each phase lands, so
whoever wakes up can see exactly what needs a human.

## Standing blockers (from `docs/ASSUMPTIONS.md` § Blockers)

These need **you**, not code, and they block a public launch rather than a phase:

| # | What is needed | Why it cannot be automated |
|---|---|---|
| B1 | QRIS gateway identity + API docs, and a PJP to settle through | Phase 13 can implement against an env-driven internal contract, but no revenue is possible without the real gateway, and perorangan onboarding (KTP + NPWP + a bank account matching the business name) is lead-time work |
| B2 | Tax/PPN treatment and store privacy-label text | Legal, not technical; invoices and a UU PDP privacy notice must be true |
| B3 | SMTP host/user/password and a From address | Every AI action is gated on a verified email, so with the dev file sink no real account can be onboarded. Provider swap is config-only once the credentials exist |

## Phase-specific credentials that will be needed as the phases land

| Phase | What |
|---|---|
| 10 (calendar) | A Google Cloud OAuth client (id, secret, registered redirect URI). `GOOGLE_LOGIN_CLIENT_ID`/`SECRET`/`REDIRECT_URI` are empty in `.env`, and the same client serves sign-in-with-Google from Phase 1 |
| 13 (payments) | The B1 values, plus `QRIS_BASE_URL`/`QRIS_API_KEY`/`QRIS_CALLBACK_TOKEN` |
| 14 (mobile) | An Apple Developer account (APNs `.p8` key + key id + team id) and a Firebase project (`google-services.json`, FCM v1 service account) |
| 15 (release) | Apple Developer ID certificate for notarization, a Windows code-signing certificate, and the production host (A45 says the current VPS is a 1-month trial and must never be production) |
| 15 (release) | A Cloudflare R2 bucket + credentials for production object storage (A15) |
