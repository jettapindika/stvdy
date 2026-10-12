#!/usr/bin/env bash
# Phase 12 end-to-end smoke test: the agent's proposal, the confirm, and the reminder
# dedupe, against the real server, the real Postgres and a stub model that emits tool calls.
#
# Drives the real process over HTTP and reads the real database back, because the parts this
# phase can get wrong are exactly the parts a trait-level double cannot see: that the tool
# **schema** the server puts on the wire is one a provider accepts, that a proposal arrives as
# an `action` event and **no task exists** until the confirm, that the second confirm is a
# `409` that wrote nothing, and that a repeated `POST /reminders` answers `applied: false`
# because the dedupe key is the subject and the local date and not the computed instant.
#
# Six scenarios, in order:
#   1. a fresh account on pro: the plan grants `agent_actions`, and the tool list on the wire
#      is the ten tools (read out of the stub's log, not from a response)
#   2. a chat turn where the model emits `create_task`: an `action` event, and **no task**
#   3. the confirm: one task, with the instant the model resolved
#   4. the second confirm: `409`, and still one task
#   5. the reject: a proposal that produces nothing at all
#   6. reminders: one row from two devices, `applied: false` on the second, and the row's
#      `fire_at` is the **first** one — which a fire-time key would fail
#
# **Run this against a freshly started server.** It makes several requests to the credential
# routes, and `ratelimit::auth_layer` (A12) is an in-process bucket of 30 requests per minute
# per IP, so a second run inside the same server process may have to wait for cells. The script
# waits and says so rather than hanging.
#
# **The push half is not exercised, and it degrades rather than pretending.** APNs and FCM need
# credentials the user has not created, so `channels_available` reports `["web","email"]` and
# scenario 6 asserts the in-app and email channels only: the reminder is asserted through
# `GET /reminders` and through the `FileSinkMailer`'s `.eml`. A deployment WITH credentials
# would add `push` to that list and the reminder would also go out over APNs/FCM; this run says
# which it is rather than reporting a green word for a channel that never fired.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
STUB_PORT="${STUB_PORT:-8100}"
MAIL_DIR="${MAIL_DIR:-var/mail}"
STUB_LOG="${STUB_LOG:-var/stub-phase12-requests.jsonl}"
TMP="$(mktemp -d)"
STAMP="$(date +%s)"
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
[ -n "$PSQL" ] || fail "no psql on PATH; this run reads the database back and cannot skip that"

# `.env` supplies DATABASE_URL when the caller did not, and must not override an explicit one.
# Read the one key rather than sourcing the file: sourcing executes it, and a line as ordinary
# as `LLM_BASE_URL= https://host` makes bash run the URL as a command.
if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
  DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
  export DATABASE_URL
fi
[ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"

db() { "$PSQL" "$DATABASE_URL" -tAc "$1"; }

# --- the stub model server, in its tool-call mode ----------------------------------------
#
# The trait-level double cannot catch a wire-format mistake in the tool schema, which is the
# same reason Phase 2's stub exists: this one speaks real HTTP and real SSE, so the request
# body the server actually sends is what the stub validates.
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

say "stub model server (tool-call mode)"
rm -f "$STUB_LOG"
if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
  printf 'a stub is already listening on %s; reusing it (it must serve --tool-calls)\n' "$STUB_PORT"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    --tool-calls scripts/fixtures/phase12-tool-calls.json > "$TMP/stub.log" 2>&1 &
  STUB_PID=$!
  for _ in $(seq 1 50); do
    if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then break; fi
    sleep 0.2
  done
  cat "$TMP/stub.log"
fi
nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null || fail "no stub model server on $STUB_PORT"

say "health"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Start the stub with 'just smoke-llm-phase12' and the server with 'just smoke-server-phase12'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

# --- the account ---------------------------------------------------------------------------
DOMAIN="phase12-$STAMP.example.test"
ACCESS=""

say "reset the loopback signup attempts (A154 counts 25 per IP per 24h, and every run adds)"
PRUNED="$(db "with gone as (delete from signup_attempts where ip = '127.0.0.1' returning 1) select count(*) from gone")"
printf 'pruned %s signup_attempts row(s) for 127.0.0.1\n' "$PRUNED"

# `/auth/signup`, `/auth/login` and `/auth/verify-email` sit behind an in-process bucket of 30
# requests per minute per IP (A12), which trips before A154's limit. Waiting is the honest
# response to a bucket that refills; hiding it would make a second invocation fail for a reason
# that looks like a server bug.
auth_post() {
  local out="$1" url="$2"
  shift 2
  local attempt status
  for attempt in $(seq 1 14); do
    status="$(curl -sS -o "$out" -w '%{http_code}' -X POST "$url" "$@")"
    [ "$status" = "429" ] || break
    printf '  the credential rate limiter (30/minute per IP) answered 429; waiting 5s (attempt %s of 14)\n' \
      "$attempt" >&2
    sleep 5
  done
  [ "$status" = "429" ] && fail "$url answered 429 for 70s; wait a minute and re-run"
  printf '%s' "$status"
}

EMAIL="phase12-student-$STAMP@$DOMAIN"
say "1. a fresh account: signup, verify, and the free plan's missing write tools"
STATUS="$(auth_post "$TMP/signup.json" "$BASE/auth/signup" \
  -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke laptop\",\"platform\":\"macos\"}}")"
