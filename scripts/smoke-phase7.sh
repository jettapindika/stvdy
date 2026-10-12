#!/usr/bin/env bash
# Phase 7 end-to-end smoke test: entitlements, credits and limits, against the real
# server, the real Postgres and a stub model.
#
# Drives the real process over HTTP and reads the real database back, because the parts
# this phase can get wrong are exactly the parts a trait-level double cannot see: the
# status code and the five-key envelope on the wire, the numbers inside it, whether the
# refusal wrote a ledger row, whether the ceiling stopped a spend before the provider call,
# and whether a per-IP limit survives a restart. The Rust suites assert all of these too —
# this run is the one that proves the *wired process* does, and it prints the numbers it
# asserted so the output is evidence rather than a green word.
#
# Eight scenarios, in order:
#   1. a fresh account: `GET /usage` against the plan matrix quoted from config/plans.toml
#   2. an exhausted account: the daily cap, by name, with limit and used
#   3. a concurrent-spend run: eight 1-credit spends against a five-credit balance
#   4. a downgrade: pro -> free keeps every row and flags the over-limit ones read-only
#   5. a refused upload: 21 MB against a 20 MB `max_file_mb`
#   6. the ceiling pause: a spend at the plan's provider-cost ceiling
#   7. the §5 tamper rows: the status each one gets from the real server
#   8. the signup limiter: the per-domain rule, its public-domain exemption, the per-IP rule
#
# **Run this against a freshly started server.** It makes ~15 requests to the credential
# routes, and `ratelimit::auth_layer` (A12) is an in-process bucket of **30 requests, then
# one refill per minute** per IP — so a second run inside the same server process has to
# wait for cells, and a third mostly cannot run. That is the server's own control and not a
# property of this script; the run says so plainly when it happens instead of hanging.
#
# Needs a browser? No. Unlike Phase 6 this run prints no PDF, so CHROME_PATH is not
# needed and a server without one is fine.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
# The port the SERVER was started with (LLM_BASE_URL), not a private one: the server is
# what talks to the stub, so the stub the script watches must be the stub the server
# calls. Start the server with
#   LLM_BASE_URL=http://127.0.0.1:8100/v1
# and leave this at its default, or set both to the same value.
STUB_PORT="${STUB_PORT:-8100}"
# This must match the SERVER's `MAIL_DIR`, because the server is what writes the
# verification mail: the script only reads the newest `.eml` from it.
MAIL_DIR="${MAIL_DIR:-var/mail}"
PLANS="${PLANS:-config/plans.toml}"
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

# --- the stub model server ---------------------------------------------------------------
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

say "stub model server"
STUB_LOG="${STUB_LOG:-var/stub-phase7-requests.jsonl}"
rm -f "$STUB_LOG"

if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
  echo "a stub is already listening on $STUB_PORT; reusing it"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    > "$TMP/stub.log" 2>&1 &
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
  || fail "no server at $BASE. Start the stub with 'just smoke-llm-phase7' and the server with 'just smoke-server-phase7'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

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
for name in ("free", "pro", "power"):
    p = plans["plans"][name]
    print(
        f"{name}: name={p['name']} workspaces={p['workspaces']} sources_total={p['sources_total']}"
        f" max_file_mb={p['max_file_mb']} storage_mb={p['storage_mb']} devices_max={p['devices_max']}"
        f" credits_per_month={p['credits_per_month']} chat_msgs_per_day={p['chat_msgs_per_day']}"
        f" web_searches_per_day={p['web_searches_per_day']}"
        f" reports_per_month={p['reports_per_month']}"
        f" export_formats={','.join(p['export_formats'])}"
        f" byok={str(p['byok']).lower()}"
        f" ceiling_usd={plans['provider_cost_ceiling_usd'][name]}"
    )
print("credits: " + " ".join(f"{k}={v}" for k, v in sorted(plans["credits"].items())))
PY
cat "$TMP/matrix.txt"
m() { python3 -c "
import sys
key = sys.argv[1]
for line in open('$TMP/matrix.txt'):
    if line.startswith('free: ') and key in line:
        print(line.split(key + '=')[1].split()[0]); break
" "$1"; }
FREE_WORKSPACES="$(m workspaces)"
FREE_MAX_FILE_MB="$(m max_file_mb)"
FREE_CHAT_PER_DAY="$(m chat_msgs_per_day)"
FREE_CEILING="$(python3 -c "
import sys, tomllib
print(tomllib.load(open('$PLANS','rb'))['provider_cost_ceiling_usd']['free'])")"
[ -n "$FREE_WORKSPACES" ] || fail "could not read the free plan's workspaces from $PLANS"

# --- accounts ----------------------------------------------------------------------------
#
# One per scenario, so a per-account control (the spend cap, the daily cap, the ceiling)
# cannot be tripped by a neighbouring scenario. The email domain is per run, because A154's
# domain rule counts non-public domains over 24h and a fixed one would accumulate across
# runs until a re-run was refused for a reason that has nothing to do with this script.
DOMAIN="phase7-$STAMP.example.test"
ACCESS=""

# The per-IP limiter counts *every* attempt, including this run's own, and its window is
# 24h: without this the eleventh re-run of this script in a day would fail at signup and
# read like a server bug. This deletes only the loopback address's rows, which is what a
# developer's machine signs up from.
say "reset the loopback signup attempts (A154 counts 25 per IP per 24h, and every run here adds)"
PRUNED="$(db "with gone as (delete from signup_attempts where ip = '127.0.0.1' returning 1) select count(*) from gone")"
printf 'pruned %s signup_attempts row(s) for 127.0.0.1\n' "$PRUNED"

signup() { # $1 = a short label used in the address
  local label="$1"
  local email="phase7-$label-$STAMP@$DOMAIN"
  local status
  status="$(auth_post "$TMP/signup-$label.json" "$BASE/auth/signup" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke laptop\",\"platform\":\"macos\"}}")"
  [ "$status" = "201" ] || fail "signup for $label answered $status: $(cat "$TMP/signup-$label.json")"
  # Newest first by name: the dev sink writes `<unix-millis>-<local part>.eml`.
  local mail
  mail="$(find "$MAIL_DIR" -name '*.eml' -type f | sort | tail -1)"
  local token
  token="$(grep -o 'token=[A-Za-z0-9_-]*' "$mail" | head -1 | cut -d= -f2)"
  status="$(auth_post "$TMP/verify-$label.json" "$BASE/auth/verify-email" \
    -H 'content-type: application/json' -d "{\"token\":\"$token\"}")"
  [ "$status" = "200" ] || fail "verifying $label answered $status: $(cat "$TMP/verify-$label.json")"
  field '["access_token"]' < "$TMP/signup-$label.json"
}

