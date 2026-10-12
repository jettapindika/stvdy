#!/usr/bin/env bash
# Phase 13 end-to-end smoke test: the QRIS money path, against the real server, the real
# Postgres, and a gateway this run names out loud.
#
# Drives the real process over HTTP and reads the real database back, because the parts
# this phase can get wrong are exactly the parts a trait-level double cannot see: that the
# invoice the server *sends* carries the order id, the amount and the expiry we stored;
# that the signature over the raw bytes is what the route verifies; that a duplicate
# callback changes nothing in `subscriptions` or `credit_ledger`; that a mismatched amount
# lands in `review` with both numbers written down; and that the poller — the same
# `apply_paid` the webhook calls — does nothing to a payment the webhook already applied.
#
# **Which gateway this run used, printed and never implied.** B1 is unresolved: the real
# gateway has no identity, no docs and no response shapes, so this run exercises the wire
# the server sends against **`scripts/stub-qris.py`** (the mock) unless `QRIS_BASE_URL`
# points somewhere other than loopback, in which case it is the **sandbox** and this script
# only signs callbacks — it cannot mint invoices at a gateway it does not control. Either
# way the run prints the mode and the base URL in its first line, and nothing here claims a
# live gateway was exercised.
#
# Seven scenarios, in order:
#   1. a checkout: a pending row, the config's amount, a QR string, and a 15-minute expiry
#      — plus the invoice request the gateway actually received
#   2. a verified webhook: the plan active, one subscription, one `sub:` grant, and a
#      second device's token reading `pro`
#   3. the duplicate: the same event id twice (one activation), then the same payment under
#      a **fresh** event id (still one activation) — the two defences are different
#   4. a mismatched amount: refused into `review`, with nothing activated
#   5. an expired invoice: a payment made after the code died is `review`, and the poll
#      route reads `expired` rather than extending it
#   6. the poller: nothing happens to a payment the webhook already applied (asserted
#      structurally — the gateway is never asked about it), and a *pending* payment the
#      gateway reports paid is activated by the same code path
#   7. the margin report: the free/paid split, printed
#
# **Run this against a freshly started server.** It makes several requests to the
# credential routes, and `ratelimit::auth_layer` (A12) is an in-process bucket of 30
# requests per minute per IP, so a second run inside the same server process may have to
# wait for cells. The script waits and says so rather than hanging.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
GATEWAY_PORT="${GATEWAY_PORT:-}"
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

# --- configuration, read one key at a time ------------------------------------------------
#
# `.env` supplies these when the caller did not, and must not override an explicit value.
# Read the keys rather than sourcing the file: sourcing executes it, and a line as ordinary
# as `QRIS_BASE_URL= https://host` (a space after `=`) makes bash run the URL as a command.
env_key() { # $1 = key, $2 = default
  local value="${!1:-}"
  if [ -z "$value" ] && [ -f .env ]; then
    value="$(grep -E "^$1=" .env | tail -1 | cut -d= -f2- | sed -e 's/[[:space:]]*$//')"
  fi
  printf '%s' "${value:-$2}"
}

if [ -z "${DATABASE_URL:-}" ]; then
  DATABASE_URL="$(env_key DATABASE_URL "")"
fi
[ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"
db() { "$PSQL" "$DATABASE_URL" -tAc "$1"; }

GATEWAY_URL="$(env_key QRIS_BASE_URL "")"
QRIS_API_KEY="$(env_key QRIS_API_KEY "")"
CALLBACK_TOKEN="$(env_key QRIS_CALLBACK_TOKEN "")"
SIGN_SCHEME="$(env_key QRIS_SIGNATURE_SCHEME hmac_sha256_hex)"
SIGN_HEADER="$(env_key QRIS_SIGNATURE_HEADER X-Callback-Signature)"
TTL_MINUTES="$(env_key QRIS_INVOICE_TTL_MINUTES 15)"
MAIL_DIR="$(env_key MAIL_DIR "$MAIL_DIR")"

[ -n "$GATEWAY_URL" ] || fail "no QRIS_BASE_URL. The server refuses to mint an invoice \
without a gateway (502 provider_error), so this run cannot start a payment. Start the \
server with the stub, as 'just smoke-server-phase13' does, or point QRIS_BASE_URL at the \
sandbox."
[ -n "$CALLBACK_TOKEN" ] || fail "no QRIS_CALLBACK_TOKEN: nothing can be signed, so no \
callback could be verified."

# Which gateway this is. A loopback base URL is the stub this script starts; anything else
# is a gateway somebody else runs, and the run says so rather than assuming.
case "$GATEWAY_URL" in
  http://127.0.0.1:*|http://localhost:*) GATEWAY_MODE="mock (scripts/stub-qris.py)"; MOCK=1 ;;
  *) GATEWAY_MODE="sandbox (an external gateway at QRIS_BASE_URL)"; MOCK="" ;;
esac
# The stub's port is the base URL's own port, so a run with a different one starts the stub
# where the server is already looking.
if [ -z "$GATEWAY_PORT" ]; then
  GATEWAY_PORT="$(printf '%s' "$GATEWAY_URL" | sed -E 's#^https?://[^:/]+:([0-9]+).*$#\1#')"
  case "$GATEWAY_PORT" in
    ''|*[!0-9]*) GATEWAY_PORT=8200 ;;
  esac
