#!/usr/bin/env bash
# Phase 2 end-to-end smoke test: retrieval, streaming chat, citation validation.
#
# Drives the real server over HTTP against the local Postgres, with the real
# embedder and the real pdfium, and a STUB model server that speaks real HTTP and
# real SSE (scripts/stub-llm.py). The stub is deliberate: the Rust suite replaces
# the provider at the trait boundary, so it cannot catch a wire-format mistake in
# the request body or the frame parser. This run exercises both.
#
# Not a test suite (that is crates/server/tests): this proves the wired process
# works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
# The port the SERVER was started with (LLM_BASE_URL), not a private one: the
# server is what talks to the stub, so the stub the script watches must be the
# stub the server calls. Start the server with
#   LLM_BASE_URL=http://127.0.0.1:8100/v1
# and leave this at its default, or set both to the same value.
STUB_PORT="${STUB_PORT:-8100}"
STUB_URL="http://127.0.0.1:$STUB_PORT"
MAIL_DIR="${MAIL_DIR:-var/mail}"
TMP="$(mktemp -d)"
EMAIL="phase2-$(date +%s)@example.test"
PASSWORD="correct horse battery"

say() { printf '\n=== %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
field() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

PSQL="$(command -v psql || true)"
if [ -z "$PSQL" ]; then
  for candidate in /opt/homebrew/opt/postgresql@*/bin/psql /usr/local/opt/postgresql@*/bin/psql; do
    if [ -x "$candidate" ]; then PSQL="$candidate"; break; fi
  done
fi

# --- the stub model server -------------------------------------------------
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

say "stub model server"
# A deterministic log path, so a stub started outside this script (by hand, or by
# another run) still leaves its request bodies where this script looks.
STUB_LOG="${STUB_LOG:-var/stub-requests.jsonl}"
rm -f "$STUB_LOG"

if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
  # Already listening: reuse it. Only safe if it was started with --log pointing
  # at STUB_LOG, which the assertion below checks.
  echo "a stub is already listening on $STUB_PORT; reusing it"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" > "$TMP/stub.log" 2>&1 &
  STUB_PID=$!
  for _ in $(seq 1 50); do
    if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
      break
    fi
    sleep 0.2
  done
  cat "$TMP/stub.log"
fi
nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null || fail "no stub model server on $STUB_PORT"

say "health"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Start the stub with 'just smoke-llm' and the server with 'just smoke-server'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

say "signup and verify ($EMAIL)"
curl -fsS -X POST "$BASE/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke laptop\",\"platform\":\"macos\"}}" \
  > "$TMP/signup.json"
ACCESS=$(field '["access_token"]' < "$TMP/signup.json")
MAIL=$(ls -t "$MAIL_DIR"/*.eml | head -1)
TOKEN=$(grep -o 'token=[A-Za-z0-9_-]*' "$MAIL" | head -1 | cut -d= -f2)
curl -fsS -X POST "$BASE/auth/verify-email" -H 'content-type: application/json' \
  -d "{\"token\":\"$TOKEN\"}" > /dev/null

say "workspace with an Indonesian source"
curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Jaringan Komputer"}' > "$TMP/ws.json"
WS=$(field '["workspace"]["id"]' < "$TMP/ws.json")

DOKUMEN="Jaringan komputer adalah kumpulan komputer yang saling terhubung untuk berbagi data. Topologi star menghubungkan setiap komputer ke satu perangkat pusat bernama switch. Jika kabel pada satu komputer terputus, komputer lain tetap dapat berkomunikasi. Kelebihan topologi star adalah kemudahan menambah perangkat tanpa mengganggu jaringan secara keseluruhan."
curl -fsS -X POST "$BASE/workspaces/$WS/sources" -H "authorization: Bearer $ACCESS" \
  -F kind=text -F title="Catatan Jaringan" -F "text=$DOKUMEN" > "$TMP/source.json"
SRC=$(field '["source"]["id"]' < "$TMP/source.json")

say "poll to ready"
STATUS=unknown
for _ in $(seq 1 120); do
  curl -fsS "$BASE/sources/$SRC/status" -H "authorization: Bearer $ACCESS" > "$TMP/status.json"
  STATUS=$(field '["status"]' < "$TMP/status.json")
  case "$STATUS" in ready|failed) break ;; esac
  sleep 1
done
cat "$TMP/status.json"
[ "$STATUS" = "ready" ] || fail "the Indonesian source did not reach ready"

say "chat: streamed answer, Indonesian query, citation validated"
# The server must have a provider configured, or every chat step below fails
# with an opaque 502. Checked here, with the fix named.
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/workspaces/$WS/chat" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' -d '{"message":""}')
[ "$CODE" != "502" ] || fail "the server has no model provider; restart it with LLM_BASE_URL=$STUB_URL/v1 (just smoke-server)"
# The stub answers with [1] (sent) and [9] (never sent). The server must resolve
# the first and drop the second.
curl -fsS -N -X POST "$BASE/workspaces/$WS/chat" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"message\":\"perangkat pusat yang menghubungkan setiap komputer pada topologi star\"}" \
  > "$TMP/stream.txt"
cat "$TMP/stream.txt"

python3 - "$TMP/stream.txt" <<'PY'
import json, sys

raw = open(sys.argv[1], encoding="utf-8").read()
events = {}
for block in raw.split("\n\n"):
    name, data = None, None
    for line in block.splitlines():
        if line.startswith("event:"):
            name = line[6:].strip()
        elif line.startswith("data:"):
            data = line[5:].strip()
    if name and data:
        events.setdefault(name, []).append(json.loads(data))

assert "token" in events, f"no token events: {list(events)}"
assert "citations" in events, f"no citations event: {list(events)}"
assert "usage" in events, f"no usage event: {list(events)}"
assert "done" in events, f"no done event: {list(events)}"
assert "error" not in events, f"unexpected error event: {events.get('error')}"

streamed = "".join(e["t"] for e in events["token"])
assert "oxidative" in streamed or "ATP" in streamed, streamed

citations = events["citations"][0]["citations"]
assert len(citations) == 1, f"the fabricated label must be dropped, got {citations}"
assert citations[0]["type"] == "chunk"
# A pasted-text source has no page, so the citation carries null rather than an
# invented page number. A PDF source carries its real page.
assert citations[0]["page"] is None, citations[0]

spent = events["usage"][0]["credits_spent"]
remaining = events["usage"][0]["credits_remaining"]
assert spent == 1, spent
print(f"streamed {len(events['token'])} token events, {len(citations)} citation, "
      f"spent {spent}, {remaining} left")
PY

say "no rejected marker ever reached the streamed text"
# The invariant is about what the user sees WHILE the answer arrives, not only
# what is stored: the token loop holds back a partial marker and drops an unknown
# one before emitting it. Asserted on the token events themselves.
python3 - "$TMP/stream.txt" <<'PYEOF'
import json, sys

raw = open(sys.argv[1], encoding="utf-8").read()
streamed = ""
for block in raw.split("\n\n"):
    if block.startswith("event: token"):
        streamed += json.loads(block.split("data:", 1)[1].strip())["t"]

assert "[9]" not in streamed, f"a rejected marker reached the streamed text: {streamed!r}"
assert "[1]" in streamed, f"a sent label must still reach the user: {streamed!r}"
print("streamed text:", repr(streamed[:80]))
PYEOF

say "the fabricated label is gone from the stored text"
curl -fsS "$BASE/workspaces/$WS/messages" -H "authorization: Bearer $ACCESS" > "$TMP/thread.json"
python3 - "$TMP/thread.json" <<'PY'
import json, sys

messages = json.load(open(sys.argv[1]))["messages"]
assert len(messages) == 2, f"expected a user turn and an assistant turn, got {len(messages)}"
assert messages[0]["role"] == "user"
assert messages[1]["role"] == "assistant"
assert "[9]" not in messages[1]["content"], messages[1]["content"]
assert "[1]" in messages[1]["content"], messages[1]["content"]
assert len(messages[1]["citations"]) == 1, messages[1]["citations"]
print("stored:", repr(messages[1]["content"][:70]))
PY

say "the labelled prompt reached the model"
# The stub writes every request body it served, so the smoke run can assert the
# prompt carried the label block. A response-only check cannot see this, and it is
# the assertion a wrong composition would slip past.
python3 - "$STUB_LOG" <<'PY'
import json, sys, pathlib

path = pathlib.Path(sys.argv[1])
if not path.exists():
    print("the stub did not record request bodies; skipping the prompt assertion")
    raise SystemExit(0)

bodies = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
assert bodies, "the stub recorded no requests"
system = bodies[-1]["messages"][0]["content"]
assert system.startswith("You are "), "the composed prompt must begin with the base module"
assert "[1]" in system, "the prompt must label the chunks"
assert "topologi star" in system or "Jaringan" in system, "the Indonesian text must be in the prompt"
assert "{{" not in system, "no prompt variable may survive composition"
print("prompt carried the label block and the Indonesian source")
PY

say "the ledger and the reported balance agree"
if [ -n "$PSQL" ]; then
  set -a; . ./.env; set +a
  # Scoped to this account: the table holds every smoke run's user, so an
  # unscoped sum would compare this user's balance against everyone's.
  LEDGER=$("$PSQL" "$DATABASE_URL" -tAc "select coalesce(sum(l.delta),0) from credit_ledger l join users u on u.id = l.user_id where u.email = '$EMAIL'")
  REMAINING=$(python3 -c "
import json
raw = open('$TMP/stream.txt', encoding='utf-8').read()
for block in raw.split('\n\n'):
    if block.startswith('event: usage'):
        print(json.loads(block.split('data:')[1].strip())['credits_remaining'])
        break
")
  [ "$LEDGER" = "$REMAINING" ] || fail "the ledger ($LEDGER) and the reported balance ($REMAINING) disagree"
  printf 'ledger %s == reported %s\n' "$LEDGER" "$REMAINING"
else
  echo "no psql on PATH; skipped the ledger cross-check"
fi

say "a second device sees the thread"
curl -fsS -X POST "$BASE/auth/login" -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke phone\",\"platform\":\"ios\"}}" \
  > "$TMP/login.json"
ACCESS2=$(field '["access_token"]' < "$TMP/login.json")
curl -fsS "$BASE/workspaces/$WS/messages" -H "authorization: Bearer $ACCESS2" > "$TMP/thread2.json"
python3 - "$TMP/thread2.json" <<'PY'
import json, sys
messages = json.load(open(sys.argv[1]))["messages"]
assert len(messages) == 2, messages
print("second device sees", len(messages), "turns")
PY

say "cross-account isolation"
# The intruder's email is verified first, on purpose: an unverified account is
# refused by the A43 check before ownership is even looked at, so leaving it
# unverified would make this assert 403 and never test the ownership rule.
INTRUDER="intruder-$(date +%s)@example.test"
curl -fsS -X POST "$BASE/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"$INTRUDER\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Intruder\",\"platform\":\"web\"}}" \
  > "$TMP/other.json"
OTHER=$(field '["access_token"]' < "$TMP/other.json")
IMAIL=$(ls -t "$MAIL_DIR"/*.eml | head -1)
ITOKEN=$(grep -o 'token=[A-Za-z0-9_-]*' "$IMAIL" | head -1 | cut -d= -f2)
curl -fsS -X POST "$BASE/auth/verify-email" -H 'content-type: application/json' \
  -d "{\"token\":\"$ITOKEN\"}" > /dev/null

# POST is the chat route; the message list is a GET, so each is probed with its
# own method.
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/workspaces/$WS/chat" \
  -H "authorization: Bearer $OTHER" -H 'content-type: application/json' -d '{"message":"hi"}')
[ "$CODE" = "404" ] || fail "another account could chat in this workspace: $CODE"

CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/workspaces/$WS/messages" \
  -H "authorization: Bearer $OTHER")
[ "$CODE" = "404" ] || fail "the message list leaked to another account: $CODE"

say "PASS"
