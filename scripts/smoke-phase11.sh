#!/usr/bin/env bash
# Phase 11 end-to-end smoke test: the syllabus import, the review screen's data, the
# deterministic planner and the accept that writes the blocks — against the real server, the
# real Postgres and a stub model.
#
# Drives the real process over HTTP and reads the real database back, because the parts this
# phase can get wrong are exactly the parts a trait-level double cannot see:
#
#   * that **nothing** reaches `events`, `tasks` or `goals` until the student confirms, and
#     that a confirm writes rows for the kept indices and no others;
#   * that the review list arrives **pre-ordered, flagged, and with no score** (A261) — the
#     honesty rule is about a number that must not reach the screen, and only a real response
#     can show it is absent;
#   * that an exam at **06:00 Jakarta** lands on the previous UTC day and still reads back as
#     the 16th in Jakarta — the half of gate 6 the roadmap singles this phase out for, and the
#     half a UTC-bucketed implementation gets wrong;
#   * that the accepted plan respects the study window, the daily maximum and busy time, and
#     that a replan **keeps the pinned blocks exactly** (contract § 6 step 7), which is what
#     makes the preview's drag-adjust safe.
#
# The Rust suites assert all of these too. This run is the one that proves the *wired process*
# does, and it prints the numbers it asserted so the output is evidence rather than a green
# word.
#
# Nine scenarios, in order:
#   1. a fresh account on free: the plan refusal by name, and the one allowed import
#   2. the second import: 422 limit_reached, and nothing written
#   3. the extraction: the review list, its order, its flag, and no score
#   4. the Jakarta pair, quoted
#   5. the confirm: rows for the kept items only, and the tables empty before it
#   6. the plan: deficit, warnings, and the window every block sits inside
#   7. the accept: the blocks as events, and the second accept refused
#   8. the replan: the accepted blocks come back pinned, unchanged
#   9. the prompts the model actually received, read out of the stub's log
#
# **Run this against a freshly started server.** Like Phase 7 it makes several requests to the
# credential routes, and `ratelimit::auth_layer` (A12) is an in-process bucket of 30 requests
# per minute per IP, so a second run inside the same server process may have to wait for cells.
# The script waits and says so rather than hanging.
#
# The stub must be started with `scripts/fixtures/phase11-answers.json`: the extraction and the
# effort estimate are both strict-JSON generators, so the Phase 2 default (a prose answer) would
# fail both. `just smoke-llm-phase11` does that.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
# The port the SERVER was started with (LLM_BASE_URL), not a private one: the server is what
# talks to the stub, so the stub this script watches must be the stub the server calls.
STUB_PORT="${STUB_PORT:-8100}"
# This must match the SERVER's `MAIL_DIR`, because the server is what writes the verification
# mail: the script only reads the newest `.eml` from it.
MAIL_DIR="${MAIL_DIR:-var/mail}"
PLANS="${PLANS:-config/plans.toml}"
# Where the stub appends every request body. The last scenario reads it, because the prompt the
# model received is not visible in any response.
STUB_LOG="${STUB_LOG:-var/stub-phase11-requests.jsonl}"
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
# `.env` supplies DATABASE_URL when the caller did not, and must not override an explicit one:
# a run against a scratch database would otherwise be answered from the shared dev database and
# report a missing row that is present.
if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
  DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
  export DATABASE_URL
fi
[ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"

db() { "$PSQL" "$DATABASE_URL" -tAc "$1"; }

# --- the plan matrix, quoted from the file the server charges from ------------------------
#
# Every number asserted below is read from `config/plans.toml` rather than written here, so a
# retuned config moves this run with it. A literal in this script would make the run pass
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
        f"{name}: planner={p['planner']}"
        f" syllabus_imports_total={p['syllabus_imports_total']}"
        f" credits_per_month={p['credits_per_month']}"
    )
PY
cat "$TMP/matrix.txt"
matrix() { python3 -c "
import sys
key = sys.argv[1]
for line in open('$TMP/matrix.txt'):
    if line.startswith(sys.argv[2] + ': ') and key in line:
        print(line.split(key + '=')[1].split()[0]); break
" "$1" "$2"; }
IMPORT_COST="$(python3 -c "
import tomllib
print(tomllib.load(open('$PLANS','rb'))['credits']['syllabus_import'])")"
PLAN_COST="$(python3 -c "
import tomllib
print(tomllib.load(open('$PLANS','rb'))['credits']['plan_generation'])")"
[ -n "$IMPORT_COST" ] || fail "could not read [credits] syllabus_import from $PLANS"
[ -n "$PLAN_COST" ] || fail "could not read [credits] plan_generation from $PLANS"
FREE_PLANNER="$(matrix planner free)"
FREE_IMPORTS="$(matrix syllabus_imports_total free)"
[ "$FREE_PLANNER" = "manual" ] || fail "free's planner is $FREE_PLANNER; scenario 1 asserts the refusal"
[ "$FREE_IMPORTS" = "1" ] || fail "free's syllabus_imports_total is $FREE_IMPORTS; scenario 2 asserts the limit"

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
    "$STUB_PORT" "scripts/fixtures/phase11-answers.json"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    --answers-file scripts/fixtures/phase11-answers.json > "$TMP/stub.log" 2>&1 &
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
  || fail "no server at $BASE. Start the stub with 'just smoke-llm-phase11' and the server with 'just smoke-server-phase11'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

# --- the account --------------------------------------------------------------------------
DOMAIN="phase11-$STAMP.example.test"
ACCESS=""

say "reset the loopback signup attempts (A154 counts 25 per IP per 24h, and every run here adds)"
PRUNED="$(db "with gone as (delete from signup_attempts where ip = '127.0.0.1' returning 1) select count(*) from gone")"
printf 'pruned %s signup_attempts row(s) for 127.0.0.1\n' "$PRUNED"

# A credential request, retried across the auth limiter's refill window.
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
    printf '      and re-run) or A154 per-IP signup limit is in play.\n' >&2
    exit 1
  fi
  printf '%s' "$status"
}

