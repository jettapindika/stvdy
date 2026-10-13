#!/usr/bin/env bash
# A demo account with real data in it, driven entirely through the public API.
#
# **Why this exists.** The design's frames are *populated*: five courses, twelve
# sources, fourteen flashcards due, quiz feedback, a suggested plan. A fresh dev
# account renders the empty state of every screen, so "does this screen match the
# design" is unanswerable without data — and the alternative, seeding the database
# directly, would produce rows no code path can produce, which is how a screen ends
# up matching a design and failing on a real account.
#
# So this script is a **client**. It signs up, verifies, creates workspaces with
# course codes and exam dates, uploads sources, generates a deck and a quiz, grades
# cards, and stops. Everything it writes went through the same routes the app
# calls, which means the state it leaves behind is reachable by a student.
#
# **It does not invent numbers the app would not compute.** It grades cards rather
# than writing `due_at` directly, so a due count is one the scheduler produced; and
# it prints what it actually made at the end instead of claiming the design's
# figures. If a band is empty afterwards, that is the real state of the account.
#
# Usage:
#   scripts/seed-demo.sh                 # against http://localhost:8080
#   API=http://host:8080 scripts/seed-demo.sh
#   EMAIL=me@example.test scripts/seed-demo.sh
#
# Requires: curl, python3, and a running server. The generative steps need a
# working LLM provider — a stub is fine (`scripts/stub-llm.py`), and if it is
# missing the script says so and leaves the non-generative data in place.

set -euo pipefail

API="${API:-http://localhost:8080}"
EMAIL="${EMAIL:-demo-$(date +%s)@example.test}"
PASSWORD="${PASSWORD:-DemoPass123!}"
DEVICE_NAME="${DEVICE_NAME:-Seed script}"

say() { printf '\033[36m%s\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

# A JSON body's value, without jq: the shapes here are small and this script must
# run on a machine that has curl and python3 and nothing else.
field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for part in sys.argv[1].split("."):
    if not isinstance(value, dict):
        value = None
        break
    value = value.get(part)
print("" if value is None else value)
' "$1"
}

# `method path [json] [token]` — prints the body, fails loudly on a non-2xx.
api() {
  local method="$1" path="$2" body="${3:-}" token="${4:-}"
  local args=(-s -X "$method" "$API$path" -H 'content-type: application/json')
  [ -n "$token" ] && args+=(-H "authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")

  local out status
  out="$(curl "${args[@]}" -w $'\n%{http_code}')"
  status="${out##*$'\n'}"
  out="${out%$'\n'*}"
  case "$status" in
    2*) printf '%s' "$out" ;;
    *) die "$method $path -> $status: $out" ;;
  esac
}

# `POST /workspaces/:id/sources` is multipart, not JSON — it carries a file, a
# pasted text or a URL in the same shape. A JSON body is refused with
# "Invalid `boundary` for multipart/form-data request", so this is its own helper
# rather than a call through `api`.
upload_text() {
  local workspace="$1" title="$2" text="$3" out status
  out="$(curl -s -X POST "$API/workspaces/$workspace/sources" \
    -H "authorization: Bearer $TOKEN" \
    -F "kind=text" -F "title=$title" -F "text=$text" \
    -w $'\n%{http_code}')"
  status="${out##*$'\n'}"
  out="${out%$'\n'*}"
  case "$status" in
    2*) printf '%s' "$out" ;;
    *) die "uploading a source -> $status: $out" ;;
  esac
}

# `method path [json] [token]` — the status and body, without dying. For a call
# whose refusal is a fact about the account rather than a fault in this script: the
# workspace limit is the one case, and a script that died there would be a script
# that only works on a pro account.
api_soft() {
  local method="$1" path="$2" body="${3:-}" token="${4:-}"
  local args=(-s -X "$method" "$API$path" -H 'content-type: application/json')
  [ -n "$token" ] && args+=(-H "authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")

  local out
  out="$(curl "${args[@]}" -w $'\n%{http_code}')"
  printf '%s\n%s' "${out%$'\n'*}" "${out##*$'\n'}"
}

# Every JSON body is built by a Python program fed on **stdin** rather than through
# `-c "…"`. A `-c` form needs its own quoting inside a `$( )` inside a shell
# string, and the nested double quotes silently ate the braces out of the dict
# literals — the request arrived as `{'device': 'name': …}`, which the server
# correctly refused. A heredoc has no quoting to get wrong.
body() { python3 - "$@"; }

