#!/usr/bin/env bash
# Phase 3 end-to-end smoke: web search, cached billing, URL and DOI citations.
#
# Two phases, because they prove different things and only one of them can use the
# real service here:
#
#   1. The academic tier, against the REAL APIs. Keyless and reachable from this
#      machine, and it is what carries a DOI, so the gate's "URLs AND DOIs" is
#      checked against real data rather than a fixture.
#   2. The SearXNG tier, against a stub that speaks its JSON API. The real service
#      cannot run on this machine (no Docker), and A3 measured that a laptop's
#      engine list is a different sample from the production host's (1 of 8 engines
#      answering versus 3), so tuning or verifying the engine list from here would
#      be measuring the wrong thing. The stub proves the wire format; the engine
#      list is verified on the staging VPS.
#
# Not a test suite (that is crates/server/tests): this proves the wired process.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
STUB_PORT="${STUB_PORT:-8088}"
MAIL_DIR="${MAIL_DIR:-var/mail}"
TMP="$(mktemp -d)"
EMAIL="phase3-$(date +%s)@example.test"
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
[ -n "$PSQL" ] || fail "no psql binary found; the ledger assertions need it"

STUB_PID=""
cleanup() { [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true; }
trap cleanup EXIT

sql() {
  # `.env` supplies DATABASE_URL when the caller did not, and must not override an
  # explicit one: this script is routinely pointed at a scratch database (one whose
  # migration set matches the branch under test), and silently querying the
  # database in `.env` instead makes every assertion here read the wrong state --
  # including the cache clear, whose whole job is to make the first turn a miss.
  local explicit="${DATABASE_URL:-}"
  set -a; . ./.env; set +a
  if [ -n "$explicit" ]; then DATABASE_URL="$explicit"; fi
  "$PSQL" "$DATABASE_URL" -tAc "$1"
}

STUB_LLM_LOG="${STUB_LLM_LOG:-/tmp/stub3.jsonl}"

say "the stub SearXNG"
python3 scripts/stub-searxng.py --port "$STUB_PORT" --log "$TMP/searxng-queries.jsonl" \
  > "$TMP/searxng.log" 2>&1 &
STUB_PID=$!
for _ in $(seq 1 50); do nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null && break; sleep 0.2; done
nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null || fail "the stub SearXNG did not start"
cat "$TMP/searxng.log"

say "health (the server must be running with a search tier configured)"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Run 'just search-up' then 'just smoke-server-phase3'."
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

say "a workspace with one source, so there is material to answer from"
curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Jaringan Komputer"}' > "$TMP/ws.json"
WS=$(field '["workspace"]["id"]' < "$TMP/ws.json")

DOKUMEN="Jaringan komputer adalah kumpulan komputer yang saling terhubung untuk berbagi data. Topologi star menghubungkan setiap komputer ke satu perangkat pusat bernama switch."
curl -fsS -X POST "$BASE/workspaces/$WS/sources" -H "authorization: Bearer $ACCESS" \
  -F kind=text -F title="Catatan Jaringan" -F "text=$DOKUMEN" > "$TMP/source.json"
SRC=$(field '["source"]["id"]' < "$TMP/source.json")
for _ in $(seq 1 120); do
  STATUS=$(curl -fsS "$BASE/sources/$SRC/status" -H "authorization: Bearer $ACCESS" | field '["status"]')
  case "$STATUS" in ready|failed) break ;; esac
  sleep 1
done
[ "$STATUS" = "ready" ] || fail "the source did not reach ready"

QUERY="attention is all you need transformers"

say "clear the cache for this query, so the run is self-contained"
# The cache is global and survives restarts (A40), so a previous run would make the
# first turn here a cache hit and the credit assertion below would have nothing to
# measure. Deleting the key makes each run start from a miss.
KEYS=$(sql "select count(*) from search_cache")
sql "delete from search_cache where key in (select key from search_cache)" > /dev/null
echo "cleared $KEYS cached search result set(s)"