EMAIL="phase11-student-$STAMP@$DOMAIN"
say "1. a fresh account: signup, verify, and the free plan's structural refusal"
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
TZ_STORED="$(db "select timezone from users where id = '$UID_'")"
[ "$TZ_STORED" = "Asia/Jakarta" ] || fail "the default timezone is $TZ_STORED, not Asia/Jakarta"
printf 'account %s verified, id=%s timezone=%s\n' "$EMAIL" "$UID_" "$TZ_STORED"

curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Jaringan Komputer"}' > "$TMP/ws.json"
WS="$(field '["workspace"]["id"]' < "$TMP/ws.json")"
[ -n "$WS" ] || fail "no workspace created"

# A goal on free, so the refusal is asserted against the matrix's own `planner = "manual"`
# rather than against a plan id this script picked.
#
# **The goal is a real row, and that is a finding rather than a convenience.** The first
# version of this script sent a random uuid and expected 403; it got **404**, because
# `reserve_and_enqueue` resolves the goal through `goals::owned_goal` *before* the entitlement
# check, so a goal that does not exist is a 404 and never reaches the matrix. The ordering is
# deliberate (a foreign goal id must be a 404 rather than a 403 that confirms the row exists),
# so the refusal this scenario is about needs a goal the caller really owns.
FREE_GOAL="$(curl -fsS -X POST "$BASE/goals" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"title\":\"UAS Kripto\",\"target_date\":\"2026-12-15T23:00:00Z\",\"workspace_id\":\"$WS\",\"source_ids\":[]}" \
  | field '["goal"]["id"]')"
[ -n "$FREE_GOAL" ] || fail "no goal created on the free plan"
STATUS="$(curl -sS -o "$TMP/free-plan.json" -w '%{http_code}' -X POST \
  "$BASE/goals/$FREE_GOAL/plan" -H "authorization: Bearer $ACCESS")"
[ "$STATUS" = "403" ] || fail "a free account's plan must be 403, got $STATUS: $(cat "$TMP/free-plan.json")"
[ "$(field '["code"]' < "$TMP/free-plan.json")" = "plan_required" ] \
  || fail "the refusal must be plan_required: $(cat "$TMP/free-plan.json")"
[ "$(field '["min_plan"]' < "$TMP/free-plan.json")" = "pro" ] \
  || fail "the refusal must name the plan that grants it: $(cat "$TMP/free-plan.json")"
# And the refusal is before the charge and before the row: a refused plan must leave neither.
FREE_LEDGER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
FREE_PROPOSALS="$(db "select count(*) from plan_proposals where user_id = '$UID_'")"
[ "$FREE_PROPOSALS" = "0" ] || fail "a refused plan wrote $FREE_PROPOSALS proposal row(s)"
printf 'free plan, POST /goals/<id>/plan -> %s %s min_plan=%s (proposals=%s, balance=%s)\n' "$STATUS" \
  "$(field '["code"]' < "$TMP/free-plan.json")" "$(field '["min_plan"]' < "$TMP/free-plan.json")" \
  "$FREE_PROPOSALS" "$FREE_LEDGER"

# ==========================================================================================
# 2 and 3. The syllabus: the first import is allowed, the second is refused by count
# ==========================================================================================
SYLLABUS_TEXT='Jaringan Komputer IF2230
Kuliah Senin 08:00 ruang 3
Praktikum Rabu 13:00
Tugas 1 dikumpulkan 12 Nov pukul 23:59
Tugas 2 dikumpulkan 3 Des pukul 23:59
UAS 15 Desember pukul 06:00
Materi UAS kumulatif'