# A credential request, retried across the auth limiter's refill window.
#
# `/auth/signup`, `/auth/login` and `/auth/verify-email` sit behind an in-process bucket of
# **30 requests per minute per IP** (`ratelimit::AUTH_PER_MINUTE`, A12) — a different control
# from A154's per-IP signup limit, and the one that trips first: this run makes 14 credential
# requests, so a re-run inside the same minute is refused until the bucket refills. Waiting
# is the honest response to a bucket that refills; hiding it would be a run that fails on its
# second invocation for a reason that looks like a server bug.
#
# A 429 that does *not* clear in this window is A154's limiter, whose count is in
# `signup_attempts` (this run prunes its own address above), and the failure message says so.
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

# The account's id, from its address.
uid() { db "select id from users where email = 'phase7-$1-$STAMP@$DOMAIN'"; }

# A workspace with one small Indonesian text source, ingested and ready.
workspace() { # $1 = token, $2 = name
  curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $1" \
    -H 'content-type: application/json' -d "{\"name\":\"$2\"}" > "$TMP/ws.json"
  local ws
  ws="$(field '["workspace"]["id"]' < "$TMP/ws.json")"
  curl -fsS -X POST "$BASE/workspaces/$ws/sources" -H "authorization: Bearer $1" \
    -F kind=text -F title="Catatan" \
    -F "text=Three-way handshake membuka koneksi TCP. UDP tidak memakai handshake sehingga lebih cepat." \
    > "$TMP/source.json"
  local src
  src="$(field '["source"]["id"]' < "$TMP/source.json")"
  for _ in $(seq 1 120); do
    local status
    status="$(curl -fsS "$BASE/sources/$src/status" -H "authorization: Bearer $1" | field '["status"]')"
    [ "$status" = "ready" ] && break
    sleep 0.25
  done
  [ "$(curl -fsS "$BASE/sources/$src/status" -H "authorization: Bearer $1" | field '["status"]')" = "ready" ] \
    || fail "the source did not reach ready"
  printf '%s' "$ws"
}

# ==========================================================================================
# 1. A fresh account
# ==========================================================================================
say "1. a fresh account: GET /usage against the plan matrix"
ACCESS="$(signup fresh)"
curl -fsS "$BASE/usage" -H "authorization: Bearer $ACCESS" > "$TMP/usage.json"
python3 - "$TMP/usage.json" "$FREE_WORKSPACES" "$FREE_MAX_FILE_MB" "$FREE_CHAT_PER_DAY" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
workspaces, max_file_mb, chat_per_day = sys.argv[2], sys.argv[3], sys.argv[4]
assert body["plan"] == "free", body["plan"]
assert body["name"] == "Free", body["name"]
assert body["credits"] == {"monthly": 60, "purchased": 0, "total": 60}, body["credits"]
assert body["subscription"]["status"] in ("none", "active"), body["subscription"]
assert body["limits"]["workspaces"]["limit"] == int(workspaces), body["limits"]["workspaces"]
assert body["limits"]["max_file_mb"]["limit"] == int(max_file_mb), body["limits"]["max_file_mb"]
assert body["limits"]["chat_msgs_per_day"]["limit"] == int(chat_per_day), body["limits"]["chat_msgs_per_day"]
assert body["limits"]["max_file_mb"]["used"] is None, "an axis with no counter is null, never 0"
assert body["limits"]["google_accounts_max"]["limit"] == 0, "free allows zero Google accounts"
assert body["features"]["export_formats"] == ["md"], body["features"]["export_formats"]
assert body["features"]["byok"] is False, body["features"]
assert body["provider_cost"] == {"paused": False, "resets_at": body["provider_cost"]["resets_at"]}, body["provider_cost"]
assert body["provider_cost"]["resets_at"].endswith("Z"), body["provider_cost"]
# Gate 7(d): the cost block is `credits::costs`, and the file it comes from is the file
# this script just quoted.
assert body["costs"]["chat_message"] == 1, body["costs"]
assert body["costs"]["report_outline"] == 3 and body["costs"]["report_section"] == 6, body["costs"]
assert "spent_usd" not in json.dumps(body), "GET /usage must not carry our unit economics (A159)"
assert "ceiling_usd" not in json.dumps(body), "GET /usage must not carry the ceiling (A159)"
print(f"plan={body['plan']} credits={body['credits']} workspaces_limit={body['limits']['workspaces']['limit']}"
      f" max_file_mb={body['limits']['max_file_mb']['limit']} chat/day={body['limits']['chat_msgs_per_day']['limit']}"
      f" export_formats={body['features']['export_formats']} costs.chat_message={body['costs']['chat_message']}"
      f" costs.report_outline={body['costs']['report_outline']} costs.report_section={body['costs']['report_section']}"
      f" provider_cost.paused={body['provider_cost']['paused']} resets_at={body['provider_cost']['resets_at']}")
PY

# ==========================================================================================
# 2. An exhausted account
# ==========================================================================================
say "2. an exhausted account: the daily chat cap, by name"
EXHAUSTED="$(signup exhausted)"
EXHAUSTED_ID="$(uid exhausted)"
EXHAUSTED_WS="$(workspace "$EXHAUSTED" "Habis")"
# The cap is counted from `usage_events` at read time (A147), so seeding the events is
# seeding the cap — there is no counter row to write.
db "insert into usage_events (user_id, action, credits)
    select '$EXHAUSTED_ID', 'chat_message', 1 from generate_series(1, $FREE_CHAT_PER_DAY)" > /dev/null
SEEDED="$(db "select count(*) from usage_events where user_id = '$EXHAUSTED_ID' and action = 'chat_message'")"
[ "$SEEDED" = "$FREE_CHAT_PER_DAY" ] || fail "seeded $SEEDED chat events, wanted $FREE_CHAT_PER_DAY"
STATUS="$(curl -sS -o "$TMP/refused.json" -w '%{http_code}' -X POST "$BASE/workspaces/$EXHAUSTED_WS/chat" \
  -H "authorization: Bearer $EXHAUSTED" -H 'content-type: application/json' \
  -d '{"message":"one too many"}')"
[ "$STATUS" = "422" ] || fail "the cap must be 422, got $STATUS: $(cat "$TMP/refused.json")"
[ "$(field '["code"]' < "$TMP/refused.json")" = "limit_reached" ] \
  || fail "the cap's code must be limit_reached: $(cat "$TMP/refused.json")"
LIMIT="$(field '["limit"]' < "$TMP/refused.json")"
USED="$(field '["used"]' < "$TMP/refused.json")"
[ "$LIMIT" = "$FREE_CHAT_PER_DAY" ] || fail "limit is $LIMIT, the matrix says $FREE_CHAT_PER_DAY"
[ "$USED" = "$FREE_CHAT_PER_DAY" ] || fail "used is $USED, the cap is $FREE_CHAT_PER_DAY"
MIN_PLAN="$(field '["min_plan"]' < "$TMP/refused.json")"
[ "$MIN_PLAN" = "pro" ] || fail "a free cap must name the plan that lifts it, got $MIN_PLAN"
printf 'status=%s code=limit_reached limit=%s used=%s min_plan=%s\n' "$STATUS" "$LIMIT" "$USED" "$MIN_PLAN"

# ==========================================================================================
# 3. A concurrent-spend run
# ==========================================================================================
#
# Eight 1-credit chats fired at once against a five-credit balance. The point is not that
# five succeed: it is that **no spend can double-spend**, which is what
# `pg_advisory_xact_lock` plus the balance read inside the transaction buy. A run that let
# two requests share the fifth credit would leave a negative bucket, so both halves are
# asserted: the successes, and the balance.
#
# The per-account spend cap is 10/minute (A155) and this fires 8, so the cap is not what
# produces the three refusals.
say "3. a concurrent-spend run: 8 x 1 credit against a 5-credit balance"
RACE="$(signup race)"
RACE_ID="$(uid race)"
RACE_WS="$(workspace "$RACE" "Balapan")"
# The signup grant is 60 monthly credits; take 55 back so the balance is exactly 5. A
# `period_key` of null keeps this row outside the grant's partial unique index.
db "insert into credit_ledger (user_id, delta, bucket, reason) values ('$RACE_ID', -55, 'monthly', 'smoke')" > /dev/null
BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$RACE_ID'")"
[ "$BEFORE" = "5" ] || fail "the balance should be 5, is $BEFORE"

for index in $(seq 1 8); do
  (
    code="$(curl -sS -o "$TMP/race-$index.json" -w '%{http_code}' -X POST \
      "$BASE/workspaces/$RACE_WS/chat" -H "authorization: Bearer $RACE" \
      -H 'content-type: application/json' -d "{\"message\":\"race $index\"}")"
    printf '%s %s\n' "$index" "$code" > "$TMP/race-$index.status"
  ) &
done
wait
OK=0
REFUSED=0
REFUSED_INDEX=""
for index in $(seq 1 8); do
  code="$(cut -d' ' -f2 < "$TMP/race-$index.status")"
  case "$code" in
    200) OK=$((OK + 1)) ;;
    402)
      REFUSED=$((REFUSED + 1))
      [ -n "$REFUSED_INDEX" ] || REFUSED_INDEX="$index"
      ;;
    *) fail "spend $index answered $code: $(cat "$TMP/race-$index.json")" ;;
  esac
