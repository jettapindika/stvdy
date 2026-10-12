#!/usr/bin/env bash
# Phase 8 end-to-end smoke test: the sync engine, against the real server, the real
# Postgres and two real devices.
#
# **No model stub is needed.** Nothing on the sync path calls an LLM: a pull reads
# `change_log`, a push applies rows and an SSE hint polls `max(seq)`. Phase 6 and 7 needed
# a stub because their runs drove chat and generation; this one would be inventing a model
# step to look thorough, so it does not have one. `just smoke-server-phase7` is a fine
# server for this run (it is pointed at a stub, and this script never reaches it).
#
# It drives the real process over HTTP and reads the real database back, because the parts
# this phase can get wrong are the parts a trait-level double cannot see: the exact
# `seq` the server hands back, whether a replay wrote a second `change_log` row, whether a
# tombstone's payload is `null` on the wire, which account's seqs a pull contains, and
# whether a revoked device's token is refused. The Rust suites assert all of these too —
# this run is the one that proves the *wired process* does, and it prints the numbers it
# asserted so the output is evidence rather than a green word.
#
# Ten scenarios, in order:
#   1. two accounts, and two devices on the first
#   2. device 1 pushes an Indonesian task; the server assigns a seq
#   3. device 2 pulls and sees the same row at the same seq, byte-identical
#   4. the same `mutation_id` replayed: the stored result, and nothing applied
#   5. device 2 tombstones the task; device 1 pulls `op: delete` with `data: null`
#   6. device 1 edits against the tombstone; the tombstone wins and the loser is told
#   7. a batch of `Message`, `Source` and a task: two refusals and one apply
#   8. account B's pull contains none of account A's seqs, and A's none of B's
#   9. a cursor below the retained history: 410, then a full resync that loses nothing
#  10. device 2 revoked: its next pull is 401
#
# **Run this against a freshly started server.** It makes 5 credential requests, and
# `ratelimit::auth_layer` (A12) is an in-process bucket of 30 requests per minute per IP,
# so a second run inside the same minute waits for cells (the helper below says so rather
# than hanging).
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
MAIL_DIR="${MAIL_DIR:-var/mail}"
TMP="$(mktemp -d)"
STAMP="$(date +%s)"
PASSWORD="correct horse battery"

say() { printf '\n=== %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
field() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }
uuid() { python3 -c "import uuid; print(uuid.uuid4())"; }
now() { python3 -c "import datetime; print(datetime.datetime.now(datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ'))"; }

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
# one. Read the one key rather than sourcing the file: sourcing executes it, and a line as
# ordinary as `LLM_BASE_URL= https://host` (a space after `=`) makes bash run the URL as a
# command, which under `set -e` aborts the run.
if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
  DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
  export DATABASE_URL
fi
[ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"

db() { "$PSQL" "$DATABASE_URL" -tAc "$1"; }

say "health"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Start one first: 'just smoke-server-phase7'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

# --- accounts ----------------------------------------------------------------------------
#
# The per-IP signup limiter counts *every* attempt over a 24h window (A154, 25 per IP), and
# every run of this script adds to it. This deletes only the loopback address's rows, which
# is what a developer's machine signs up from.
DOMAIN="phase8-$STAMP.example.test"

say "reset the loopback signup attempts (A154 counts 25 per IP per 24h)"
PRUNED="$(db "with gone as (delete from signup_attempts where ip = '127.0.0.1' returning 1) select count(*) from gone")"
printf 'pruned %s signup_attempts row(s) for 127.0.0.1\n' "$PRUNED"

# A credential request, retried across the auth limiter's refill window (A12: 30/minute/IP).
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
  [ "$status" != "429" ] || fail "$url answered 429 for 70s; wait a minute and re-run"
  printf '%s' "$status"
}