say "2. the import on free: ${IMPORT_COST} credits, and a draft that is not yet a row"
TASKS_BEFORE="$(db "select count(*) from tasks where user_id = '$UID_'")"
GOALS_BEFORE="$(db "select count(*) from goals where user_id = '$UID_'")"
EVENTS_BEFORE="$(db "select count(*) from events where user_id = '$UID_'")"
LEDGER_BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
STATUS="$(curl -sS -o "$TMP/upload.json" -w '%{http_code}' -X POST \
  "$BASE/workspaces/$WS/syllabus" -H "authorization: Bearer $ACCESS" \
  -F "text=$SYLLABUS_TEXT" -F "title=Silabus Jaringan")"
[ "$STATUS" = "202" ] || fail "the upload must be 202, got $STATUS: $(cat "$TMP/upload.json")"
DRAFT="$(field '["draft_id"]' < "$TMP/upload.json")"
SOURCE="$(field '["source_id"]' < "$TMP/upload.json")"
KIND="$(db "select kind from sources where id = '$SOURCE'")"
[ "$KIND" = "syllabus" ] || fail "the source kind is $KIND, not syllabus"
LEDGER_AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
[ "$LEDGER_AFTER" -eq "$((LEDGER_BEFORE - IMPORT_COST))" ] \
  || fail "the import charge is not $IMPORT_COST: $LEDGER_BEFORE -> $LEDGER_AFTER"
printf 'upload -> 202 draft=%s source=%s kind=%s | balance %s -> %s (%s credits)\n' \
  "$DRAFT" "$SOURCE" "$KIND" "$LEDGER_BEFORE" "$LEDGER_AFTER" "$IMPORT_COST"

say "poll the draft to ready"
DRAFT_STATUS=""
for _ in $(seq 1 60); do
  CODE="$(curl -sS -o "$TMP/draft.json" -w '%{http_code}' "$BASE/syllabus/$DRAFT/draft" \
    -H "authorization: Bearer $ACCESS")"
  if [ "$CODE" = "409" ]; then sleep 1; continue; fi
  [ "$CODE" = "200" ] || fail "GET /syllabus/$DRAFT/draft answered $CODE: $(cat "$TMP/draft.json")"
  DRAFT_STATUS="$(field '["status"]' < "$TMP/draft.json")"
  [ "$DRAFT_STATUS" = "ready" ] && break
  [ "$DRAFT_STATUS" = "failed" ] \
    && fail "the extraction failed: $(field '["error"]' < "$TMP/draft.json"). Is the server pointed at the stub (LLM_BASE_URL=$STUB_URL/v1)?"
  sleep 1
done
[ "$DRAFT_STATUS" = "ready" ] || fail "the draft did not reach ready (status $DRAFT_STATUS)"

say "3. the review list: pre-ordered, flagged, and carrying no score (A261)"
python3 - "$TMP/draft.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
items = body["items"]
assert items, body
# The order is the server's, and it is the point of the score the server does not send: the
# item it was least sure of is first. The stub's reply has confidences 0.95, 0.55, 0.4, 0.9,
# 0.85 — so the first item is the 0.4 deadline and the second the 0.55 meeting.
assert items[0]["kind"] == "deadline" and items[0]["title"] == "Tugas 1", items[0]
assert items[1]["kind"] == "class" and items[1]["title"] == "Praktikum Jaringan", items[1]
assert items[0]["needs_check"] is True, items[0]
assert items[1]["needs_check"] is True, items[1]
assert items[-1]["needs_check"] is False, items[-1]
for item in items:
    assert "confidence" not in item, f"the score reached the screen: {item}"
    assert item["evidence"], f"an item with no evidence quote cannot be checked: {item}"
print(f"items={len(items)} first={items[0]['title']!r} needs_check={sum(1 for i in items if i['needs_check'])}"
      f" no-confidence-field=yes evidence=present")
PY
[ "$(db "select count(*) from syllabus_drafts where id = '$DRAFT' and status = 'ready'")" = "1" ] \
  || fail "the draft is not ready in the database"

say "4. the Jakarta pair, quoted (gate 6)"
python3 - "$TMP/draft.json" <<'PY'
import json, sys
from datetime import datetime, timezone
from zoneinfo import ZoneInfo
items = json.load(open(sys.argv[1]))["items"]
exam = next(i for i in items if i["kind"] == "exam")
deadline = next(i for i in items if i["kind"] == "deadline")
jakarta = ZoneInfo("Asia/Jakarta")

# The fixture (`scripts/fixtures/phase11-answers.json`) states the exam at 06:00 Jakarta on
# the 15th of December, so the sharp property is that its UTC date is the **14th**.
assert exam["when"] == "2026-12-14T23:00:00Z", exam["when"]
exam_utc = datetime.fromisoformat(exam["when"].replace("Z", "+00:00"))
assert exam_utc.date().isoformat() == "2026-12-14", exam_utc
assert exam_utc.astimezone(jakarta).date().isoformat() == "2026-12-15", exam_utc
assert exam_utc.astimezone(jakarta).hour == 6, exam_utc