fi
printf 'gateway: %s at %s | signature %s in %s | invoice ttl %s min\n' \
  "$GATEWAY_MODE" "$GATEWAY_URL" "$SIGN_SCHEME" "$SIGN_HEADER" "$TTL_MINUTES"
if [ "$GATEWAY_MODE" != "mock (scripts/stub-qris.py)" ]; then
  printf 'note: the sandbox mode signs callbacks for the run; the run cannot mint invoices\n'
  printf '      at a gateway it does not control, so it reads the invoice the server stored.\n'
  printf '      B1 is unresolved, so no gateway here is the vendor this product will use.\n'
fi

# --- the stub gateway, when the base URL is ours -------------------------------------------
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

case "$GATEWAY_MODE" in
  "mock (scripts/stub-qris.py)")
    say "the stub QRIS gateway"
    STUB_LOG="${STUB_LOG:-var/stub-phase13-requests.jsonl}"
    mkdir -p "$(dirname "$STUB_LOG")"
    rm -f "$STUB_LOG"
    if nc -z 127.0.0.1 "$GATEWAY_PORT" 2>/dev/null; then
      printf 'a listener is already on %s; using it and starting none (its own log, if any, is not this run'"'"'s)\n' \
        "$GATEWAY_PORT"
    else
      python3 scripts/stub-qris.py --port "$GATEWAY_PORT" --api-key "$QRIS_API_KEY" \
        --log "$STUB_LOG" > "$TMP/stub.log" 2>&1 &
      STUB_PID="$!"
      for _ in $(seq 1 40); do
        nc -z 127.0.0.1 "$GATEWAY_PORT" 2>/dev/null && break
        sleep 0.25
      done
      if ! nc -z 127.0.0.1 "$GATEWAY_PORT" 2>/dev/null; then
        cat "$TMP/stub.log"
        fail "the stub gateway did not start on $GATEWAY_PORT"
      fi
    fi
    nc -z 127.0.0.1 "$GATEWAY_PORT" 2>/dev/null || fail "no stub gateway on $GATEWAY_PORT"
    ;;
esac

say "health"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Start the stub with 'just smoke-qris' and the server with 'just smoke-server-phase13'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

# --- the prices, quoted from the file the server charges from ------------------------------
#
# Every number asserted below is read from `config/plans.toml` rather than written here, so
# a retuned config moves this run with it. A literal in this script would make the run pass
# against a server whose config disagrees with it — which is the one failure the amount
# assertion exists to catch.
say "the prices, from $PLANS"
[ -f "$PLANS" ] || fail "no plan config at $PLANS"
PRO_MONTHLY="$(python3 -c "
import tomllib; print(tomllib.load(open('$PLANS','rb'))['plans']['pro']['price_idr_monthly'])")"
PRO_YEARLY="$(python3 -c "
import tomllib; print(tomllib.load(open('$PLANS','rb'))['plans']['pro']['price_idr_yearly'])")"
PRO_CREDITS="$(python3 -c "
import tomllib; print(tomllib.load(open('$PLANS','rb'))['plans']['pro']['credits_per_month'])")"
[ -n "$PRO_MONTHLY" ] || fail "could not read pro's monthly price from $PLANS"
printf 'pro: monthly=%s yearly=%s credits_per_month=%s\n' \
  "$PRO_MONTHLY" "$PRO_YEARLY" "$PRO_CREDITS"

# --- accounts -------------------------------------------------------------------------------
DOMAIN="phase13-$STAMP.example.test"

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
    printf '      and re-run) or A154 per-IP signup limit is in play: %s attempts are recorded\n' \
      "$(db "select count(*) from signup_attempts where ip = '127.0.0.1'")" >&2
    printf '      for 127.0.0.1 and the limit is 25 per 24h. %s\n' "$(cat "$out")" >&2
    exit 1
  fi
  printf '%s' "$status"
}

signup() { # $1 = a short label used in the address
  local label="$1"
  local email="phase13-$label-$STAMP@$DOMAIN"
  local status
  status="$(auth_post "$TMP/signup-$label.json" "$BASE/auth/signup" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$email\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke laptop\",\"platform\":\"macos\"}}")"
  [ "$status" = "201" ] || fail "signup for $label answered $status: $(cat "$TMP/signup-$label.json")"
  # Newest first by name: the dev sink writes `<unix-millis>-<local part>.eml`.
  local mail
  mail="$(find "$MAIL_DIR" -name '*.eml' -type f | sort | tail -1)"
  [ -n "$mail" ] || fail "no verification mail in $MAIL_DIR; is the server writing there?"
  local token
  token="$(grep -o 'token=[A-Za-z0-9_-]*' "$mail" | head -1 | cut -d= -f2)"
  [ -n "$token" ] || fail "the verification mail has no token: $mail"
  status="$(auth_post "$TMP/verify-$label.json" "$BASE/auth/verify-email" \
    -H 'content-type: application/json' -d "{\"token\":\"$token\"}")"
  [ "$status" = "200" ] || fail "verifying $label answered $status: $(cat "$TMP/verify-$label.json")"
  field '["access_token"]' < "$TMP/signup-$label.json"
}

