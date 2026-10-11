#!/usr/bin/env bash
# Phase 4 end-to-end smoke test: deck generation, chunk-linked cards, and the
# derived SM-2 schedule.
#
# Drives the real server over HTTP against the local Postgres, with a STUB model
# server that speaks real HTTP (scripts/stub-llm.py) and answers with a fixed
# cards JSON. The stub is deliberate: the Rust suite replaces the provider at the
# trait boundary, so it cannot catch a wire-format mistake in the generation
# request. This run exercises the actual request body and the actual reply parse.
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
EMAIL="phase4-$(date +%s)@example.test"
PASSWORD="correct horse battery"

# The stub answers with this string, so the deck's card count and text are fixed.
# The label `1` resolves against the one chunk the prompt sends.
CARDS_JSON='{"cards":[{"front":"Apa fungsi three-way handshake?","back":"Membuka koneksi TCP.","label":1}]}'

say() { printf '\n=== %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
field() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }
uuid() { python3 -c 'import uuid; print(uuid.uuid4())'; }

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
  # Already listening: reuse it. It must have been started with the cards answer
  # below, or generation will not parse.
  echo "a stub is already listening on $STUB_PORT; reusing it"
  echo "it must have been started with --answer '$CARDS_JSON'"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    --answer "$CARDS_JSON" > "$TMP/stub.log" 2>&1 &
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
# Newest first by name: the dev sink writes `<unix-millis>-<local part>.eml`.
MAIL=$(find "$MAIL_DIR" -name '*.eml' -type f | sort | tail -1)
TOKEN=$(grep -o 'token=[A-Za-z0-9_-]*' "$MAIL" | head -1 | cut -d= -f2)
curl -fsS -X POST "$BASE/auth/verify-email" -H 'content-type: application/json' \
  -d "{\"token\":\"$TOKEN\"}" > /dev/null

say "workspace with an Indonesian source"
curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Jaringan Komputer"}' > "$TMP/ws.json"
WS=$(field '["workspace"]["id"]' < "$TMP/ws.json")

DOKUMEN="Three-way handshake adalah proses membuka koneksi TCP antara klien dan server. Klien mengirim paket SYN, server membalas dengan SYN-ACK, lalu klien mengirim ACK. Setelah ketiga paket ini bertukar, koneksi TCP siap digunakan untuk mengirim data."
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

say "create a deck from the source"
# The server must have a provider configured, or the job fails for good (the deck
# reaches `failed` and the poll below names the reason). The stub is it.
curl -fsS -X POST "$BASE/workspaces/$WS/decks" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"title\":\"Jaringan Komputer\",\"source_ids\":[\"$SRC\"],\"count\":1}" \
  > "$TMP/deck.json"
DECK=$(field '["deck"]["id"]' < "$TMP/deck.json")
[ "$(field '["deck"]["status"]' < "$TMP/deck.json")" = "generating" ] \
  || fail "a fresh deck must be generating"

say "poll until the deck settles"
DECK_STATUS=unknown
for _ in $(seq 1 120); do
  curl -fsS "$BASE/decks/$DECK" -H "authorization: Bearer $ACCESS" > "$TMP/deck-state.json"
  DECK_STATUS=$(field '["deck"]["status"]' < "$TMP/deck-state.json")
  case "$DECK_STATUS" in ready|failed) break ;; esac
  sleep 1
done
cat "$TMP/deck-state.json"
[ "$DECK_STATUS" = "ready" ] \
  || fail "the deck did not reach ready (status $DECK_STATUS): $(field '["deck"]["error"]' < "$TMP/deck-state.json"). Is the server pointed at the stub (LLM_BASE_URL=$STUB_URL/v1)?"
[ "$(field '["deck"]["card_count"]' < "$TMP/deck-state.json")" = "1" ] \
  || fail "the stub returned one card, the deck does not have exactly one"
CARD=$(field '["cards"][0]["id"]' < "$TMP/deck-state.json")
FRONT=$(field '["cards"][0]["front"]' < "$TMP/deck-state.json")
CHUNK=$(field '["cards"][0]["chunk_id"]' < "$TMP/deck-state.json")
case "$FRONT" in *three-way\ handshake*) ;; *) fail "the card is not the stub's text: $FRONT" ;; esac
[ "$CHUNK" != "None" ] && [ -n "$CHUNK" ] \
  || fail "the card was not linked to the chunk it cited: $CHUNK"

say "the cited chunk is in the study queue"
curl -fsS "$BASE/decks/$DECK/due?limit=10" -H "authorization: Bearer $ACCESS" > "$TMP/due.json"
python3 - "$TMP/due.json" "$CARD" <<'PY'
import json, sys

due = json.load(open(sys.argv[1]))
ids = [card["id"] for card in due["cards"]]
assert sys.argv[2] in ids, f"a fresh card must be due, got {ids}"
print(f"{len(ids)} due card(s); now={due['now']}")
PY