# Signs up one account, verifies its email from the dev mail sink, and prints its access
# token. The device name distinguishes the two devices of account A (the spec's "sign in
# on a second laptop").
signup() { # $1 = label, $2 = device name
  local label="$1" device="$2"
  local email="phase8-$label-$STAMP@$DOMAIN"
  local status
  status="$(auth_post "$TMP/signup-$label.json" "$BASE/auth/signup" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"$device\",\"platform\":\"macos\"}}")"
  [ "$status" = "201" ] || fail "signup for $label answered $status: $(cat "$TMP/signup-$label.json")"
  # The dev sink writes `<unix-millis>-<local part>.eml`; newest by name is newest by time.
  local mail token
  mail="$(find "$MAIL_DIR" -name '*.eml' -type f | sort | tail -1)"
  [ -n "$mail" ] || fail "no verification mail in $MAIL_DIR"
  token="$(grep -o 'token=[A-Za-z0-9_-]*' "$mail" | head -1 | cut -d= -f2)"
  status="$(auth_post "$TMP/verify-$label.json" "$BASE/auth/verify-email" \
    -H 'content-type: application/json' -d "{\"token\":\"$token\"}")"
  [ "$status" = "200" ] || fail "verifying $label answered $status: $(cat "$TMP/verify-$label.json")"
  printf '%s' "$(field '["access_token"]' < "$TMP/signup-$label.json")"
}

login() { # $1 = label, $2 = device name -> a second device on the same account
  local label="$1" device="$2"
  local email="phase8-$label-$STAMP@$DOMAIN"
  local status
  status="$(auth_post "$TMP/login-$label.json" "$BASE/auth/login" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"$device\",\"platform\":\"ios\"}}")"
  [ "$status" = "200" ] || fail "login for $label answered $status: $(cat "$TMP/login-$label.json")"
  printf '%s' "$(field '["access_token"]' < "$TMP/login-$label.json")"
}

uid() { db "select id from users where email = 'phase8-$1-$STAMP@$DOMAIN'"; }

# ==========================================================================================
# 1. Two accounts, and two devices on the first
# ==========================================================================================
say "1. two accounts, and two devices on the first"
A="$(signup a "Smoke laptop")"
A_ID="$(uid a)"
A_DEVICE="$(field '["device_id"]' < "$TMP/signup-a.json")"
B="$(signup b "Other laptop")"
B_ID="$(uid b)"
# The second device is a *login*, not a second account: the exit gate's "one account's
# writes reach every device" is about two devices on one account.
A2="$(login a "Smoke phone")"
A2_DEVICE="$(field '["device_id"]' < "$TMP/login-a.json")"
[ "$A_DEVICE" != "$A2_DEVICE" ] || fail "the two devices share an id"
printf 'account A=%s device1=%s device2=%s | account B=%s\n' "$A_ID" "$A_DEVICE" "$A2_DEVICE" "$B_ID"

push() { # $1 = token, $2 = device id, $3 = mutations JSON, $4 = output file
  local token="$1" device="$2" mutations="$3" out="$4"
  local status
  status="$(curl -sS -o "$out" -w '%{http_code}' -X POST "$BASE/sync/push" \
    -H "authorization: Bearer $token" -H 'content-type: application/json' \
    -d "{\"device_id\":\"$device\",\"mutations\":$mutations}")"
  [ "$status" = "200" ] || fail "POST /sync/push answered $status: $(cat "$out")"
}

pull() { # $1 = token, $2 = query string, $3 = output file -> prints the status code
  curl -sS -o "$3" -w '%{http_code}' "$BASE/sync/pull?$2" -H "authorization: Bearer $1"
}

# A `tasks` mutation carries `{"patch": …, "base": …}` (A176): `base: null` is a create.
task_mutation() { # $1 = mutation id, $2 = task id, $3 = title, $4 = base JSON or null
  printf '{"mutation_id":"%s","entity":"task","op":"upsert","id":"%s","payload":{"patch":{"title":"%s"},"base":%s},"client_ts":"%s"}' \
    "$1" "$2" "$3" "$4" "$(now)"
}

