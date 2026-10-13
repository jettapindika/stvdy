#!/usr/bin/env bash
#
# Run the workspace checks — `fmt`, `clippy`, `test` — on the staging host instead of on the
# laptop.
#
# **Why the host.** `target/` is the largest thing this repository produces and the least
# valuable: a debug build of this workspace plus its test targets was **62 GB** on the
# developer's machine, against 5 MB of source under `crates/`. Nothing about a compiler
# invocation needs to be local, and the host builds the same code anyway.
#
# **It is the same toolchain the image builds with, not a second one.** The container is
# `rust:1.94.1-trixie`, the `FROM` of the `Dockerfile`'s build stage, so a check that passes
# here and a check that passes in CI are the same compiler and the same `rust-toolchain.toml`
# pin. Installing a toolchain on the host would be a second source of truth for a version
# that is already written down in both places.
#
# **The source is copied, not mounted.** `tar` over `ssh`, into a directory that is wiped
# first, so the tree the check sees is the tree that was sent. `bsdtar` restores each file's
# own mtime, which is what makes the volume below worth having: a file nobody touched keeps
# the timestamp cargo fingerprinted last time, so the run recompiles what changed rather than
# everything.
#
# **The compiled artifacts are a Docker volume, not a directory in the tree**, so wiping the
# source costs nothing and a re-run reuses the dependency graph.
#
# **The database is a container this script owns.** 53 of the 57 `crates/server/tests`
# suites are `#[sqlx::test]`, which creates and drops a scratch database per case, so the
# check needs a reachable Postgres with pgvector and `CREATEDB`. It is **not** the staging
# instance: that one holds the deployment's data, and a check that creates and drops databases
# under it can take staging down. `stvdy-check-pg` is its own container on its own network,
# with its own volume and no published port. `sqlx::test` applies `migrations/` to each
# scratch database itself, which is why nothing runs `sqlx migrate run` here.
#
# **The disk is the constraint, and the guard is not decoration.** This host is 45 GB with the
# staging stack on the same filesystem — postgres, the server image, the build cache. The
# script refuses to start below 5 GB free, and a watchdog kills the container if free space
# falls below 2 GB while it runs: the check failing loudly is the only acceptable direction for
# this to fail in. `CARGO_PROFILE_DEV_DEBUG=0` (and the same for the `test` profile, which
# inherits `dev`) is most of what keeps the artifacts small — a check never reads a debugger's
# line table — and `CARGO_INCREMENTAL=0` gives up a few seconds of rebuild time for the
# gigabytes that incremental state would take.
#
# **What this does not cover, and it is not an oversight.** `src-tauri` is excluded from the
# workspace (root `Cargo.toml`), and the shell is where the native work lives: an iOS bundle
# needs Xcode, an Android one needs the SDK and a JDK, and a Linux `cargo check` of it needs
# `libwebkit2gtk-4.1-dev`. `just check-shell` therefore stays on the Mac, and so does
# everything under `src-tauri/gen/`. `just check-web` is Node. The workspace — `crates/core`,
# `crates/sync`, `crates/server`, where the server, the migrations and the domain logic are —
# is what moves. `.github/workflows/ci.yml` runs all three on every push; this is the same
# check without a push.
#
#   bash scripts/remote-check.sh                    # everything, on the staging host
#   bash scripts/remote-check.sh ubuntu@host clippy # one mode, another host
#
set -euo pipefail

host="${1:-ubuntu@129.225.10.139}"
mode="${2:-all}"
root="$(cd "$(dirname "$0")/.." && pwd)"

# The build context, which is the `Dockerfile`'s own `COPY` list plus the two files that change
# what the compiler does: `clippy.toml` (this workspace denies `unwrap_used`, `expect_used` and
# `panic`, and the test relaxations live there) and `.cargo/config.toml`. A path missing from
# this list is a check that passes on a tree the image would not build.
paths=(
  Cargo.toml Cargo.lock rust-toolchain.toml clippy.toml .cargo
  crates migrations prompts templates config assets
)

case "$mode" in
  fmt)    run="cargo fmt --all -- --check" ;;
  clippy) run="cargo clippy --workspace --all-targets -- -D warnings" ;;
  test)   run="cargo test --workspace" ;;
  all)
    # The order `just check-rust` uses, and for the same reason: a formatting failure is free
    # to find and a test failure is not.
    run="cargo fmt --all -- --check \
      && cargo clippy --workspace --all-targets -- -D warnings \
      && cargo test --workspace"
    ;;
  *)
    echo "unknown mode: $mode (fmt|clippy|test|all)" >&2
    exit 2
    ;;