done
AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$RACE_ID'")"
NEGATIVE="$(db "select count(*) from (select bucket, sum(delta) as s from credit_ledger
                where user_id = '$RACE_ID' group by bucket) b where b.s < 0")"
printf 'balance before=%s after=%s | successes=%s refusals=%s | negative buckets=%s\n' \
  "$BEFORE" "$AFTER" "$OK" "$REFUSED" "$NEGATIVE"
[ "$OK" = "5" ] || fail "five spends should fit a five-credit balance, got $OK"
[ "$REFUSED" = "3" ] || fail "three spends should be refused, got $REFUSED"
[ "$AFTER" = "0" ] || fail "the balance should be exactly 0, is $AFTER"
[ "$NEGATIVE" = "0" ] || fail "a bucket went negative"
# Which of the eight lost the race is not deterministic, so the refusal is read from
# whichever one got the 402 rather than from a fixed index.
REFUSAL_CODE="$(field '["code"]' < "$TMP/race-$REFUSED_INDEX.json")"
REFUSAL_PLAN="$(field '["min_plan"]' < "$TMP/race-$REFUSED_INDEX.json")"
printf 'the refusal (spend %s): code=%s min_plan=%s message=%s\n' "$REFUSED_INDEX" "$REFUSAL_CODE" \
  "$REFUSAL_PLAN" "$(field '["message"]' < "$TMP/race-$REFUSED_INDEX.json")"
