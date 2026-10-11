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

## Prerequisites

- Rust 1.94.1 (`rust-toolchain.toml` pins it; sqlx 0.9 needs ≥1.94.0)
- Node 24 (`.nvmrc`; ≥22.22 required by React Router 8)
- **Postgres with pgvector** — Docker, a local install, or hosted Supabase
- `just` (`brew install just`) for the dev recipes
- For mobile later: Xcode (iOS), Android Studio + NDK (Android)

## Quick start

```bash
cp .env.example .env          # the server loads this automatically (dotenvy)

# Database: Docker, an existing local Postgres with pgvector, or Supabase.
docker compose up -d postgres

cargo install sqlx-cli --no-default-features --features postgres,rustls
sqlx migrate run              # 17 migrations

cargo run -p stvdy-server
curl localhost:8080/health    # {"env":"dev","plans":3,"status":"ok","version":"0.1.0"}

# Web app (separate terminal)
npm --prefix app install
npm --prefix app run dev      # http://localhost:5173

# Desktop app (separate terminal, from the repo root)
just tauri                    # or: ./app/node_modules/.bin/tauri dev
```

Recipes: `just db-up`, `just migrate`, `just server`, `just web`, `just tauri`,
`just search-up`, `just check`.

### If port 5432 is already taken

A local Postgres (Homebrew `postgresql@17`, say) holds 5432, so
`docker compose up postgres` cannot bind it. Pick one:

```bash
brew services stop postgresql@17     # free the port for the container, or
createdb stvdy                       # keep the local server and point .env at it:
#   DATABASE_URL=postgres://<your-user>@localhost:5432/stvdy
```

Either works: the schema needs only the `vector` and `pgcrypto` extensions, which
`sqlx migrate run` creates. CI runs the pinned Postgres 16
(`pgvector/pgvector:pg16`), so if your local version differs, treat CI's result as
authoritative — see `docs/ASSUMPTIONS.md` § Local development environment.

### What needs which service

| Service | Needed for |
|---|---|
| Postgres + pgvector | everything (migrations, tests, the API) |
| MinIO (`docker compose up -d minio minio-init`) | source uploads, Phase 1+ |
| SearXNG (`just search-up`) | web search only, Phase 3+ |

The `#[sqlx::test]` suites create a scratch database per test, so **Postgres must
be reachable before `just check`** — otherwise those tests fail loudly. That is
deliberate: a skipped test would look green.

## Supabase dev database

Dev and early staging can run against a hosted Supabase Postgres instead of the
compose one — project `stvdy` (`kugefzknygvjmgwanjkk`, `ap-southeast-1`, org
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

Migrations are sqlx's, and `supabase db push` cannot see them — it only reads
`supabase/migrations/`, which does not exist here, so it reports "Remote database
is up to date" while pushing nothing. Use `just migrate-supabase` (or start the
server, which migrates on boot). The CLI's `db query`, `db dump`, `db lint` and
`db advisors` all work against the linked project and are useful as-is.

`crates/server` connects as `postgres` and does its own auth, so nothing here
uses the PostgREST Data API — but Supabase grants `anon` and `authenticated` full
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
runs during workspace clippy — which means the workspace commands skip it
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
prompts/         versioned prompt modules
templates/       print templates (cheatsheet PDF)
migrations/      sqlx migrations; 0017 attaches sync triggers and must stay last
docs/            SPEC.md, ASSUMPTIONS.md, ARCHITECTURE.md, API.md, SYNC.md, RELEASE.md
```

## Documentation

- `docs/SPEC.md` — the requirement document of record.
- `docs/ASSUMPTIONS.md` — every decision the spec left open, plus blockers.
- `docs/ARCHITECTURE.md` — layers, provider traits, native capability matrix,
  prototype results.
- `docs/API.md` — routes, conventions, SSE event shapes.
- `docs/SYNC.md` — cursor model, push allowlist, conflict rules, offline scope.
- `docs/superpowers/plans/` — the master roadmap and per-phase plans.