esac

echo "=== $host: sending $(printf '%s ' "${paths[@]}")"
# The guard runs before the copy, because the copy is what makes the disk smaller first: the
# build cache is what holds this host's free space down, and the two commands that give it back
# are named rather than run — pruning costs the next staging image build its cache.
free_gb="$(ssh "$host" "df --output=avail -BG / | tail -1 | tr -dc '0-9'")"
if [ "$free_gb" -lt 5 ]; then
  echo "only ${free_gb}G free on $host (/); the check volume needs ~5G beside the staging stack." >&2
  echo "  docker builder prune                    # the image build cache, which is the large one" >&2
  echo "  docker volume rm stvdy-check-target     # forces a cold recompile" >&2
  exit 1
fi

# `--no-mac-metadata` (and `COPYFILE_DISABLE`, which is what the same thing is called when it
# comes from the environment): without them macOS's `tar` writes an AppleDouble `._name` sidecar
# next to every file, so the shipped tree had 602 `.rs` files where the repository has 301 — and
# GNU tar on the host prints a warning per extended attribute besides. `--no-xattrs` alone does
# not stop the sidecars.
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf - -C "$root" "${paths[@]}" \
  | ssh "$host" 'rm -rf ~/stvdy-check/src && mkdir -p ~/stvdy-check/src \
      && tar -xzf - -C ~/stvdy-check/src'

# The check database. Created once and kept: the volume is what makes a second run cheap, and
# a container that is already up is left alone rather than recreated, because `sqlx::test`
# leaves its scratch databases behind when a run is killed.
echo "=== $host: the check database"
ssh "$host" "bash -s" <<'REMOTE'
set -euo pipefail
docker network inspect stvdy-check >/dev/null 2>&1 || docker network create stvdy-check >/dev/null
if [ -z "$(docker ps --quiet --filter name=^stvdy-check-pg$)" ]; then
  docker rm --force stvdy-check-pg >/dev/null 2>&1 || true
  docker run --detach --name stvdy-check-pg --network stvdy-check \
    --env POSTGRES_USER=postgres --env POSTGRES_PASSWORD=postgres --env POSTGRES_DB=stvdy \
    --volume stvdy-check-pgdata:/var/lib/postgresql/data \
    pgvector/pgvector:pg16 >/dev/null
fi
for _ in $(seq 1 60); do
  if docker exec stvdy-check-pg pg_isready --username postgres --dbname stvdy >/dev/null 2>&1; then
    echo "ready"
    exit 0
  fi
  sleep 1
done
echo "stvdy-check-pg did not become ready in 60s" >&2
exit 1
REMOTE

echo "=== $host: $mode"
# `sh -c` and **not** `sh -lc`: a login shell sources `/etc/profile`, whose `PATH` on Debian
# omits `/usr/local/cargo/bin` — the directory the image's own `PATH` puts `cargo` in — so the
# login form fails with `cargo: not found` inside an image that is named after cargo.
ssh "$host" "bash -s" <<REMOTE
set -euo pipefail
name=stvdy-check-run
docker rm --force \$name >/dev/null 2>&1 || true
docker run --detach --name \$name \\
  --network stvdy-check \\
  --volume "\$HOME/stvdy-check/src:/src" \\
  --volume stvdy-check-target:/src/target \\
  --workdir /src \\
  --env DATABASE_URL=postgres://postgres:postgres@stvdy-check-pg:5432/stvdy \\
  --env CARGO_PROFILE_DEV_DEBUG=0 \\
  --env CARGO_PROFILE_TEST_DEBUG=0 \\
  --env CARGO_INCREMENTAL=0 \\
  --env CARGO_TERM_COLOR=always \\
  rust:1.94.1-trixie \\
  sh -c '$run' >/dev/null

(
  while docker inspect --format '{{.State.Running}}' \$name 2>/dev/null | grep -q true; do
    avail=\$(df --output=avail -BG / | tail -1 | tr -dc '0-9')
    if [ "\$avail" -lt 2 ]; then
      echo "!!! disk guard: \${avail}G free — killing the check rather than the staging stack" >&2
      docker kill \$name >/dev/null
      break
    fi
    sleep 20
  done
) &
watchdog=\$!

docker logs --follow \$name
wait \$watchdog 2>/dev/null || true
code=\$(docker inspect --format '{{.State.ExitCode}}' \$name)
docker rm \$name >/dev/null
echo "=== $mode: exit \$code"
exit \$code
REMOTE