[ "$REFUSAL_CODE" = "insufficient_credits" ] || fail "a spent-out balance must be insufficient_credits"
[ "$REFUSAL_PLAN" = "pro" ] || fail "a spent-out account must be told which plan raises the allowance"

# ==========================================================================================
# 4. A downgrade
# ==========================================================================================
#
# Pro -> free with three workspaces. `users.plan_id` alone grants nothing (A146), so the
# downgrade is the subscription's status changing; the read-only flags are recomputed by
# the `reset_counters` job, which is enqueued here the way the scheduler enqueues it.
# Two halves, and the second is the one the spec cares about: the over-limit rows are
# flagged, and every row is still there and still readable.
say "4. a downgrade: pro -> free keeps every row and flags the over-limit ones"
DOWN="$(signup downgrade)"
DOWN_ID="$(uid downgrade)"
# The admin plan switch is API-only in this phase and needs an admin account, so this
# writes the two rows the switch writes: the stored plan and a live subscription.
db "update users set plan_id = 'pro' where id = '$DOWN_ID'" > /dev/null
db "insert into subscriptions
      (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
    values ('$DOWN_ID', 'pro', 'active', 'monthly', now() - interval '1 day', now() + interval '29 days')
    on conflict (user_id) where status in ('active', 'grace') do nothing" > /dev/null
for name in Satu Dua Tiga; do
  curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $DOWN" \
    -H 'content-type: application/json' -d "{\"name\":\"$name\"}" > /dev/null
done
PLAN_BEFORE="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $DOWN" | field '["plan"]')"
COUNT_BEFORE="$(curl -fsS "$BASE/workspaces" -H "authorization: Bearer $DOWN" | field '["workspaces"]' \
  | python3 -c 'import ast,sys; print(len(ast.literal_eval(sys.stdin.read())))')"
printf 'before: plan=%s workspaces=%s read_only=%s\n' "$PLAN_BEFORE" "$COUNT_BEFORE" \
  "$(db "select count(*) from workspaces where user_id = '$DOWN_ID' and read_only")"
[ "$PLAN_BEFORE" = "pro" ] || fail "the pro plan should be effective with a live subscription, got $PLAN_BEFORE"
[ "$COUNT_BEFORE" = "3" ] || fail "three workspaces should exist before the downgrade, got $COUNT_BEFORE"

# The downgrade, and then the rollover job that recomputes the flags.
db "update subscriptions set status = 'expired' where user_id = '$DOWN_ID'" > /dev/null
db "insert into jobs (kind, payload) values ('reset_counters', '{\"user_id\":\"$DOWN_ID\"}'::jsonb)" > /dev/null
ROLLED=""
for _ in $(seq 1 60); do
  ROLLED="$(db "select count(*) from jobs where kind = 'reset_counters'
                 and payload->>'user_id' = '$DOWN_ID' and status = 'done'")"
  [ "$ROLLED" = "1" ] && break
  sleep 0.5
done
if [ "$ROLLED" != "1" ]; then
  # The one failure this loop can have that is not a bug in this phase: the `jobs` table is
  # shared, so **any** other `stvdy-server` on this database claims rows from the same queue.
  # A process whose image predates Phase 7 answers `unknown job kind reset_counters` — it is
  # an old binary, not a broken handler, and the message says which so nobody hunts a
  # handler that is present in the source.
  JOB_ERROR="$(db "select coalesce(error, '') from jobs where kind = 'reset_counters'
                   and payload->>'user_id' = '$DOWN_ID' order by created_at desc limit 1")"
  case "$JOB_ERROR" in
    *"unknown job kind"*)
      fail "another stvdy-server claimed this run's reset_counters job and does not know the \
kind, so its image predates Phase 7. It shares the jobs table through DATABASE_URL. Stop it \
(check 'lsof -nP -iTCP:8099 -sTCP:LISTEN') or run this against a database only one server \
uses. The job's own error: $JOB_ERROR"
      ;;
  esac
  fail "the reset_counters job did not finish; is the worker running? (last job error: ${JOB_ERROR:-none})"