# A second device's token for the same account: a login, because a second signup for an
# address that exists is a 409 and would be proving nothing about this account.
login_second_device() { # $1 = email, $2 = platform
  local status
  status="$(auth_post "$TMP/login-second.json" "$BASE/auth/login" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$1\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Phone\",\"platform\":\"$2\"}}")"
  [ "$status" = "200" ] || fail "the second device's login answered $status: $(cat "$TMP/login-second.json")"
  field '["access_token"]' < "$TMP/login-second.json"
}

uid() { db "select id from users where email = 'phase13-$1-$STAMP@$DOMAIN'"; }
email_of() { printf 'phase13-%s-%s@%s' "$1" "$STAMP" "$DOMAIN"; }

# The signature, computed the way the gateway would: over the **raw bytes** we then post.
#
# `--data-binary` rather than `-d`, and the body file is written by the same python that
# signed it: a shell that re-encoded the JSON would sign different bytes than it sent, and
# the signature would fail for a reason that looks like a server bug.
cat > "$TMP/sign.py" <<'PY'
import base64, hashlib, hmac, json, sys

src, out, scheme, secret = sys.argv[1:5]
event = json.load(open(src))
body = json.dumps(event, separators=(",", ":")).encode()
open(out, "wb").write(body)

mac = hmac.new(secret.encode(), body, hashlib.sha256)
if scheme == "hmac_sha256_hex":
    print(mac.hexdigest())
elif scheme == "hmac_sha256_base64":
    print(base64.b64encode(mac.digest()).decode())
elif scheme == "shared_token":
    print(secret)
else:
    raise SystemExit(f"unknown QRIS_SIGNATURE_SCHEME {scheme!r}")
PY

post_webhook() { # $1 = output file, $2 = the event as a JSON file
  local sig
  sig="$(python3 "$TMP/sign.py" "$2" "$TMP/event-body.json" "$SIGN_SCHEME" "$CALLBACK_TOKEN")"
  [ -n "$sig" ] || fail "signing the callback produced nothing"
  curl -sS -o "$1" -w '%{http_code}' -X POST "$BASE/webhooks/qris" \
    -H 'content-type: application/json' -H "$SIGN_HEADER: $sig" \
    --data-binary "@$TMP/event-body.json"
}

# A paid event, as `mapping::webhook_from_body` reads it.
paid_event() { # $1 = out file, $2 = event id, $3 = order id, $4 = amount, $5 = paid_at
  python3 - "$1" "$2" "$3" "$4" "$5" <<'PY'
import json, sys
out, event_id, order_id, amount, paid_at = sys.argv[1:6]
json.dump(
    {
        "event_id": event_id,
        "kind": "paid",
        "order_id": order_id,
        "amount_idr": int(amount),
        "currency": "IDR",
        "paid_at": paid_at,
        "provider_payment_id": f"gw-{event_id}",
    },
    open(out, "w"),
)
PY
}

now_rfc3339() { python3 -c "import datetime; print(datetime.datetime.now(datetime.UTC).isoformat().replace('+00:00','Z'))"; }

# The gateway's request log, for the assertions about what we *sent*.
gw_requests() { curl -fsS "$GATEWAY_URL/_control/requests"; }

# ==========================================================================================
# 1. A checkout
# ==========================================================================================
say "1. a checkout: a pending row, the config's amount, a QR string, a $TTL_MINUTES-minute expiry"
CHECKOUT="$(signup checkout)"
CHECKOUT_ID="$(uid checkout)"
CHECKOUT_EMAIL="$(email_of checkout)"
STATUS="$(curl -sS -o "$TMP/payment.json" -w '%{http_code}' -X POST "$BASE/billing/checkout" \
  -H "authorization: Bearer $CHECKOUT" -H 'content-type: application/json' \
  -d '{"plan_id":"pro","billing_period":"monthly"}')"
[ "$STATUS" = "200" ] || fail "the checkout answered $STATUS: $(cat "$TMP/payment.json")"

PAYMENT_ID="$(field '["id"]' < "$TMP/payment.json")"
ORDER_ID="$(field '["order_id"]' < "$TMP/payment.json")"
AMOUNT="$(field '["amount_idr"]' < "$TMP/payment.json")"
QR="$(field '["qr_string"]' < "$TMP/payment.json")"
EXPIRES="$(field '["expires_at"]' < "$TMP/payment.json")"
P_STATUS="$(field '["status"]' < "$TMP/payment.json")"
printf 'checkout: status=%s amount=%s expires_at=%s order_id=%s\n' \
  "$P_STATUS" "$AMOUNT" "$EXPIRES" "$ORDER_ID"

[ "$P_STATUS" = "pending" ] || fail "a fresh checkout is pending, got $P_STATUS"
[ "$AMOUNT" = "$PRO_MONTHLY" ] || fail "the amount is $AMOUNT; $PLANS says pro monthly is $PRO_MONTHLY"
[ -n "$QR" ] && [ "$QR" != "None" ] || fail "no QR string on the invoice"
python3 - "$EXPIRES" "$TTL_MINUTES" <<'PY'
import datetime, sys
expires = datetime.datetime.fromisoformat(sys.argv[1].replace("Z", "+00:00"))
ttl = int(sys.argv[2])
now = datetime.datetime.now(datetime.UTC)
minutes = (expires - now).total_seconds() / 60
assert 0 < minutes <= ttl, f"the expiry is {minutes:.1f} minutes away, wanted at most {ttl}"
assert minutes > ttl - 2, f"the expiry is {minutes:.1f} minutes away, wanted about {ttl}"
print(f"expiry is {minutes:.1f} minutes away, from a {ttl}-minute ttl")
PY
# The stored row is what the webhook checks, so the response and the row are compared.
STORED="$(db "select status || '|' || amount_idr || '|' || coalesce(qr_string,'') from payments where id = '$PAYMENT_ID'")"
[ "$STORED" = "pending|$AMOUNT|$QR" ] || fail "the stored row disagrees with the response: $STORED"