# ==========================================================================================
# 2. Device 1 pushes an Indonesian task
# ==========================================================================================
say "2. device 1 pushes a task with an Indonesian title"
TASK="$(uuid)"
M1="$(uuid)"
TITLE="Ringkasan bab tiga: sel dan membran"
push "$A" "$A_DEVICE" "[$(task_mutation "$M1" "$TASK" "$TITLE" null)]" "$TMP/push1.json"
SEQ1="$(field '["results"][0]["seq"]' < "$TMP/push1.json")"
[ "$(field '["results"][0]["status"]' < "$TMP/push1.json")" = "applied" ] \
  || fail "the first push was not applied: $(cat "$TMP/push1.json")"
[ "$SEQ1" -gt 0 ] || fail "the applied seq is $SEQ1"
printf 'device 1 pushed task %s -> applied{seq=%s}\n' "$TASK" "$SEQ1"

# ==========================================================================================
# 3. Device 2 pulls and sees the same row at the same seq
# ==========================================================================================
say "3. device 2 pulls the row at the same seq"
STATUS="$(pull "$A2" "cursor=0" "$TMP/pull2.json")"
[ "$STATUS" = "200" ] || fail "the initial pull answered $STATUS: $(cat "$TMP/pull2.json")"
python3 - "$TMP/pull2.json" "$TASK" "$SEQ1" "$TITLE" <<'PY'
import json, sys
body, task, seq, title = json.load(open(sys.argv[1])), sys.argv[2], int(sys.argv[3]), sys.argv[4]
seen = [c for c in body["changes"] if c["id"] == task]
assert seen, f"the task is not in the pull: {body['changes']}"
change = seen[0]
assert change["seq"] == seq, f"seq {change['seq']} != the pushed {seq}"
assert change["entity"] == "task" and change["op"] == "upsert", change
assert change["data"]["title"] == title, change["data"].get("title")
assert "user_id" not in change["data"], "the payload must not carry user_id (A168)"
assert "file_key" not in json.dumps(body), "no storage key may reach a client (A168)"
print(f"device 2 pulled {len(body['changes'])} change(s); the task is there at seq={seq},"
      f" title={change['data']['title']!r}, next_cursor={body['next_cursor']},"
      f" has_more={body['has_more']}")
PY

# ==========================================================================================
# 4. The same mutation_id replayed applies nothing
# ==========================================================================================
#
# The assertion is not "the response looks the same": it is that the row's `updated_at` and
# the account's `change_log` count are both unchanged. A route that applied the mutation a
# second time would move one of them.
say "4. the replayed mutation returns the stored result and applies nothing"
BEFORE_UPDATED="$(db "select updated_at from tasks where id = '$TASK'")"
BEFORE_COUNT="$(db "select count(*) from change_log where user_id = '$A_ID'")"
push "$A" "$A_DEVICE" "[$(task_mutation "$M1" "$TASK" "$TITLE" null)]" "$TMP/push1-replay.json"
AFTER_UPDATED="$(db "select updated_at from tasks where id = '$TASK'")"
AFTER_COUNT="$(db "select count(*) from change_log where user_id = '$A_ID'")"
[ "$BEFORE_UPDATED" = "$AFTER_UPDATED" ] \
  || fail "the replay moved updated_at: $BEFORE_UPDATED -> $AFTER_UPDATED"
[ "$BEFORE_COUNT" = "$AFTER_COUNT" ] \
  || fail "the replay wrote a change_log row: $BEFORE_COUNT -> $AFTER_COUNT"
python3 - "$TMP/push1.json" "$TMP/push1-replay.json" <<'PY'
import json, sys
first, replay = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
assert first == replay, f"the replayed result differs: {first} != {replay}"
print(f"replay returned {replay['results'][0]} (identical), and both updated_at and the"
      f" change_log count are unchanged")
PY