fi

PLAN_AFTER="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $DOWN" | field '["plan"]')"
READ_ONLY="$(db "select count(*) from workspaces where user_id = '$DOWN_ID' and read_only")"
TOTAL="$(db "select count(*) from workspaces where user_id = '$DOWN_ID' and deleted_at is null")"
COUNT_AFTER="$(curl -fsS "$BASE/workspaces" -H "authorization: Bearer $DOWN" | field '["workspaces"]' \
  | python3 -c 'import ast,sys; print(len(ast.literal_eval(sys.stdin.read())))')"
printf 'after:  plan=%s workspaces=%s read_only=%s (free allows %s)\n' \
  "$PLAN_AFTER" "$COUNT_AFTER" "$READ_ONLY" "$FREE_WORKSPACES"
[ "$PLAN_AFTER" = "free" ] || fail "an expired subscription must drop the account to free, got $PLAN_AFTER"
[ "$COUNT_AFTER" = "3" ] || fail "a downgrade must keep every row readable, got $COUNT_AFTER"
[ "$TOTAL" = "3" ] || fail "no row may be deleted by a downgrade"
[ "$READ_ONLY" = "1" ] || fail "exactly the over-limit workspace must be read-only, got $READ_ONLY"
# And the flag is a write-side flag: the read above succeeded, which is the spec's
# "viewable and exportable".
printf 'the downgrade kept 3 rows, flagged %s read-only, and every row is still readable\n' "$READ_ONLY"

# ==========================================================================================
# 5. A refused upload
# ==========================================================================================
say "5. a refused upload: 21 MB against a $FREE_MAX_FILE_MB MB max_file_mb"
UPLOAD="$(signup upload)"
UPLOAD_WS="$(workspace "$UPLOAD" "Unggah")"
BIG="$TMP/big.pdf"
dd if=/dev/zero of="$BIG" bs=1m count=21 2>/dev/null
SIZE="$(wc -c < "$BIG" | tr -d ' ')"
STATUS="$(curl -sS -o "$TMP/upload.json" -w '%{http_code}' -X POST "$BASE/workspaces/$UPLOAD_WS/sources" \
  -H "authorization: Bearer $UPLOAD" -F kind=pdf -F "file=@$BIG;filename=big.pdf")"
[ "$STATUS" = "422" ] || fail "an over-size upload must be 422, got $STATUS: $(cat "$TMP/upload.json")"
[ "$(field '["code"]' < "$TMP/upload.json")" = "limit_reached" ] \
  || fail "an over-size upload must be limit_reached: $(cat "$TMP/upload.json")"
UPLOAD_LIMIT="$(field '["limit"]' < "$TMP/upload.json")"
printf 'uploaded %s bytes, refused %s limit_reached limit=%s used=%s\n' \
  "$SIZE" "$STATUS" "$UPLOAD_LIMIT" "$(field '["used"]' < "$TMP/upload.json")"
EXPECTED_BYTES="$(python3 -c "print(int('$FREE_MAX_FILE_MB') * 1024 * 1024)")"
[ "$UPLOAD_LIMIT" = "$EXPECTED_BYTES" ] \
  || fail "the file ceiling should be $EXPECTED_BYTES bytes, the refusal says $UPLOAD_LIMIT"
STORED="$(db "select count(*) from sources where workspace_id = '$UPLOAD_WS' and deleted_at is null")"
printf 'sources in the workspace after the refusal: %s (the refused bytes were never stored)\n' "$STORED"
[ "$STORED" = "1" ] || fail "the refused upload left a row behind"

# ==========================================================================================
# 6. The ceiling pause
# ==========================================================================================
#
# The ceiling is enforced inside the spend's own transaction, so this seeds the month's
# provider cost at exactly the plan's ceiling and drives a real spend. A refusal that still
# charged would be worth nothing, so the ledger and the usage rows are read back too.
say "6. the ceiling pause: a spend at the free plan's \$$FREE_CEILING provider ceiling"
CEILING_ACC="$(signup ceiling)"
CEILING_ID="$(uid ceiling)"
CEILING_WS="$(workspace "$CEILING_ACC" "Langit")"
db "insert into usage_events (user_id, action, credits, provider_cost_usd)
    values ('$CEILING_ID', 'chat_message', 0, $FREE_CEILING)" > /dev/null
BALANCE_BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$CEILING_ID'")"
EVENTS_BEFORE="$(db "select count(*) from usage_events where user_id = '$CEILING_ID'")"
STATUS="$(curl -sS -o "$TMP/ceiling.json" -w '%{http_code}' -X POST "$BASE/workspaces/$CEILING_WS/chat" \
  -H "authorization: Bearer $CEILING_ACC" -H 'content-type: application/json' \
  -d '{"message":"one more"}')"
[ "$STATUS" = "422" ] || fail "the ceiling must refuse with 422, got $STATUS: $(cat "$TMP/ceiling.json")"
[ "$(field '["code"]' < "$TMP/ceiling.json")" = "limit_reached" ] \
  || fail "the ceiling's code must be limit_reached: $(cat "$TMP/ceiling.json")"