if [ "$GATEWAY_MODE" = "mock (scripts/stub-qris.py)" ]; then
  # **What we sent.** The stub derives the invoice from the request, so a QR string that is
  # the gateway's own is proof the invoice request carried this order id and this amount —
  # and the stub's 401 on a wrong `Authorization` is what makes the auth header observable.
  [ "$QR" = "STUBQRIS:$ORDER_ID:$AMOUNT" ] \
    || fail "the QR string is $QR, not the stub's derivation from this request"
  python3 - "$(gw_requests)" "$ORDER_ID" "$AMOUNT" <<'PY'
import json, sys
requests = json.loads(sys.argv[1])["requests"]
order_id, amount = sys.argv[2], int(sys.argv[3])
sent = [r for r in requests if r.get("path") == "/invoices" and r.get("method") == "POST"]
assert sent, f"the gateway received no invoice request: {requests}"
last = sent[-1]
assert last["body"]["order_id"] == order_id, last
assert last["body"]["amount_idr"] == amount, last
assert last["body"]["expires_at"], last
assert last["auth"], "the invoice request carried no Authorization header"
print(f"the gateway received POST /invoices with order_id={order_id} amount_idr={amount}")
PY
  printf 'the gateway received the invoice request, and the QR string is its own derivation\n'
else
  printf 'the sandbox minted the QR string (stored length %s); the run cannot derive it\n' "${#QR}"
fi

# ==========================================================================================
# 2. A verified webhook
# ==========================================================================================
say "2. a verified webhook: the plan active, one subscription, one grant, a second device"
SUBS_BEFORE="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_BEFORE="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
[ "$SUBS_BEFORE" = "0" ] || fail "a fresh account already has $SUBS_BEFORE subscription(s)"

PAID_AT="$(now_rfc3339)"
paid_event "$TMP/event-1.json" "evt-$STAMP-1" "$ORDER_ID" "$AMOUNT" "$PAID_AT"
STATUS="$(post_webhook "$TMP/hook-1.json" "$TMP/event-1.json")"
[ "$STATUS" = "200" ] || fail "the webhook answered $STATUS: $(cat "$TMP/hook-1.json")"
printf 'webhook: status=%s duplicate=%s status=%s\n' "$STATUS" \
  "$(field '["duplicate"]' < "$TMP/hook-1.json")" "$(field '["status"]' < "$TMP/hook-1.json")"
[ "$(field '["duplicate"]' < "$TMP/hook-1.json")" = "False" ] || fail "the first callback is not a duplicate"
[ "$(field '["status"]' < "$TMP/hook-1.json")" = "activated" ] \
  || fail "the first callback must activate: $(cat "$TMP/hook-1.json")"

ROW="$(db "select status || '|' || coalesce(paid_at::text,'') from payments where id = '$PAYMENT_ID'")"
case "$ROW" in paid\|*) ;; *) fail "the payment is not paid: $ROW" ;; esac
SUBS_AFTER="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID' and status = 'active'")"
LEDGER_AFTER="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
GRANTED="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
PERIOD_END="$(db "select current_period_end from subscriptions where user_id = '$CHECKOUT_ID' and status = 'active'")"
printf 'applied: subscriptions %s -> %s | sub: grants %s -> %s (delta %s) | period_end %s\n' \
  "$SUBS_BEFORE" "$SUBS_AFTER" "$LEDGER_BEFORE" "$LEDGER_AFTER" "$GRANTED" "$PERIOD_END"
[ "$SUBS_AFTER" = "1" ] || fail "the webhook wrote $SUBS_AFTER active subscriptions, wanted 1"
[ "$LEDGER_AFTER" = "1" ] || fail "the webhook wrote $LEDGER_AFTER subscription grants, wanted 1"
[ "$GRANTED" = "$PRO_CREDITS" ] || fail "the grant is $GRANTED credits; the matrix says $PRO_CREDITS"
python3 - "$PAID_AT" "$PERIOD_END" <<'PY'
import datetime, sys
paid = datetime.datetime.fromisoformat(sys.argv[1].replace("Z", "+00:00"))
end = datetime.datetime.fromisoformat(sys.argv[2].replace("Z", "+00:00"))
days = (end - paid).total_seconds() / 86400
assert 29.9 < days < 30.1, f"a monthly period is 30 days, this one is {days:.2f}"
print(f"the period is {days:.2f} days: a fresh 30-day period from paid_at (A296)")
PY