# The evening case, which a UTC-bucketing implementation gets right by accident: 23:59
# Jakarta on the 12th is 16:59Z on the same day.
assert deadline["when"] == "2026-11-12T16:59:00Z", deadline["when"]
deadline_utc = datetime.fromisoformat(deadline["when"].replace("Z", "+00:00"))
assert deadline_utc.astimezone(jakarta).date().isoformat() == "2026-11-12", deadline_utc

print("06:00 Jakarta on the 15th -> " + exam["when"]
      + " (UTC date 14, Jakarta date " + exam_utc.astimezone(jakarta).date().isoformat() + ")")
print("23:59 Jakarta on the 12th -> " + deadline["when"] + " (same UTC day)")
PY

say "2b. the second import is refused by count, and writes no draft"
STATUS="$(curl -sS -o "$TMP/upload2.json" -w '%{http_code}' -X POST \
  "$BASE/workspaces/$WS/syllabus" -H "authorization: Bearer $ACCESS" \
  -F "text=Second syllabus" -F "title=Silabus Kedua")"
[ "$STATUS" = "422" ] || fail "free's second import must be 422, got $STATUS: $(cat "$TMP/upload2.json")"
[ "$(field '["code"]' < "$TMP/upload2.json")" = "limit_reached" ] \
  || fail "the count refusal must be limit_reached: $(cat "$TMP/upload2.json")"
DRAFTS="$(db "select count(*) from syllabus_drafts where user_id = '$UID_'")"
[ "$DRAFTS" = "1" ] || fail "a refused import wrote a draft: $DRAFTS"
printf 'second import on free -> %s %s | drafts=%s\n' "$STATUS" \
  "$(field '["code"]' < "$TMP/upload2.json")" "$DRAFTS"

# ==========================================================================================
# 5. The confirm: the only writer of the rows
# ==========================================================================================
say "5. the confirm: nothing was written before it, and only the kept items after"
TASKS_NOW="$(db "select count(*) from tasks where user_id = '$UID_'")"
GOALS_NOW="$(db "select count(*) from goals where user_id = '$UID_'")"
EVENTS_NOW="$(db "select count(*) from events where user_id = '$UID_'")"
[ "$TASKS_NOW" = "$TASKS_BEFORE" ] && [ "$GOALS_NOW" = "$GOALS_BEFORE" ] \
  && [ "$EVENTS_NOW" = "$EVENTS_BEFORE" ] \
  || fail "an extraction wrote rows before the confirm: tasks $TASKS_BEFORE->$TASKS_NOW goals $GOALS_BEFORE->$GOALS_NOW events $EVENTS_BEFORE->$EVENTS_NOW"
printf 'before the confirm: tasks=%s goals=%s events=%s (unchanged since the upload)\n' \
  "$TASKS_NOW" "$GOALS_NOW" "$EVENTS_NOW"

# Keep everything except the second meeting: the drop is what proves the confirm writes the
# kept indices and no others.
python3 - "$TMP/draft.json" "$TMP/confirm.json" <<'PY'
import json, sys
items = json.load(open(sys.argv[1]))["items"]
dropped = next(i for i in items if i["title"] == "Praktikum Jaringan")
kept = [{"index": i["index"], "keep": True} for i in items if i["index"] != dropped["index"]]
json.dump({"items": kept}, open(sys.argv[2], "w"))
print(f"confirming {len(kept)} of {len(items)} items; dropped index {dropped['index']}")
PY
STATUS="$(curl -sS -o "$TMP/confirmed.json" -w '%{http_code}' -X POST \
  "$BASE/syllabus/$DRAFT/confirm" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d "@$TMP/confirm.json")"
[ "$STATUS" = "200" ] || fail "the confirm answered $STATUS: $(cat "$TMP/confirmed.json")"
python3 - "$TMP/confirmed.json" "$TMP/draft.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
items = json.load(open(sys.argv[2]))["items"]
# The expectations come from the draft the confirm was built from, not from a literal: the
# counts are a function of which items were kept, so a hardcoded number would stop asserting
# anything the moment the fixture changed.
kept = [i for i in items if i["index"] != next(x for x in items if x["title"] == "Praktikum Jaringan")["index"]]
deadlines = [i for i in kept if i["kind"] == "deadline"]
exams = [i for i in kept if i["kind"] == "exam"]
meetings = [i for i in kept if i["kind"] == "class"]
assert body["tasks"] == len(deadlines), (body, deadlines)
assert body["goals"] == len(exams), (body, exams)
assert body["events"] >= len(meetings), (body, meetings)
print(f"confirm -> events={body['events']} tasks={body['tasks']} goals={body['goals']} status={body['status']}"
      f" (kept {len(meetings)} meeting(s), {len(deadlines)} deadline(s), {len(exams)} exam(s))")
PY
MEETINGS="$(db "select count(*) from events where user_id = '$UID_' and kind = 'class'")"
DROPPED="$(db "select count(*) from events where user_id = '$UID_' and title = 'Praktikum Jaringan'")"
[ "$DROPPED" = "0" ] || fail "a dropped item was written: $DROPPED events for the dropped meeting"
printf 'class events=%s | the dropped meeting wrote %s rows\n' "$MEETINGS" "$DROPPED"
# A second confirm is a 409 rather than a second set of rows.
STATUS="$(curl -sS -o "$TMP/reconfirm.json" -w '%{http_code}' -X POST \
  "$BASE/syllabus/$DRAFT/confirm" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d "@$TMP/confirm.json")"
