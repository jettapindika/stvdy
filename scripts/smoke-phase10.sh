#!/usr/bin/env bash
# Phase 10 end-to-end smoke test: the ICS feed out, its revocable token, the ICS import's
# two validation points, and the plan split — against the real server and the real Postgres.
#
# Drives the real process over HTTP, because the parts this phase can get wrong are the
# parts a trait-level double cannot see: that a regenerated token retires the old one
# **and the new one works**, that the feed's three headers are what they claim to be, that
# an Indonesian title with a comma and a semicolon survives the feed's escaping and folding
# (the data half of gate 6 — a broken `SUMMARY` line is not a cosmetic problem), that a
# private ICS URL is refused at save time with a readable reason, and that `POST
# /calendar/sync` is refused on free because free has no Google connection to sync.
#
# ## What this run does NOT exercise, and why
#
# **A live Google exchange.** The consent routes landed with Task 6 and are asserted here for
# their *refusals*: `GET /calendar/google/connect` on free is a 403 `plan_required`, and the
# callback with an unknown state is a 303 to the app's deep link carrying `status=error` and
# no token. What no run can exercise is a successful exchange, because the OAuth client does
# not exist in this repo — so the callback's happy path is `crates/server/tests/calendar.rs`'s
# only as far as the state handling goes, and the exchange itself is named there as the
# assertion that needs the real client.
#
# **A successful ICS fetch.** The importer's guard is the same SSRF guard ingestion uses
# (A233), and it refuses `127.0.0.1` by design — that is the control, not an obstacle. So
# this run cannot point the real fetcher at a local stub file: a run that did, and passed,
# would be proving the guard absent. What is asserted here is the save-time validation (a
# literal private address is a 400 with a reason) and the fetch-time outcome (a
# `.invalid` host resolves nowhere, so the connection records an error rather than
# reporting itself healthy). The recurrence import, the conditional GET and the 410
# fallback are `crates/server/tests/calendar.rs`'s, where the fetcher seam injects a fake.
#
# **Run this against a freshly started server.** Like Phases 7 and 9 it makes several
# requests to the credential routes, and `ratelimit::auth_layer` (A12) is an in-process
# bucket of 30 requests per minute per IP, so a second run inside the same server process
# may have to wait for cells. The script waits and says so rather than hanging.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
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

# --- the database, and why it is read rather than sourced --------------------------------
#
# Read the one key rather than sourcing the file: sourcing executes it, and a line as
# ordinary as `LLM_BASE_URL= https://host` makes bash run the URL as a command (Phase 5's
# note, and the reason every smoke script since reads instead of sourcing).
if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
  DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
  export DATABASE_URL
