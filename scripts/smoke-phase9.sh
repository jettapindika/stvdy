#!/usr/bin/env bash
# Phase 9 end-to-end smoke test: tasks, quick capture, break-down, events, sessions and
# settings, against the real server, the real Postgres and a stub model.
#
# Drives the real process over HTTP and reads the real database back, because the parts
# this phase can get wrong are exactly the parts a trait-level double cannot see: that a
# quick capture **spends two credits and writes no task**, that the confirm is what writes
# one, that a break-down saves nothing until the student accepts it, that a session's bound
# is compared against the *device's* own clock rather than the server's, and that a
# Jakarta deadline survives the round trip instead of landing on the previous day. The Rust
# suites assert all of these too — this run is the one that proves the *wired process* does,
# and it prints the numbers it asserted so the output is evidence rather than a green word.
#
# Nine scenarios, in order:
#   1. a fresh account: the free plan refuses both generative actions, by name
#   2. the settings round trip: a partial write, wrapping quiet hours, a refused channel
#   3. an Indonesian capture: two credits spent, a Jakarta evening, and **no task**
#   4. the confirm: the draft becomes one row, with the instant the model resolved
#   5. the break-down: a proposal and no subtasks, then two accepted subtasks in order
#   6. an event: the overlap window and the calendar-owned column
#   7. sessions: the device's clock, the bound, the replay, and `GET /activity`
#   8. deleting a parent: the whole subtree, and a change_log row per child
#   9. the prompts the model actually received, read out of the stub's log
#
# **Run this against a freshly started server.** Like Phase 7 it makes several requests to
# the credential routes, and `ratelimit::auth_layer` (A12) is an in-process bucket of 30
# requests per minute per IP, so a second run inside the same server process may have to
# wait for cells. The script waits and says so rather than hanging.
#
# The stub must be started with `scripts/fixtures/phase9-answers.json`: a capture and a
# break-down are strict-JSON generators, so the Phase 2 default (a prose answer) would fail
# both. `just smoke-llm-phase9` does that.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
# The port the SERVER was started with (LLM_BASE_URL), not a private one: the server is
# what talks to the stub, so the stub this script watches must be the stub the server
# calls. Start the server with
#   LLM_BASE_URL=http://127.0.0.1:8100/v1
# and leave this at its default, or set both to the same value.
STUB_PORT="${STUB_PORT:-8100}"
# This must match the SERVER's `MAIL_DIR`, because the server is what writes the
# verification mail: the script only reads the newest `.eml` from it.
MAIL_DIR="${MAIL_DIR:-var/mail}"
PLANS="${PLANS:-config/plans.toml}"
# Where the stub appends every request body. The last scenario reads it, because the
# prompt the model received is not visible in any response.
STUB_LOG="${STUB_LOG:-var/stub-phase9-requests.jsonl}"
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

# --- the database, and why it is read rather than sourced -------------------------------
#
# `.env` supplies DATABASE_URL when the caller did not, and must not override an explicit
# one: a run against a scratch database would otherwise be answered from the shared dev
# database and report a missing row that is present.
#
# Read the one key rather than sourcing the file. Sourcing executes it, and a line as
# ordinary as `LLM_BASE_URL= https://host` (a space after `=`) makes bash run the URL as a
# command: under `set -e` that aborts the run, and under `set +e` it silently empties the
# variable. This script needs one value, so it reads one value.
if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
  DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
  export DATABASE_URL