# The plan, on the device that did not pay. The plan is not a JWT claim (A22): entitlements
# are loaded per request, so a token minted after the webhook sees it.
SECOND="$(login_second_device "$CHECKOUT_EMAIL" "web")"
curl -fsS "$BASE/usage" -H "authorization: Bearer $SECOND" > "$TMP/usage.json"
PLAN="$(field '["plan"]' < "$TMP/usage.json")"
SUB_STATUS="$(field '["subscription"]["status"]' < "$TMP/usage.json")"
printf 'the second device: plan=%s subscription=%s period_end=%s\n' \
  "$PLAN" "$SUB_STATUS" "$(field '["subscription"]["period_end"]' < "$TMP/usage.json")"
[ "$PLAN" = "pro" ] || fail "the second device sees $PLAN, not the paid plan"
[ "$SUB_STATUS" = "active" ] || fail "the second device sees subscription $SUB_STATUS"

# ==========================================================================================
# 3. The duplicate — both defences
# ==========================================================================================
say "3. the duplicate: the same event twice, then the same payment under a fresh event id"
PAID_BEFORE="$(db "select paid_at from payments where id = '$PAYMENT_ID'")"
STATUS="$(post_webhook "$TMP/hook-2.json" "$TMP/event-1.json")"
[ "$STATUS" = "200" ] || fail "the duplicate answered $STATUS: $(cat "$TMP/hook-2.json")"
DUP="$(field '["duplicate"]' < "$TMP/hook-2.json")"
DUP_STATUS="$(field '["status"]' < "$TMP/hook-2.json")"
printf 'the same event id again: duplicate=%s status=%s\n' "$DUP" "$DUP_STATUS"
[ "$DUP" = "True" ] || fail "the second identical event is not reported as a duplicate"
[ "$DUP_STATUS" = "duplicate" ] || fail "the duplicate's status is $DUP_STATUS"

# Defence two: the same payment reported under a **new** event id. A unique key on the event
# id cannot see this one; the guarded transition is what catches it.
paid_event "$TMP/event-2.json" "evt-$STAMP-2" "$ORDER_ID" "$AMOUNT" "$PAID_AT"
STATUS="$(post_webhook "$TMP/hook-3.json" "$TMP/event-2.json")"
[ "$STATUS" = "200" ] || fail "the re-report answered $STATUS: $(cat "$TMP/hook-3.json")"
REPORT="$(field '["status"]' < "$TMP/hook-3.json")"
printf 'the same payment, a fresh event id: duplicate=%s status=%s\n' \
  "$(field '["duplicate"]' < "$TMP/hook-3.json")" "$REPORT"
[ "$REPORT" = "already_paid" ] || fail "a second report of a paid payment must be already_paid, got $REPORT"

SUBS_NOW="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_NOW="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
PAID_AFTER="$(db "select paid_at from payments where id = '$PAYMENT_ID'")"
EVENTS="$(db "select count(*) from webhook_events where event_id in ('evt-$STAMP-1','evt-$STAMP-2')")"
printf 'after both replays: subscriptions=%s grants=%s paid_at unchanged=%s webhook_events=%s\n' \
  "$SUBS_NOW" "$LEDGER_NOW" "$([ "$PAID_BEFORE" = "$PAID_AFTER" ] && echo yes || echo no)" "$EVENTS"
[ "$SUBS_NOW" = "1" ] || fail "a replay wrote a second subscription: $SUBS_NOW"
[ "$LEDGER_NOW" = "1" ] || fail "a replay wrote a second grant: $LEDGER_NOW"
[ "$PAID_BEFORE" = "$PAID_AFTER" ] || fail "a replay moved paid_at"
[ "$EVENTS" = "2" ] || fail "the audit rows are wrong: $EVENTS"

# ==========================================================================================
# 4. A mismatched amount
# ==========================================================================================
say "4. a mismatched amount: refused into review, with nothing activated"
MISMATCH="$(signup mismatch)"
MISMATCH_ID="$(uid mismatch)"
STATUS="$(curl -sS -o "$TMP/payment-2.json" -w '%{http_code}' -X POST "$BASE/billing/checkout" \
  -H "authorization: Bearer $MISMATCH" -H 'content-type: application/json' \
  -d '{"plan_id":"pro","billing_period":"monthly"}')"
[ "$STATUS" = "200" ] || fail "the second checkout answered $STATUS: $(cat "$TMP/payment-2.json")"
ORDER_2="$(field '["order_id"]' < "$TMP/payment-2.json")"
PAYMENT_2="$(field '["id"]' < "$TMP/payment-2.json")"
paid_event "$TMP/event-3.json" "evt-$STAMP-3" "$ORDER_2" "1" "$(now_rfc3339)"
STATUS="$(post_webhook "$TMP/hook-4.json" "$TMP/event-3.json")"
[ "$STATUS" = "200" ] || fail "the mismatched callback answered $STATUS: $(cat "$TMP/hook-4.json")"
printf 'a 1-IDR callback against a %s-IDR invoice: status=%s\n' "$PRO_MONTHLY" \
  "$(field '["status"]' < "$TMP/hook-4.json")"
[ "$(field '["status"]' < "$TMP/hook-4.json")" = "review" ] || fail "a mismatch must be review"
REVIEW="$(db "select status || ' :: ' || coalesce(review_reason,'') from payments where id = '$PAYMENT_2'")"
printf 'the row: %s\n' "$REVIEW"
case "$REVIEW" in review*) ;; *) fail "the payment did not land in review: $REVIEW" ;; esac
python3 - "$REVIEW" <<'PY'
import sys
row = sys.argv[1]
assert "1 IDR" in row and "10000 IDR" in row, f"both numbers must be written down: {row}"
PY
[ "$(db "select count(*) from subscriptions where user_id = '$MISMATCH_ID'")" = "0" ] \
  || fail "a mismatched amount activated a subscription"
