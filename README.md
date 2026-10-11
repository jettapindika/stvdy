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
- Node 24 (≥22.22 required by React Router 8)
- Docker (Postgres + pgvector, MinIO)
- For mobile later: Xcode (iOS), Android Studio + NDK (Android)

## Quick start

```bash
docker compose up -d                 # Postgres 16 + pgvector, MinIO
cp .env.example .env                 # dev defaults are usable as-is
cargo install sqlx-cli --no-default-features --features postgres,rustls
sqlx migrate run                     # 17 migrations

cargo run -p stvdy-server            # API on 127.0.0.1:8080
curl localhost:8080/health           # {"status":"ok","plans":3,...}

npm --prefix app install
npm --prefix app run dev             # web on localhost:5173
./app/node_modules/.bin/tauri dev    # desktop shell (from the repo root)
```

If you already run Postgres locally on 5432, point `DATABASE_URL` at it (it must
have pgvector available) or stop it before `docker compose up`.

## Quality gates

```bash
just check          # fmt --check, clippy -D warnings, cargo test
npm --prefix app test
npm --prefix app run build
```

Every phase must leave all of these green. `cargo clippy --all-targets` compiles
test code too, so `clippy.toml` relaxes `unwrap_used`/`expect_used`/`panic`
inside tests while non-test code is denied them.

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