CEILING_PLAN="$(field '["min_plan"]' < "$TMP/ceiling.json")"
[ "$CEILING_PLAN" = "pro" ] || fail "the ceiling must name the plan whose ceiling is higher, got $CEILING_PLAN"
# Dollars are not plan-matrix numbers (A148): a client that rendered these would read
# "the limit is 0.5 and you have 0.5".
[ "$(field '["limit"]' < "$TMP/ceiling.json")" = "None" ] || fail "limit must be null on the ceiling"
[ "$(field '["used"]' < "$TMP/ceiling.json")" = "None" ] || fail "used must be null on the ceiling"
BALANCE_AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$CEILING_ID'")"
EVENTS_AFTER="$(db "select count(*) from usage_events where user_id = '$CEILING_ID'")"
[ "$BALANCE_AFTER" = "$BALANCE_BEFORE" ] || fail "a ceiling refusal wrote a ledger row"
[ "$EVENTS_AFTER" = "$EVENTS_BEFORE" ] || fail "a ceiling refusal recorded usage"
printf 'status=%s code=limit_reached min_plan=%s limit=null used=null | ledger %s -> %s | usage rows %s -> %s\n' \
  "$STATUS" "$CEILING_PLAN" "$BALANCE_BEFORE" "$BALANCE_AFTER" "$EVENTS_BEFORE" "$EVENTS_AFTER"
# And the pause is visible where the user looks, without the dollars.
curl -fsS "$BASE/usage" -H "authorization: Bearer $CEILING_ACC" > "$TMP/usage-paused.json"
PAUSED="$(field '["provider_cost"]["paused"]' < "$TMP/usage-paused.json")"
printf 'GET /usage provider_cost.paused=%s resets_at=%s\n' "$PAUSED" \
  "$(field '["provider_cost"]["resets_at"]' < "$TMP/usage-paused.json")"
[ "$PAUSED" = "True" ] || fail "a paused account must read paused=true from /usage"
python3 -c "
import json
body = json.load(open('$TMP/usage-paused.json'))
text = json.dumps(body)
assert 'spent_usd' not in text and 'ceiling_usd' not in text, text
assert str($FREE_CEILING) not in text, 'the dollars must not be on the wire'
print('no dollar figure on the paused view')
"

# ==========================================================================================
# 7. The §5 tamper rows
# ==========================================================================================
#
# `docs/THREAT_MODEL.md` §5 is the normative list, and `crates/server/tests/tamper.rs` has a
# case per "fails" row. This section drives the ones a live HTTP client can reach and prints
# the status each got. The three it cannot drive are named with the reason: two need the
# Phase 8 sync route and one needs the Phase 13 checkout route, neither of which exists in
# this phase — so "not drivable here" is the honest entry, not a pass.
say "7. the §5 \"fails\" rows: what the real server answers"
ROWS="$(python3 - docs/THREAT_MODEL.md <<'PY'
import sys
rows, in_section = [], False
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith("## "):
        in_section = line.startswith("## 5.")
        continue
    if not in_section or not line.startswith("| "):
        continue
    cells = [c.strip() for c in line.split("|")]
    if len(cells) > 2 and cells[2].startswith("**Fails.**"):
        rows.append(cells[1])
print(len(rows))
for row in rows:
    print(row)
PY
)"
ROW_COUNT="$(printf '%s\n' "$ROWS" | head -1)"
printf '§5 carries %s "Fails." row(s)\n' "$ROW_COUNT"
[ "$ROW_COUNT" = "13" ] || fail "§5 should carry 13 fails rows, found $ROW_COUNT"

TAMPER="$(signup tamper)"
TAMPER_ID="$(uid tamper)"
TAMPER_WS="$(workspace "$TAMPER" "Perusak")"

# Row: Unlock a paid feature by editing UI state.
STATUS="$(curl -sS -o "$TMP/plan-gate.json" -w '%{http_code}' \
  "$BASE/reports/01920000-0000-7000-8000-000000000000/export?format=pdf" \
  -H "authorization: Bearer $TAMPER")"
printf 'unlock a paid feature      : %s %s (the gate runs before the row is read)\n' \
  "$STATUS" "$(field '["code"]' < "$TMP/plan-gate.json")"
[ "$STATUS" = "403" ] || fail "the plan gate must be 403"
[ "$(field '["code"]' < "$TMP/plan-gate.json")" = "plan_required" ] || fail "the plan gate's code"

# Row: Edit the JWT to add a plan or raise limits.
#
# The forged token is signed with the server's **own** secret, so it authenticates: the
# assertion is that the extra claims changed nothing, which is why the plan read back is
# compared against the real account's plan rather than merely "not 500".
JWT_SECRET="$(grep -E '^JWT_SECRET=' .env | tail -1 | cut -d= -f2-)"
[ -n "$JWT_SECRET" ] || fail "no JWT_SECRET in .env; the forged-claim row cannot be driven"
FORGED="$(python3 - "$ACCESS" "$JWT_SECRET" <<'PY'
import base64, hashlib, hmac, json, sys
token, secret = sys.argv[1], sys.argv[2]
def b64(raw): return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
header, payload, _ = token.split(".")
claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
claims.update({"plan": "power", "devices_max": 10, "limits": {"workspaces": 1000}})
head = b64(json.dumps({"alg": "HS256", "typ": "JWT"}, separators=(",", ":")).encode())
body = b64(json.dumps(claims, separators=(",", ":")).encode())
sig = b64(hmac.new(secret.encode(), f"{head}.{body}".encode(), hashlib.sha256).digest())
print(f"{head}.{body}.{sig}")
PY
)"
FORGED_PLAN="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $FORGED" | field '["plan"]')"
REAL_PLAN="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $TAMPER" | field '["plan"]')"
printf 'edit the JWT               : forged plan claim -> /usage plan=%s (real token: %s)\n' \
  "$FORGED_PLAN" "$REAL_PLAN"