[ "$(db "select count(*) from credit_ledger where user_id = '$MISMATCH_ID' and period_key like 'sub:%'")" = "0" ] \
  || fail "a mismatched amount granted credits"

# ==========================================================================================
# 5. An expired invoice
# ==========================================================================================
#
# Two different facts, and the server keeps them apart (A295): a payment made *after* the
# code expired is `review` unconditionally, because a switch that could activate it would be
# a switch that sells expired invoices. The run drives the expiry by moving the row's
# `expires_at` into the past, which is what a student who waited too long leaves behind.
say "5. an expired invoice: paid after the code died, and the poll route says so"
EXPIRED="$(signup expired)"
EXPIRED_ID="$(uid expired)"
STATUS="$(curl -sS -o "$TMP/payment-3.json" -w '%{http_code}' -X POST "$BASE/billing/checkout" \
  -H "authorization: Bearer $EXPIRED" -H 'content-type: application/json' \
  -d '{"plan_id":"pro","billing_period":"yearly"}')"
[ "$STATUS" = "200" ] || fail "the third checkout answered $STATUS: $(cat "$TMP/payment-3.json")"
ORDER_3="$(field '["order_id"]' < "$TMP/payment-3.json")"
PAYMENT_3="$(field '["id"]' < "$TMP/payment-3.json")"
[ "$(field '["amount_idr"]' < "$TMP/payment-3.json")" = "$PRO_YEARLY" ] \
  || fail "the yearly amount is not the config's $PRO_YEARLY"

db "update payments set expires_at = now() - interval '1 minute' where id = '$PAYMENT_3'" > /dev/null

# **The late callback first, while the row is still `pending`.** The poll route re-checks
# expiry and *persists* it, so asking the poll first would terminalise the row and the
# callback would then hit the guarded transition (`where status = 'pending'`), update zero
# rows and answer `already_paid` — a correct answer to a different question. The order is
# therefore the order a real late payment arrives in: the money moves, then someone looks.
paid_event "$TMP/event-4.json" "evt-$STAMP-4" "$ORDER_3" "$PRO_YEARLY" "$(now_rfc3339)"
STATUS="$(post_webhook "$TMP/hook-5.json" "$TMP/event-4.json")"
[ "$STATUS" = "200" ] || fail "the late callback answered $STATUS: $(cat "$TMP/hook-5.json")"
LATE="$(db "select status || ' :: ' || coalesce(review_reason,'') from payments where id = '$PAYMENT_3'")"
printf 'a payment made after the expiry: %s\n' "$LATE"
case "$LATE" in review*) ;; *) fail "a payment after the expiry must be review: $LATE" ;; esac
case "$LATE" in *"expired at"*) ;; *) fail "the reason must name the expiry: $LATE" ;; esac
[ "$(db "select count(*) from subscriptions where user_id = '$EXPIRED_ID'")" = "0" ] \
  || fail "an expired invoice activated a subscription"

# And the poll route re-checks expiry on its own, so a student whose clock is wrong cannot
# extend their own invoice by not noticing. Driven on a **second** expired invoice, because
# the one above is now `review` and a poll of it would prove nothing about the re-check.
STATUS="$(curl -sS -o "$TMP/payment-5.json" -w '%{http_code}' -X POST "$BASE/billing/checkout" \
  -H "authorization: Bearer $EXPIRED" -H 'content-type: application/json' \
  -d '{"plan_id":"pro","billing_period":"monthly"}')"
[ "$STATUS" = "200" ] || fail "the fifth checkout answered $STATUS: $(cat "$TMP/payment-5.json")"
PAYMENT_5="$(field '["id"]' < "$TMP/payment-5.json")"
db "update payments set expires_at = now() - interval '1 minute' where id = '$PAYMENT_5'" > /dev/null
curl -fsS "$BASE/billing/payments/$PAYMENT_5" -H "authorization: Bearer $EXPIRED" > "$TMP/poll-expired.json"
READ_BACK="$(field '["status"]' < "$TMP/poll-expired.json")"
STORED_5="$(db "select status from payments where id = '$PAYMENT_5'")"
printf 'the poll route on a pending-but-expired invoice: reads %s, the row is %s\n' "$READ_BACK" "$STORED_5"
[ "$READ_BACK" = "expired" ] || fail "an expired invoice must read expired, got $READ_BACK"
[ "$STORED_5" = "expired" ] || fail "the poll must persist the expiry, the row is $STORED_5"

# ==========================================================================================
# 6. The poller
# ==========================================================================================
#
# Two halves, and the first is the one that makes the second mean something.
#
#   (a) **After the webhook, the poller changes nothing — structurally.** The proof is not
#       that the numbers are unchanged but that the gateway is **never asked**: the poll
#       pass selects `status = 'pending'` rows, and a paid row is not one. Asserting only
#       the numbers would pass against a poller that asked and then did nothing.
#   (b) **A pending payment the gateway reports paid is activated**, by the same
#       `apply_paid` the webhook calls. Without this half, (a) is what a poller that never
#       ran would also report.
say "6. the poller: nothing for a paid row (never asked), everything for a pending one"

