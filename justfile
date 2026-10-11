set dotenv-load := true

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

# --- quality gates (must all pass before a phase is called done) ---
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

fmt:
    cargo fmt --all

test:
    cargo test --workspace

# Regenerate the committed offline query cache after changing any query.
prepare:
    cargo sqlx prepare --workspace