say "review the card twice, out of order"
# Two reviews of one card, the LATER one sent first: the server replays the whole
# history in `(reviewed_at, id)` order, so arrival order must not matter. `hard`
# then `good` moves both ease and the interval, so a replay that ignored one row
# would disagree visibly.
EARLIER="2026-01-01T00:00:00Z"
LATER="2026-01-03T00:00:00Z"
curl -fsS -X POST "$BASE/flashcards/$CARD/review" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"id\":\"$(uuid)\",\"rating\":\"good\",\"reviewed_at\":\"$LATER\"}" > "$TMP/review-1.json"
[ "$(field '["applied"]' < "$TMP/review-1.json")" = "True" ] || fail "the first review was not applied"
curl -fsS -X POST "$BASE/flashcards/$CARD/review" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"id\":\"$(uuid)\",\"rating\":\"hard\",\"reviewed_at\":\"$EARLIER\"}" > "$TMP/review-2.json"
[ "$(field '["applied"]' < "$TMP/review-2.json")" = "True" ] || fail "the second review was not applied"

say "the server's schedule and a local replay of the same two rows agree"
curl -fsS "$BASE/decks/$DECK" -H "authorization: Bearer $ACCESS" > "$TMP/after.json"
python3 - "$TMP/after.json" "$EARLIER" "$LATER" <<'PY'
import json, sys
from datetime import datetime, timedelta

after = json.load(open(sys.argv[1]))["cards"][0]
earlier = datetime.fromisoformat(sys.argv[2].replace("Z", "+00:00"))
later = datetime.fromisoformat(sys.argv[3].replace("Z", "+00:00"))

# `crates/core/src/sm2.rs`, reproduced here. The two rows, in (reviewed_at, id)
# order: the earlier `hard`, then the later `good`.
QUALITY = {"again": 0, "hard": 3, "good": 4, "easy": 5}
reviews = [("hard", earlier), ("good", later)]

ease, interval, repetitions, due = 2.5, 0, 0, None
for rating, reviewed_at in reviews:
    q = QUALITY[rating]
    if q < 3:
        repetitions = 0
        interval = 1
    else:
        repetitions += 1
        if repetitions == 1:
            interval = 1
        elif repetitions == 2:
            interval = 6
        else:
            interval = max(round(interval * ease), 1)
    ease = max(ease + 0.1 - (5 - q) * (0.08 + (5 - q) * 0.02), 1.3)
    due = reviewed_at + timedelta(days=interval)

server_due = datetime.fromisoformat(after["due_at"].replace("Z", "+00:00"))
print(f"server: ease={after['ease']} interval_days={after['interval_days']} due_at={after['due_at']}")
print(f"replay: ease={ease} interval_days={interval} due_at={due.isoformat()}")

assert abs(after["ease"] - ease) < 1e-4, (after["ease"], ease)
assert after["interval_days"] == interval, (after["interval_days"], interval)
assert server_due == due, (server_due, due)
# The schedule is dated from the LATER review, not the last one to arrive.
assert due == later + timedelta(days=interval), due
print("the schedules agree, and the due date is dated from the later review")
PY

say "the generation's usage row names the action and the prompt version"
# The stub's reply may or may not carry a `usage` block, so token counts and cost
# are not asserted: a `None` cost is the honest record when the provider omits one
# (A41), and `record_usage` is a no-op when the row cannot be found. The fields the
# job controls are the action and the prompt version.
if [ -n "$PSQL" ]; then
  set -a
  # shellcheck source=/dev/null
  . ./.env
  set +a
  ROW=$("$PSQL" "$DATABASE_URL" -tAc "select action || '|' || coalesce(prompt_version, '') from usage_events where user_id = (select id from users where email = '$EMAIL') and action = 'generate_flashcards' order by created_at desc limit 1")
  [ -n "$ROW" ] || fail "the generation left no usage row for generate_flashcards"
  [ "$ROW" = "generate_flashcards|flashcards.v1" ] \
    || fail "the usage row does not name the action and prompt version: $ROW"
  printf 'usage row: %s\n' "$ROW"
else
  echo "no psql on PATH; skipped the usage-row assertion"
fi

say "the generation called the model with the composed prompt"
# The stub writes every request body it served, so the smoke run can assert the
# request was non-streaming (the shape `generate_flashcards` uses) and carried the
# label block. A response-only check cannot see this.
python3 - "$STUB_LOG" <<'PY'
import json, pathlib, sys

path = pathlib.Path(sys.argv[1])
if not path.exists():
    print("the stub did not record request bodies; skipping the prompt assertion")
    raise SystemExit(0)

bodies = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
assert bodies, "the stub recorded no requests"
body = bodies[-1]
assert body.get("stream") is False, "generation must not stream"
assert body.get("response_format", {}).get("type") == "json_object", "generation must ask for JSON"
system = body["messages"][0]["content"]
assert "[1]" in system, "the prompt must label the chunks"
assert "three-way handshake" in system, "the Indonesian source must be in the prompt"
assert "{{" not in system, "no prompt variable may survive composition"
print("the generation request was non-streaming JSON and carried the labelled Indonesian source")
PY

say "PASS"
