#!/usr/bin/env bash
#
# Copy the build context to the staging host and build the server image there.
#
# The image is the deployment unit and there is no registry: the context is rsynced to
# `<host>:~/stvdy/src` and `docker build` runs on the host, natively `amd64`, because the
# laptop is `arm64` and a cross-build of this workspace is slower than the copy.
#
# **Every directory here is compiled in, and a missing one is a runtime defect rather than a
# build failure.** `migrations/` is embedded by `sqlx::migrate!`, and `prompts/`,
# `templates/`, `config/` and `assets/` by `include_str!`/`rust-embed`. An image built from a
# context with a stale `migrations/` carries code for a schema its database does not have, and
# `--migrate-only` cannot repair it because it runs *that image's* embedded set — the symptom
# is one route answering 500 with a column name in it (`docs/OPERATIONS.md` § Staging).
#
#   bash scripts/staging-sync.sh [host] [tag]
#
set -euo pipefail

host="${1:-ubuntu@129.225.10.139}"
tag="${2:-stvdy-server:0.1.2}"
root="$(cd "$(dirname "$0")/.." && pwd)"

echo "=== syncing the build context to $host:~/stvdy/src"
rsync -az --delete \
  --exclude '.git' --exclude 'target' --exclude 'node_modules' --exclude 'app/node_modules' \
  --exclude 'app/dist' --exclude 'var' --exclude '.fastembed_cache' --exclude 'src-tauri' \
  --exclude '.superpowers' --exclude 'docs' --exclude 'supabase' --exclude '.env' \
  "$root"/Cargo.toml "$root"/Cargo.lock "$root"/rust-toolchain.toml "$root"/clippy.toml \
  "$root"/.dockerignore "$root"/Dockerfile "$root"/docker-compose.yml "$root"/justfile \
  "$root"/.env.example "$root"/crates "$root"/migrations "$root"/prompts "$root"/templates \
  "$root"/config "$root"/assets "$root"/scripts \
  "$host:~/stvdy/src/"

echo "=== building $tag on the host (this takes ~25 minutes from cold)"
ssh "$host" "cd ~/stvdy/src && docker build -t $tag ."

echo "=== built"
ssh "$host" "docker image inspect $tag --format '{{.Id}} {{.Created}} {{.Size}}'"

# The deployment artifacts, which are **not** part of the build context: nothing compiles
# them, so the rsync above would not notice a change and the host would keep running whatever
# was last copied by hand. That is how a service can exist in this repository and not on the
# host — `gopay-bridge` did, and a deploy that looked green had no payment gateway in it.
#
# The repository is the source of truth for all three: they are derived from this tree, and
# the host copies were byte-identical to these. `.env.staging` is deliberately **not** here —
# it is the one file on the host that is not derived from this tree, and it holds the
# deployment's secrets and its filled-in values. The template ships beside it under a name
# nothing reads, so a new key (`QRIS_MERCHANT_ID`, say) is one diff away rather than a
# paragraph in a document.
echo "=== syncing the deployment artifacts to $host:~/stvdy"
scp "$root/deploy/staging/docker-compose.staging.yml" "$host:~/stvdy/docker-compose.staging.yml"
scp "$root/deploy/staging/nginx.conf" "$host:~/stvdy/web/nginx.conf"
scp "$root/deploy/staging/env.example" "$host:~/stvdy/.env.staging.example"

# A fourth thing the compose file needs and this repository does not contain: the gateway's
# own build context, `~/stvdy/gopay-bridge-src/`, which comes from the gateway's repository
# (its README is the API document). It is checked rather than shipped, because a copy of
# another project's source is not something this script should be able to invent.
if ! ssh "$host" "test -f ~/stvdy/gopay-bridge-src/Dockerfile"; then
  echo "warning: ~/stvdy/gopay-bridge-src/Dockerfile is missing on $host." >&2
  echo "         The compose file builds the gateway from it; a deploy without it leaves" >&2
  echo "         checkout answering 502 provider_error. See docs/OPERATIONS.md § Payments." >&2
fi