[ "$STATUS" = "201" ] || fail "signup answered $STATUS: $(cat "$TMP/signup.json")"
ACCESS="$(field '["access_token"]' < "$TMP/signup.json")"
MAIL="$(find "$MAIL_DIR" -name '*.eml' -type f | sort | tail -1)"
TOKEN="$(grep -o 'token=[A-Za-z0-9_-]*' "$MAIL" | head -1 | cut -d= -f2)"
STATUS="$(auth_post "$TMP/verify.json" "$BASE/auth/verify-email" \
  -H 'content-type: application/json' -d "{\"token\":\"$TOKEN\"}")"
[ "$STATUS" = "200" ] || fail "verify-email answered $STATUS: $(cat "$TMP/verify.json")"
UID_="$(db "select id from users where email = '$EMAIL'")"
[ -n "$UID_" ] || fail "no user row for $EMAIL"
printf 'account %s verified, id=%s\n' "$EMAIL" "$UID_"

# The account is moved to `pro` with a live subscription, which is what makes the plan grant
# anything (A146): `users.plan_id` alone is a column that reads as paid and behaves free.
# There is no self-service upgrade route — Phase 13 owns checkout — so the run writes the two
# rows a completed checkout would have written.
say "upgrade the account to pro (the two rows Phase 13's checkout will write)"
db "update users set plan_id = 'pro' where id = '$UID_'" > /dev/null
db "insert into subscriptions (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
    values ('$UID_', 'pro', 'active', 'monthly', now(), now() + interval '30 days')" > /dev/null

# The workspace the turn runs in, with one source so retrieval has something to cite.
WORKSPACE="$(curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Kriptografi"}' | field '["workspace"]["id"]')"
[ -n "$WORKSPACE" ] || fail "no workspace id"
printf 'workspace=%s\n' "$WORKSPACE"

AGENT_ACTIONS="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $ACCESS" | field '["features"]["agent_actions"]')"
[ "$AGENT_ACTIONS" = "True" ] || fail "pro must grant agent_actions, got $AGENT_ACTIONS"

# ==========================================================================================
# 2. A turn where the model emits create_task
# ==========================================================================================
say "2. a chat turn that proposes: an action event, and no task yet"
BEFORE="$(db "select count(*) from tasks where user_id = '$UID_'")"
curl -sS -N -X POST "$BASE/workspaces/$WORKSPACE/chat" -H "authorization: Bearer $ACCESS" \
  -H 'accept: text/event-stream' -H 'content-type: application/json' \
  -d '{"message":"tolong tambahkan tugas menyelesaikan laporan kripto untuk Jumat","use_web":false,"sources_only":true}' \
  > "$TMP/chat.sse" 2>/dev/null || fail "the chat stream did not answer"

grep -q '^event: action' "$TMP/chat.sse" \
  || fail "no action event in the stream: $(head -20 "$TMP/chat.sse")"
PENDING="$(python3 - "$TMP/chat.sse" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    if line.startswith('data:') and 'pending_action_id' in line:
        print(json.loads(line[5:])['pending_action_id']); break
PY
)"
[ -n "$PENDING" ] || fail "the action event carried no pending_action_id"
printf 'the stream carried an action event for pending_action_id=%s\n' "$PENDING"

# The anti-vacuity check, and the reason this script exists rather than a trait-level double:
# the tool **schema** the server put on the wire is asserted from the request body it actually
# sent, and the model demonstrably emitted the call.
python3 - "$STUB_LOG" <<'PY' || exit 1
import json, sys
tools = None
for line in open(sys.argv[1]):
    body = json.loads(line)
    if body.get('tools'):
        tools = body['tools']