fi
[ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"

db() { "$PSQL" "$DATABASE_URL" -tAc "$1"; }

# --- the plan matrix, quoted from the file the server charges from ------------------------
#
# Every number asserted below is read from `config/plans.toml` rather than written here, so
# a retuned config moves this run with it. A literal in this script would make the run pass
# against a server whose config disagrees with it — which is the one failure this whole
# scenario exists to catch.
say "the plan matrix, from $PLANS"
[ -f "$PLANS" ] || fail "no plan config at $PLANS"
python3 - "$PLANS" > "$TMP/matrix.txt" <<'PY'
import sys, tomllib
plans = tomllib.load(open(sys.argv[1], "rb"))
credits = plans["credits"]
print("credits: " + " ".join(f"{k}={v}" for k, v in sorted(credits.items())))
for name in ("free", "pro", "power"):
    p = plans["plans"][name]
    print(
        f"{name}: quick_capture={str(p['quick_capture']).lower()}"
        f" task_ai_breakdown={str(p['task_ai_breakdown']).lower()}"
        f" reminder_channels={','.join(p['reminder_channels'])}"
        f" credits_per_month={p['credits_per_month']}"
    )
PY
cat "$TMP/matrix.txt"
BREAKDOWN_COST="$(python3 -c "
import sys, tomllib
print(tomllib.load(open('$PLANS','rb'))['credits']['task_breakdown'])")"
[ -n "$BREAKDOWN_COST" ] || fail "could not read [credits] task_breakdown from $PLANS"
# A190: quick capture is priced as `task_breakdown`, because the matrix has no
# `quick_capture` key. This run asserts the *absence* of that key as well, so a later phase
# that adds one gets a visible failure here rather than a silent price change.
if python3 -c "
import tomllib, sys
sys.exit(0 if 'quick_capture' in tomllib.load(open('$PLANS','rb'))['credits'] else 1)"; then
  fail "config/plans.toml now has a quick_capture credit key; A190's price no longer applies"
fi
m() { python3 -c "
import sys
key = sys.argv[1]
for line in open('$TMP/matrix.txt'):
    if line.startswith('free: ') and key in line:
        print(line.split(key + '=')[1].split()[0]); break
" "$1"; }
FREE_CAPTURE="$(m quick_capture)"
FREE_BREAKDOWN="$(m task_ai_breakdown)"
PRO_TELEGRAM="$(python3 -c "
import tomllib
channels = tomllib.load(open('$PLANS','rb'))['plans']['pro']['reminder_channels']
print('yes' if 'telegram' in channels else 'no')")"
[ "$FREE_CAPTURE" = "false" ] || fail "the free plan grants quick_capture; scenario 1 asserts the refusal"
[ "$PRO_TELEGRAM" = "no" ] || fail "pro already grants telegram; scenario 2 asserts the refusal"

# --- the stub model server ---------------------------------------------------------------
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

say "stub model server"
rm -f "$STUB_LOG"
if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
  printf 'a stub is already listening on %s; reusing it (it must serve %s)\n' \
    "$STUB_PORT" "scripts/fixtures/phase9-answers.json"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    --answers-file scripts/fixtures/phase9-answers.json > "$TMP/stub.log" 2>&1 &
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
  || fail "no server at $BASE. Start the stub with 'just smoke-llm-phase9' and the server with 'just smoke-server-phase9'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

# --- the account --------------------------------------------------------------------------
#
# The per-IP limiter counts *every* attempt, including this run's own, and its window is
# 24h: without this the eleventh re-run of this script in a day would fail at signup and
# read like a server bug. This deletes only the loopback address's rows, which is what a
# developer's machine signs up from.
DOMAIN="phase9-$STAMP.example.test"
ACCESS=""

say "reset the loopback signup attempts (A154 counts 25 per IP per 24h, and every run here adds)"
PRUNED="$(db "with gone as (delete from signup_attempts where ip = '127.0.0.1' returning 1) select count(*) from gone")"
printf 'pruned %s signup_attempts row(s) for 127.0.0.1\n' "$PRUNED"

# A credential request, retried across the auth limiter's refill window.
#
# `/auth/signup`, `/auth/login` and `/auth/verify-email` sit behind an in-process bucket of
# 30 requests per minute per IP (`ratelimit::AUTH_PER_MINUTE`, A12) — a different control
# from A154's per-IP signup limit, and the one that trips first. Waiting is the honest
# response to a bucket that refills; hiding it would be a run that fails on its second
# invocation for a reason that looks like a server bug.
auth_post() { # $1 = output file, $2 = url, rest = curl arguments
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
  if [ "$status" = "429" ]; then
    printf 'FAIL: %s answered 429 for 70s. Either the auth bucket is still full (wait a minute\n' "$url" >&2
    printf '      and re-run) or A154 per-IP signup limit is in play: %s attempts are recorded\n' \
      "$(db "select count(*) from signup_attempts where ip = '127.0.0.1'")" >&2
    printf '      for 127.0.0.1 and the limit is 25 per 24h. %s\n' "$(cat "$out")" >&2
    exit 1
  fi
  printf '%s' "$status"
}

EMAIL="phase9-student-$STAMP@$DOMAIN"
say "1. a fresh account: signup, verify, and the free plan's two refusals"
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

# The matrix's own answer, not a literal in this script: free has `quick_capture = false`
# and `task_ai_breakdown = false`, so both generative routes are 403 `plan_required` and
# name the plan that grants them. The price is read from `config/plans.toml` above, so a
# retuned config moves this run with it.
#
# The break-down's id is a random uuid: the entitlement check runs **first** in both
# handlers, so a free account is refused before anything is looked up. That ordering is the
# point of the assertion — a refusal that depended on the row existing would be a refusal a
# paid account's malformed request could slip past.
for route in quick-capture breakdown; do
  if [ "$route" = "quick-capture" ]; then
    URL="$BASE/tasks/quick-capture"
    BODY='{"text":"selesaikan tugas kripto hari Jumat"}'
  else
    URL="$BASE/tasks/$(python3 -c 'import uuid; print(uuid.uuid4())')/breakdown"
    BODY='{}'
  fi
  STATUS="$(curl -sS -o "$TMP/free-$route.json" -w '%{http_code}' -X POST \
    "$URL" -H "authorization: Bearer $ACCESS" \
    -H 'content-type: application/json' -d "$BODY")"
  [ "$STATUS" = "403" ] || fail "a free account's $route must be 403, got $STATUS: $(cat "$TMP/free-$route.json")"
  [ "$(field '["code"]' < "$TMP/free-$route.json")" = "plan_required" ] \
    || fail "$route's refusal must be plan_required: $(cat "$TMP/free-$route.json")"
  [ "$(field '["min_plan"]' < "$TMP/free-$route.json")" = "pro" ] \
    || fail "$route must name the plan that grants it: $(cat "$TMP/free-$route.json")"
  printf 'free plan, %-13s -> %s %s min_plan=%s\n' "$route" "$STATUS" \
    "$(field '["code"]' < "$TMP/free-$route.json")" "$(field '["min_plan"]' < "$TMP/free-$route.json")"
done

# The account is moved to `pro` with a live subscription, which is what makes the plan grant
# anything (A146): `users.plan_id` alone is a column that reads as paid and behaves free.
# There is no self-service upgrade route in this phase — Phase 13 owns checkout — so the
# run writes the two rows a completed checkout would have written.
say "upgrade the account to pro (the two rows Phase 13's checkout will write)"
db "update users set plan_id = 'pro' where id = '$UID_'" > /dev/null
db "insert into subscriptions (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
    values ('$UID_', 'pro', 'active', 'monthly', now(), now() + interval '30 days')" > /dev/null
PLAN="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $ACCESS" | field '["plan"]')"
[ "$PLAN" = "pro" ] || fail "the account should read pro, got $PLAN"
BALANCE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
printf 'plan=%s balance=%s (the signup grant)\n' "$PLAN" "$BALANCE"

# ==========================================================================================
# 2. The settings round trip
# ==========================================================================================
say "2. settings: a partial write, quiet hours that wrap midnight, and a refused channel"
curl -fsS -X PUT "$BASE/settings" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d '{"language":"id","quiet_start_hour":22,"quiet_end_hour":7,"block_len_min":45}' \
  > "$TMP/settings.json"
python3 - "$TMP/settings.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
s = body["settings"]
assert s["language"] == "id", s
assert (s["quiet_start_hour"], s["quiet_end_hour"]) == (22, 7), "the window must wrap midnight (A202)"
assert s["block_len_min"] == 45, s
# A partial write: nothing this body did not name may move.
assert s["study_start_hour"] == 8 and s["study_end_hour"] == 22, "a partial write reset an unnamed field"
assert s["max_study_min_per_day"] == 240 and s["break_min"] == 10, s
print(f"PUT /settings -> language={s['language']} quiet={s['quiet_start_hour']}->{s['quiet_end_hour']}"
      f" block_len_min={s['block_len_min']} study={s['study_start_hour']}-{s['study_end_hour']}"
      f" max_study_min_per_day={s['max_study_min_per_day']} break_min={s['break_min']}")
PY
# The timezone lives on `users` (A203), so it is returned beside the row and written to it.
curl -fsS -X PUT "$BASE/settings" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"timezone":"Asia/Jakarta"}' > "$TMP/settings-tz.json"
TZ_ANSWER="$(field '["timezone"]' < "$TMP/settings-tz.json")"
[ "$TZ_ANSWER" = "Asia/Jakarta" ] || fail "the timezone did not round trip, got $TZ_ANSWER"
STORED_TZ="$(db "select timezone from users where id = '$UID_'")"
[ "$STORED_TZ" = "Asia/Jakarta" ] || fail "users.timezone is $STORED_TZ, not Asia/Jakarta"
LANGUAGE="$(field '["settings"]["language"]' < "$TMP/settings-tz.json")"
[ "$LANGUAGE" = "id" ] || fail "the language was reset by an unrelated partial write: $LANGUAGE"
printf 'timezone=%s on users, language still %s after a second partial write\n' "$STORED_TZ" "$LANGUAGE"

