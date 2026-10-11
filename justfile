set dotenv-load := true

# NOTE: `just check` needs a reachable Postgres, because the `#[sqlx::test]`
# suites create a scratch database per test. Run `just db-up` and ensure
# DATABASE_URL is set (see .env.example). A skipped test would look green, so
# these fail loudly instead.

# --- dev infrastructure ---
db-up:
    docker compose up -d postgres minio minio-init

db-down:
    docker compose down

migrate:
    sqlx migrate run

# --- run ---
server:
    cargo run -p stvdy-server

web:
    npm --prefix app run dev

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
check-shell:
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
