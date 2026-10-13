#!/usr/bin/env bash
#
# Deploy the staging stack, on the A45 trial host. **Not production**: that host is a
# one-month trial, so the QRIS webhook URL, the Google OAuth redirect URI, the updater
# manifest and user data are all forbidden there (A45, `docs/OPERATIONS.md` § Staging).
#
# Runs **on the host**, from the deployment root (`~/stvdy`), which holds:
#
#   docker-compose.staging.yml   from `deploy/staging/docker-compose.staging.yml`
#   docker-compose.yml           the host's own SearXNG stack (A45 provisioned it), same
#                                compose project, so both files share `stvdy_default`
#   web/nginx.conf               from `deploy/staging/nginx.conf`
#   web/dist/                    the SPA bundle, built against the public origin:
#                                `VITE_API_BASE_URL=https://stvdy.ztvn.my.id/api npm --prefix app run build`
#                                (nginx serves the bundle and `/api/` on :80; nothing listens on :8080)
#   .env.staging                 from `deploy/staging/env.example`, filled in
#   .env                         compose interpolation only: POSTGRES_PASSWORD, SERVER_IMAGE
#   data/objects, data/mail      chowned to 10001 (the image's `stvdy` user, A338)
#
# The image is the deployment unit — API, worker and scheduler are three tasks in one process —
# and the migration is a step rather than a side effect (A346): `--migrate-only` runs to
# completion and exits without binding a port, before any replica starts.
#
# This script assumes the image already exists on the host. `scripts/staging-sync.sh` is what
# puts it there: it rsyncs the build context and builds, and its file list is the set of
# directories the build compiles in — a context missing `migrations/` produces an image whose
# code and schema disagree, which shows up as a 500 on one route rather than a failed build.
#
#   bash scripts/staging-sync.sh          # from the laptop, once per code change
#   bash scripts/staging-deploy.sh        # on the host, from the deployment root
#
# Run it as `scripts/staging-deploy.sh` and not from a copy at the root: the smoke pass is its
# sibling (`$(dirname "$0")/staging-smoke.sh`), so a copy elsewhere fails at the last step with
# `./staging-smoke.sh: No such file or directory` — after it has already deployed, which reads
# like a deploy failure rather than a path one.
#
set -euo pipefail

root="${1:-$HOME/stvdy}"
cd "$root"

export SERVER_IMAGE="${SERVER_IMAGE:-stvdy-server:0.1.3}"
compose=(docker compose -f docker-compose.staging.yml)

echo "=== image"
docker image inspect "$SERVER_IMAGE" --format '{{.Id}} {{.Created}}'

echo "=== postgres"
"${compose[@]}" up -d postgres

echo "=== migrate (must finish before any replica starts)"
"${compose[@]}" run --rm server --migrate-only

echo "=== the replica"
"${compose[@]}" up -d server

echo "=== the SPA and the proxy"
# `web/nginx.conf` and `web/dist/` are **bind mounts**, and Docker does not track a bind
# mount's contents — and a file bind-mounted by path is resolved to an inode at container
# creation, so a `scp` that replaces the file leaves the running container reading the old one.
# A new `nginx.conf` therefore needs the container recreated, and the config is validated
# **first** with a throwaway container: a syntax error found after the recreate is a site that
# is down with no working previous version.
#
# The validation runs as a one-off of the `web` service rather than a bare `docker run`,
# because the config names the upstream `server` and only a container on the compose network
# can resolve it — `host not found in upstream "server"` is what a detached validation says.
"${compose[@]}" run --rm --no-deps --entrypoint nginx web -t
"${compose[@]}" up -d --force-recreate web

echo "=== health (through nginx, which is the only public entry point)"
for _ in $(seq 1 60); do
  if curl -fsS localhost/api/health 2>/dev/null | grep -q '"status":"ok"'; then break; fi
  sleep 1
done
health="$(curl -fsS localhost/api/health)"
echo "$health"
# A proxy that is not stripping the `/api` prefix answers this route from the SPA's
# `try_files` fallback instead, with the index document and a 200 — so "the request
# succeeded" is not the check; "the body is the server's JSON" is.
case "$health" in
*'"status":"ok"'*) ;;
*)
  echo "the API is not answering through the proxy (see the body above)" >&2
  exit 1
  ;;
esac
printf 'spa    %s\n' "$(curl -fsS -o /dev/null -w '%{http_code}' localhost/)"
printf 'assets %s\n' "$(curl -fsS -o /dev/null -w '%{http_code}' localhost/api/assets/katex/katex.min.css)"

echo
echo "=== the end-to-end pass (signup → the mail sink → verify → login)"
# Through nginx on `/api`, which is the only way the API is reachable now that the server
# publishes no port of its own — so this also proves the proxy's prefix stripping works.
bash "$(dirname "$0")/staging-smoke.sh" http://localhost/api