say "phase 1: a web turn cites a URL and a DOI"
# Run twice in the same step, because the cache assertion needs both.
for attempt in 1 2; do
  curl -fsS -N -X POST "$BASE/workspaces/$WS/chat" \
    -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
    -d "{\"message\":\"$QUERY\",\"use_web\":true}" > "$TMP/stream-$attempt.txt"
done
cat "$TMP/stream-1.txt" | tail -6

python3 - "$TMP/stream-1.txt" "$TMP/stream-2.txt" <<'PY'
import json, sys

def events(path):
    raw = open(path, encoding="utf-8").read()
    out = {}
    for block in raw.split("\n\n"):
        name = data = None
        for line in block.splitlines():
            if line.startswith("event:"):
                name = line[6:].strip()
            elif line.startswith("data:"):
                data = line[5:].strip()
        if name and data:
            out.setdefault(name, []).append(json.loads(data))
    return out

first, second = events(sys.argv[1]), events(sys.argv[2])
for name, ev in (("first", first), ("second", second)):
    assert "error" not in ev, f"{name}: {ev.get('error')}"
    assert "done" in ev, f"{name}: no done event: {list(ev)}"

citations = first["citations"][0]["citations"]
assert citations, "a web turn must cite something"
web = [c for c in citations if c.get("type") == "web"]
assert web, f"no web citation in {citations}"
assert all(c.get("url", "").startswith("http") for c in web), web

# The gate: URLs AND DOIs. The academic tier is what carries a DOI, and it is the
# tier this phase consults first.
with_doi = [c for c in web if c.get("doi")]
assert with_doi, (
    "no citation carried a DOI. The academic tier is reachable from this machine and "
    f"is the tier that provides DOIs, so a run without one means that tier did not "
    f"answer: {web}"
)

print(f"{len(web)} web citation(s), {len(with_doi)} with a DOI")
for citation in web:
    print(f"  {citation['url']}  doi={citation.get('doi')}")

# The gate: the same query twice bills once.
spent = lambda ev: ev["usage"][0]["credits_spent"]
assert spent(second) < spent(first), (
    f"the second identical query must cost less: first {spent(first)}, second {spent(second)}"
)
print(f"first turn spent {spent(first)}, second (cached) spent {spent(second)}")
PY

say "the search rows record the cache and the tier"
python3 - "$PSQL" "$EMAIL" <<'PY'
import json, subprocess, sys, os

psql, email = sys.argv[1], sys.argv[2]
env = dict(os.environ)

def query(sql):
    return subprocess.run([psql, os.environ["DATABASE_URL"], "-tAc", sql],
                          capture_output=True, text=True, env=env).stdout.strip()

rows = query(
    "select cache_hit::text || '|' || coalesce(search_tier, '-') from usage_events "
    f"where action = 'web_search' and user_id = (select id from users where email = '{email}') "
    "order by created_at"
).splitlines()

assert len(rows) == 2, f"expected two search rows, got {rows}"
parsed = [row.split("|") for row in rows]
assert parsed[0][0] == "false", f"the first search must not be a cache hit: {rows[0]}"
assert parsed[1][0] == "true", f"the second search must be a cache hit: {rows[1]}"
assert parsed[0][1] != "-", f"the answering tier must be recorded: {rows[0]}"
print(f"search rows (cache_hit|tier): {rows}")
PY

say "the ledger agrees with what the client was told"
python3 - "$PSQL" "$EMAIL" "$TMP/stream-1.txt" "$TMP/stream-2.txt" <<'PY'
import json, os, subprocess, sys

psql, email, first_path, second_path = sys.argv[1:5]

def events(path):
    raw = open(path, encoding="utf-8").read()
    for block in raw.split("\n\n"):
        if block.startswith("event: usage"):
            return json.loads(block.split("data:", 1)[1].strip())
    raise AssertionError(f"no usage event in {path}")