fi
[ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"

db() { "$PSQL" "$DATABASE_URL" -tAc "$1"; }

# --- the plan matrix, quoted from the file the server reads ------------------------------
#
# The split asserted below is read from `config/plans.toml` rather than written here, so a
# retuned config moves this run with it. A literal would make the run pass against a server
# whose config disagrees with it — the one failure this scenario exists to catch.
say "the plan matrix, from $PLANS"
[ -f "$PLANS" ] || fail "no plan config at $PLANS"
python3 - "$PLANS" > "$TMP/matrix.txt" <<'PY'
import sys, tomllib
plans = tomllib.load(open(sys.argv[1], "rb"))["plans"]
for name in ("free", "pro", "power"):
    p = plans[name]
    print(f"{name}: google_calendar={str(p['google_calendar']).lower()} google_accounts_max={p['google_accounts_max']}")
PY
cat "$TMP/matrix.txt"
FREE_GOOGLE="$(python3 -c "
import tomllib
print('yes' if tomllib.load(open('$PLANS','rb'))['plans']['free']['google_calendar'] else 'no')")"
[ "$FREE_GOOGLE" = "no" ] || fail "the free plan now grants google_calendar; scenario 6 asserts the refusal"
# The matrix has no ICS switch at all, and that absence *is* the split: the feed and the
# import are rows every plan carries, so there is nothing to gate them on.
python3 -c "
import tomllib, sys
p = tomllib.load(open('$PLANS','rb'))['plans']['free']
for key in ('ics', 'ics_feed', 'calendar_feed', 'calendar_import'):
    if key in p:
        sys.exit(0)
sys.exit(1)" && fail "config/plans.toml now has an ICS switch; the feed and import are ungated by design"

say "health"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Start one with 'just smoke-server-phase10'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

# --- the account --------------------------------------------------------------------------
#
# The per-IP limiter counts every attempt, including this run's own, over a 24h window, so
# without this the eleventh re-run in a day would fail at signup and read like a server bug.
DOMAIN="phase10-$STAMP.example.test"
say "reset the loopback signup attempts (A154 counts 25 per IP per 24h)"
PRUNED="$(db "with gone as (delete from signup_attempts where ip = '127.0.0.1' returning 1) select count(*) from gone")"
printf 'pruned %s signup_attempts row(s) for 127.0.0.1\n' "$PRUNED"

# A credential request, retried across the auth limiter's refill window. `/auth/signup`,
# `/auth/login` and `/auth/verify-email` sit behind an in-process bucket of 30/minute/IP
# (A12), a different control from A154's per-IP signup limit and the one that trips first.
auth_post() { # $1 = output file, $2 = url, rest = curl arguments
  local out="$1" url="$2"
  shift 2
  local attempt status
  for attempt in $(seq 1 14); do
    status="$(curl -sS -o "$out" -w '%{http_code}' -X POST "$url" "$@")"
    [ "$status" = "429" ] || break
    printf '  the credential rate limiter answered 429; waiting 5s (attempt %s of 14)\n' "$attempt" >&2
    sleep 5
  done
  if [ "$status" = "429" ]; then
    printf 'FAIL: %s answered 429 for 70s. Wait a minute and re-run.\n' "$url" >&2
    exit 1
  fi
  printf '%s' "$status"
}

EMAIL="phase10-student-$STAMP@$DOMAIN"
say "1. a fresh account: signup and verify"
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

AUTH=(-H "authorization: Bearer $ACCESS")
# Authenticated JSON, printing the status so a refusal is assertable rather than fatal.
ajson() { # $1 = output file, $2 = method, $3 = path, $4 = body (empty for none)
  local out="$1" method="$2" path="$3" body="${4:-}"
  if [ -n "$body" ]; then
    curl -sS -o "$out" -w '%{http_code}' -X "$method" "$BASE$path" \
      "${AUTH[@]}" -H 'content-type: application/json' -d "$body"
  else
    curl -sS -o "$out" -w '%{http_code}' -X "$method" "$BASE$path" "${AUTH[@]}"
  fi
}

say "2. the feed token: none yet, then a rotation that is revoke-and-replace"
STATUS="$(ajson "$TMP/token0.json" GET /calendar/ics-token)"
[ "$STATUS" = "200" ] || fail "GET /calendar/ics-token answered $STATUS"
[ "$(field '["ics_feed_token"]' < "$TMP/token0.json")" = "None" ] \
  || fail "a fresh account already has a feed token: $(cat "$TMP/token0.json")"
printf 'a fresh account has no feed token, and the read answers null rather than minting one\n'

STATUS="$(ajson "$TMP/token1.json" POST /calendar/ics-token/regenerate '{}')"
[ "$STATUS" = "200" ] || fail "regenerate answered $STATUS: $(cat "$TMP/token1.json")"
TOKEN1="$(field '["ics_feed_token"]' < "$TMP/token1.json")"
[ "${#TOKEN1}" -ge 40 ] || fail "the feed token is ${#TOKEN1} characters; 32 bytes of base64 is 43"
STATUS="$(ajson "$TMP/token2.json" POST /calendar/ics-token/regenerate '{}')"
TOKEN2="$(field '["ics_feed_token"]' < "$TMP/token2.json")"
[ "$TOKEN1" != "$TOKEN2" ] || fail "regenerating returned the same token"
# The token the student copies must be the one the settings screen can read back: a screen
# that could only obtain the link by rotating it would break every subscribed device each
# time it opened.
STATUS="$(ajson "$TMP/token3.json" GET /calendar/ics-token)"
[ "$(field '["ics_feed_token"]' < "$TMP/token3.json")" = "$TOKEN2" ] \
  || fail "the read-back token is not the one regeneration minted"
printf 'token rotated twice: %s -> %s, and the read-back matches the second\n' \
  "${TOKEN1:0:6}..." "${TOKEN2:0:6}..."

say "3. the feed itself: three headers, and an Indonesian title that survives escaping"
# A title with a comma **and** a semicolon, in Indonesian. Both are ICS TEXT metacharacters:
# a comma inside an unescaped `SUMMARY` ends the value early for a strict parser and shifts
# every later field, and a semicolon is the parameter separator. The title is also long
# enough to need folding, which is where a multi-byte boundary silently mangles a string.
TITLE="UAS Jaringan Komputer; bab 3,4 dan 5"
STARTS="2026-06-10T01:00:00Z"
ENDS="2026-06-10T03:00:00Z"
STATUS="$(ajson "$TMP/event.json" POST /events \
  "{\"kind\":\"class\",\"title\":\"$TITLE\",\"starts_at\":\"$STARTS\",\"ends_at\":\"$ENDS\"}")"
[ "$STATUS" = "201" ] || fail "creating the event answered $STATUS: $(cat "$TMP/event.json")"
printf 'event created: %s\n' "$TITLE"

curl -sS -D "$TMP/feed.headers" -o "$TMP/feed.ics" "$BASE/cal/$TOKEN2.ics"
grep -qi '^content-type: text/calendar' "$TMP/feed.headers" \
  || fail "the feed is not text/calendar: $(grep -i '^content-type' "$TMP/feed.headers")"
grep -qi '^cache-control: no-store' "$TMP/feed.headers" \
  || fail "the feed is cacheable: a stale copy served to a revoked token is the leak rotation closes"
grep -qi '^x-robots-tag: noindex' "$TMP/feed.headers" \
  || fail "the feed is indexable: a subscription URL is what a crawler finds in a share link"
printf 'headers: %s | %s | %s\n' \
  "$(grep -i '^content-type' "$TMP/feed.headers" | tr -d '\r')" \
  "$(grep -i '^cache-control' "$TMP/feed.headers" | tr -d '\r')" \
  "$(grep -i '^x-robots-tag' "$TMP/feed.headers" | tr -d '\r')"

VEVENTS="$(grep -c '^BEGIN:VEVENT' "$TMP/feed.ics" || true)"
[ "$VEVENTS" = "1" ] || fail "the feed carries $VEVENTS VEVENT(s), expected exactly the one event"
# The escaping, read out of the document rather than trusted: `\,` and `\;` are the ICS
# TEXT escapes, and the unescaped forms would be a broken line rather than a cosmetic
# difference.
grep -q 'UAS Jaringan Komputer\\; bab 3\\,4 dan 5' "$TMP/feed.ics" \
  || fail "the Indonesian title was not escaped for ICS TEXT: $(grep '^SUMMARY' "$TMP/feed.ics")"
grep -q '^UID:' "$TMP/feed.ics" || fail "the VEVENT carries no UID, so no client can cache on it"
grep -q '^SEQUENCE:' "$TMP/feed.ics" || fail "the VEVENT carries no SEQUENCE (A230)"
# Two renders of an unchanged row are byte-identical (A230), which is what stops every
# client re-importing the whole feed on every poll.
curl -sS -o "$TMP/feed2.ics" "$BASE/cal/$TOKEN2.ics"
cmp -s "$TMP/feed.ics" "$TMP/feed2.ics" \
  || fail "two renders of an unchanged feed differ: a feed that reorders is one every client re-imports"
printf 'feed: 1 VEVENT, UID+SEQUENCE present, escaping intact, two renders byte-identical\n'

say "4. a rotated token retires the old one, and the new one works"
OLD="$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/cal/$TOKEN1.ics")"
[ "$OLD" = "404" ] || fail "the retired token answered $OLD, expected 404"
NEW="$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/cal/$TOKEN2.ics")"
[ "$NEW" = "200" ] || fail "the live token answered $NEW, expected 200"
# And the suffix is required rather than optional, so one token has exactly one URL.
NOSUFFIX="$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/cal/$TOKEN2")"
[ "$NOSUFFIX" = "404" ] || fail "/cal/<token> without .ics answered $NOSUFFIX, expected 404"
printf 'retired token 404, live token 200, and the .ics suffix is required (%s)\n' "$NOSUFFIX"