# A channel the plan does not grant is refused per channel, before the write (A201), and
# the refusal names the plan that grants it.
STATUS="$(curl -sS -o "$TMP/channel.json" -w '%{http_code}' -X PUT "$BASE/settings" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d '{"reminder_channels":["web","telegram"]}')"
[ "$STATUS" = "403" ] || fail "a telegram channel on pro must be 403, got $STATUS: $(cat "$TMP/channel.json")"
[ "$(field '["code"]' < "$TMP/channel.json")" = "plan_required" ] \
  || fail "the channel refusal must be plan_required: $(cat "$TMP/channel.json")"
CHANNELS="$(db "select reminder_channels from user_settings where user_id = '$UID_'")"
[ "$CHANNELS" = '["web"]' ] || fail "a refused channel changed the row: $CHANNELS"
printf 'telegram on pro -> %s %s min_plan=%s, and the row is still %s\n' "$STATUS" \
  "$(field '["code"]' < "$TMP/channel.json")" "$(field '["min_plan"]' < "$TMP/channel.json")" "$CHANNELS"

# Quiet hours are a pair: half a window is a 400 rather than a guess about the other half.
STATUS="$(curl -sS -o "$TMP/half-quiet.json" -w '%{http_code}' -X PUT "$BASE/settings" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d '{"quiet_start_hour":23}')"
[ "$STATUS" = "400" ] || fail "half a quiet window must be 400, got $STATUS: $(cat "$TMP/half-quiet.json")"
printf 'half a quiet window -> %s %s\n' "$STATUS" "$(field '["message"]' < "$TMP/half-quiet.json")"

