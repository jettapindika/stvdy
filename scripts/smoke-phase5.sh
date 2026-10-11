#!/usr/bin/env bash
# Phase 5 end-to-end smoke test: quiz generation, server-side MCQ grading, the
# derived score, and the weak-topic summary.
#
# Drives the real server over HTTP against the local Postgres, with a STUB model
# server that speaks real HTTP (scripts/stub-llm.py) and answers with a fixed
# questions JSON. The stub is deliberate: the Rust suite replaces the provider at
# the trait boundary, so it cannot catch a wire-format mistake in the generation
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
EMAIL="phase5-$(date +%s)@example.test"
PASSWORD="correct horse battery"

# The stub answers with this string, so the quiz's question count, text and answer
# are fixed. Label `1` resolves against the one chunk the prompt sends, and the
# MCQ's `answer` is copied verbatim from its options, which the contract requires
# and the grader depends on.
QUESTIONS_JSON='{"questions":[{"kind":"mcq","prompt":"Apa fungsi three-way handshake?","options":["Membuka koneksi TCP.","Menutup koneksi TCP.","Mengenkripsi data.","Mengompres data."],"answer":"Membuka koneksi TCP.","explanation":"Handshake menegosiasikan parameter sebelum data dikirim.","label":1,"key_points":null},{"kind":"short","prompt":"Mengapa UDP lebih cepat daripada TCP?","options":null,"answer":"Karena UDP tidak memakai handshake.","explanation":"Tanpa handshake tidak ada putaran tambahan sebelum data dikirim.","label":1,"key_points":["tidak ada handshake","tidak menjamin urutan paket"]}]}'

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
STUB_LOG="${STUB_LOG:-var/stub-requests.jsonl}"
rm -f "$STUB_LOG"

if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
  # Already listening: reuse it. It must have been started with the questions
  # answer below, or generation will not parse.
  echo "a stub is already listening on $STUB_PORT; reusing it"
  echo "it must have been started with the questions JSON answer"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    --answer "$QUESTIONS_JSON" > "$TMP/stub.log" 2>&1 &
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

DOKUMEN="Three-way handshake adalah proses membuka koneksi TCP antara klien dan server. Klien mengirim paket SYN, server membalas dengan SYN-ACK, lalu klien mengirim ACK. UDP tidak memakai handshake sehingga lebih cepat tetapi tidak menjamin urutan paket."
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

say "create a quiz from the source"
curl -fsS -X POST "$BASE/workspaces/$WS/quizzes" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"title\":\"Jaringan Komputer\",\"source_ids\":[\"$SRC\"],\"count\":10,\"difficulty\":\"medium\",\"types\":\"mixed\"}" \
  > "$TMP/quiz.json"
QUIZ=$(field '["quiz"]["id"]' < "$TMP/quiz.json")
[ "$(field '["quiz"]["status"]' < "$TMP/quiz.json")" = "generating" ] \
  || fail "a fresh quiz must be generating"

say "poll until the quiz settles"
QUIZ_STATUS=unknown
for _ in $(seq 1 120); do
  curl -fsS "$BASE/quizzes/$QUIZ" -H "authorization: Bearer $ACCESS" > "$TMP/quiz-state.json"
  QUIZ_STATUS=$(field '["quiz"]["status"]' < "$TMP/quiz-state.json")
  case "$QUIZ_STATUS" in ready|failed) break ;; esac
  sleep 1
done
cat "$TMP/quiz-state.json"
[ "$QUIZ_STATUS" = "ready" ] \
  || fail "the quiz did not reach ready (status $QUIZ_STATUS): $(field '["quiz"]["error"]' < "$TMP/quiz-state.json"). Is the server pointed at the stub (LLM_BASE_URL=$STUB_URL/v1)?"
[ "$(field '["quiz"]["question_count"]' < "$TMP/quiz-state.json")" = "2" ] \
  || fail "the stub returned two questions, the quiz does not have exactly two"

say "every question is linked to the Indonesian chunk it cited"
python3 - "$TMP/quiz-state.json" <<'PY'
import json, sys

quiz = json.load(open(sys.argv[1]))
questions = quiz["questions"]
assert len(questions) == 2, questions