[ "$STATUS" = "409" ] || fail "a second confirm must be 409, got $STATUS: $(cat "$TMP/reconfirm.json")"
printf 'second confirm -> %s %s\n' "$STATUS" "$(field '["code"]' < "$TMP/reconfirm.json")"

# The exam's own goal, named rather than picked by `limit 1`: the free-plan refusal above
# created a goal of its own, and a bare `select id from goals` returns both rows.
GOAL_ID="$(db "select id from goals where user_id = '$UID_' and title = 'UAS Jaringan' and deleted_at is null")"
TARGET_UTC="$(db "select to_char(target_date at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') from goals where id = '$GOAL_ID'")"
TARGET_LOCAL="$(db "select to_char(target_date at time zone 'Asia/Jakarta', 'YYYY-MM-DD HH24:MI') from goals where id = '$GOAL_ID'")"
printf 'the exam goal: target_date %s UTC = %s Asia/Jakarta\n' "$TARGET_UTC" "$TARGET_LOCAL"

# The plan needs a paid plan: free's planner is `manual`.
say "upgrade the account to pro (the two rows Phase 13's checkout will write)"
db "update users set plan_id = 'pro' where id = '$UID_'" > /dev/null
db "insert into subscriptions (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
    values ('$UID_', 'pro', 'active', 'monthly', now(), now() + interval '30 days')" > /dev/null
PLAN="$(curl -fsS "$BASE/usage" -H "authorization: Bearer $ACCESS" | field '["plan"]')"
[ "$PLAN" = "pro" ] || fail "the account should read pro, got $PLAN"
printf 'plan=%s\n' "$PLAN"

# ==========================================================================================
# 6. The plan: the deficit, the warnings, and the window
# ==========================================================================================
say "6. POST /goals/<id>/plan -> 202, ${PLAN_COST} credits, then poll"
LEDGER_BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
STATUS="$(curl -sS -o "$TMP/plan.json" -w '%{http_code}' -X POST \
  "$BASE/goals/$GOAL_ID/plan" -H "authorization: Bearer $ACCESS")"
[ "$STATUS" = "202" ] || fail "the plan request must be 202, got $STATUS: $(cat "$TMP/plan.json")"
PROPOSAL="$(field '["proposal_id"]' < "$TMP/plan.json")"
LEDGER_AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
[ "$LEDGER_AFTER" -eq "$((LEDGER_BEFORE - PLAN_COST))" ] \
  || fail "the plan charge is not $PLAN_COST: $LEDGER_BEFORE -> $LEDGER_AFTER"

P_STATUS=""
for _ in $(seq 1 60); do
  curl -fsS "$BASE/plans/$PROPOSAL" -H "authorization: Bearer $ACCESS" > "$TMP/proposal.json"
  P_STATUS="$(field '["proposal"]["status"]' < "$TMP/proposal.json")"
  [ "$P_STATUS" = "ready" ] && break
  [ "$P_STATUS" = "failed" ] \
    && fail "the plan failed: $(field '["proposal"]["error"]' < "$TMP/proposal.json")"
  sleep 1
done
[ "$P_STATUS" = "ready" ] || fail "the proposal did not reach ready (status $P_STATUS)"

python3 - "$TMP/proposal.json" <<'PY'
import json, sys
from datetime import datetime
from zoneinfo import ZoneInfo
body = json.load(open(sys.argv[1]))["proposal"]
proposal = body["proposal"]
blocks = proposal["blocks"]
assert blocks, proposal
jakarta = ZoneInfo("Asia/Jakarta")
def local(iso):
    return datetime.fromisoformat(iso.replace("Z", "+00:00")).astimezone(jakarta)
for block in blocks:
    start, end = local(block["starts_at"]), local(block["ends_at"])
    assert start.hour >= 8, f"a block starts before 08:00 Jakarta: {start}"
    assert end.hour <= 22 or (end.hour == 0 and end.minute == 0), f"a block ends after 22:00 Jakarta: {end}"
# `day_load` names **every** day of the horizon — a day with capacity and nothing planned is a
# real row — so the property is that each block is on a day the planner named, not the converse.
# (The first version of this assertion had it backwards and failed against a correct server;
# the Rust gate's copy of it had the same mistake.)
named = {d["date"] for d in proposal["day_load"]}
assert named, proposal
for block in blocks:
    day = local(block["starts_at"]).date().isoformat()
    assert day in named, f"the block at {block['starts_at']} is on Jakarta's {day}, which day_load does not name"
with_blocks = sum(1 for d in named if any(local(b["starts_at"]).date().isoformat() == d for b in blocks))
print(f"blocks={len(blocks)} deficit_min={proposal['deficit_min']} warnings={proposal['warnings']}")
print("first block: " + blocks[0]["starts_at"] + " -> " + local(blocks[0]["starts_at"]).strftime("%Y-%m-%d %H:%M Asia/Jakarta"))
print(f"day_load: {len(named)} day(s) in the horizon, {with_blocks} of them holding a block")
print("day_load minutes: " + ", ".join(f"{d['date']}={d['planned_min']}/{d['capacity_min']}" for d in proposal["day_load"] if d["planned_min"] > 0))
PY

# ==========================================================================================
# 7. The accept
# ==========================================================================================
say "7. accept: the proposal becomes events, and a second accept is refused"
python3 - "$TMP/proposal.json" "$TMP/accept.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))["proposal"]
blocks = [{
    "starts_at": b["starts_at"],
    "ends_at": b["ends_at"],
    "goal_id": b["goal_id"],
    "topic": b["topic"],
    "kind": "review" if b["kind"] == "Review" else "new_material",
} for b in body["proposal"]["blocks"]]
json.dump({"proposal_id": body["id"], "blocks": blocks}, open(sys.argv[2], "w"))
print(f"accepting {len(blocks)} block(s)")
PY
BLOCKS_BEFORE="$(db "select count(*) from events where user_id = '$UID_' and kind = 'study_block'")"
STATUS="$(curl -sS -o "$TMP/accepted.json" -w '%{http_code}' -X POST "$BASE/plans/accept" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' -d "@$TMP/accept.json")"
[ "$STATUS" = "200" ] || fail "the accept answered $STATUS: $(cat "$TMP/accepted.json")"
BLOCKS_AFTER="$(db "select count(*) from events where user_id = '$UID_' and kind = 'study_block'")"
printf 'accept -> %s | study_block events %s -> %s\n' "$STATUS" "$BLOCKS_BEFORE" "$BLOCKS_AFTER"
ORPHANS="$(db "select count(*) from events where user_id = '$UID_' and kind = 'study_block' and goal_id is null")"
[ "$ORPHANS" = "0" ] || fail "$ORPHANS accepted block(s) have no goal_id"
printf 'every accepted block carries its goal (A263): orphans=%s\n' "$ORPHANS"
STATUS="$(curl -sS -o "$TMP/reaccept.json" -w '%{http_code}' -X POST "$BASE/plans/accept" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' -d "@$TMP/accept.json")"
[ "$STATUS" = "409" ] || fail "a second accept must be 409, got $STATUS: $(cat "$TMP/reaccept.json")"
printf 'second accept -> %s %s\n' "$STATUS" "$(field '["code"]' < "$TMP/reaccept.json")"

# A drag that the server refuses: the block moved onto a class event. The client refuses this
# locally too (A264 is the control, this is the courtesy).
#
# **It needs a fresh proposal, and that is the finding.** The first version of this scenario
# reused the proposal it had just accepted, so it hit the *status* arm — 409 "this proposal is
# accepted and cannot be accepted again" — and proved nothing about the block validation. The
# two arms are different code paths and the batch-validation one is the one A264 is about, so
# this asks for a second proposal and drags a block onto a real class event.
say "7b. a dragged block that overlaps a class is refused, and nothing is written"
CLASS_START="$(db "select starts_at from events where user_id = '$UID_' and kind = 'class' order by starts_at limit 1")"
[ -n "$CLASS_START" ] || fail "no class event to overlap; the confirm's meeting did not expand"
STATUS="$(curl -sS -o "$TMP/plan2.json" -w '%{http_code}' -X POST \
  "$BASE/goals/$GOAL_ID/plan" -H "authorization: Bearer $ACCESS")"
[ "$STATUS" = "202" ] || fail "the second plan request must be 202, got $STATUS: $(cat "$TMP/plan2.json")"
PROPOSAL2="$(field '["proposal_id"]' < "$TMP/plan2.json")"
for _ in $(seq 1 60); do
  curl -fsS "$BASE/plans/$PROPOSAL2" -H "authorization: Bearer $ACCESS" > "$TMP/proposal2.json"
  P2_STATUS="$(field '["proposal"]["status"]' < "$TMP/proposal2.json")"
  [ "$P2_STATUS" = "ready" ] && break
  [ "$P2_STATUS" = "failed" ] && fail "the second plan failed: $(cat "$TMP/proposal2.json")"
  sleep 1
done
[ "$P2_STATUS" = "ready" ] || fail "the second proposal did not reach ready (status $P2_STATUS)"
python3 - "$TMP/proposal2.json" "$CLASS_START" "$TMP/accept-overlap.json" <<'PY'
import json, sys
from datetime import datetime, timedelta, timezone
view = json.load(open(sys.argv[1]))["proposal"]
body = view["proposal"]
class_start = datetime.fromisoformat(sys.argv[2].replace(" ", "T"))
if class_start.tzinfo is None:
    class_start = class_start.replace(tzinfo=timezone.utc)
start = class_start + timedelta(minutes=10)
end = start + timedelta(minutes=50)
blocks = [{
    "starts_at": b["starts_at"],
    "ends_at": b["ends_at"],
    "goal_id": b["goal_id"],
    "topic": b["topic"],
    "kind": "review" if b["kind"] == "Review" else "new_material",
} for b in body["blocks"]]
# A **pinned** block is the one block the client may not move: `apply::resolve_pinned` names
# its committed row by the exact range the server supplied, so a moved pinned block finds
# nothing and refuses the whole batch with 409. The first version of this scenario dragged
# `blocks[0]`, which after a first accept *is* pinned, and got exactly that 409 — the server
# was right and the scenario was wrong. So the drag targets the first block that is not kept.
NIL = "00000000-0000-0000-0000-000000000000"
target = next(i for i, b in enumerate(blocks) if not (b["goal_id"] == NIL and b["topic"] == "Pinned"))
blocks[target] = {**blocks[target],
                  "starts_at": start.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                  "ends_at": end.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
json.dump({"proposal_id": view["id"], "blocks": blocks}, open(sys.argv[3], "w"))
print(f"moving block {target} onto the class at {sys.argv[2]}")
PY
BLOCKS_BEFORE_REFUSAL="$(db "select count(*) from events where user_id = '$UID_' and kind = 'study_block'")"
STATUS="$(curl -sS -o "$TMP/overlap.json" -w '%{http_code}' -X POST "$BASE/plans/accept" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' -d "@$TMP/accept-overlap.json")"
[ "$STATUS" = "400" ] || fail "an overlapping block must be 400 validation, got $STATUS: $(cat "$TMP/overlap.json")"
[ "$(field '["code"]' < "$TMP/overlap.json")" = "validation" ] \
  || fail "the overlap refusal must be validation: $(cat "$TMP/overlap.json")"
BLOCKS_AFTER_REFUSAL="$(db "select count(*) from events where user_id = '$UID_' and kind = 'study_block'")"
[ "$BLOCKS_AFTER_REFUSAL" = "$BLOCKS_BEFORE_REFUSAL" ] \
  || fail "a refused batch wrote $((BLOCKS_AFTER_REFUSAL - BLOCKS_BEFORE_REFUSAL)) row(s)"
printf 'an overlapping block -> %s %s | study_block events unchanged at %s\n' "$STATUS" \
  "$(field '["message"]' < "$TMP/overlap.json")" "$BLOCKS_AFTER_REFUSAL"

# ==========================================================================================
# 8. The replan: the accepted blocks come back pinned and unchanged
# ==========================================================================================
say "8. replan: the committed blocks are pinned, and a replan keeps them exactly (A265)"
LEDGER_BEFORE="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
STATUS="$(curl -sS -o "$TMP/replan.json" -w '%{http_code}' -X POST "$BASE/plan/replan" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"goal_id\":\"$GOAL_ID\"}")"
[ "$STATUS" = "202" ] || fail "the replan must be 202, got $STATUS: $(cat "$TMP/replan.json")"
REPLAN="$(field '["proposal_id"]' < "$TMP/replan.json")"
LEDGER_AFTER="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$UID_'")"
[ "$LEDGER_AFTER" -eq "$((LEDGER_BEFORE - PLAN_COST))" ] \
  || fail "the replan charge is not $PLAN_COST: $LEDGER_BEFORE -> $LEDGER_AFTER"