# ==========================================================================================
# 3. An Indonesian capture: two credits, a Jakarta evening, and no task
# ==========================================================================================
say "3. quick capture: an Indonesian line, ${BREAKDOWN_COST} credits, and no task"
TASKS_BEFORE="$(db "select count(*) from tasks where user_id = '$UID_'")"
LEDGER_BEFORE="$(db "select count(*) from credit_ledger where user_id = '$UID_'")"
BALANCE_BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
STATUS="$(curl -sS -o "$TMP/capture.json" -w '%{http_code}' -X POST "$BASE/tasks/quick-capture" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d '{"text":"selesaikan tugas kripto hari Jumat"}')"
[ "$STATUS" = "200" ] || fail "quick capture answered $STATUS: $(cat "$TMP/capture.json")"
python3 - "$TMP/capture.json" "$BREAKDOWN_COST" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
cost = int(sys.argv[2])
draft = body["draft"]
# The stub resolves "Jumat" to 2026-10-16T23:59+07:00, so the instant is fixed and the
# assertion is about the *offset being kept*, not about the model's arithmetic. Read as
# UTC, that deadline would be 2026-10-16T23:59Z — seven hours late, and a different day
# for anything due before 07:00 local.
assert draft["due_at"] == "2026-10-16T16:59:00Z", draft["due_at"]
assert draft["title"] == "Selesaikan tugas kripto", draft
assert draft["priority"] == 1 and draft["estimate_min"] == 90, draft
assert draft["warnings"] == [], f"a clean reply must warn about nothing: {draft['warnings']}"
assert body["credits_spent"] == cost, body
assert body["prompt_version"] == "quick_capture.v1", body
print(f"draft title={draft['title']!r} due_at={draft['due_at']} (23:59 Asia/Jakarta)"
      f" priority={draft['priority']} estimate_min={draft['estimate_min']}"
      f" credits_spent={body['credits_spent']} credits_remaining={body['credits_remaining']}"
      f" prompt_version={body['prompt_version']}")
PY
TASKS_AFTER="$(db "select count(*) from tasks where user_id = '$UID_'")"
LEDGER_AFTER="$(db "select count(*) from credit_ledger where user_id = '$UID_'")"
BALANCE_AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
[ "$TASKS_AFTER" = "$TASKS_BEFORE" ] \
  || fail "a capture wrote a task: $TASKS_BEFORE before, $TASKS_AFTER after (A193)"
[ "$LEDGER_AFTER" -eq "$((LEDGER_BEFORE + 1))" ] \
  || fail "a capture must write exactly one ledger row: $LEDGER_BEFORE -> $LEDGER_AFTER"
[ "$BALANCE_AFTER" -eq "$((BALANCE_BEFORE - BREAKDOWN_COST))" ] \
  || fail "the charge is not $BREAKDOWN_COST: $BALANCE_BEFORE -> $BALANCE_AFTER"
printf 'tasks %s -> %s (unchanged, A193) | ledger rows %s -> %s | balance %s -> %s\n' \
  "$TASKS_BEFORE" "$TASKS_AFTER" "$LEDGER_BEFORE" "$LEDGER_AFTER" "$BALANCE_BEFORE" "$BALANCE_AFTER"