mcq, short = questions[0], questions[1]
assert mcq["kind"] == "mcq", mcq
assert len(mcq["options"]) == 4, mcq
assert mcq["answer"] in mcq["options"], "the answer must be one of the options, copied verbatim"
assert short["kind"] == "short", short
assert short["options"] is None, short
assert short["key_points"], "a short answer must carry key points to self-assess against"

# Roadmap gate 6: the questions are the Indonesian the model returned, and each is
# linked to a chunk rather than left unattributed.
for question in questions:
    assert question["chunk_id"], f"a question lost its chunk link: {question}"
    assert "three-way handshake" in question["prompt"] or "UDP" in question["prompt"], question
print(f"{len(questions)} question(s), all linked: {[q['kind'] for q in questions]}")
PY

say "answer the MCQ wrongly, then the short answer, and mark it"
MCQ_ID=$(field '["questions"][0]["id"]' < "$TMP/quiz-state.json")
SHORT_ID=$(field '["questions"][1]["id"]' < "$TMP/quiz-state.json")

# The wrong option, and the body claims it was right. The server grades the MCQ, so
# the claim must be ignored: this is the assertion that proves it.
curl -fsS -X POST "$BASE/quizzes/$QUIZ/attempts" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"question_id\":\"$MCQ_ID\",\"user_answer\":\"Menutup koneksi TCP.\",\"correct\":true}" \
  > "$TMP/attempt-1.json"
[ "$(field '["attempt"]["correct"]' < "$TMP/attempt-1.json")" = "False" ] \
  || fail "the server must grade the MCQ and ignore the client's claim: $(cat "$TMP/attempt-1.json")"

curl -fsS -X POST "$BASE/quizzes/$QUIZ/attempts" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"question_id\":\"$SHORT_ID\",\"user_answer\":\"Karena tidak ada handshake.\"}" \
  > "$TMP/attempt-2.json"
[ "$(field '["attempt"]["correct"]' < "$TMP/attempt-2.json")" = "None" ] \
  || fail "an unmarked short answer must store null, not a guess: $(cat "$TMP/attempt-2.json")"

say "an unmarked answer does not lower the score, and marking it does"
curl -fsS "$BASE/quizzes/$QUIZ/results" -H "authorization: Bearer $ACCESS" > "$TMP/results-1.json"
python3 - "$TMP/results-1.json" <<'PY'
import json, sys

summary = json.load(open(sys.argv[1]))["summary"]
print(f"answered={summary['answered']} graded={summary['graded']} score={summary['score']}")
assert summary["total"] == 2, summary
assert summary["answered"] == 2, "both questions have an answer submitted"
assert summary["graded"] == 1, "only the MCQ has been judged"
assert summary["score"] == 0.0, summary
# The denominator is the graded attempts, so one wrong MCQ is 0 of 1, not 0 of 2.
assert summary["topics"], "the wrong answer must produce a weak topic"
PY

curl -fsS -X POST "$BASE/quizzes/$QUIZ/attempts" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"question_id\":\"$SHORT_ID\",\"user_answer\":\"Karena tidak ada handshake.\",\"correct\":true}" \
  > "$TMP/attempt-3.json"

say "the server's score and a local replay of the same answers agree"
curl -fsS "$BASE/quizzes/$QUIZ/results" -H "authorization: Bearer $ACCESS" > "$TMP/results-2.json"
python3 - "$TMP/results-2.json" "$SRC" <<'PY'
import json, sys

results = json.load(open(sys.argv[1]))
summary = results["summary"]
source_id = sys.argv[2]

# The two graded answers, in the order they were given: the MCQ wrong, the short
# answer right. `crates/core/src/quiz.rs` computes exactly this.
graded = [False, True]
correct = sum(1 for value in graded if value)
score = correct / len(graded)

print(f"server: score={summary['score']} correct={summary['correct']}/{summary['graded']}")
print(f"replay: score={score} correct={correct}/{len(graded)}")

assert summary["graded"] == 2, summary
assert summary["correct"] == correct, (summary["correct"], correct)
assert abs(summary["score"] - score) < 1e-9, (summary["score"], score)

