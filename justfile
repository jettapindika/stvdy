set dotenv-load := true

# NOTE: `just check` needs a reachable Postgres, because the `#[sqlx::test]`
# suites create a scratch database per test. Run `just db-up` and ensure
# DATABASE_URL is set (see .env.example). A skipped test would look green, so
# these fail loudly instead.

# --- dev infrastructure ---
# No `minio-init`: the server creates the bucket at boot (A15 removed the
# pullable `mc` image, so there is no sidecar to run).
db-up:
    docker compose up -d postgres minio

# SearXNG is only needed for Phase 3+ (web search). Kept separate so the common
# case does not pull an extra image.
search-up:
    docker compose up -d searxng

# Phase 0 exit check: the JSON must have a non-empty `results` and an empty
# `unresponsive_engines`. A non-JSON body (the "Verifying your browser…" HTML
# page) or a `results` of `[]` means the engines are not answering. `-T` because
# the output is piped, and the request runs inside the container so it does not
# depend on the host port mapping.
db-search:
    docker compose exec -T searxng wget -qO- "http://127.0.0.1:8080/search?q=spaced+repetition&format=json"

db-down:
    docker compose down

migrate:
    sqlx migrate run

# --- Supabase dev/staging database (see README § Supabase dev database) ---
#
# These source `.env.supabase` instead of `.env`, so `DATABASE_URL` points at the
# hosted project rather than the local docker Postgres. That separation is
# deliberate: `#[sqlx::test]` creates and drops a scratch database per test, so
# pointing `just check` at a hosted instance would churn databases on it and
# spend the 500 MB quota (A47).
#
# `set -a` exports what the file defines, and the sourcing has to happen in the
# same shell as the command because each recipe line is its own shell.

migrate-supabase:
    @test -f .env.supabase || { echo "missing .env.supabase — cp .env.supabase.example .env.supabase and fill in the database password"; exit 1; }
    @set -a; . ./.env.supabase; set +a; sqlx migrate run

# --- run ---
server:
    cargo run -p stvdy-server

server-supabase:
    @test -f .env.supabase || { echo "missing .env.supabase — cp .env.supabase.example .env.supabase and fill in the database password"; exit 1; }
    @set -a; . ./.env.supabase; set +a; cargo run -p stvdy-server

web:
    npm --prefix app run dev

# --- Phase 2 smoke: the server, a stub model, and the end-to-end chat run ---
#
# The pairing that matters and is otherwise invisible: the SERVER must be started
# with LLM_BASE_URL pointing at the stub's port, because the server is what talks
# to the model. `scripts/smoke-phase2.sh` drives the server and starts its own
# stub on the same port, so this recipe exists to make the pairing explicit
# instead of leaving a re-runner to see an opaque 502 (or a burned credit) at the
# chat step.
#
# Two terminals: `just smoke-llm` then `just smoke-server`, then
# `just smoke-phase2`.

# The stub model server. Speaks real HTTP and SSE, and logs every request body so
# the smoke run can assert on the prompt the model actually received.
smoke-llm:
    python3 scripts/stub-llm.py --port 8100 --log var/stub-requests.jsonl

# The server, pointed at the stub. Started separately from `server` so the real
# provider configuration is never overwritten by a test one.
smoke-server:
    LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

# Drives the running server. Needs `just smoke-llm` and `just smoke-server` up.
smoke-phase2:
    BASE=http://localhost:8099 bash scripts/smoke-phase2.sh

# The Tauri CLI resolves `src-tauri` as a subfolder of the working directory, so
# these must run from the repo root. The CLI binary is the one npm installed.
tauri:
    ./app/node_modules/.bin/tauri dev

tauri-build:
    ./app/node_modules/.bin/tauri build

# --- quality gates (all must pass before a phase is called done) ---

check: check-rust check-web check-shell

check-rust:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

check-web:
    npm --prefix app test
    npm --prefix app run build

# src-tauri is excluded from the workspace (so `tauri_build::build()` never runs
# during workspace clippy), which means the workspace commands skip it entirely.
# Without this target the shell's keychain code, IPC commands, and Phase 8's
# sync engine would accumulate unlinted and untested.
#
# It depends on the web build on purpose: `generate_context!()` panics at compile
# time when `frontendDist` (../app/dist) is missing, and dist/ is gitignored. So a
# fresh checkout cannot lint or test the shell until the SPA has been built — the
# dependency makes that explicit rather than relying on check-web happening to run
# first.
check-shell: check-web
    cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
    cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
    cargo test --manifest-path src-tauri/Cargo.toml

fmt:
    cargo fmt --all
    cargo fmt --manifest-path src-tauri/Cargo.toml --all

test:
    cargo test --workspace

# Regenerate the committed offline query cache after changing any query.
prepare:
    cargo sqlx prepare --workspace
