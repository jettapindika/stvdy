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

tauri:
    cargo tauri dev

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