# An instant `days` from today at `hour:minute`, UTC, in the format the API takes.
# `date -v` is BSD and `date -d` is GNU; the dev machines are macOS and CI is
# Linux, so both forms are tried. An empty result means neither worked, and the
# caller dies rather than posting `""` as a timestamp.
at() {
  local days="$1" hour="$2" minute="${3:-00}" out
  out="$(date -u -v"+${days}d" -v"${hour}H" -v"${minute}M" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
      || date -u -d "+${days} days ${hour}:${minute}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
      || true)"
  [ -n "$out" ] || die "could not compute a date ${days} days out (neither BSD nor GNU date)"
  printf '%s' "$out"
}

# The dev server writes mail to var/mail instead of sending it (B3), so the
# verification token is read from the file the server just wrote.
verify_from_mail() {
  local address="$1" file token
  file="$(grep -rl -- "To: $address" var/mail 2>/dev/null | head -1)"
  [ -n "$file" ] || die "no mail for $address — is the server running from the repo root?"
  # The token is base64url, so it contains `-` and `_`: a `[A-Za-z0-9]*` pattern
  # stops at the first underscore and posts a truncated token, which the server
  # refuses as invalid — the same answer an expired link gives.
  token="$(grep -o 'token=[A-Za-z0-9_-]*' "$file" | head -1 | cut -d= -f2)"
  [ -n "$token" ] || die "no verification token in $file"
  # The token is an argument, not an environment variable: the shell already has a
  # `TOKEN` holding the *access* token by the time this runs, and reading the wrong
  # one produced a 400 that read like an expired link.
  api POST /auth/verify-email "$(body "$token" <<'PY'
import json, sys
print(json.dumps({'token': sys.argv[1]}))
PY
)" >/dev/null
}

say "Signing up $EMAIL against $API"
TOKEN="$(api POST /auth/signup "$(body "$EMAIL" "$PASSWORD" "$DEVICE_NAME" <<'PY'
import json, sys
print(json.dumps({
    'email': sys.argv[1],
    'password': sys.argv[2],
    'device': {'name': sys.argv[3], 'platform': 'web'},
}))
PY
)" | field access_token)"
[ -n "$TOKEN" ] || die "signup returned no access token"

# The heredocs cannot see the shell's variables, so the one value a Python body
# needs from the shell — the first source's id, further down — is exported there.
verify_from_mail "$EMAIL"
say "Verified."

# The courses the design's sidebar and cards show. A code, a name and an exam date
# are all real fields now (`migrations/0038`), and `color` is assigned by the
# server, so this script does not choose one.
COURSES=(
  "Cell biology|BIO 201|2026-11-05"
  "Statistics|STAT 102|2026-10-30"
  "Cryptography|CS 304|2026-11-02"
  "Cognitive psychology|PSY 210|2026-11-05"
  "Computer networks|CS 302|2026-10-29"
)

# A paragraph per course, so retrieval has something to retrieve and the citations
# in a chat answer point at real text.
SOURCE_TEXT='Membrane transport is how a cell moves material across its boundary. Diffusion needs no energy: a substance moves down its own gradient until the two sides agree. Osmosis is the special case where the substance is water, and it is the reason a red blood cell swells in fresh water.

Facilitated diffusion still needs no energy, but it needs a protein. A channel lets an ion through a pore; a carrier changes shape to move its passenger. The distinction matters because a poison that blocks the protein stops the transport while leaving the gradient intact.

Active transport runs against the gradient and therefore costs ATP. The sodium potassium pump moves three sodium ions out for every two potassium ions in, which is what makes the inside of the cell negative. That voltage is the basis of every nerve impulse in this course.'