[ "$FORGED_PLAN" = "$REAL_PLAN" ] || fail "a forged plan claim changed the effective plan"

# Row: Grant credits locally, or replay a grant. The only grant route is admin-only, and a
# non-admin gets 404 rather than 403 (A7) — so there is no user-reachable grant at all.
STATUS="$(curl -sS -o "$TMP/grant.json" -w '%{http_code}' -X POST "$BASE/admin/users/$TAMPER_ID/credits" \
  -H "authorization: Bearer $TAMPER" -H 'content-type: application/json' \
  -d '{"credits":1000,"bucket":"purchased"}')"
printf 'grant credits locally      : %s %s (admin-only, 404 not 403)\n' "$STATUS" "$(field '["code"]' < "$TMP/grant.json")"
[ "$STATUS" = "404" ] || fail "a non-admin grant must be 404"

# Row: Read another user's rows.
OTHER_WS="$(workspace "$ACCESS" "Orang lain")"
STATUS="$(curl -sS -o "$TMP/cross.json" -w '%{http_code}' "$BASE/workspaces/$OTHER_WS" \
  -H "authorization: Bearer $TAMPER")"
printf 'read another user'"'"'s rows  : %s %s\n' "$STATUS" "$(field '["code"]' < "$TMP/cross.json")"
[ "$STATUS" = "404" ] || fail "a cross-tenant read must be 404"
# The owner reads the same route, so the 404 above is about ownership and not about the row
# being absent — the half that keeps a scoping bug from passing as a security boundary.
STATUS="$(curl -sS -o "$TMP/owned.json" -w '%{http_code}' "$BASE/workspaces/$TAMPER_WS" \
  -H "authorization: Bearer $TAMPER")"
printf 'the owner reads the same : %s\n' "$STATUS"
[ "$STATUS" = "200" ] || fail "the owner must read their own workspace: $(cat "$TMP/owned.json")"

# Rows: Call the LLM/search/Google directly; no response body may carry a provider key.
# Every body this run has collected is scanned, which is a weaker check than the tamper
# suite's (that one drives the chat route and reads the stream) and a real one here.
# `|| true`: grep exits 1 when it matches nothing, and under `pipefail` that would abort a
# run whose result is exactly the one wanted.
KEYS="$( { grep -rEl 'stvdy-provider-key|sk-[A-Za-z0-9]{20}' "$TMP" 2>/dev/null || true; } | wc -l | tr -d ' ')"
printf 'no provider key in a body : %s of the %s collected response file(s) carried a key-shaped string\n' \
  "$KEYS" "$(find "$TMP" -name '*.json' | wc -l | tr -d ' ')"
[ "$KEYS" = "0" ] || fail "a response body carried something key-shaped"

# The rows that are not reachable over HTTP in this phase, named rather than skipped.
printf 'forge a message / mark a source ready / write derived card fields: not drivable here —\n'
printf '  the sync push route is Phase 8 and is not registered (POST /sync/push answers %s);\n' \
  "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/sync/push" -H "authorization: Bearer $TAMPER")"
printf '  all three are asserted by crates/server/tests/tamper.rs at the decide() boundary.\n'
printf 'enable checkout on iOS/Android: not drivable here — the checkout route is Phase 13;\n'
printf '  the rule is Phase 7 code (billing::checkout_allowed) and tamper.rs::an_ios_checkout_is_refused asserts it.\n'

# ==========================================================================================
# 8. The signup limiter, both rules
# ==========================================================================================
#
# A154 makes the limiter database-backed so it survives a restart, which means the count is
# the input: seeding attempts is exactly the state that many real signups leave behind, and
# each drive below is a real request through the real route. Both rules are exercised,
# because they answer differently — the per-IP rule refuses, and the *domain* rule is
# **exempt** for a public mailbox domain, which is the half that would otherwise look like a
# missing control.
#
# The 25-signup loop is `crates/server/tests/abuse.rs::the_per_ip_signup_limit_trips_and_logs`'s
# job — it also asserts the log line, which this script cannot read from the server's stdout.
# Here the address is a different one for the seeded rows, so the two rules can be told apart.
say "8. the signup limiter: the per-domain rule, its public-domain exemption, and the per-IP rule"
PER_IP="$(grep -c 'SIGNUP_PER_IP_PER_DAY' crates/server/src/ratelimit.rs)"
[ "$PER_IP" -ge 1 ] || fail "SIGNUP_PER_IP_PER_DAY is gone; the limiter was renamed"
LIMIT_IP="$(python3 -c "
import re
print(re.search(r'SIGNUP_PER_IP_PER_DAY: u32 = (\d+)', open('crates/server/src/ratelimit.rs').read()).group(1))")"
LIMIT_DOMAIN="$(python3 -c "
import re
print(re.search(r'SIGNUP_PER_DOMAIN_PER_DAY: u32 = (\d+)', open('crates/server/src/ratelimit.rs').read()).group(1))")"
printf 'configured: %s per IP per 24h, %s per non-public domain per 24h\n' "$LIMIT_IP" "$LIMIT_DOMAIN"