# The weak-topic summary is grouped by **place in the document**, not by question: the
# key is (source_id, page). Both stub questions cite label 1, so both are one topic, and
# this asserts exactly that. A grouping by question would produce two rows here and would
# tell a student nothing about where to look again.
topics = summary["topics"]
assert len(topics) == 1, f"both questions came from one place, so there is one topic: {topics}"
assert topics[0]["source_id"] == source_id, (topics[0], source_id)
assert topics[0]["graded"] == 2, topics[0]
assert topics[0]["correct"] == 1, topics[0]
# The topic is keyed by (source_id, page). A `kind=text` source has no pages, so its page
# is null and its chunks group together: that is the correct grouping, not a missing
# value. What must hold is that the topic names the source.
assert topics[0]["source_title"], "a topic carries the source it came from"
print(f"one topic: {topics[0]['source_title']} page {topics[0]['page']} "
      f"({topics[0]['correct']}/{topics[0]['graded']})")
PY

say "the quiz is resumable: the questions come back with what was already answered"
# The attempt is stored, so a reload or a second device must be able to pick the quiz up
# rather than restart at question 1. `GET /quizzes/:id` is the call the screen makes on
# load, so the evidence is that it returns the attempt alongside the question.
curl -fsS "$BASE/quizzes/$QUIZ" -H "authorization: Bearer $ACCESS" > "$TMP/resume.json"
python3 - "$TMP/resume.json" <<'PY'
import json, sys

quiz = json.load(open(sys.argv[1]))
mcq, short = quiz["questions"][0], quiz["questions"][1]

assert mcq["attempt"] is not None, "the answered MCQ must carry its attempt back"
assert mcq["attempt"]["user_answer"] == "Menutup koneksi TCP.", mcq["attempt"]
assert mcq["attempt"]["correct"] is False, "the server's grade is what is stored"
assert short["attempt"] is not None, "the marked short answer must carry its attempt back"
assert short["attempt"]["correct"] is True, short["attempt"]
print("resumed: mcq answered and graded, short answer answered and marked")
PY

say "an attempt carries the device's clock, and that clock decides which is latest"
# The attempt already stored was stamped by the server (no `answered_at` was sent). This
# posts a *later* answer with an explicit client timestamp and asserts the client's clock
# is what orders them: without the field, both would carry the server's instant and the
# winner would be a coin flip.
python3 - "$TMP/resume.json" "$TMP/next_clock.txt" <<'PY'
import json, sys
from datetime import datetime, timedelta

attempt = json.load(open(sys.argv[1]))["questions"][0]["attempt"]
stored = datetime.fromisoformat(attempt["answered_at"].replace("Z", "+00:00"))
# An hour later, which is inside the 24h tolerance and later than anything the server
# has stamped, so it must become the reported attempt.
later = stored + timedelta(hours=1)
open(sys.argv[2], "w").write(later.strftime("%Y-%m-%dT%H:%M:%SZ"))
print(f"the stored attempt is stamped {attempt['answered_at']}; sending {later.isoformat()}")
PY
LATER_CLOCK="$(cat "$TMP/next_clock.txt")"

curl -fsS -X POST "$BASE/quizzes/$QUIZ/attempts" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"question_id\":\"$MCQ_ID\",\"user_answer\":\"Membuka koneksi TCP.\",\"answered_at\":\"$LATER_CLOCK\"}" \
  > "$TMP/clock.json"
[ "$(field '["attempt"]["answered_at"]' < "$TMP/clock.json")" = "$LATER_CLOCK" ] \
  || fail "the device's clock must be stored as given: $(cat "$TMP/clock.json")"

curl -fsS "$BASE/quizzes/$QUIZ" -H "authorization: Bearer $ACCESS" > "$TMP/resume2.json"
python3 - "$TMP/resume2.json" "$LATER_CLOCK" <<'PY'
import json, sys

attempt = json.load(open(sys.argv[1]))["questions"][0]["attempt"]
assert attempt["answered_at"] == sys.argv[2], (
    "the attempt stamped latest must be the one reported: " + json.dumps(attempt)
)
assert attempt["user_answer"] == "Membuka koneksi TCP.", attempt
assert attempt["correct"] is True, "the server grades the MCQ it was just sent"
print(f"latest attempt: answered_at={attempt['answered_at']} correct={attempt['correct']}")
PY