# The courses this account may actually hold. The free plan caps workspaces at two,
# so a script that assumed five would die on the third and leave a half-seeded
# account behind. The refusal is read and reported instead, and everything below
# works with however many courses exist — which is also what makes this script
# usable to check the *empty* and *nearly empty* states, not just the populated one.
WORKSPACES=()
for entry in "${COURSES[@]}"; do
  IFS='|' read -r name code exam <<<"$entry"
  say "Creating $code — $name"
  result="$(api_soft POST /workspaces "$(body "$name" "$code" "$exam" <<'PY'
import json, sys
print(json.dumps({
    'name': sys.argv[1],
    'course_code': sys.argv[2],
    'exam_date': sys.argv[3],
}))
PY
)" "$TOKEN")"
  result_body="${result%$'\n'*}"
  result_status="${result##*$'\n'}"
  case "$result_status" in
    201)
      workspace="$(printf '%s' "$result_body" | field workspace.id)"
      WORKSPACES+=("$workspace")
      ;;
    422)
      say "  the plan allows ${#WORKSPACES[@]} workspaces — stopping here."
      say "  Upgrade the account (Settings → Plan, or POST /admin/users/:id/plan) and"
      say "  re-run to seed the rest. What follows uses the courses that exist."
      break
      ;;
    *)
      die "POST /workspaces -> $result_status: $result_body"
      ;;
  esac

  # A pasted-text source: it needs no object storage, so this script does not
  # depend on MinIO being up.
  upload_text "$workspace" "Lecture notes — $name" "$SOURCE_TEXT" >/dev/null

  # An hour of study, so Today's study band and the dashboard have something real
  # to report rather than a zero.
  #
  # Two required fields, both for the same reason — a study session is written by
  # the device that ran the timer, so it carries its own id (A8, the client mints
  # it when the timer starts) and its own clock. The server refuses a session
  # longer than the gap between its start and that clock (A204). So this is a real
  # 08:00–09:00 block with the device clock at 09:00, not a pair that contradicts.
  api POST /sessions "$(body "$workspace" "$(at 0 8)" "$(at 0 9)" "$(python3 -c 'import uuid; print(uuid.uuid4())')" <<'PY'
import json, sys
print(json.dumps({
    'id': sys.argv[4],
    'workspace_id': sys.argv[1],
    'duration_min': 60,
    'started_at': sys.argv[2],
    'client_ts': sys.argv[3],
}))
PY
)" "$TOKEN" >/dev/null
done

# A second source on one course, so the "4 sources" vs "5 sources" distinction on
# the cards is visible rather than every course reading the same.
upload_text "${WORKSPACES[0]}" "Reading — chapter 9" "$SOURCE_TEXT" >/dev/null

say "Waiting for ingestion"
state=pending
for _ in $(seq 1 30); do
  state="$(api GET "/workspaces/${WORKSPACES[0]}/sources" '' "$TOKEN" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("sources", [])
print("ready" if rows and all(r.get("status") in ("ready", "failed") for r in rows) else "pending")
')"
  [ "$state" = "ready" ] && break
  sleep 2
done
[ "$state" = "ready" ] || die "ingestion did not settle in 60s"

# Tasks with real due dates and estimates: the planner's week summary and Today's
# deadlines band both read these, and every figure they show is a count of rows.
TASKS=(
  "Membrane transport lab report|0|18|120|1"
  "Probability problem set|1|17|90|1"
  "Crypto assignment|2|23|180|2"
  "Reading response: attention|3|20|60|2"
  "Subnetting exercises|4|19|75|3"
  "Revise for the BIO midterm|0|9|240|1"
  "Statistics revision cards|1|12|45|3"
)
for entry in "${TASKS[@]}"; do
  IFS='|' read -r title course hour estimate priority <<<"$entry"
  # A task is filed against a course, so a task for a course this account could not
  # create is skipped rather than filed against nothing. The count printed at the
  # end reports what was actually made.
  [ -n "${WORKSPACES[$course]:-}" ] || continue
  due="$(at "$course" "$hour" 00)"
  api POST /tasks "$(body "$title" "${WORKSPACES[$course]}" "$due" "$estimate" "$priority" <<'PY'
import json, sys
print(json.dumps({
    'title': sys.argv[1],
    'workspace_id': sys.argv[2],
    'due_at': sys.argv[3],
    'estimate_min': int(sys.argv[4]),
    'priority': int(sys.argv[5]),
}))
PY
)" "$TOKEN" >/dev/null
done