R_STATUS=""
for _ in $(seq 1 60); do
  curl -fsS "$BASE/plans/$REPLAN" -H "authorization: Bearer $ACCESS" > "$TMP/replan-ready.json"
  R_STATUS="$(field '["proposal"]["status"]' < "$TMP/replan-ready.json")"
  [ "$R_STATUS" = "ready" ] && break
  [ "$R_STATUS" = "failed" ] \
    && fail "the replan failed: $(field '["proposal"]["error"]' < "$TMP/replan-ready.json")"
  sleep 1
done
[ "$R_STATUS" = "ready" ] || fail "the replan did not reach ready (status $R_STATUS)"

# An **accepted** proposal stays `accepted`: its blocks are real rows in the student's calendar,
# and marking it superseded would say the plan was replaced when the study time is still there.
# The supersede rule is for the goal's *outstanding* proposals (`planning`/`ready`), which is
# what the first version of this assertion got wrong — it expected `superseded` and the server
# was right. The second proposal from 7b is the one that must be superseded by this replan.
OLD_STATUS="$(curl -fsS "$BASE/plans/$PROPOSAL" -H "authorization: Bearer $ACCESS" | field '["proposal"]["status"]')"
[ "$OLD_STATUS" = "accepted" ] || fail "the accepted proposal is $OLD_STATUS, not accepted"
STALE_STATUS="$(curl -fsS "$BASE/plans/$PROPOSAL2" -H "authorization: Bearer $ACCESS" | field '["proposal"]["status"]')"
[ "$STALE_STATUS" = "superseded" ] \
  || fail "the goal's outstanding proposal is $STALE_STATUS, not superseded"