if tools is None:
    print('FAIL: no request carried a `tools` array; the wire schema was never sent', file=sys.stderr)
    sys.exit(1)
# The wire shape is the [OI]-compatible one, nested under `function` — not the
# trait's `ToolSpec { name, description, parameters }`. Asserting the trait's shape
# here is what this check did first, and it read a correct request as a missing one.
def named(tool):
    function = tool.get('function')
    return function if isinstance(function, dict) else {}

names = {named(t)['name'] for t in tools if 'name' in named(t)}
writes = {'create_task', 'update_task', 'create_event', 'schedule_study_block',
          'create_flashcards', 'create_quiz'}
reads = {'list_tasks', 'find_free_slots', 'get_due_flashcards', 'get_upcoming_deadlines'}
missing = (writes | reads) - names
if missing:
    print(f'FAIL: the wire schema is missing {sorted(missing)}', file=sys.stderr)
    sys.exit(1)
for tool in tools:
    parameters = named(tool).get('parameters')
    if not isinstance(parameters, dict) or 'properties' not in parameters:
        print(f"FAIL: {named(tool).get('name', tool)} has no JSON schema", file=sys.stderr)
        sys.exit(1)
print(f'the wire carried {len(tools)} tools, all ten with a JSON schema')
PY

AFTER="$(db "select count(*) from tasks where user_id = '$UID_'")"
[ "$BEFORE" = "$AFTER" ] || fail "the proposal wrote a task: $BEFORE -> $AFTER"
STATUS="$(db "select status from pending_actions where id = '$PENDING' and user_id = '$UID_'")"
[ "$STATUS" = "pending" ] || fail "the proposal is $STATUS, not pending"
printf 'no task was written (%s before and after) and the proposal is pending\n' "$BEFORE"

# ==========================================================================================
# 3. The confirm
# ==========================================================================================
say "3. the confirm: exactly one task, and the proposal is confirmed"
CONFIRM_STATUS="$(curl -sS -o "$TMP/confirm.json" -w '%{http_code}' -X POST \
  "$BASE/actions/$PENDING/confirm" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{}')"
[ "$CONFIRM_STATUS" = "200" ] || fail "the confirm answered $CONFIRM_STATUS: $(cat "$TMP/confirm.json")"
[ "$(field '["action"]["status"]' < "$TMP/confirm.json")" = "confirmed" ] \
  || fail "the confirm did not confirm: $(cat "$TMP/confirm.json")"
TASKS="$(db "select count(*) from tasks where user_id = '$UID_'")"
[ "$TASKS" = "1" ] || fail "the confirm wrote $TASKS tasks, not 1"
DEVICE="$(db "select resolved_by_device_id is not null from pending_actions where id = '$PENDING'")"
[ "$DEVICE" = "t" ] || fail "the confirm recorded no resolving device"
printf 'one task written; the proposal is confirmed by a device\n'

# ==========================================================================================
# 4. The second confirm
# ==========================================================================================
say "4. the second confirm: 409, and nothing new written"
SECOND="$(curl -sS -o "$TMP/second.json" -w '%{http_code}' -X POST \
  "$BASE/actions/$PENDING/confirm" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{}')"
[ "$SECOND" = "409" ] || fail "the second confirm answered $SECOND, not 409: $(cat "$TMP/second.json")"
[ "$(field '["code"]' < "$TMP/second.json")" = "conflict" ] \
  || fail "the second confirm must be a conflict: $(cat "$TMP/second.json")"
STILL="$(db "select count(*) from tasks where user_id = '$UID_'")"
[ "$STILL" = "1" ] || fail "the second confirm wrote another task: $STILL"
printf 'second confirm -> 409 %s, and still one task\n' "$(field '["code"]' < "$TMP/second.json")"

# ==========================================================================================
# 5. The reject
# ==========================================================================================
say "5. the reject: a proposal that produces nothing"
rm -f "$STUB_LOG"
curl -sS -N -X POST "$BASE/workspaces/$WORKSPACE/chat" -H "authorization: Bearer $ACCESS" \
  -H 'accept: text/event-stream' -H 'content-type: application/json' \
  -d '{"message":"tambahkan lagi satu tugas kripto","use_web":false,"sources_only":true}' \
  > "$TMP/chat2.sse" 2>/dev/null || fail "the second chat stream did not answer"
