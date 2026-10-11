#!/usr/bin/env bash
# Phase 1 end-to-end smoke test.
#
# Drives the real server over HTTP against the local Postgres, with the real
# embedder and the real pdfium library. This is the acceptance run the roadmap
# asks for: signup, email verification, a second device, a PDF upload that
# becomes chunks with embeddings, and a signed download.
#
# Not a test suite (that is crates/server/tests): this proves the wired process
# works, including the parts a test replaces with a double.
set -euo pipefail

BASE="${BASE:-http://localhost:8080}"
MAIL_DIR="${MAIL_DIR:-var/mail}"
TMP="$(mktemp -d)"
EMAIL="smoke-$(date +%s)@example.test"
PASSWORD="correct horse battery"

say() { printf '\n=== %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }

# `jq` is not assumed; a tiny python reader keeps this portable.
field() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

# Reads one chunk id straight from the database. There is no list endpoint by
# design: a client learns chunk ids from citations (Phase 2). The route itself is
# asserted from the API side by crates/server/tests/sources.rs.
#
# `psql` is not on PATH on a stock macOS (Homebrew keeps it under the versioned
# prefix), so the search is explicit rather than assumed.
PSQL="$(command -v psql || true)"
if [ -z "$PSQL" ]; then
  for candidate in /opt/homebrew/opt/postgresql@*/bin/psql /usr/local/opt/postgresql@*/bin/psql; do
    if [ -x "$candidate" ]; then PSQL="$candidate"; break; fi
  done
fi

chunk_id() {
  set -a; . ./.env; set +a
  [ -n "$PSQL" ] || fail "no psql binary found; cannot read a chunk id"
  "$PSQL" "$DATABASE_URL" -tAc \
    "select id from chunks where source_id = '$1' order by idx limit 1"
}

say "health"
curl -fsS "$BASE/health" | tee "$TMP/health.json"
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

say "signup ($EMAIL)"
curl -fsS -X POST "$BASE/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke laptop\",\"platform\":\"macos\",\"app_version\":\"0.1.0\"}}" \
  > "$TMP/signup.json"
ACCESS=$(field '["access_token"]' < "$TMP/signup.json")
REFRESH=$(field '["refresh_token"]' < "$TMP/signup.json")
field '["user"]["email_verified"]' < "$TMP/signup.json" | grep -qx False || fail "a new account must be unverified"

say "verification mail"
MAIL=$(ls -t "$MAIL_DIR"/*.eml | head -1)
grep -q "$EMAIL" "$MAIL" || fail "no mail for $EMAIL"
TOKEN=$(grep -o 'token=[A-Za-z0-9_-]*' "$MAIL" | head -1 | cut -d= -f2)
[ -n "$TOKEN" ] || fail "no verification token in $MAIL"
curl -fsS -X POST "$BASE/auth/verify-email" -H 'content-type: application/json' \
  -d "{\"token\":\"$TOKEN\"}" > "$TMP/verify.json"
field '["verified"]' < "$TMP/verify.json" | grep -qx True || fail "verification failed"

say "verified state"
curl -fsS "$BASE/auth/me" -H "authorization: Bearer $ACCESS" | field '["email_verified"]' | grep -qx True || fail "/auth/me still unverified"

say "second device (login)"
curl -fsS -X POST "$BASE/auth/login" -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke phone\",\"platform\":\"ios\"}}" \
  > "$TMP/login.json"
ACCESS2=$(field '["access_token"]' < "$TMP/login.json")

say "devices"
curl -fsS "$BASE/devices" -H "authorization: Bearer $ACCESS2" > "$TMP/devices.json"
python3 - "$TMP/devices.json" <<'PY'
import json, sys
devices = json.load(open(sys.argv[1]))["devices"]
names = sorted(d["name"] for d in devices)
# Membership, not equality: the run signs in more than twice on purpose (the
# family-revocation step adds one).
assert "Smoke laptop" in names and "Smoke phone" in names, names
assert sum(1 for d in devices if d["current"]) == 1, "exactly one device is current"
print("devices:", names)
PY

say "workspace"
curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Smoke workspace"}' > "$TMP/ws.json"
WS=$(field '["workspace"]["id"]' < "$TMP/ws.json")

say "text source"
TEXT="Mitochondria are the powerhouse of the cell. They generate most of the cell's supply of adenosine triphosphate. Oxidative phosphorylation happens across the inner membrane. The citric acid cycle feeds electrons into the chain."
printf '%s' "$TEXT" > "$TMP/notes.txt"
curl -fsS -X POST "$BASE/workspaces/$WS/sources" -H "authorization: Bearer $ACCESS" \
  -F kind=text -F title="Cell notes" -F "text=$TEXT" > "$TMP/source_text.json"
SRC_TEXT=$(field '["source"]["id"]' < "$TMP/source_text.json")

say "pdf source"
# A real, text-bearing PDF: cupsfilter ships with macOS and needs no extra tool.
cupsfilter -m application/pdf "$TMP/notes.txt" > "$TMP/notes.pdf" 2>/dev/null \
  || fail "cupsfilter could not produce a PDF"
head -c 5 "$TMP/notes.pdf" | grep -q '%PDF-' || fail "not a PDF"
curl -fsS -X POST "$BASE/workspaces/$WS/sources" -H "authorization: Bearer $ACCESS" \
  -F kind=pdf -F title="Cell notes (PDF)" \
  -F "file=@$TMP/notes.pdf;type=application/pdf" > "$TMP/source_pdf.json"
SRC_PDF=$(field '["source"]["id"]' < "$TMP/source_pdf.json")

say "poll to ready"
for src in "$SRC_TEXT" "$SRC_PDF"; do
  STATUS=unknown
  for _ in $(seq 1 120); do
    curl -fsS "$BASE/sources/$src/status" -H "authorization: Bearer $ACCESS" > "$TMP/status.json"
    STATUS=$(field '["status"]' < "$TMP/status.json")
    case "$STATUS" in
      ready|failed) break ;;
    esac
    sleep 1
  done
  printf 'source %s -> ' "$src"
  cat "$TMP/status.json"
  [ "$STATUS" = "ready" ] || fail "source $src did not reach ready"
  CHUNKS=$(field '["chunk_count"]' < "$TMP/status.json")
  [ "$CHUNKS" -ge 1 ] || fail "source $src produced no chunks"
done

say "chunk read"
CHUNK=$(chunk_id "$SRC_PDF")
[ -n "$CHUNK" ] || fail "no chunk row for $SRC_PDF"
curl -fsS "$BASE/chunks/$CHUNK" -H "authorization: Bearer $ACCESS" > "$TMP/chunk.json"
python3 - "$TMP/chunk.json" <<'PY'
import json, sys
chunk = json.load(open(sys.argv[1]))
assert chunk["content"].strip(), "the chunk is empty"
assert chunk["page"] == 1, chunk["page"]
assert chunk["idx"] == 0, chunk["idx"]
print("chunk:", chunk["idx"], "page", chunk["page"], repr(chunk["content"][:60]))
PY

say "signed file URL"
curl -fsS "$BASE/sources/$SRC_PDF/file" -H "authorization: Bearer $ACCESS" > "$TMP/file.json"
URL=$(field '["url"]' < "$TMP/file.json")
# The URL is built from OBJECTS_PUBLIC_BASE_URL (the deployment's own origin), and
# the signature covers `source_id:exp`, not the host. This run listens on a
# non-default port, so the origin is rewritten here to keep the two consistent;
# nothing about the signature depends on it.
URL=$(printf '%s' "$URL" | sed "s|^http://[^/]*|$BASE|")
curl -fsS "$URL" -o "$TMP/downloaded.pdf"
cmp -s "$TMP/notes.pdf" "$TMP/downloaded.pdf" || fail "the downloaded file differs from the upload"
printf 'signed URL works, %s bytes\n' "$(wc -c < "$TMP/downloaded.pdf" | tr -d ' ')"

say "signed URL rejects a tampered signature"
BAD=$(printf '%s' "$URL" | sed 's/sig=./sig=0/')
CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BAD")
[ "$CODE" = "403" ] || fail "a bad signature must be 403, got $CODE"

say "cross-account isolation"
curl -fsS -X POST "$BASE/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"other-$(date +%s)@example.test\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Intruder\",\"platform\":\"web\"}}" \
  > "$TMP/other.json"
OTHER=$(field '["access_token"]' < "$TMP/other.json")
for path in "sources/$SRC_PDF/status" "workspaces/$WS/sources"; do
  CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/$path" -H "authorization: Bearer $OTHER")
  [ "$CODE" = "404" ] || fail "$path leaked to another account: $CODE"
done

say "refresh rotation"
curl -fsS -X POST "$BASE/auth/refresh" -H 'content-type: application/json' \
  -d "{\"refresh_token\":\"$REFRESH\"}" > "$TMP/refresh.json"
ROTATED=$(field '["refresh_token"]' < "$TMP/refresh.json")
[ "$ROTATED" != "$REFRESH" ] || fail "refresh must rotate"
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/auth/refresh" \
  -H 'content-type: application/json' -d "{\"refresh_token\":\"$REFRESH\"}")
[ "$CODE" = "401" ] || fail "a replayed refresh token must be 401, got $CODE"
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/auth/refresh" \
  -H 'content-type: application/json' -d "{\"refresh_token\":\"$ROTATED\"}")
[ "$CODE" = "401" ] || fail "reuse must revoke the family, got $CODE"

# This runs last on purpose: revoking the family also kills that device's access
# token, which is the intended semantics (the whole chain is assumed stolen), so
# nothing after this point may depend on `$ACCESS`.
CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/auth/me" -H "authorization: Bearer $ACCESS")
[ "$CODE" = "401" ] || fail "the revoked family's access token must stop working, got $CODE"

say "PASS"