# The request body, not the response: `$TMP/accepted.json` holds `{event_ids}` and the body the
# client sent is `$TMP/accept.json`. The first version read the response and got `KeyError:
# 'blocks'`, which is the shape of a test reading the wrong file rather than a server bug.
python3 - "$TMP/accept.json" "$TMP/replan-ready.json" <<'PY'
import json, sys
committed = json.load(open(sys.argv[1]))["blocks"]
replan = json.load(open(sys.argv[2]))["proposal"]["proposal"]["blocks"]
ranges = {(b["starts_at"], b["ends_at"]) for b in replan}
for block in committed:
    assert (block["starts_at"], block["ends_at"]) in ranges, (
        f"the replan dropped a committed block: {block['starts_at']} is not in the new plan")
pinned = [b for b in replan if b["pinned"]]
assert len(pinned) >= len(committed), (
    f"the replan must pin every committed block: {len(pinned)} pinned of {len(committed)} committed")
kept = [b for b in pinned if b["topic"] == "Pinned" and b["goal_id"] == "00000000-0000-0000-0000-000000000000"]
print(f"committed={len(committed)} pinned={len(pinned)} kept-with-nil-goal={len(kept)} of {len(replan)} block(s)")
PY
printf 'replan -> %s | the accepted proposal is %s, the outstanding one is %s\n' \
  "$R_STATUS" "$OLD_STATUS" "$STALE_STATUS"