# A device whose clock is years ahead must not be able to pin itself to the top of every
# read.
FUTURE_STATUS=$(curl -s -o "$TMP/future.json" -w '%{http_code}' -X POST "$BASE/quizzes/$QUIZ/attempts" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"question_id\":\"$MCQ_ID\",\"user_answer\":\"Membuka koneksi TCP.\",\"answered_at\":\"2099-01-01T00:00:00Z\"}")
[ "$FUTURE_STATUS" = "400" ] || fail "an attempt years in the future must be refused, got $FUTURE_STATUS"

say "a replayed attempt id applies once"
ATTEMPT_ID=$(python3 -c 'import uuid; print(uuid.uuid4())')
REPLAY_BODY="{\"id\":\"$ATTEMPT_ID\",\"question_id\":\"$MCQ_ID\",\"user_answer\":\"Membuka koneksi TCP.\"}"
curl -fsS -X POST "$BASE/quizzes/$QUIZ/attempts" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d "$REPLAY_BODY" > "$TMP/replay-1.json"
[ "$(field '["applied"]' < "$TMP/replay-1.json")" = "True" ] || fail "the first attempt was not applied"
curl -fsS -X POST "$BASE/quizzes/$QUIZ/attempts" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d "$REPLAY_BODY" > "$TMP/replay-2.json"
[ "$(field '["applied"]' < "$TMP/replay-2.json")" = "False" ] \
  || fail "a replayed attempt id must be ignored rather than applied twice"

say "the generation's usage row names the action, the prompt version, and the cost"
# `record_usage` is best-effort for the *reporting* half, so this asserts it ran:
# the action and the prompt version are the fields the job controls, and the
# provider cost is the one the margin report reads (A41). `scripts/stub-llm.py`
# answers a non-streaming request with a `usage` block, so the cost must be present;
# a provider that omits one leaves it null, which is why this is asserted here
# against a stub whose reply is known.
if [ -n "$PSQL" ]; then
  # `.env` supplies DATABASE_URL when the caller did not, and must not override an
  # explicit one: a run against a scratch database would otherwise be answered from
  # the shared dev database and report a missing row that is present.
  #
  # Read the one key rather than sourcing the file. Sourcing executes it, and a line as
  # ordinary as `LLM_BASE_URL= https://host` (a space after `=`) makes bash run the URL
  # as a command: under `set -e` that aborts the run, and under `set +e` it silently
  # empties the variable. This script needs one value, so it reads one value.
  if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
    DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
    export DATABASE_URL
  fi
  [ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"
  ROW=$("$PSQL" "$DATABASE_URL" -tAc "select action || '|' || coalesce(prompt_version, '') || '|' || case when provider_cost_usd is null then 'no-cost' else 'cost' end || '|' || case when credits_spent > 0 then 'still-charged' else 'consumed' end from usage_events ue join quizzes q on q.usage_event_id = ue.id where ue.user_id = (select id from users where email = '$EMAIL') and ue.action = 'generate_quiz' order by ue.created_at desc limit 1")
  [ -n "$ROW" ] || fail "the generation left no usage row for generate_quiz"
  [ "$ROW" = "generate_quiz|quiz.v1|cost|consumed" ] \
    || fail "the usage row is not the expected one: $ROW (want generate_quiz|quiz.v1|cost|consumed)"
  printf 'usage row: %s\n' "$ROW"
else
  echo "no psql on PATH; skipped the usage-row assertion"
fi

say "the generation called the model with the composed prompt"
# The stub writes every request body it served, so the smoke run can assert the
# request was non-streaming (the shape `generate_quiz` uses), carried the label
# block, and filled every variable the module declares as required.
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
prompt = body["messages"][0]["content"]
for expected in ["three-way handshake", "[1]", "difficulty", "medium", "mixed"]:
    assert expected in prompt, f"the prompt is missing {expected!r}"
assert "{{" not in prompt, "no unfilled variable may reach the model"
assert "<grounding_and_citations>" in prompt, "the grounding section is the guard rail"
assert "<security_untrusted_content>" in prompt, "the security section is the guard rail"
print("the prompt carried the source, its labels, the settings and both guard rails")
PY

printf '\n=== PASS\n'