say "5. an ICS URL is validated when it is saved"
# A literal private address is refused without DNS, which is the save-time half of A246 and
# the same check ingestion uses. The fetch-time half is the guard, and it is proven in
# crates/server/tests/ssrf.rs — a run that pointed the real fetcher at 127.0.0.1 and passed
# would be proving the guard absent.
STATUS="$(ajson "$TMP/private.json" POST /calendar/ics '{"url":"http://169.254.169.254/feed.ics"}')"
[ "$STATUS" = "400" ] || fail "a metadata address answered $STATUS, expected 400"
[ "$(field '["code"]' < "$TMP/private.json")" = "validation" ] \
  || fail "the refusal is not a validation: $(cat "$TMP/private.json")"
printf 'a private ICS URL is a 400 validation: %s\n' "$(field '["message"]' < "$TMP/private.json")"

say "6. the plan split: an ICS connection is free, and the Google sync is not"
# A `.invalid` host is reserved by RFC 2606 and resolves nowhere, so the connection is
# created and its first pass records a failure rather than reaching anything real. That is
# the honest wired behaviour for a host that does not resolve, and it is also the only
# fetch this run may perform.
STATUS="$(ajson "$TMP/ics.json" POST /calendar/ics '{"url":"https://calendar.example.invalid/feed.ics"}')"
[ "$STATUS" = "201" ] || fail "adding an ICS URL on free answered $STATUS: $(cat "$TMP/ics.json")"
CONN="$(field '["connection"]["id"]' < "$TMP/ics.json")"
[ -n "$CONN" ] || fail "the ICS connection has no id"
printf 'free accepted an ICS connection (%s) — the matrix gives the import to every plan\n' "${CONN:0:8}"
# The same URL twice is a conflict rather than a silent second connection: two rows for one
# feed would import every class twice.
STATUS="$(ajson "$TMP/dup.json" POST /calendar/ics '{"url":"https://calendar.example.invalid/feed.ics"}')"
[ "$STATUS" = "409" ] || fail "the same ICS URL twice answered $STATUS, expected 409"

