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

# --- dev mail (B3) ---
#
# With SMTP_URL unset the server writes one .eml per message to MAIL_DIR instead
# of sending it, so a verification link is a file rather than an inbox and the
# signup screen's "check your inbox" has nowhere to arrive. This prints the
# newest message; give it text to narrow to one addressed to that address:
#
#   just mail                      # the newest message
#   just mail jettabackupacc1      # the newest one addressed to a matching address
#
# The filter matches the file's contents, not its name: the name is sanitized
# (every non-alphanumeric character becomes `_`), so an address does not survive
# in it intact.
mail match="":
    @files=$(ls -t var/mail/*.eml 2>/dev/null); \
      if [ -z "$files" ]; then \
        echo "var/mail is empty — sign up, or press 'Send a new link' in the app"; exit 1; \
      fi; \
      if [ -n "{{match}}" ]; then \
        file=$(printf '%s\n' "$files" | xargs grep -l -- "{{match}}" 2>/dev/null | head -1); \
      else \
        file=$(printf '%s\n' "$files" | head -1); \
      fi; \
      if [ -z "$file" ]; then echo "no message matching {{match}}"; exit 1; fi; \
      cat "$file"

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

# Phase 4's run. Same stub and server, and the stub must answer with a cards JSON:
# the generation job asks for a strict-JSON reply, so the phase 2 default (a prose
# answer) would fail the job for good. `smoke-llm` above serves prose; start the
# stub by hand for this one:
#   python3 scripts/stub-llm.py --port 8100 --log var/stub-requests.jsonl \
#     --answer '{"cards":[{"front":"Apa fungsi three-way handshake?","back":"Membuka koneksi TCP.","label":1}]}'
smoke-phase4:
    BASE=http://localhost:8099 bash scripts/smoke-phase4.sh

# Phase 5's run. Same stub and server, and the stub must answer with a questions JSON:
# the quiz job asks for a strict-JSON reply, so the phase 2 default (a prose answer)
# would fail the job for good. Start the stub by hand for this one:
#   python3 scripts/stub-llm.py --port 8100 --log var/stub-requests.jsonl \
#     --answer '{"questions":[{"kind":"mcq","prompt":"Apa fungsi three-way handshake?","options":["Membuka koneksi TCP.","Menutup koneksi TCP.","Mengenkripsi data.","Mengompres data."],"answer":"Membuka koneksi TCP.","explanation":"Handshake menegosiasikan parameter.","label":1,"key_points":null}]}'
smoke-phase5:
    BASE=http://localhost:8099 bash scripts/smoke-phase5.sh

# Phase 6's run. Same stub and server, but this one needs THREE answers in order — the
# report outline, a report section, and the cheatsheet's blocks — so it passes an
# --answers-file the script writes itself. The server also needs a browser for the PDF
# half: CHROME_PATH must be set on BOTH the server (which renders) and the script (which
# checks the print), and a run without one fails rather than skipping.
smoke-llm-phase6:
    python3 scripts/stub-llm.py --port 8100 --log var/stub-phase6-requests.jsonl \
      --answers-file scripts/fixtures/phase6-answers.json

smoke-server-phase6:
    CHROME_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
      LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase6:
    BASE=http://localhost:8099 bash scripts/smoke-phase6.sh

# Phase 7's run. Same stub and server as Phase 2: nothing in this phase needs a
# strict-JSON reply, so the stub's default prose answer is what the chat turns use. No
# browser is needed — this run prints no PDF — so unlike Phase 6, CHROME_PATH is not part
# of the pairing.
#
# It does read the database back (the concurrency proof, the downgrade's read-only flags,
# the ceiling's untouched ledger), so it needs `DATABASE_URL`, which it reads out of `.env`
# itself. It also prunes `signup_attempts` for 127.0.0.1 at the start, because A154's window
# is 24h and every run of this script adds to it.
smoke-llm-phase7:
    python3 scripts/stub-llm.py --port 8100 --log var/stub-phase7-requests.jsonl

smoke-server-phase7:
    LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase7:
    BASE=http://localhost:8099 bash scripts/smoke-phase7.sh

# Phase 9's run. The stub must answer with a capture JSON and a breakdown JSON: both
# generators ask for a strict-JSON reply, so the Phase 2 default (a prose answer) would fail
# them for good. That is what `--answers-file` is for, and it matches on the prompt rather
# than on the request's position so a re-run answers correctly.
#
# It reads the database back (the credit ledger, the task count, `deleted_with_parent`,
# `change_log`), so it needs `DATABASE_URL`, which it reads out of `.env` itself, and it
# prunes `signup_attempts` for 127.0.0.1 at the start because A154's window is 24h.
smoke-llm-phase9:
    python3 scripts/stub-llm.py --port 8100 --log var/stub-phase9-requests.jsonl \
      --answers-file scripts/fixtures/phase9-answers.json

smoke-server-phase9:
    LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase9:
    BASE=http://localhost:8099 bash scripts/smoke-phase9.sh

# Phase 11's run. The stub must answer with a syllabus extraction and an effort estimate:
# both are strict-JSON generators, so the Phase 2 default (a prose answer) would fail both
# for good. `--answers-file` matches on the prompt rather than on the request's position, so
# a re-run answers correctly.
#
# It reads the database back — the draft's status, the credit ledger, the `events`/`tasks`/
# `goals` counts before and after the confirm, and the accepted blocks' `goal_id` — so it
# needs `DATABASE_URL`, which it reads out of `.env` itself, and it prunes `signup_attempts`
# for 127.0.0.1 at the start because A154's window is 24h.
#
# It is a slow run on purpose: the extraction and the effort estimate are both jobs, so the
# script polls the draft and the proposal to `ready` rather than sleeping a fixed amount.
smoke-llm-phase11:
    python3 scripts/stub-llm.py --port 8100 --log var/stub-phase11-requests.jsonl \
      --answers-file scripts/fixtures/phase11-answers.json

smoke-server-phase11:
    LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase11:
    BASE=http://localhost:8099 bash scripts/smoke-phase11.sh

# Phase 8's run. **No model stub and no browser**: nothing on the sync path calls an LLM
# and nothing here renders a document, so `just smoke-server-phase7` is a fine server for
# it (the stub it points at is never reached) and CHROME_PATH is not part of the pairing.
#
# It does read the database back — the replayed mutation's unchanged `updated_at` and
# `change_log` count, the isolation check's seq sets, the compaction watermark — so it
# needs `DATABASE_URL`, which it reads out of `.env` itself. It prunes `signup_attempts`
# for 127.0.0.1 at the start, because A154's window is 24h and every run adds to it.
smoke-phase8:
    BASE=http://localhost:8099 bash scripts/smoke-phase8.sh

# Phase 10's run. **No model stub and no browser**: nothing on the calendar path calls an
# LLM and nothing here renders a document, so any of the phase 7-9 servers is a fine server
# for it — the stub it points at is never reached.
#
# The Google consent flow's refusals are asserted; a successful exchange is not, because the
# OAuth client does not exist in this repo (Task 6's live half).
#
# It reads the database back (the connection row, the stored feed token), so it needs
# `DATABASE_URL`, which it reads out of `.env` itself, and it prunes `signup_attempts` for
# 127.0.0.1 at the start because A154's window is 24h.
smoke-phase10:
    BASE=http://localhost:8099 bash scripts/smoke-phase10.sh

# Phase 10's server. The only reason this exists rather than reusing `smoke-server-phase7` is
# scenario 6's connect assertion: `GET /calendar/google/connect` answers **404** when the
# calendar OAuth client is unset, and 404 is also what an unregistered route answers, so the
# run cannot tell "not configured" from "not landed". These three values are dummy strings —
# nothing in the run reaches Google, and the refusal it asserts is the entitlement engine's,
# which fires before any request would be made.
smoke-server-phase10:
    GOOGLE_CALENDAR_CLIENT_ID=smoke-calendar-client.apps.googleusercontent.com \
      GOOGLE_CALENDAR_CLIENT_SECRET=smoke-only-not-a-secret \
      GOOGLE_CALENDAR_REDIRECT_URI=http://localhost:8099/calendar/google/callback \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

# --- Phase 3 smoke: web search against the real academic tier ---
#
# The academic tier is keyless and reachable from this machine, so the run gets
# real DOIs. The SearXNG tier is exercised against a stub, because the real
# service needs Docker and A3 measured that a laptop's engine list is a different
# sample from the production host's.
# The model stub must answer with a [W1] citation, or the run has nothing to check:
#   python3 scripts/stub-llm.py --port 8100 --log /tmp/stub3.jsonl \
#     --answer "Transformers replaced recurrence, and the paper carries a DOI [W1]."
smoke-server-phase3:
    SEARCH_PROVIDER=academic LLM_BASE_URL=http://127.0.0.1:8100/v1 \
      LLM_API_KEY=stub LLM_MODEL=stub BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase3:
    BASE=http://localhost:8099 STUB_LLM_LOG=/tmp/stub3.jsonl bash scripts/smoke-phase3.sh

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
    node scripts/check-no-cdn.mjs

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

# --- Phase 12 smoke: the agent's proposal, the confirm, and the reminder dedupe ---
#
# Same stub and server as Phase 2, but the stub must run in its **tool-call** mode: the turn
# has to make the model emit `create_task`, or the run has nothing to confirm. That is what
# `--tool-calls` is for, and it matches on the prompt rather than on the request's position so
# a re-run — which is how a failure is debugged — answers correctly rather than shifted by one.
#
# It reads the database back (the task count before and after the proposal, the single
# `reminders` row and its `fire_at`), so it needs `DATABASE_URL`, which it reads out of `.env`
# itself, and it prunes `signup_attempts` for 127.0.0.1 at the start because A154's window is
# 24h and every run adds to it.
#
# **The push half degrades rather than pretending.** APNs and FCM need credentials that do not
# exist here, so `channels_available` omits `push` and the reminder is exercised over the
# in-app and email channels only; the script prints which run it is instead of reporting a
# green word for a channel that never fired (A288).
smoke-llm-phase12:
    python3 scripts/stub-llm.py --port 8100 --log var/stub-phase12-requests.jsonl \
      --tool-calls scripts/fixtures/phase12-tool-calls.json

smoke-server-phase12:
    LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase12:
    BASE=http://localhost:8099 bash scripts/smoke-phase12.sh

# --- Phase 13 smoke: the QRIS money path, against the stub gateway ---
#
# **The gateway is a stub, and this is the honest consequence of B1.** The real gateway has
# no identity, no docs, no response shapes and no fee rates, so nothing here can exercise a
# live one: `scripts/stub-qris.py` speaks the internal contract
# `crates/server/src/integrations/payment/mapping.rs` defines, and the run prints which
# gateway it used in its first line. Pointing `QRIS_BASE_URL` somewhere other than loopback
# switches the run to "sandbox" mode, where it signs callbacks and reads back the invoice the
# server stored but cannot mint one at a gateway it does not control.
#
# The three `QRIS_*` values are dummy strings, and the signature scheme is named explicitly so
# the run's signing matches the server's verification regardless of a developer's `.env`: a
# mismatch there would look like a forged callback rather than a configuration difference.
#
# It reads the database back (the payment row, the subscription, the ledger and the audit
# rows), so it needs `DATABASE_URL`, which it reads out of `.env` itself, and it prunes
# `signup_attempts` for 127.0.0.1 at the start because A154's window is 24h.
smoke-qris:
    python3 scripts/stub-qris.py --port 8200 --api-key smoke-qris-key \
      --log var/stub-phase13-requests.jsonl

smoke-server-phase13:
    QRIS_BASE_URL=http://127.0.0.1:8200 QRIS_API_KEY=smoke-qris-key \
      QRIS_CALLBACK_TOKEN=smoke-callback-token QRIS_SIGNATURE_SCHEME=hmac_sha256_hex \
      LLM_BASE_URL=http://127.0.0.1:8100/v1 LLM_API_KEY=stub LLM_MODEL=stub \
      BIND_ADDR=127.0.0.1:8099 cargo run -p stvdy-server

smoke-phase13:
    QRIS_BASE_URL=http://127.0.0.1:8200 QRIS_API_KEY=smoke-qris-key \
      QRIS_CALLBACK_TOKEN=smoke-callback-token QRIS_SIGNATURE_SCHEME=hmac_sha256_hex \
      BASE=http://localhost:8099 bash scripts/smoke-phase13.sh

# --- release (Phase 15) ---
# The four commands an operator runs most, aliased so the command in `docs/RELEASE.md`
# and `docs/OPERATIONS.md` is the same string as the one typed here. Two documents that
# both explain the release drift within a month; one that points at the other does not.
#
# The tag is the image's, and it is the one thing an operator has to get right: every
# command below names the image it acts on, so `IMAGE=stvdy-server:v0.2.0 just image`
# and `IMAGE=stvdy-server:v0.2.0 just image-check` are the same artifact.
image_tag := env("IMAGE", "stvdy-server:local")

# Build the server image. `--shm-size` and a writable `/tmp` are the *run* flags Chromium's
# sandbox needs (A338), not build flags; `just image-check` and `docs/OPERATIONS.md` carry
# them.
image:
    docker build -t {{image_tag}} .

# Build the image, boot it against Postgres, probe `/health` and the `/assets/` route, and
# run the PDF acceptance test **inside the container** (A337). `docs/OPERATIONS.md` tells an
# operator to run this **on the production host** before deploying: "works in CI" and "works
# on this host" are different claims, and the second one is the one that ships.
image-check:
    IMAGE={{image_tag}} bash scripts/release-image.sh

# The machine-readable release gate against a directory of built artifacts (A332-A334).
# `--all` is version + macOS + Windows + the manifest, and the manifest is read from the
# same directory: the four commands `docs/RELEASE.md` § Release process step 5 lists, with
# one directory argument instead of four. It **refuses to run (exit 2)** when the updater's
# signing key is in the environment, because a verifier that can sign cannot establish that
# the signature is the operator's.
verify-release dir:
    bash scripts/verify-release-artifacts.sh --all --dir {{dir}} --manifest-dir {{dir}}

# The backup restore drill (A347). The arguments are the script's own: the dump to restore
# and the scratch database to restore into — never production. `--source-url` is optional
# and only adds the row-count comparison against the live database. It prints `=== PASS`, and
# its output goes into `docs/OPERATIONS.md` with a date, because a backup that has never been
# restored is a hope.
restore-drill dump target source="":
    bash scripts/restore-drill.sh --dump {{dump}} --target-url {{target}}{{ if source != "" { " --source-url " + source } else { "" } }}
