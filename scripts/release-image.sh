#!/usr/bin/env bash
#
# Builds the server image, boots it against a real Postgres, and runs the PDF acceptance
# test **inside** the container.
#
# Why the acceptance test runs in here rather than on a laptop: `crates/server/tests/pdf.rs`
# is `#[ignore]`d so a machine without a browser still gets a green `cargo test`, and the
# failure it catches is the one that only shows up on deploy — `find_executable` probes
# `/Applications/Google Chrome.app/…` first, so on a developer's Mac the pipeline works with
# the vendor's Chrome, and on a host with no Chrome `AppState.renderer` is `None` and the
# symptom is a PDF nobody can download (409 `conflict`), which no health check catches
# (`docs/RELEASE.md`, A129, A130).
#
# The same sequence runs in CI (`.github/workflows/ci.yml`'s `image` job). This script is its
# readable form, and it is what `docs/OPERATIONS.md` tells an operator to run **on the
# production host** before deploying a new image: "the image works in CI" and "the image works
# on this host" are different claims, and the second one is the one that ships.
#
#   bash scripts/release-image.sh
#
# Requires Docker and a reachable Postgres. Every variable has a default that matches the
# repository's `docker-compose.yml`, so `docker compose up -d postgres && bash
# scripts/release-image.sh` works on a fresh clone.
set -euo pipefail

image="${IMAGE:-stvdy-server:local}"
database_url="${DATABASE_URL:-postgres://postgres:postgres@localhost:5432/stvdy}"
jwt_secret="${JWT_SECRET:-local-only-secret-that-is-long-enough-for-prod}"
encryption_key="${ENCRYPTION_KEY_HEX:-$(printf '1%.0s' {1..64})}"
port="${PORT:-18080}"
container="stvdy-image-check"

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "=== building $image"
docker build -t "$image" .

echo
echo "=== the pin is the pin"
# An image that silently tracked Debian's `stable` would pass every other check in this
# script: the browser would start, the render would work, and the page count would change
# whenever Debian moved. So the version the image actually has installed is compared to the
# `ARG` in the Dockerfile.
#
# **It is the package version that is compared, not `chromium --version`.** The binary
# reports its upstream version alone — `Chromium 154.0.8037.92 built on Debian GNU/Linux 13
# (trixie)` — while `ARG CHROMIUM_VERSION` is Debian's package version
# (`154.0.8037.92-1~deb13u1`), so a string comparison of the two can never pass. That was
# true of the bookworm pin as well, and it went unnoticed because the build died at the
# glibc link error long before this line ran. `dpkg-query` answers the question the pin
# actually asks, and the binary's own version is checked as the prefix of it.
# `\${Version}` is escaped twice over, deliberately: the outer shell must not expand it, and
# the container's own `/bin/sh` must not either — `dpkg-query` is the thing that expands it,
# and a version of this line without the escape passes an empty format string and answers
# `dpkg-query: error in show format: may not be empty string` instead of a version.
#
# All three packages, because they are built from one source and are pinned together in the
# `Dockerfile`: `chromium-sandbox` is the setuid helper A338's sandbox needs, and a browser
# whose helper drifted from it is a renderer that refuses to start.
built="$(docker run --rm --entrypoint /bin/sh "$image" -c \
  'dpkg-query -W -f=\${Version} chromium; echo; dpkg-query -W -f=\${Version} chromium-common; echo; dpkg-query -W -f=\${Version} chromium-sandbox')"
declared="$(grep -oE '^ARG CHROMIUM_VERSION=.*' Dockerfile | cut -d= -f2)"
binary="$(docker run --rm --entrypoint /usr/bin/chromium "$image" --version)"
echo "package:  $built"
echo "declared: $declared"
echo "binary:   $binary"
# Both packages, because they are built from one source and pinning only the binary would
# let the pair drift (the Dockerfile's own comment).
if [ "$(printf '%s\n' "$built" | sort -u)" != "$declared" ]; then
  echo "the image's Chromium packages are not the declared pin" >&2
  exit 1
fi
case "$binary" in
"Chromium ${declared%%-*}"*) ;;
*)
  echo "the image's Chromium binary does not report the pinned upstream version" >&2
  exit 1
  ;;
esac

echo
echo "=== the other native dependencies are present"
docker run --rm --entrypoint /bin/sh "$image" -c '
  set -e
  test -x "$CHROME_PATH" || { echo "no Chromium at $CHROME_PATH"; exit 1; }
  test -f "$PDFIUM_LIB_PATH" || { echo "no pdfium at $PDFIUM_LIB_PATH"; exit 1; }
  test -x "$SOFFICE_BIN" || { echo "no LibreOffice at $SOFFICE_BIN"; exit 1; }
  test -d "$FASTEMBED_CACHE_DIR" || { echo "no model cache at $FASTEMBED_CACHE_DIR"; exit 1; }
  # Non-empty, because an empty directory is exactly the state that re-downloads 1.06 GB at
  # the first ingest on a host with no egress (A342).
  test -n "$(ls -A "$FASTEMBED_CACHE_DIR")" || { echo "the model cache is empty"; exit 1; }
  echo "chromium $(/usr/bin/chromium --version)"
  echo "pdfium   $(ls "$PDFIUM_LIB_PATH")"
  echo "soffice  $("$SOFFICE_BIN" --version | head -1)"
  echo "model    $(du -sh "$FASTEMBED_CACHE_DIR" | cut -f1) in $FASTEMBED_CACHE_DIR"