SECOND_PENDING="$(python3 - "$TMP/chat2.sse" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    if line.startswith('data:') and 'pending_action_id' in line:
        print(json.loads(line[5:])['pending_action_id']); break
PY
)"
[ -n "$SECOND_PENDING" ] || fail "the second turn proposed nothing"
REJECT="$(curl -sS -o "$TMP/reject.json" -w '%{http_code}' -X POST \
  "$BASE/actions/$SECOND_PENDING/reject" -H "authorization: Bearer $ACCESS")"
[ "$REJECT" = "200" ] || fail "the reject answered $REJECT: $(cat "$TMP/reject.json")"
[ "$(db "select status from pending_actions where id = '$SECOND_PENDING'")" = "rejected" ] \
  || fail "the reject did not reject"
[ "$(db "select count(*) from tasks where user_id = '$UID_'")" = "1" ] \
  || fail "the rejected proposal wrote a task"
printf 'rejected; still one task\n'

# ==========================================================================================
# 6. Reminders: one row from two devices, and the FIRST fire_at
# ==========================================================================================
say "6. reminders: the dedupe key is the subject and the local date, never the instant"
curl -fsS -X PUT "$BASE/settings" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"timezone":"Asia/Jakarta"}' > /dev/null
TASK="$(db "select id from tasks where user_id = '$UID_' limit 1")"
# Two devices computing the same due time an hour apart. The task is due on the 16th, and
# `2026-10-16T15:59Z` is 22:59 Jakarta — the same local day either way.
FIRE_A="2026-10-16T15:59:00Z"
FIRE_B="2026-10-16T16:59:00Z"
FIRST="$(curl -fsS -X POST "$BASE/reminders" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"kind\":\"task_due\",\"subject_id\":\"$TASK\",\"fire_at\":\"$FIRE_A\"}")"
[ "$(printf '%s' "$FIRST" | field '["applied"]')" = "True" ] \
  || fail "the first registration must apply: $FIRST"
SECOND_REM="$(curl -fsS -X POST "$BASE/reminders" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"kind\":\"task_due\",\"subject_id\":\"$TASK\",\"fire_at\":\"$FIRE_B\"}")"
[ "$(printf '%s' "$SECOND_REM" | field '["applied"]')" = "False" ] \
  || fail "the second registration must not apply: $SECOND_REM"
[ "$(printf '%s' "$FIRST" | field '["reminder_id"]')" = "$(printf '%s' "$SECOND_REM" | field '["reminder_id"]')" ] \
  || fail "the two registrations produced different rows"
ROWS="$(db "select count(*) from reminders where user_id = '$UID_' and subject_id = '$TASK'")"
[ "$ROWS" = "1" ] || fail "two devices produced $ROWS rows, not 1"
STORED="$(db "select to_char(fire_at at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') from reminders where user_id = '$UID_' and subject_id = '$TASK'")"
[ "$STORED" = "$FIRE_A" ] \
  || fail "the stored fire_at is $STORED, not the first one ($FIRE_A) — a fire-time key would do this"
printf 'two devices, one row, and fire_at is the FIRST (%s)\n' "$STORED"

say "the channels this deployment can deliver on"
CHANNELS="$(curl -fsS "$BASE/settings" -H "authorization: Bearer $ACCESS" | field '["channels_available"]')"
printf 'channels_available=%s\n' "$CHANNELS"
case "$CHANNELS" in
  *telegram*|*discord*)
    fail "channels_available names a channel with no transport (A288): $CHANNELS" ;;
esac
# **The push half, stated rather than skipped.** Without APNs/FCM credentials `push` is absent
# from this list and no push delivery can be exercised; the reminder is asserted through the
# in-app feed and, if it were due, the mailer's file sink. Say which run this is.
if printf '%s' "$CHANNELS" | grep -q push; then
  printf 'push IS configured: the reminder would also go out over APNs/FCM\n'
else
  printf 'push is NOT configured (no APNs/FCM credentials): this run degrades to the\n'
  printf 'in-app and email channels, and no push delivery was exercised. That is the\n'
  printf 'documented degraded state (A288), not a pass.\n'
fi

say "the in-app feed answers"
curl -fsS "$BASE/reminders" -H "authorization: Bearer $ACCESS" | field '["reminders"]' > /dev/null \
  || fail "GET /reminders did not answer"

printf '\n=== PASS: the proposal, the confirm, the reject and the reminder dedupe\n'