# ==========================================================================================
# 4. The confirm
# ==========================================================================================
say "4. the confirm: the draft becomes one row, with the instant the model resolved"
DUE_AT="$(field '["draft"]["due_at"]' < "$TMP/capture.json")"
curl -fsS -X POST "$BASE/tasks" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"title\":\"Selesaikan tugas kripto\",\"notes\":\"Bab 4 dan latihannya\",\"due_at\":\"$DUE_AT\",\"priority\":1,\"estimate_min\":90}" \
  > "$TMP/task.json"
TASK="$(field '["task"]["id"]' < "$TMP/task.json")"
STORED_DUE="$(db "select to_char(due_at at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') from tasks where id = '$TASK'")"
[ "$STORED_DUE" = "2026-10-16T16:59:00Z" ] || fail "the stored instant moved: $STORED_DUE"
TASKS_AFTER="$(db "select count(*) from tasks where user_id = '$UID_'")"
[ "$TASKS_AFTER" -eq "$((TASKS_BEFORE + 1))" ] \
  || fail "the confirm should leave one task, found $TASKS_AFTER"
printf 'POST /tasks -> id=%s due_at=%s (stored as the same instant) | tasks=%s\n' \
  "$TASK" "$STORED_DUE" "$TASKS_AFTER"

# ==========================================================================================
# 5. The break-down
# ==========================================================================================
say "5. break-down: a proposal, no subtasks, then two accepted in the proposed order"
TASKS_BEFORE="$(db "select count(*) from tasks where user_id = '$UID_'")"
BALANCE_BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
STATUS="$(curl -sS -o "$TMP/breakdown.json" -w '%{http_code}' -X POST "$BASE/tasks/$TASK/breakdown" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' -d '{}')"
[ "$STATUS" = "200" ] || fail "break-down answered $STATUS: $(cat "$TMP/breakdown.json")"
python3 - "$TMP/breakdown.json" "$BREAKDOWN_COST" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
cost = int(sys.argv[2])
p = body["proposal"]
titles = [s["title"] for s in p["subtasks"]]
assert len(titles) == 3, titles
assert p["total_estimate_min"] == sum(s["estimate_min"] for s in p["subtasks"]), p
assert body["credits_spent"] == cost, body
assert body["prompt_version"] == "task_breakdown.v1", body
print(f"proposal subtasks={titles} total_estimate_min={p['total_estimate_min']}"
      f" credits_spent={body['credits_spent']} prompt_version={body['prompt_version']}")
PY
TASKS_AFTER="$(db "select count(*) from tasks where user_id = '$UID_'")"
BALANCE_AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
[ "$TASKS_AFTER" = "$TASKS_BEFORE" ] \
  || fail "a break-down wrote subtasks: $TASKS_BEFORE before, $TASKS_AFTER after (A195)"
[ "$BALANCE_AFTER" -eq "$((BALANCE_BEFORE - BREAKDOWN_COST))" ] \
  || fail "the break-down charge is not $BREAKDOWN_COST: $BALANCE_BEFORE -> $BALANCE_AFTER"
printf 'tasks %s -> %s (unchanged, A195) | balance %s -> %s\n' \
  "$TASKS_BEFORE" "$TASKS_AFTER" "$BALANCE_BEFORE" "$BALANCE_AFTER"

# Accepting is N ordinary creates, and `position` is what keeps the order (A196): a batch
# insert shares `created_at` to the microsecond, so without it the list comes back in a
# different order than the model wrote it.
#
# The bodies are built by Python into one file each rather than interpolated into a curl
# argument: a nested `python3 -c` inside a double-quoted `-d` is three levels of quoting,
# and the shell wins that argument.
say "5b. accepting two of the three subtasks"
python3 - "$TMP/breakdown.json" "$TASK" "$TMP" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
task_id, tmp = sys.argv[2], sys.argv[3]
subtasks = body["proposal"]["subtasks"][:2]
for position, sub in enumerate(subtasks):
    with open(f"{tmp}/subtask-body-{position}.json", "w") as handle:
        json.dump(
            {
                "title": sub["title"],
                "estimate_min": sub["estimate_min"],
                "parent_id": task_id,
                "position": position,
            },
            handle,
        )
with open(f"{tmp}/subtasks.txt", "w") as handle:
    for sub in subtasks:
        handle.write(f"{sub['title']}\t{sub['estimate_min']}\n")
print(f"accepting {len(subtasks)} of {len(body['proposal']['subtasks'])} proposed subtasks")
PY
POSITION=0
while IFS=$'\t' read -r _TITLE _MINUTES; do
  curl -fsS -X POST "$BASE/tasks" -H "authorization: Bearer $ACCESS" \
    -H 'content-type: application/json' \
    -d "@$TMP/subtask-body-$POSITION.json" > "$TMP/subtask-$POSITION.json"
  POSITION=$((POSITION + 1))
done < "$TMP/subtasks.txt"
curl -fsS "$BASE/tasks?parent_id=$TASK" -H "authorization: Bearer $ACCESS" > "$TMP/children.json"
python3 - "$TMP/children.json" "$TMP/subtasks.txt" <<'PY'
import json, sys
children = json.load(open(sys.argv[1]))["tasks"]
wanted = [line.split("\t")[0] for line in open(sys.argv[2]).read().splitlines()]
got = [t["title"] for t in children]
assert got == wanted, f"the proposal's order was not kept: {got} vs {wanted}"
assert [t["position"] for t in children] == list(range(len(wanted))), children
print(f"GET /tasks?parent_id= -> {len(children)} subtasks in the proposed order: {got}")
PY
TASKS_AFTER="$(db "select count(*) from tasks where user_id = '$UID_'")"
printf 'tasks now %s (the parent plus the two accepted subtasks)\n' "$TASKS_AFTER"

# ==========================================================================================
# 6. An event
# ==========================================================================================
say "6. events: the overlap window, and a column a client may not write"
curl -fsS -X POST "$BASE/events" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d '{"kind":"exam","title":"UTS Kripto","starts_at":"2026-10-16T00:00:00Z","ends_at":"2026-10-16T00:00:00Z"}' \
  > "$TMP/event.json"
EVENT="$(field '["event"]["id"]' < "$TMP/event.json")"
SOURCE="$(field '["event"]["source"]' < "$TMP/event.json")"
[ "$SOURCE" = "app" ] || fail "a client's own event must carry source=app, got $SOURCE"
# A three-hour class that starts before the window and ends inside it. `starts_at between`
# would drop it, and the student looking at this morning's timeline would not see it.
curl -fsS -X POST "$BASE/events" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d '{"kind":"class","title":"Kelas panjang","starts_at":"2026-10-15T20:00:00Z","ends_at":"2026-10-16T04:00:00Z"}' \
  > /dev/null
curl -fsS "$BASE/events?from=2026-10-16T00:00:00Z&to=2026-10-16T02:00:00Z" \
  -H "authorization: Bearer $ACCESS" > "$TMP/window.json"
python3 - "$TMP/window.json" <<'PY'
import json, sys
events = json.load(open(sys.argv[1]))["events"]
titles = [e["title"] for e in events]
assert "Kelas panjang" in titles, f"an event overlapping the window was dropped: {titles}"
print(f"GET /events?from=&to= -> {len(events)} overlapping row(s): {titles}")
PY
STATUS="$(curl -sS -o "$TMP/calendar-column.json" -w '%{http_code}' -X POST "$BASE/events" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d '{"kind":"exam","title":"X","external_id":"abc","starts_at":"2026-10-16T00:00:00Z","ends_at":"2026-10-16T01:00:00Z"}')"
[ "$STATUS" = "400" ] || fail "a calendar-owned column must be 400, got $STATUS: $(cat "$TMP/calendar-column.json")"
printf 'source=%s | external_id from a client -> %s %s\n' "$SOURCE" "$STATUS" \
  "$(field '["message"]' < "$TMP/calendar-column.json")"

# ==========================================================================================
# 7. Sessions: the device's clock, the bound, the replay, and the activity read
# ==========================================================================================
say "7. sessions: ${BREAKDOWN_COST}-credit-free writes, the bound, and the replay"
read -r CLIENT_TS STARTED_AT <<<"$(python3 -c "
import datetime
now = datetime.datetime.now(datetime.timezone.utc)
print(now.strftime('%Y-%m-%dT%H:%M:%SZ'), (now - datetime.timedelta(minutes=50)).strftime('%Y-%m-%dT%H:%M:%SZ'))")"
SESSION="$(python3 -c "import uuid; print(uuid.uuid4())")"
STATUS="$(curl -sS -o "$TMP/session.json" -w '%{http_code}' -X POST "$BASE/sessions" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"id\":\"$SESSION\",\"started_at\":\"$STARTED_AT\",\"duration_min\":50,\"task_id\":\"$TASK\",\"client_ts\":\"$CLIENT_TS\"}")"
[ "$STATUS" = "201" ] || fail "a plausible session must be 201, got $STATUS: $(cat "$TMP/session.json")"
[ "$(field '["applied"]' < "$TMP/session.json")" = "True" ] \
  || fail "the first flush must report applied=true: $(cat "$TMP/session.json")"