# A class and a study block on each of the next three days, so the timeline has
# segments and the "now" rule has something to fall between.
for day in 0 1 2; do
  # The same guard as the tasks: an event belongs to a course, and a course this
  # account could not create has no events.
  [ -n "${WORKSPACES[$day]:-}" ] || continue
  IFS='|' read -r course_name _ _ <<<"${COURSES[$day]}"
  for slot in "09|0|Lecture" "14|0|Study block" "16|30|Seminar"; do
    IFS='|' read -r hour minute kind <<<"$slot"
    start="$(at "$day" "$hour" "$minute")"
    end="$(at "$day" "$((10#$hour + 1))" "$minute")"
    api POST /events "$(body "$kind" "$course_name" "${WORKSPACES[$day]}" "$start" "$end" <<'PY'
import json, sys
print(json.dumps({
    'title': f"{sys.argv[1]} — {sys.argv[2]}",
    'kind': 'class',
    'workspace_id': sys.argv[3],
    'starts_at': sys.argv[4],
    'ends_at': sys.argv[5],
}))
PY
)" "$TOKEN" >/dev/null
  done
done

# A deck and a quiz. Both are generative and both spend credits, which is why they
# are last: if the provider is missing, everything above is still a usable account.
say "Generating a deck (spends credits)"
SOURCE_ID="$(api GET "/workspaces/${WORKSPACES[0]}/sources" '' "$TOKEN" | python3 -c '
import json, sys
print(json.load(sys.stdin)["sources"][0]["id"])
')"
export SOURCE_ID
DECK_ID="$(api POST "/workspaces/${WORKSPACES[0]}/decks" "$(body <<'PY'
import json, os
print(json.dumps({
    'title': 'Membrane transport',
    'source_ids': [os.environ['SOURCE_ID']],
    'count': 10,
}))
PY
)" "$TOKEN" | field deck.id)"

say "Generating a quiz (spends credits)"
api POST "/workspaces/${WORKSPACES[0]}/quizzes" "$(body <<'PY'
import json, os
print(json.dumps({
    'title': 'Cell biology check',
    'source_ids': [os.environ['SOURCE_ID']],
    'count': 5,
    'difficulty': 'medium',
    'types': 'mcq',
}))
PY
)" "$TOKEN" >/dev/null

# The deck generates asynchronously; wait for its cards so Study and Today show a
# real count rather than "Generating".
say "Waiting for the deck"
deck_state=""
for _ in $(seq 1 60); do
  deck_state="$(api GET "/decks/$DECK_ID" '' "$TOKEN" | python3 -c '
import json, sys
deck = json.load(sys.stdin)["deck"]
print(deck["status"])
')"
  case "$deck_state" in
    ready) break ;;
    failed) die "deck generation failed — is the LLM provider reachable?" ;;
  esac
  sleep 2
done
[ "$deck_state" = "ready" ] || die "the deck did not finish generating in 120s"

# Grading is what moves a card's due date, and it is the only honest way to produce
# a due count: writing `due_at` directly would show a number the scheduler never
# produced. Grading a card `good` schedules it into the future, so this leaves a
# deck that is ready to review rather than a pile of overdue cards.
CARDS="$(api GET "/decks/$DECK_ID" '' "$TOKEN" | python3 -c '
import json, sys
cards = json.load(sys.stdin)["cards"]
print(" ".join(c["id"] for c in cards[:6]))
')"
graded=0
for card in $CARDS; do
  api POST "/flashcards/$card/review" '{"rating":"good"}' "$TOKEN" >/dev/null
  graded=$((graded + 1))
done

say ""
say "Done. Sign in as $EMAIL / $PASSWORD"
# Counted from what the API returns rather than from what this script intended: a
# summary that reports the plan of the script is a summary that can be wrong about
# the account, which is the one thing this whole file is careful not to do.
made_sources="$(api GET "/workspaces/${WORKSPACES[0]}/sources" '' "$TOKEN" | python3 -c '
import json, sys
print(len(json.load(sys.stdin)["sources"]))
')"
made_tasks="$(api GET /tasks '' "$TOKEN" | python3 -c '
import json, sys
print(len(json.load(sys.stdin)["tasks"]))
')"
# The events loop posts three per day only for the days a course exists — the free
# plan stops at two courses, so it is six events, not nine. Reading the count back
# is the same rule as the two above: a literal here would be a figure about the
# account that only the script's own plan could support.
made_events="$(api GET "/events?from=$(at 0 0 00)" '' "$TOKEN" | python3 -c '
import json, sys
print(len(json.load(sys.stdin)["events"]))
')"
# The deck and the quiz are the one pair that stays a literal: each is posted once,
# unconditionally, and `api` dies on a non-2xx, so reaching this line means exactly
# one of each exists. There is no branch that skips them and no loop that repeats
# them, so unlike the events there is no second value for these to be wrong about.
say "Created ${#WORKSPACES[@]} courses, ${made_sources} sources on the first course, ${made_tasks} tasks, ${made_events} events, 1 deck, 1 quiz, $graded graded reviews."
say ""
say "Every figure the screens show is computed from those rows, not from the design."
say "If a band is empty, that is this account's real state."
