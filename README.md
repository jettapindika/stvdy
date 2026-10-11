# Stvdy

An AI study OS: source-grounded research workspaces (upload your own material,
chat with cited answers, generate flashcards/quizzes/reports/print-ready
cheatsheets) plus a study planner (tasks, calendar, exam-aware scheduling, and an
assistant that proposes actions you confirm). Sold as three prepaid QRIS plans.
Ships for macOS, Windows, iOS, Android and the web, with one account syncing
every device.

## Architecture in five points

- `crates/server` (Rust/Axum + Postgres) is the single source of truth and does
  all heavy work: ingestion, embeddings, retrieval, LLM calls, planning, calendar
  sync, billing. Clients never call LLM, search or Google APIs directly.
- `crates/core` is pure logic with no web, database or IO: chunking, retrieval,
  citations, SM-2 replay, the planner, entitlements, the prompt composer and the
  document renderer. Deterministic and unit-tested against a fixed clock.
- `crates/sync` holds the wire DTOs shared by the server and the native shell.
- `app/` is one React + Vite + TypeScript SPA, used both as the web build and as
  the Tauri frontend for all four native platforms.
- `src-tauri/` is the native shell: OS keychain, local SQLite cache and outbox,
  notifications, deep links.

External capabilities (LLM, embeddings, search, payments, push, storage) sit
behind traits, so a provider is configuration rather than code.

**Trust boundary:** the client is untrusted and the server is the only
enforcement point. A client contributes authenticated intent; whether it is
permitted, affordable, or well-formed is decided server-side, per handler and per
sync mutation. See `docs/THREAT_MODEL.md` for the normative statement.

## Prerequisites

- Rust 1.94.1 (`rust-toolchain.toml` pins it; sqlx 0.9 needs ≥1.94.0)
- Node 24 (`.nvmrc`; ≥22.22 required by React Router 8)
- **Postgres with pgvector**. One of: Docker, a local install, or hosted Supabase
- `just` (`brew install just`) for the dev recipes
- For mobile later: Xcode (iOS), Android Studio + NDK (Android)

You do **not** need `sqlx-cli` just to run the server: `crates/server` applies
migrations on boot from the compile-time `sqlx::migrate!` macro. It is only
needed for `just migrate` and `just prepare`.

## Quick start

**1. Pick a database.** Both paths are supported; pick the one that matches your
machine.

```bash
# (a) Docker. Needs port 5432 free
brew services stop postgresql@17   # if a local Postgres holds 5432
just db-up                         # postgres + pgvector, MinIO, bucket

# (b) An existing local Postgres that already has pgvector
createdb stvdy                     # once
# then set DATABASE_URL in .env to your own role, e.g.
#   DATABASE_URL=postgres://$USER@localhost:5432/stvdy
```

**2. Configuration.**

```bash
# .env already exists on this machine and is configured. Do NOT overwrite it:
# .env.example holds the Docker DATABASE_URL, so copying over a working file
# reintroduces `role "postgres" does not exist`.
[ -f .env ] || cp .env.example .env
```

Then **check `DATABASE_URL` in `.env` matches the path you chose above.** The
template ships `postgres://postgres:postgres@localhost:5432/stvdy`, which is
correct for Docker and wrong for a local Homebrew Postgres (its role is your
username, and there is no `postgres` role). The symptom is
`role "postgres" does not exist`.

**3. Server** → http://localhost:8080

```bash
just server                        # or: cargo run -p stvdy-server
curl localhost:8080/health
# {"env":"dev","plans":3,"status":"ok","version":"0.1.0"}
```

Migrations and plan seeding happen at boot, so a fresh database needs no extra
step. To run them without starting the server: `just migrate`.

**4. Web app** → http://localhost:5173

```bash
npm --prefix app install
npm --prefix app run dev
```

**5. Desktop app**

```bash
just tauri
```

`just tauri` starts its own frontend via `beforeDevCommand`, so **do not also run
step 4**. The second Vite cannot bind 5173 (`strictPort: true`) and prints
`Port 5173 is already in use`. That message is harmless here (Tauri just polls
`devUrl`, which the first Vite is serving), but it looks like a failure. Run step
4 for the browser build, or step 5 for the desktop app, not both.

Recipes: `just db-up`, `just migrate`, `just server`, `just web`, `just tauri`,
`just search-up`, `just mail`, `just check`, `just prepare`. Run them from the
repo root. The Tauri CLI resolves `src-tauri` as a subdirectory of the working
directory and will not find it from `app/`.

### Email in dev

`SMTP_URL` is unset, so the server **writes** each message to `var/mail/*.eml`
instead of sending it (B3) and logs the path. A verification link is therefore a
file, not an inbox — the signup screen's "check your inbox" has nowhere to
arrive, and nothing is delivered to the address you typed:

```bash
just mail                      # print the newest message
just mail jettabackupacc1      # the newest one addressed to a matching address
```

Open the printed link while `just web` (or `just tauri`) is serving
`http://localhost:5173`: the link is built from `WEB_ORIGIN`, because that is
where the `/verify-email` route exists.

This is not optional in a fresh checkout. A43 makes a verified address a
precondition for anything that spends — chat, generation, web search and
ingestion all answer `403 email_not_verified` without it.

To send real mail, set `SMTP_URL` in `.env` and restart the server; the provider
swap is config-only (`crates/server/src/integrations/mailer.rs`). The scheme
picks the transport, and credentials come from the URL:

```bash
# implicit TLS — the usual choice
SMTP_URL=smtps://resend:re_xxxxxxxx@smtp.resend.com:465
SMTP_URL=smtps://you@gmail.com:APP_PASSWORD@smtp.gmail.com:465
# STARTTLS (mandatory whenever credentials are present)
SMTP_URL=smtp://user:pass@smtp.example.com:587
MAIL_FROM='Stvdy <no-reply@stvdy.app>'    # a display name is allowed
```

A wrong `SMTP_URL` scheme or an unloadable `MAIL_FROM` is not silent: the send is
logged as `sending the verification mail failed` with the reason, and signup still
succeeds, so the account can be recovered with `Send a new link`. The URL is never
logged.

### What needs which service

| Service | Needed for |
|---|---|
| Postgres + pgvector | everything (the API, migrations, tests) |
| Object storage (`docker compose up -d minio`) | source uploads, Phase 1+. The server creates the bucket at boot, so there is no init sidecar. |
| SearXNG (`just search-up`) | web search only, Phase 3+; needs `SEARXNG_SECRET` in `.env` |

The `#[sqlx::test]` suites create a scratch database per test, so **Postgres must
be reachable before `just check`**. Otherwise those tests fail loudly. That is
deliberate: a skipped test would look green.

## Supabase dev database

Dev and early staging can run against a hosted Supabase Postgres instead of the
compose one. Project `stvdy` (`kugefzknygvjmgwanjkk`, `ap-southeast-1`, org
Jetta). Per A47 this is **never** the production database: the free tier's 500 MB
cannot hold this pgvector workload.

```bash
supabase login
supabase link --project-ref kugefzknygvjmgwanjkk   # prompts for the DB password
cp .env.supabase.example .env.supabase             # paste the same password
just migrate-supabase                              # 17 migrations
just server-supabase                               # API against Supabase
```

`.env.supabase` is gitignored and `just check` never reads it, so the local
docker Postgres keeps owning the `#[sqlx::test]` suites. The connection string
uses the **session pooler** on 5432, because sqlx prepares statements (the
transaction pooler on 6543 does not carry them across statements) and the direct
host `db.<ref>.supabase.co` is IPv6-only.

Migrations are sqlx's, and `supabase db push` cannot see them. It only reads
`supabase/migrations/`, which does not exist here, so it reports "Remote database
is up to date" while pushing nothing. Use `just migrate-supabase` (or start the
server, which migrates on boot). The CLI's `db query`, `db dump`, `db lint` and
`db advisors` all work against the linked project and are useful as-is.

`crates/server` connects as `postgres` and does its own auth, so nothing here
uses the PostgREST Data API. But Supabase grants `anon` and `authenticated` full
access to every table in `public` and enables RLS on none of them, which would
expose all 36 tables over HTTPS to anyone holding the project's anon key. A
recreated or restored project re-applies those grants, so after any such change
run this once, which also covers tables added later:

```sql
revoke all on all tables in schema public from anon, authenticated;
revoke all on all sequences in schema public from anon, authenticated;
revoke all on all functions in schema public from anon, authenticated;
alter default privileges in schema public revoke all on tables from anon, authenticated;
alter default privileges in schema public revoke all on sequences from anon, authenticated;
alter default privileges in schema public revoke all on functions from anon, authenticated;
```

## Quality gates

```bash
just check          # all three areas: rust, web AND shell
```

| Target | Covers |
|---|---|
| `check-rust` | `fmt --check`, `clippy -D warnings`, `cargo test` (workspace) |
| `check-web` | `npm test`, `npm run build` |
| `check-shell` | `fmt`, `clippy`, `cargo test` for `src-tauri` |

`src-tauri` is excluded from the Cargo workspace so `tauri_build::build()` never
runs during workspace clippy. Which means the workspace commands skip it
entirely, hence the separate target. `check-shell` depends on `check-web` because
`generate_context!()` panics at compile time when `app/dist` is missing, and
`dist/` is gitignored.

`cargo clippy --all-targets` compiles test code too, so `clippy.toml` relaxes
`unwrap_used`/`expect_used`/`panic` inside tests while non-test code is denied
them.

## Layout

```
crates/core      pure domain logic
crates/sync      sync wire DTOs
crates/server    Axum API + job worker
app/             the shared SPA (web + Tauri frontend)
src-tauri/       native shell
config/          plans.toml, billing_policy.toml
docs/DESIGN.md   the design direction: tokens, type, shape, motion, a11y
prompts/         versioned prompt modules
templates/       print templates (cheatsheet PDF)
migrations/      sqlx migrations; 0017 attaches sync triggers and must stay last
docs/            SPEC.md, ASSUMPTIONS.md, ARCHITECTURE.md, API.md, SYNC.md,
                 RELEASE.md, THREAT_MODEL.md
```

## Documentation

- `docs/SPEC.md`. The requirement document of record.
- `docs/ASSUMPTIONS.md`. Every decision the spec left open, plus blockers.
- `docs/ARCHITECTURE.md`. Layers, provider traits, trust boundary, native
  capability matrix, prototype results.
- `docs/API.md`. Routes, conventions, SSE event shapes.
- `docs/SYNC.md`. Cursor model, push allowlist, conflict rules, offline scope.
- `docs/THREAT_MODEL.md`. The normative trust boundary: assets, attacker
  classes, what a modified client can and cannot do, residual risks, and the
  per-phase verification obligations that follow.
- `docs/DESIGN.md`. The design direction: the token palette and what each role
  means, type, shape, motion, the accessibility checks, and which earlier
  decisions the current spec supersedes.
- `docs/superpowers/plans/`. The master roadmap and per-phase plans.