run_job() { # $1 = kind, $2 = payload json
  local kind="$1" payload="$2"
  local job_id
  # `head -1`: `psql` prints the `INSERT 0 1` command tag after the returned id, and taking
  # the whole output would make the id `"<uuid>\nINSERT 0 1"`.
  job_id="$(db "insert into jobs (kind, payload) values ('$kind', '$payload'::jsonb) returning id" | head -1)"
  local status=""
  for _ in $(seq 1 120); do
    status="$(db "select status from jobs where id = '$job_id'")"
    [ "$status" = "done" ] && break
    [ "$status" = "failed" ] && break
    sleep 0.5
  done
  if [ "$status" != "done" ]; then
    local error
    error="$(db "select coalesce(error,'') from jobs where id = '$job_id'")"
    case "$error" in
      *"unknown job kind"*)
        fail "another stvdy-server claimed this run's $kind job and does not know the kind, so \
its image predates this phase. It shares the jobs table through DATABASE_URL. Stop it (check \
'lsof -nP -iTCP:8099 -sTCP:LISTEN') or run this against a database only one server uses. \
The job's own error: $error"
        ;;
    esac
    fail "the $kind job did not finish (status=$status, error=${error:-none})"
  fi
  printf '  %s: done\n' "$kind"
}

# (a) The gateway is told nothing about the paid order, and nothing moves.
#
# The structural half — "the gateway was never asked" — is only observable in mock mode,
# because it reads the stub's own request log. Against a sandbox the run says so and falls
# back to the numbers, which is a weaker check and is labelled as one.
SUBS_SNAPSHOT="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_SNAPSHOT="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID'")"
if [ "$MOCK" = "1" ]; then
  ASKED_BEFORE="$(gw_requests | python3 -c "
import json,sys
print(sum(1 for r in json.load(sys.stdin)['requests'] if r.get('path') == '/invoices/$ORDER_ID'))")"
  # The gateway *would* say paid if it were asked, so "nothing changed" is not an accident
  # of the stub having nothing to say.
  curl -fsS -X POST "$GATEWAY_URL/_control/status" -H 'content-type: application/json' \
    -d "{\"order_id\":\"$ORDER_ID\",\"status\":{\"status\":\"paid\",\"paid_at\":\"$PAID_AT\",\"amount_idr\":$AMOUNT,\"provider_payment_id\":\"stub-late\"}}" \
    > /dev/null
  run_job poll_payment '{}'
  ASKED_AFTER="$(gw_requests | python3 -c "
import json,sys
print(sum(1 for r in json.load(sys.stdin)['requests'] if r.get('path') == '/invoices/$ORDER_ID'))")"
  SUBS_NOW="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
  LEDGER_NOW="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID'")"
  printf '  the paid order: asked %s -> %s time(s) | subscriptions %s -> %s | ledger rows %s -> %s\n' \
    "$ASKED_BEFORE" "$ASKED_AFTER" "$SUBS_SNAPSHOT" "$SUBS_NOW" "$LEDGER_SNAPSHOT" "$LEDGER_NOW"
  [ "$ASKED_AFTER" = "$ASKED_BEFORE" ] \
    || fail "the poller asked the gateway about a payment that is no longer pending"
else
  printf '  the sandbox cannot be scripted or read back, so the poller runs against it and\n'
  printf '  only the numbers are asserted (the "never asked" half needs the stub'"'"'s log)\n'
  run_job poll_payment '{}'
  SUBS_NOW="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
  LEDGER_NOW="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID'")"
  printf '  subscriptions %s -> %s | ledger rows %s -> %s\n' \
    "$SUBS_SNAPSHOT" "$SUBS_NOW" "$LEDGER_SNAPSHOT" "$LEDGER_NOW"
fi
[ "$SUBS_NOW" = "$SUBS_SNAPSHOT" ] || fail "the poller changed the subscription count"
[ "$LEDGER_NOW" = "$LEDGER_SNAPSHOT" ] || fail "the poller wrote ledger rows"

# (b) A pending payment, which the gateway reports paid. Mock mode only, because the answer
# has to be scripted: a sandbox would honestly report `pending` and this half would prove
# nothing.
if [ "$MOCK" = "1" ]; then
POLLED="$(signup polled)"
POLLED_ID="$(uid polled)"
STATUS="$(curl -sS -o "$TMP/payment-4.json" -w '%{http_code}' -X POST "$BASE/billing/checkout" \
  -H "authorization: Bearer $POLLED" -H 'content-type: application/json' \
  -d '{"plan_id":"pro","billing_period":"monthly"}')"
[ "$STATUS" = "200" ] || fail "the fourth checkout answered $STATUS: $(cat "$TMP/payment-4.json")"
ORDER_4="$(field '["order_id"]' < "$TMP/payment-4.json")"
PAYMENT_4="$(field '["id"]' < "$TMP/payment-4.json")"
curl -fsS -X POST "$GATEWAY_URL/_control/status" -H 'content-type: application/json' \
  -d "{\"order_id\":\"$ORDER_4\",\"status\":{\"status\":\"paid\",\"paid_at\":\"$(now_rfc3339)\",\"amount_idr\":$PRO_MONTHLY,\"provider_payment_id\":\"stub-poll\"}}" \
  > /dev/null