'

echo
echo "=== migrating (this must finish before the server starts: A346)"
# The same environment the replica gets, and that is not redundant: the **image** sets
# `APP_ENV=prod` (its own `ENV`, not this script's), `--migrate-only` loads and validates the
# whole `AppConfig` before it touches the database (`main.rs`), and a prod config refuses to
# build without `JWT_SECRET` (≥32 chars), a non-zero `ENCRYPTION_KEY_HEX` **and** a
# `WEB_ORIGIN`. So the documented one-liner carrying only `DATABASE_URL` exited on
# `JWT_SECRET must be set …` and then on `WEB_ORIGIN must be set …` without ever migrating;
# `docs/OPERATIONS.md` and `docs/RELEASE.md` carry the corrected stanza.
docker run --rm --network host \
  -e DATABASE_URL="$database_url" \
  -e APP_ENV=prod \
  -e JWT_SECRET="$jwt_secret" \
  -e ENCRYPTION_KEY_HEX="$encryption_key" \
  -e WEB_ORIGIN=http://localhost:5173 \
  "$image" --migrate-only

echo
echo "=== booting"
# The three flags Chromium's sandbox needs, and each is a distinct mechanism.
#
# `--shm-size` and `--tmpfs /tmp` are the resources it needs: without them the browser dies with
# a memory error that reads like a code bug (A338). `--security-opt seccomp=unconfined` is the
# **permission** it needs and it is the one that is easy to leave out: Docker's default seccomp
# profile blocks the namespace the renderer's sandbox moves into, so the browser dies with
# `Failed to move to new namespace … errno = Operation not permitted` and
# `Zygote process exited prematurely`, and the server then **hangs** waiting for a browser that is
# gone — no error, no bound port. It grants no capability (unlike the `--cap-add SYS_ADMIN`
# alternative, which the SUID helper path needs only where unprivileged user namespaces are
# unavailable). The deployment stanza is in `docs/OPERATIONS.md`.
cleanup
docker run -d --name "$container" --network host \
  --shm-size=1g --tmpfs /tmp --security-opt seccomp=unconfined \
  -e DATABASE_URL="$database_url" \
  -e APP_ENV=prod \
  -e JWT_SECRET="$jwt_secret" \
  -e ENCRYPTION_KEY_HEX="$encryption_key" \
  -e WEB_ORIGIN=http://localhost:5173 \
  -e BIND_ADDR="0.0.0.0:$port" \
  -e ASSETS_BASE_URL="http://127.0.0.1:$port" \
  -e STORAGE_BACKEND=local \
  -e STORAGE_LOCAL_DIR=/tmp/objects \
  "$image" >/dev/null

for _ in $(seq 1 60); do
  if curl -fsS "localhost:$port/health" >/dev/null 2>&1; then break; fi
  sleep 1
done
health="$(curl -fsS "localhost:$port/health")"
echo "health: $health"
case "$health" in
*'"status":"ok"'*) ;;
*) echo "the server did not become healthy" >&2; exit 1 ;;
esac

echo
echo "=== the print assets are served from inside the image"
# The route the renderer rewrites the template's CDN references to (A339). A 404 here is the
# failure that produces a PDF full of literal `$…$`, so it is checked before the render.
for path in katex/katex.min.css katex/katex.min.js katex/contrib/auto-render.min.js fonts.css; do
  code="$(curl -s -o /dev/null -w '%{http_code}' "localhost:$port/assets/$path")"
  if [ "$code" != 200 ]; then
    echo "GET /assets/$path answered $code" >&2
    exit 1
  fi
  echo "200 /assets/$path"
done

echo
echo "=== the PDF acceptance test, inside the image"
# The same command CI's `pdf` job runs on a bare runner, against this image's own Chromium,
# pdfium and assets. The binary's name carries a cargo hash, so it is resolved here rather
# than hard-coded.
test_bin="$(docker exec "$container" sh -c 'ls /opt/tests/deps/pdf-* 2>/dev/null | head -1')"
if [ -z "$test_bin" ]; then
  echo "no pdf test binary in /opt/tests/deps" >&2
  docker exec "$container" ls /opt/tests/deps | head >&2
  exit 1
fi
echo "running $test_bin"
# `--ignored` is what runs the `#[ignore]`d acceptance test. It asserts the PDF is six A4
# pages and that its extracted text contains no raw `$` — so a pass here proves the assets
# loaded, because a missing KaTeX leaves the formulas as literal LaTeX. It needs no
# database: the document is rendered from sample blocks compiled into the test.
docker exec "$container" sh -c \
  "cd /opt/tests && CHROME_PATH=/usr/bin/chromium ./deps/$(basename "$test_bin") --ignored --nocapture"

echo
echo "=== PASS"