# The same id again: a timer that flushes after a restart is the same append-only row
# (A206), so the replay must be a no-op that says so rather than a second row.
STATUS="$(curl -sS -o "$TMP/session-replay.json" -w '%{http_code}' -X POST "$BASE/sessions" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"id\":\"$SESSION\",\"started_at\":\"$STARTED_AT\",\"duration_min\":50,\"task_id\":\"$TASK\",\"client_ts\":\"$CLIENT_TS\"}")"
[ "$STATUS" = "201" ] || fail "a replay must still answer 201, got $STATUS: $(cat "$TMP/session-replay.json")"
[ "$(field '["applied"]' < "$TMP/session-replay.json")" = "False" ] \
  || fail "a replay must report applied=false: $(cat "$TMP/session-replay.json")"
ROWS="$(db "select count(*) from study_sessions where user_id = '$UID_'")"
[ "$ROWS" = "1" ] || fail "a replay wrote a second row: $ROWS"
printf 'session %s started_at=%s duration_min=50 client_ts=%s -> applied=true, replay -> applied=false, rows=%s\n' \
  "$SESSION" "$STARTED_AT" "$CLIENT_TS" "$ROWS"

# The bound is compared against the *device's* own clock (A204), so a session whose length
# exceeds the elapsed time it reports is refused. Comparing against the server's clock
# would refuse a legitimate offline session from a device running fast.
STATUS="$(curl -sS -o "$TMP/session-long.json" -w '%{http_code}' -X POST "$BASE/sessions" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"id\":\"$(python3 -c 'import uuid; print(uuid.uuid4())')\",\"started_at\":\"$STARTED_AT\",\"duration_min\":600,\"client_ts\":\"$CLIENT_TS\"}")"
[ "$STATUS" = "400" ] || fail "a 600-minute session over a 50-minute elapsed time must be 400, got $STATUS: $(cat "$TMP/session-long.json")"
[ "$(field '["code"]' < "$TMP/session-long.json")" = "validation" ] \
  || fail "the bound's refusal must be validation: $(cat "$TMP/session-long.json")"
