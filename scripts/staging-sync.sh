#!/usr/bin/env bash
#
# Put the current commit of this repository onto the staging host, and build the server
# image there.
#
# **The host pulls from GitHub.** The build context is a clone at `<host>:~/stvdy/src`,
# and this script moves it to a commit rather than copying a working tree: a deployment
# should be reproducible from a pushed commit, and an rsync of a developer's tree can
# ship uncommitted work that no one can look up afterwards. It also means the host is
# self-sufficient — a rebuild there needs no laptop.
#
# The image is the deployment unit and there is no registry, so `docker build` runs on
# the host, natively `amd64`, because the laptop is `arm64` and a cross-build of this
# workspace is slower than the fetch.
#
# **Every directory in the context is compiled in, and a missing one is a runtime defect
# rather than a build failure.** `migrations/` is embedded by `sqlx::migrate!`, and
# `prompts/`, `templates/`, `config/` and `assets/` by `include_str!`/`rust-embed`. An
# image built from a tree whose `migrations/` is stale carries code for a schema its
# database does not have, and `--migrate-only` cannot repair it because it runs *that
# image's* embedded set — the symptom is one route answering 500 with a column name in it
# (`docs/OPERATIONS.md` § Staging). A git checkout makes that class of mistake impossible:
# the tree is whatever the commit says it is.
#
#   bash scripts/staging-sync.sh [host] [tag] [ref]
#
set -euo pipefail

host="${1:-ubuntu@129.225.10.139}"
tag="${2:-stvdy-server:0.1.3}"
ref="${3:-main}"
root="$(cd "$(dirname "$0")/.." && pwd)"

# Refuse to deploy a commit nobody can look up. The host pulls, so anything that is only
# in the working tree is invisible to it — and a deploy that silently ships the previous
# commit is worse than one that fails.
if [ "$ref" = "main" ]; then
  local_head="$(git -C "$root" rev-parse HEAD)"
  remote_head="$(git -C "$root" ls-remote origin main | cut -f1)"
  if [ "$local_head" != "$remote_head" ]; then
    echo "warning: HEAD ($local_head) is not origin/main ($remote_head)." >&2
    echo "         The host builds what was pushed, so this deploys the pushed commit," >&2
    echo "         not your working tree. Push first if that is not what you want." >&2
  fi
fi

echo "=== $host: fetching $ref into ~/stvdy/src"
ssh "$host" "
  set -e
  if [ -d ~/stvdy/src/.git ]; then
    git -C ~/stvdy/src fetch --depth 1 origin $ref
    git -C ~/stvdy/src reset --hard FETCH_HEAD
  else
    rm -rf ~/stvdy/src
    git clone --depth 1 --branch $ref https://github.com/jettapindika/stvdy.git ~/stvdy/src
  fi
  git -C ~/stvdy/src log --oneline -1
"

echo "=== building $tag on the host (this takes ~25 minutes from cold)"
ssh "$host" "cd ~/stvdy/src && docker build -t $tag ."

echo "=== built"
ssh "$host" "docker image inspect $tag --format '{{.Id}} {{.Created}} {{.Size}}'"

# The deployment artifacts, which are **not** part of the build context: nothing compiles
# them, so a git pull would not put them anywhere the deploy reads, and the host would keep
# running whatever was last copied by hand. That is how a service can exist in this
# repository and not on the host — `gopay-bridge` did, and a deploy that looked green had no
# payment gateway in it.
#
# `.env.staging` is deliberately **not** shipped: it is the one file on the host that is not
# derived from this tree, and it holds the deployment's secrets and its filled-in values. The
# template lands beside it under a name nothing reads, so a new key (`QRIS_MERCHANT_ID`, say)
# is one `diff` away rather than a paragraph in a document.
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