# ==========================================================================================
# 5. Device 2 tombstones the task; device 1 pulls the tombstone
# ==========================================================================================
say "5. device 2 tombstones the task; device 1 pulls op=delete with data=null"
M2="$(uuid)"
push "$A2" "$A2_DEVICE" \
  "[{\"mutation_id\":\"$M2\",\"entity\":\"task\",\"op\":\"delete\",\"id\":\"$TASK\",\"payload\":null,\"client_ts\":\"$(now)\"}]" \
  "$TMP/push2.json"
SEQ2="$(field '["results"][0]["seq"]' < "$TMP/push2.json")"
[ "$(field '["results"][0]["status"]' < "$TMP/push2.json")" = "applied" ] \
  || fail "the tombstone was not applied: $(cat "$TMP/push2.json")"
STATUS="$(pull "$A" "cursor=0" "$TMP/pull5.json")"
[ "$STATUS" = "200" ] || fail "device 1's pull answered $STATUS: $(cat "$TMP/pull5.json")"
python3 - "$TMP/pull5.json" "$TASK" "$SEQ2" <<'PY'
import json, sys
body, task, seq = json.load(open(sys.argv[1])), sys.argv[2], int(sys.argv[3])
seen = [c for c in body["changes"] if c["id"] == task]
assert seen, "the task's changes are missing"
last = seen[-1]
assert last["op"] == "delete" and last["seq"] == seq, last
assert last["data"] is None, f"a tombstone must carry data: null, got {last['data']!r}"
print(f"device 1 sees {len(seen)} change(s) for the task; the last is seq={seq}"
      f" op=delete data=null")
PY
# Device 1's cursor after this pull: the anchor, which is the account's max(seq) then.
CURSOR1="$(field '["next_cursor"]' < "$TMP/pull5.json")"
[ "$CURSOR1" -ge "$SEQ2" ] || fail "device 1's cursor $CURSOR1 is below the tombstone $SEQ2"
printf 'device 1 cursor after the pull: %s\n' "$CURSOR1"

# ==========================================================================================
# 6. Device 1 edits against the tombstone: the tombstone wins
# ==========================================================================================
say "6. device 1 edits against the tombstone; the tombstone is the winner"
M3="$(uuid)"
push "$A" "$A_DEVICE" \
  "[{\"mutation_id\":\"$M3\",\"entity\":\"task\",\"op\":\"upsert\",\"id\":\"$TASK\",\"payload\":{\"patch\":{\"title\":\"Dihapus lalu diedit\"},\"base\":{\"title\":\"$TITLE\",\"deleted_at\":null}},\"client_ts\":\"$(now)\"}]" \
  "$TMP/push3.json"
python3 - "$TMP/push3.json" "$TASK" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))["results"][0]
assert result["status"] == "conflict", result
assert result["winner"]["id"] == sys.argv[2], result["winner"]
assert result["winner"]["deleted_at"] is not None, "the winner must be the tombstone"
print(f"the edit got status=conflict and the winner is the tombstone"
      f" (deleted_at={result['winner']['deleted_at']})")
PY
STILL="$(db "select deleted_at is not null from tasks where id = '$TASK'")"
[ "$STILL" = "t" ] || fail "the conflict rewrote the tombstoned row"
printf 'the row is still tombstoned on the server\n'