STATUS="$(ajson "$TMP/conns.json" GET /calendar/connections)"
[ "$STATUS" = "200" ] || fail "GET /calendar/connections answered $STATUS"
python3 - "$TMP/conns.json" <<'PY' || fail "the connections list is wrong"
import json, sys
body = json.load(open(sys.argv[1]))
rows = body["connections"]
assert len(rows) == 1, f"expected one connection, got {len(rows)}"
row = rows[0]
assert row["provider"] == "ics", row
# A249: never a token of any kind, and no ICS URL either.
for banned in ("refresh_token", "refresh_token_enc", "ics_feed_token", "ics_url", "access_token"):
    assert banned not in row, f"{banned} leaked into ConnectionView: {row}"
for key in ("id", "provider", "account_email", "status", "error", "last_synced_at", "study_calendar_id"):
    assert key in row, f"ConnectionView is missing {key}"
print(f"connections: {len(rows)}, provider={row['provider']}, status={row['status']}, no token and no URL")
PY

# Google is the one gated thing in this phase, and it is gated by the matrix rather than by
# a literal: free has `google_calendar = false`, so the sync route refuses through the
# entitlement engine before it looks at any connection.
STATUS="$(ajson "$TMP/sync.json" POST /calendar/sync '{}')"
[ "$STATUS" = "403" ] || fail "POST /calendar/sync on free answered $STATUS, expected 403"
[ "$(field '["code"]' < "$TMP/sync.json")" = "plan_required" ] \
  || fail "the sync refusal is not plan_required: $(cat "$TMP/sync.json")"