# ==========================================================================================
# 9. The prompts the model actually received
# ==========================================================================================
#
# A response-only check cannot see this: the draft looks the same whether or not the prompt
# carried the student's zone, and a date resolved in UTC lands on the wrong day with nothing on
# screen to say so. So the run reads the stub's log, which is what the server actually sent.
say "9. the prompts, read out of $STUB_LOG"
[ -s "$STUB_LOG" ] || fail "the stub logged no request bodies at $STUB_LOG (was it started with --log?)"
python3 - "$STUB_LOG" <<'PY'
import json, sys
lines = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
extraction = [l for l in lines if "Extract schedule information from the provided syllabus text" in l["messages"][0]["content"]]
effort = [l for l in lines if "Estimate study effort for an exam goal" in l["messages"][0]["content"]]
assert extraction, "no syllabus-extraction prompt in the stub's log"
assert effort, "no effort-estimate prompt in the stub's log"

OPEN, CLOSE = "<user_context_data>", "</user_context_data>"
def inside_a_data_block(prompt, needle):
    at = prompt.find(needle)
    if at < 0:
        return False
    opened = prompt.rfind(OPEN, 0, at)
    return opened >= 0 and prompt.find(CLOSE, opened) > at

for name, requests in (("syllabus.v1", extraction), ("planner_effort.v1", effort)):
    prompt = requests[-1]["messages"][0]["content"]
    assert "Asia/Jakarta" in prompt, f"{name}: the prompt did not carry the student's zone"
    print(f"{name}: zone=Asia/Jakarta chars={len(prompt)}")

# The student's own document is data, not instructions: the composer wraps every retrieved
# source in `<sources>`, which is the delimiter that stops a syllabus from steering the model.
# (The first version of this check looked for `<user_context_data>`, which is the *variable*
# block the module declares — the document itself arrives as a source. Found by running it.)
syllabus = extraction[-1]["messages"][0]["content"]
assert "<sources>" in syllabus and "</sources>" in syllabus, "the document is not delimited"
for needle in ("Jaringan Komputer", "UAS 15 Desember"):
    assert inside_a_data_block(syllabus, needle) or (
        syllabus.rfind("<sources>", 0, syllabus.find(needle))
        < syllabus.find(needle)
        < syllabus.find("</sources>", syllabus.find(needle))
    ), (
        f"{needle!r} is interpolated outside a <sources> block, so the model reads the document "
        "as an instruction"
    )
# The goal title is the variable block the effort module declares and lists as untrusted. The
# title is the goal the plan was asked for (`UAS Jaringan`), not the topic (`Silabus Jaringan`).
goal_title = effort[-1]["messages"][0]["content"]
assert inside_a_data_block(goal_title, "UAS Jaringan"), (
    "the goal title must be delimited as data in the effort prompt"
)
print("syllabus.v1: the document sits inside <sources>; planner_effort.v1: the goal title inside <user_context_data>")
# Reported rather than asserted, because the fix is the prompt module's: `topics_json` carries
# the student's own **source titles**, and it is interpolated outside the data block. A source
# titled to look like an instruction is a student-controlled string reaching the model
# undelimited — the same class A254 fixed for `semester_context`. Not this slice's file.
topics_at = goal_title.find("Topics (each with its source size")
block_end = goal_title.find("</user_context_data>")
if topics_at > block_end:
    print("NOTE: topics_json sits outside <user_context_data>, and it carries the student's own "
          "source titles — a prompt-injection surface for the prompt module's owner, not this slice's")
PY

printf '\n=== PASS\n'