# ==========================================================================================
# 7. A batch with two server-owned entities and one writable one
# ==========================================================================================
#
# The refusal must not abort the batch: `Message` and `Source` are the two rows the threat
# model's §5 calls security-relevant, and the task in the same batch must still apply.
say "7. a batch of Message, Source and task: read_only_entity twice, one apply"
TASK2="$(uuid)"
TITLE2="Baca ulang catatan biologi"
push "$A" "$A_DEVICE" "[
  {\"mutation_id\":\"$(uuid)\",\"entity\":\"message\",\"op\":\"upsert\",\"id\":\"$(uuid)\",\"payload\":{\"role\":\"assistant\",\"content\":\"forged\"},\"client_ts\":\"$(now)\"},
  {\"mutation_id\":\"$(uuid)\",\"entity\":\"source\",\"op\":\"upsert\",\"id\":\"$(uuid)\",\"payload\":{\"status\":\"ready\"},\"client_ts\":\"$(now)\"},
  $(task_mutation "$(uuid)" "$TASK2" "$TITLE2" null)
]" "$TMP/push7.json"
python3 - "$TMP/push7.json" "$TASK2" <<'PY'
import json, sys
results = json.load(open(sys.argv[1]))["results"]
assert len(results) == 3, results
for result, entity in zip(results[:2], ("message", "source")):
    assert result["status"] == "rejected", result
    assert result["code"] == "read_only_entity", f"{entity}: {result}"
assert results[2]["status"] == "applied", results[2]
print(f"message -> rejected{{code=read_only_entity}}, source -> rejected{{code=read_only_entity}},"
      f" task -> applied{{seq={results[2]['seq']}}} (the refusals did not abort the batch)")
PY
SEQ3="$(field '["results"][2]["seq"]' < "$TMP/push7.json")"

# ==========================================================================================
# 8. The isolation property, asserted on seq values
# ==========================================================================================
say "8. account B's pull contains none of account A's seqs, and A's none of B's"
# One change on B, so the assertion cannot pass by iterating nothing.
B_TASK="$(uuid)"
push "$B" "$(field '["device_id"]' < "$TMP/signup-b.json")" \
  "[$(task_mutation "$(uuid)" "$B_TASK" "Tugas milik akun B" null)]" "$TMP/push-b.json"
B_SEQ="$(field '["results"][0]["seq"]' < "$TMP/push-b.json")"
STATUS="$(pull "$B" "cursor=0" "$TMP/pull-b.json")"
[ "$STATUS" = "200" ] || fail "account B's pull answered $STATUS: $(cat "$TMP/pull-b.json")"
STATUS="$(pull "$A" "cursor=0" "$TMP/pull-a.json")"
[ "$STATUS" = "200" ] || fail "account A's pull answered $STATUS: $(cat "$TMP/pull-a.json")"
A_SEQS="$(db "select coalesce(string_agg(seq::text, ','), '') from change_log where user_id = '$A_ID'")"
B_SEQS="$(db "select coalesce(string_agg(seq::text, ','), '') from change_log where user_id = '$B_ID'")"
python3 - "$TMP/pull-a.json" "$TMP/pull-b.json" "$A_SEQS" "$B_SEQS" "$A_ID" "$B_ID" <<'PY'
import json, sys
a_pull, b_pull = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
a_seqs = {int(s) for s in sys.argv[3].split(",") if s}
b_seqs = {int(s) for s in sys.argv[4].split(",") if s}
a_pulled = {c["seq"] for c in a_pull["changes"]}
b_pulled = {c["seq"] for c in b_pull["changes"]}
assert a_pulled and b_pulled, "one of the pulls is empty, so this assertion proves nothing"
assert not (a_pulled & b_seqs), f"account A's pull contains B's seqs: {sorted(a_pulled & b_seqs)}"
assert not (b_pulled & a_seqs), f"account B's pull contains A's seqs: {sorted(b_pulled & a_seqs)}"
assert a_pulled <= a_seqs and b_pulled <= b_seqs, "a pull contains a seq no change_log row has"
print(f"A's pull: {len(a_pulled)} seq(s), all of A's {len(a_seqs)}; B's pull: {len(b_pulled)}"
      f" seq(s), all of B's {len(b_seqs)}; the intersection is empty both ways")
PY

# ==========================================================================================
# 9. A cursor below the retained history: 410, then a full resync
# ==========================================================================================
#
# A real 90-day retention window cannot be aged inside a smoke run, so this ages the rows
# instead and lets the **real job** decide what to delete: every change_log row of account
# A at or below device 1's cursor is backdated past both windows, then the daily
# `compact_change_log` job is enqueued and the watermark it writes is read back. That is
# the same code path the scheduler runs; only `changed_at` is moved.
say "9. a cursor below the retained history: 410 resync_required, then a full resync"
AGED="$(db "with aged as (
    update change_log set changed_at = now() - interval '400 days'
     where user_id = '$A_ID' and seq <= $CURSOR1 returning 1)
  select count(*) from aged")"
[ "$AGED" -gt 0 ] || fail "nothing was aged; the cursor was already past every row"
db "insert into jobs (kind, payload, dedupe_key)
    values ('compact_change_log', '{}'::jsonb, 'smoke-phase8-$STAMP')" > /dev/null
WATERMARK="0"
for _ in $(seq 1 60); do
  WATERMARK="$(db "select coalesce((select min_retained_seq from change_log_watermark where user_id = '$A_ID'), 0)")"
  [ "$WATERMARK" -gt "$CURSOR1" ] && break
  sleep 0.5
done
[ "$WATERMARK" -gt "$CURSOR1" ] \
  || fail "the compaction job did not move the watermark past device 1's cursor $CURSOR1 (watermark=$WATERMARK)"
printf 'the real compact_change_log job deleted the %s aged row(s) and wrote watermark=%s (device 1 cursor=%s)\n' \
  "$AGED" "$WATERMARK" "$CURSOR1"
STATUS="$(pull "$A" "cursor=$CURSOR1" "$TMP/pull-stale.json")"
[ "$STATUS" = "410" ] || fail "a stale cursor must be 410, got $STATUS: $(cat "$TMP/pull-stale.json")"
[ "$(field '["code"]' < "$TMP/pull-stale.json")" = "resync_required" ] \
  || fail "the 410 must be resync_required: $(cat "$TMP/pull-stale.json")"
printf 'pull with cursor=%s -> 410 resync_required\n' "$CURSOR1"
# The remedy, and the surviving history: the task pushed in scenario 7 is still there and
# the two aged changes are not.
STATUS="$(pull "$A" "cursor=0" "$TMP/pull-resync.json")"
[ "$STATUS" = "200" ] || fail "the full resync answered $STATUS: $(cat "$TMP/pull-resync.json")"
python3 - "$TMP/pull-resync.json" "$TASK2" "$TITLE2" "$CURSOR1" <<'PY'
import json, sys
body, task, title, cut = json.load(open(sys.argv[1])), sys.argv[2], sys.argv[3], int(sys.argv[4])
seqs = [c["seq"] for c in body["changes"]]
assert all(s > cut for s in seqs), f"a compacted seq came back: {[s for s in seqs if s <= cut]}"
seen = [c for c in body["changes"] if c["id"] == task]
assert seen and seen[-1]["data"]["title"] == title, "the surviving row did not arrive"
print(f"pull from 0 -> 200, {len(seqs)} surviving change(s), none at or below {cut},"
      f" and the task is there: {seen[-1]['data']['title']!r}")
PY

# ==========================================================================================
# 10. A revoked device
# ==========================================================================================
say "10. device 2 revoked: its next pull is 401"
STATUS="$(curl -sS -o "$TMP/revoke.json" -w '%{http_code}' -X DELETE \
  "$BASE/devices/$A2_DEVICE" -H "authorization: Bearer $A")"
[ "$STATUS" = "204" ] || fail "revoking device 2 answered $STATUS: $(cat "$TMP/revoke.json")"
STATUS="$(pull "$A2" "cursor=0" "$TMP/pull-revoked.json")"
[ "$STATUS" = "401" ] || fail "a revoked device must be 401, got $STATUS: $(cat "$TMP/pull-revoked.json")"
printf 'device 2 revoked (204); its pull -> 401 %s\n' "$(field '["code"]' < "$TMP/pull-revoked.json")"
# And device 1, which was not revoked, still pulls: the revocation is per device.
STATUS="$(pull "$A" "cursor=0" "$TMP/pull-alive.json")"
[ "$STATUS" = "200" ] || fail "device 1 was collateral damage of the revocation: $STATUS"
printf 'device 1 still pulls: %s\n' "$STATUS"

printf '\n=== PASS\n'