run_job poll_payment '{}'
POLLED_STATUS="$(db "select status from payments where id = '$PAYMENT_4'")"
POLLED_SUBS="$(db "select count(*) from subscriptions where user_id = '$POLLED_ID' and status = 'active'")"
POLLED_GRANTS="$(db "select count(*) from credit_ledger where user_id = '$POLLED_ID' and period_key like 'sub:%'")"
printf '  a lost webhook, recovered by polling: payment=%s subscriptions=%s grants=%s\n' \
  "$POLLED_STATUS" "$POLLED_SUBS" "$POLLED_GRANTS"
[ "$POLLED_STATUS" = "paid" ] || fail "the poller did not apply a paid answer: $POLLED_STATUS"
[ "$POLLED_SUBS" = "1" ] || fail "the poller wrote $POLLED_SUBS subscriptions, wanted 1"
[ "$POLLED_GRANTS" = "1" ] || fail "the poller wrote $POLLED_GRANTS grants, wanted 1"

# And a second pass over the now-paid row changes nothing: the same idempotency the webhook
# has, because it is the same function.
run_job poll_payment '{}'
POLLED_SUBS_2="$(db "select count(*) from subscriptions where user_id = '$POLLED_ID' and status = 'active'")"
POLLED_GRANTS_2="$(db "select count(*) from credit_ledger where user_id = '$POLLED_ID' and period_key like 'sub:%'")"
printf '  the same poll a second time: subscriptions %s -> %s | grants %s -> %s\n' \
  "$POLLED_SUBS" "$POLLED_SUBS_2" "$POLLED_GRANTS" "$POLLED_GRANTS_2"
[ "$POLLED_SUBS_2" = "$POLLED_SUBS" ] || fail "a second poll wrote a second subscription"
[ "$POLLED_GRANTS_2" = "$POLLED_GRANTS" ] || fail "a second poll wrote a second grant"
else
  printf '  the sandbox half of the poller is not driven: a real gateway would report\n'
  printf '  `pending` for a payment that has not been made, which proves nothing. The\n'
  printf '  activation is covered by crates/server/tests/payment_jobs.rs.\n'
fi

# ==========================================================================================
# 7. The margin report's split
# ==========================================================================================
#
# The admin flag is set in the database because the smoke run has no admin console: the
# route takes `AdminUser`, which reads `users.is_admin` per request, so the flag is what an
# operator would set once. The account is a **customer** in the report, and the report
# excludes admins from both sides — so the account that reads it is the one that pays, and
# the numbers below are its own.
say "7. the margin report: the free/paid split"
db "update users set is_admin = true where id = '$CHECKOUT_ID'" > /dev/null
FROM="$(python3 -c "
import datetime; print((datetime.datetime.now(datetime.UTC) - datetime.timedelta(days=40)).isoformat().replace('+00:00','Z'))")"
TO="$(python3 -c "
import datetime; print((datetime.datetime.now(datetime.UTC) + datetime.timedelta(days=1)).isoformat().replace('+00:00','Z'))")"
STATUS="$(curl -sS -o "$TMP/margin.json" -w '%{http_code}' \
  "$BASE/admin/margin?from=$FROM&to=$TO" -H "authorization: Bearer $CHECKOUT")"
[ "$STATUS" = "200" ] || fail "the margin report answered $STATUS: $(cat "$TMP/margin.json")"
python3 - "$TMP/margin.json" "$PRO_MONTHLY" "$(env_key IDR_PER_USD 17889)" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
price, rate = int(sys.argv[2]), int(sys.argv[3])
assert report["idr_per_usd"] == rate, f"the fx rate is printed as an input: {report['idr_per_usd']}"
assert report["months"], "the range covers a month with data"
month = report["months"][-1]
plans = {row["plan_id"]: row for row in month["per_plan"]}
assert "pro" in plans, f"the month the payment landed in has no pro row: {plans}"
pro = plans["pro"]
assert pro["revenue_idr"] >= price, f"revenue {pro['revenue_idr']} is under the one payment"
assert pro["byok_cost_usd"] == 0.0, "no call here used a student's own key"
print(f"month {month['month']}: revenue {month['revenue_idr']} IDR, gateway fee {month['gateway_fee_idr']} IDR")
for plan_id in sorted(plans):
    row = plans[plan_id]
    print(
        f"  {plan_id}: provider_cost ${row['provider_cost_usd']:.6f} "
        f"byok ${row['byok_cost_usd']:.6f} revenue {row['revenue_idr']} IDR "
        f"gross_margin {row['gross_margin_idr']} IDR"
    )
free = plans.get("free")
if free is None:
    print("  free: no row this month (no free-tier spend was recorded)")
else:
    print(f"  free: provider_cost ${free['provider_cost_usd']:.6f} revenue {free['revenue_idr']} IDR")
print(
    f"  utilization {month['free_utilization']} | conversion of free {month['conversion_of_free']} "
    f"| of total {month['conversion_of_total']} | paying {month['paying']} free {month['free_accounts']}"
)
print(f"  tripwires {json.dumps(report['tripwires'])}")
PY
printf 'the free tier and the paid tier are separate rows, with the fx rate printed beside them\n'

printf '\n=== PASS\n'