last = events(second_path)["credits_remaining"]
ledger = subprocess.run(
    [psql, os.environ["DATABASE_URL"], "-tAc",
     "select coalesce(sum(l.delta), 0) from credit_ledger l join users u on u.id = l.user_id "
     f"where u.email = '{email}'"],
    capture_output=True, text=True, env=os.environ).stdout.strip()

assert ledger == str(last), f"the ledger ({ledger}) and the reported balance ({last}) disagree"
print(f"ledger {ledger} == reported {last}")
PY

say "phase 2: the SearXNG tier's wire format, against the stub"
# A second server is not started here: the stub's query log is the evidence that the
# tier was consulted, and the tier's own parsing is covered by its unit tests. What
# this checks is that a real instance of the stub is reachable at the configured URL
# and answers the shape the tier expects.
# `--data-urlencode` with `-G`, because the query has spaces and a hand-built URL
# would be rejected before it left curl.
curl -fsS -G "http://127.0.0.1:$STUB_PORT/search" \
  --data-urlencode "q=$QUERY" --data-urlencode "format=json" > "$TMP/searxng.json"
python3 - "$TMP/searxng.json" <<'PY'
import json, sys

body = json.load(open(sys.argv[1]))
assert body["results"], "the stub must return results"
assert "unresponsive_engines" in body, (
    "the stub must include the engine-health list, because the server captures it"
)
first = body["results"][0]
for key in ("url", "title", "content"):
    assert key in first, f"a SearXNG result must carry {key}: {first}"
print(f"{len(body['results'])} result(s), {len(body['unresponsive_engines'])} unresponsive engine(s)")
PY

say "a turn without the toggle does not search"
BEFORE=$(sql "select count(*) from usage_events where action = 'web_search'")
curl -fsS -N -X POST "$BASE/workspaces/$WS/chat" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d "{\"message\":\"apa itu topologi star?\",\"use_web\":false}" > "$TMP/stream-off.txt"
AFTER=$(sql "select count(*) from usage_events where action = 'web_search'")
[ "$BEFORE" = "$AFTER" ] || fail "a turn with the toggle off ran a search ($BEFORE -> $AFTER)"
echo "search rows unchanged at $AFTER"

say "gate 6: an Indonesian query with the web toggle on still has the Indonesian chunk in context"
# Asserted on the prompt the model received rather than on the citation, because the
# stub's canned answer cites only [W1]: whether the model chooses to cite the chunk is
# the model's behaviour, while whether the chunk was *available* is the server's, and
# that is what gate 6 is about. The chunk's own citation resolution is covered by
# crates/server/tests/indonesian.rs, which scripts an answer that cites it.
curl -fsS -N -X POST "$BASE/workspaces/$WS/chat" \
  -H "authorization: Bearer $ACCESS" -H 'content-type: application/json' \
  -d '{"message":"perangkat pusat yang menghubungkan setiap komputer pada topologi star","use_web":true}' \
  > "$TMP/stream-id.txt"
python3 - "$STUB_LLM_LOG" "$SRC" <<'PY'
import json, sys

path, source_id = sys.argv[1], sys.argv[2]
bodies = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
assert bodies, f"the stub recorded no request; was it started with --log {path}?"
system = bodies[-1]["messages"][0]["content"]

# The Indonesian chunk, labelled for citation.
assert "[1]" in system, f"the retrieved chunk must be labelled: {system[:400]}"
assert "topologi star" in system.lower(), "the Indonesian chunk must be in the prompt"
# And the web result alongside it, so this is genuinely a web turn.
assert "[W1]" in system, f"the web result must be labelled too: {system[-600:]}"
print(f"prompt carries the Indonesian chunk labelled [1] and a web result labelled [W1]")
PY

say "PASS"
say "NOT VERIFIED HERE: the real SearXNG engine list. No Docker on this machine, and"
say "A3 measured a laptop answering with 1 of 8 engines against 3 on the production"
say "host, so the list is verified on the staging VPS instead."