printf 'POST /calendar/sync on free: 403 %s (min_plan=%s)\n' \
  "$(field '["code"]' < "$TMP/sync.json")" "$(field '["min_plan"]' < "$TMP/sync.json")"

# The consent flow's first leg landed with Task 6, so this is now a real assertion rather
# than a note: free has `google_calendar = false` **and** `google_accounts_max = 0`, and the
# route checks the boolean first, so the refusal names the plan that has the feature rather
# than reporting "0 of 0 accounts".
#
# It needs a server with the calendar OAuth client configured (`just smoke-server-phase10`
# sets dummy values; nothing here reaches Google). Without it the route answers 404 — the
# documented behaviour for a half-configured client — and a 404 is also what an unregistered
# route answers, so this scenario cannot tell those apart from outside. That is why the
# configuration is the run's precondition rather than something the script guesses at.
STATUS="$(ajson "$TMP/connect.json" GET /calendar/google/connect)"
if [ "$STATUS" = "404" ]; then
  fail "GET /calendar/google/connect answered 404. Either GOOGLE_CALENDAR_CLIENT_ID is unset on this server (start it with 'just smoke-server-phase10') or the route is unregistered; this scenario cannot distinguish them, which is why it requires the client."
fi
[ "$STATUS" = "403" ] || fail "GET /calendar/google/connect on free answered $STATUS, expected 403"
[ "$(field '["code"]' < "$TMP/connect.json")" = "plan_required" ] \
  || fail "the connect refusal is not plan_required: $(cat "$TMP/connect.json")"
printf 'GET /calendar/google/connect on free: 403 %s (min_plan=%s)\n' \
  "$(field '["code"]' < "$TMP/connect.json")" "$(field '["min_plan"]' < "$TMP/connect.json")"

# The callback is public and answers a redirect in every case, including its refusals: the
# browser is sitting on Google's redirect URI, and a JSON error page there is a dead end.
# A state nobody minted is refused, and the deep link carries a status and a reason — never
# a token (A235/A48). Nothing here needs the OAuth client, which is why it is asserted.
CALLBACK="$(curl -sS -o /dev/null -w '%{http_code} %{redirect_url}' \
  "$BASE/calendar/google/callback?code=abc&state=not-a-real-state")"
printf 'GET /calendar/google/callback with an unknown state: %s\n' "$CALLBACK"
case "$CALLBACK" in
  "302 stvdy://oauth/calendar?"*) ;;
  *) fail "the callback answered '$CALLBACK', expected a 302 to the app's deep link" ;;
esac
case "$CALLBACK" in
  *status=error*) ;;
  *) fail "the refusal did not carry a status the panel can render: $CALLBACK" ;;
esac
case "$CALLBACK" in
  *refresh*|*access_token*|*token=*) fail "the deep link leaked a credential: $CALLBACK" ;;
esac

say "7. the database agrees"
ROWS="$(db "select count(*) from calendar_connections where user_id = '$UID_' and provider = 'ics'")"
[ "$ROWS" = "1" ] || fail "the database holds $ROWS ICS connections, expected 1"
TOKEN_COL="$(db "select ics_feed_token from users where id = '$UID_'")"
[ "$TOKEN_COL" = "$TOKEN2" ] || fail "the stored token is not the one the route returned"
printf 'calendar_connections: %s ICS row(s); users.ics_feed_token matches the live token\n' "$ROWS"

printf '\n=== PASS\n'