printf 'a 600-minute session over 50 elapsed -> %s %s\n' "$STATUS" \
  "$(field '["message"]' < "$TMP/session-long.json")"

FROM="$(python3 -c "
import datetime
print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=1)).strftime('%Y-%m-%dT00:00:00Z'))")"
TO="$(python3 -c "
import datetime
print((datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(days=1)).strftime('%Y-%m-%dT00:00:00Z'))")"
curl -fsS "$BASE/sessions?from=$FROM&to=$TO" -H "authorization: Bearer $ACCESS" > "$TMP/sessions.json"
curl -fsS "$BASE/activity?from=$FROM&to=$TO" -H "authorization: Bearer $ACCESS" > "$TMP/activity.json"
python3 - "$TMP/sessions.json" "$TMP/activity.json" "$SESSION" <<'PY'
import json, sys
sessions = json.load(open(sys.argv[1]))["sessions"]
activity = json.load(open(sys.argv[2]))
assert [s["id"] for s in sessions] == [sys.argv[3]], sessions
assert activity["sessions"] == [sessions[0]["started_at"]], (activity, sessions)
# The streak's three inputs, in one round trip; the rule that turns them into a streak is
# the client's (`app/src/features/today/day.ts`), because the Today view works offline and
# a server route cannot (A205).
assert set(activity) == {"reviews", "attempts", "sessions"}, activity
print(f"GET /sessions -> 1 row duration_min={sessions[0]['duration_min']} | "
      f"GET /activity -> reviews={len(activity['reviews'])} attempts={len(activity['attempts'])} "
      f"sessions={len(activity['sessions'])}")
PY

# ==========================================================================================
# 8. Deleting a parent
# ==========================================================================================
say "8. deleting a parent: the subtree, and a change_log row per child"
LIVE_BEFORE="$(db "select count(*) from tasks where user_id = '$UID_' and deleted_at is null")"
CHILDREN="$(db "select count(*) from tasks where user_id = '$UID_' and parent_id = '$TASK' and deleted_at is null")"
[ "$CHILDREN" -ge 1 ] || fail "the accepted subtasks are not under the parent"
STATUS="$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE "$BASE/tasks/$TASK" -H "authorization: Bearer $ACCESS")"
[ "$STATUS" = "204" ] || fail "deleting a task must be 204, got $STATUS"
LIVE_AFTER="$(db "select count(*) from tasks where user_id = '$UID_' and deleted_at is null")"
[ "$LIVE_AFTER" -eq "$((LIVE_BEFORE - 1 - CHILDREN))" ] \
  || fail "the delete did not take the subtree: $LIVE_BEFORE live, $CHILDREN children, $LIVE_AFTER left"