# The domain rule, from a *different* address so the per-IP rule cannot be what answers.
db "insert into signup_attempts (ip, email_domain)
    select '198.51.100.9', 'ugm.ac.id' from generate_series(1, $LIMIT_DOMAIN)" > /dev/null
DOMAIN_STATUS="$(curl -sS -o "$TMP/domain-limited.json" -w '%{http_code}' -X POST "$BASE/auth/signup" \
  -H 'content-type: application/json' \
  -d "{\"email\":\"farm-$STAMP@ugm.ac.id\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Laptop\",\"platform\":\"macos\"}}")"
[ "$DOMAIN_STATUS" = "429" ] || fail "a full non-public domain must refuse with 429, got $DOMAIN_STATUS: $(cat "$TMP/domain-limited.json")"
printf 'a non-public domain at its limit  : %s %s\n' "$DOMAIN_STATUS" \
  "$(field '["code"]' < "$TMP/domain-limited.json")"
# The credential bucket (A12, 30/minute over every `/auth` route) answers 429 too, so a
# login from the same address is what tells the two apart: it does not touch the signup
# limiter, and a 401 proves the bucket is not what refused the signup above.
DOMAIN_LOGIN="$(curl -sS -o "$TMP/domain-login.json" -w '%{http_code}' -X POST "$BASE/auth/login" \
  -H 'content-type: application/json' \
  -d "{\"email\":\"nobody-domain-$STAMP@example.test\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Laptop\",\"platform\":\"macos\"}}")"
[ "$DOMAIN_LOGIN" != "429" ] \
  || fail "the credential bucket is saturated, so the 429 above proves nothing about the domain rule; re-run in a minute"
printf '  the same address can still log in: %s (so the bucket is not what refused it)\n' "$DOMAIN_LOGIN"

# The exemption: the same volume for gmail.com must NOT refuse, because a farm uses gmail
# too and a domain limit there stops no farm while blocking the product's own growth (A154).
db "insert into signup_attempts (ip, email_domain)
    select '198.51.100.9', 'gmail.com' from generate_series(1, $LIMIT_DOMAIN)" > /dev/null
GMAIL_STATUS="$(auth_post "$TMP/gmail.json" "$BASE/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"public-$STAMP@gmail.com\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Laptop\",\"platform\":\"macos\"}}")"
[ "$GMAIL_STATUS" = "201" ] || fail "a public mailbox domain must be exempt, got $GMAIL_STATUS: $(cat "$TMP/gmail.json")"
printf 'the same volume for gmail.com    : %s (exempt from the domain rule)\n' "$GMAIL_STATUS"

# The per-IP rule: this run's own address, at its limit.
db "delete from signup_attempts where ip = '127.0.0.1'" > /dev/null
db "insert into signup_attempts (ip, email_domain)
    select '127.0.0.1', 'phase7-$STAMP.example.test' from generate_series(1, $LIMIT_IP)" > /dev/null
SEEDED_ATTEMPTS="$(db "select count(*) from signup_attempts where ip = '127.0.0.1'")"
# Not `auth_post`: this request is *supposed* to be refused.
STATUS="$(curl -sS -o "$TMP/rate-limited.json" -w '%{http_code}' -X POST "$BASE/auth/signup" \
  -H 'content-type: application/json' \
  -d "{\"email\":\"one-too-many-$STAMP@$DOMAIN\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Laptop\",\"platform\":\"macos\"}}")"
[ "$STATUS" = "429" ] || fail "the per-IP limit must refuse with 429, got $STATUS: $(cat "$TMP/rate-limited.json")"
[ "$(field '["code"]' < "$TMP/rate-limited.json")" = "rate_limited" ] \
  || fail "the per-IP refusal must be rate_limited: $(cat "$TMP/rate-limited.json")"
# Which 429 this is. Two different controls answer the same way, and they are worth
# distinguishing: `ratelimit::auth_layer` is an in-process 30-requests-per-minute bucket over
# *all* the credential routes (A12), while A154's limiter is database-backed and covers
# signup alone. The auth bucket is the one a back-to-back re-run trips, and it would refuse
# a login too — so a login from the same address is the discriminator. A 401 (the account
# does not exist) proves the bucket is not what refused the signup above.
LOGIN_STATUS="$(curl -sS -o "$TMP/login.json" -w '%{http_code}' -X POST "$BASE/auth/login" \
  -H 'content-type: application/json' \
  -d "{\"email\":\"nobody-$STAMP@example.test\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Laptop\",\"platform\":\"macos\"}}")"
[ "$LOGIN_STATUS" != "429" ] \
  || fail "the credential bucket is saturated, so the 429 above proves nothing about A154's limiter; re-run in a minute"
printf 'attempts recorded for 127.0.0.1=%s (limit %s) | next signup -> %s %s | a login from the same address -> %s\n' \
  "$SEEDED_ATTEMPTS" "$LIMIT_IP" "$STATUS" "$(field '["code"]' < "$TMP/rate-limited.json")" "$LOGIN_STATUS"

printf '\n=== PASS\n'