LOGGED="$(db "select count(*) from change_log where user_id = '$UID_' and entity = 'tasks' and op = 'delete'")"
[ "$LOGGED" -eq "$((1 + CHILDREN))" ] \
  || fail "each tombstoned row needs its own change_log entry: $LOGGED for 1 + $CHILDREN"
FLAGGED="$(db "select count(*) from tasks where user_id = '$UID_' and parent_id = '$TASK' and deleted_with_parent")"
[ "$FLAGGED" = "$CHILDREN" ] \
  || fail "the cascade must flag its descendants for a later restore: $FLAGGED of $CHILDREN"
curl -fsS "$BASE/tasks?parent_id=$TASK" -H "authorization: Bearer $ACCESS" > "$TMP/after-delete.json"
LEFT="$(field '["tasks"]' < "$TMP/after-delete.json" | python3 -c 'import sys,json; print(len(json.load(sys.stdin)))')"
[ "$LEFT" = "0" ] || fail "a tombstoned child is still readable: $LEFT rows"
printf 'live tasks %s -> %s | %s child(ren) tombstoned | change_log delete rows=%s | flagged=%s\n' \
  "$LIVE_BEFORE" "$LIVE_AFTER" "$CHILDREN" "$LOGGED" "$FLAGGED"

# ==========================================================================================
# 9. The prompts the model actually received
# ==========================================================================================
#
# A response-only check cannot see this: the draft and the proposal look the same whether or
# not the prompt carried the student's zone and a data block. Both failures are silent —
# a date resolved in UTC lands on the wrong day, and an undelimited capture line is read as
# an instruction — so the run reads the stub's log, which is what the server actually sent.
say "9. the prompts, read out of $STUB_LOG"
[ -s "$STUB_LOG" ] || fail "the stub logged no request bodies at $STUB_LOG (was it started with --log?)"
python3 - "$STUB_LOG" <<'PY'
import json, sys

lines = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
assert len(lines) >= 2, f"expected a capture and a break-down, found {len(lines)} request(s)"
capture = [l for l in lines if "Turn the user's text into a task draft" in l["messages"][0]["content"]]
breakdown = [l for l in lines if "Break this task into concrete subtasks" in l["messages"][0]["content"]]
assert capture, "no capture prompt in the stub's log"
assert breakdown, "no break-down prompt in the stub's log"

OPEN, CLOSE = "<user_context_data>", "</user_context_data>"


def inside_a_data_block(prompt: str, needle: str) -> bool:
    """Whether `needle` sits between a data-block opening and its closing tag.

    Counting the tags would be wrong here, and this is worth stating because the naive
    version of this check passes for the wrong reason: the base template's own prose
    *names* `<user_context_data>` once while explaining the block, so the opening count is
    always one higher than the closing count. What matters is the property Phase 6's guard
    asserts — the student's text is inside a block, so the model reads it as data.
    """
    at = prompt.find(needle)
    if at < 0:
        return False
    opened = prompt.rfind(OPEN, 0, at)
    return opened >= 0 and prompt.find(CLOSE, opened) > at


for name, requests, needle in (
    ("quick_capture.v1", capture, "selesaikan tugas kripto"),
    ("task_breakdown.v1", breakdown, "Selesaikan tugas kripto"),
):
    prompt = requests[-1]["messages"][0]["content"]
    # The student's zone, not the server's and not the machine's: an Indonesian capture
    # resolved in UTC puts a Friday-evening deadline on the wrong day, and nothing in the
    # response would say so.
    assert "Asia/Jakarta" in prompt, f"{name}: the prompt did not carry the student's zone"
    assert inside_a_data_block(prompt, needle), (
        f"{name}: {needle!r} is interpolated outside a <user_context_data> block, so the "
        "model reads the student's own text as an instruction"
    )
    # A192: the caller's language, from `user_settings.language`, which this run set to id.
    # The line asserted is `json_contract.v1.md`'s, which is rendered with the *module's*
    # own table — so it is the one both modules carry, and it is exactly the line that an
    # `Optional(SOURCE_LANGUAGE)` default would have filled with "the same language as the
    # source material" instead. Neither module has sources.
    assert "- Write all human-readable text in id." in prompt, (
        f"{name}: {{output_language}} was not filled with the caller's language"
    )
    print(f"{name}: zone=Asia/Jakarta data_block=yes output_language=id chars={len(prompt)}")
PY

printf '\n=== PASS\n'
