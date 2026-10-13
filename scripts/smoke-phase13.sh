#!/usr/bin/env bash
# Phase 13 end-to-end smoke test: the QRIS money path, against the real server, the real
# Postgres, and a gateway this run scripts.
#
# Drives the real process over HTTP and reads the real database back, because the parts this
# phase can get wrong are exactly the parts a trait-level double cannot see: that the invoice
# the server *sends* carries the order id, the price and the lifetime, and that the figure it
# stores is the **payable amount the gateway assigned**; that the client's own poll is what
# applies a paid answer; that a replay changes nothing in `subscriptions`, `credit_ledger` or
# `gateway_events`; that a settlement for the bare price lands in `review` with both numbers
# written down; and that the reconciliation sweep joins the gateway's feed to our rows **by
# amount**.
#
# **The gateway never pushes.** B1 resolved to a self-hosted QRIS bridge (`gopay-bridge`) that
# is *polled*: the server's only ingress is its own status reads, so there is no callback to
# sign and no payload to forge, and a local run can only produce "the buyer paid" by being the
# thing the server asks. That is what `scripts/stub-qris.py` is, and this run drives it through
# its `/_control` routes. The run prints the gateway and its base URL in its first line, and
# nothing here claims a live gateway was exercised.
#
# Nine scenarios, in order:
#   1. a checkout: a pending row, the config's price, the gateway's **payable** (the price plus
#      its unique offset), a QR string the stub derived from this request, and the invoice
#      request the gateway actually received
#   2. a status word the bridge never sends: the server refuses it rather than obeying it
#   3. the client's poll activates the payment: the plan active, one subscription, one `sub:`
#      grant, one `gateway_events` row, and a second device's token reading `pro`
#   4. the replay: the same answer twice — one activation, one audit row, and the gateway is
#      never asked again (the guarded transition's own subject is gone with the callback, and
#      the run says so where it used to be asserted)
#   5. a settlement for the bare price: `review`, with both numbers in `review_reason` — the
#      assertion `payments.payable_amount_idr` exists for
#   6. an expired invoice: the poll route reads and persists `expired` rather than extending it
#   7. the poller: nothing for a payment the client's poll already applied (asserted
#      structurally — the gateway is never asked about it), and a *pending* payment the gateway
#      reports paid is activated by the same code path, without double-activating
#   8. the reconciliation sweep: the feed's join is on the amount
#   9. the margin report: the free/paid split, printed
#
# **Run this against a freshly started server.** It makes several requests to the credential
# routes, and `ratelimit::auth_layer` (A12) is an in-process bucket of 30 requests per minute
# per IP, so a second run inside the same server process may have to wait for cells. The script
# waits and says so rather than hanging.
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
MERCHANT_ID="$(env_key QRIS_MERCHANT_ID "")"
TTL_MINUTES="$(env_key QRIS_INVOICE_TTL_MINUTES 15)"
RECONCILE_HOURS="$(env_key QRIS_RECONCILE_HOURS 48)"
MAIL_DIR="$(env_key MAIL_DIR "$MAIL_DIR")"

[ -n "$GATEWAY_URL" ] || fail "no QRIS_BASE_URL. The server refuses to mint an invoice \
without a gateway (502 provider_error), so this run cannot start a payment. Start the \
server with the stub, as 'just smoke-server-phase13' does."
[ -n "$MERCHANT_ID" ] || fail "no QRIS_MERCHANT_ID. The server refuses to boot with a base \
URL and no merchant id (A301), and this run reads the sweep's own query back, so it needs \
the same value the server was started with."

# Which gateway this is. A loopback base URL is the stub this script starts; anything else is
# a gateway somebody else runs — and a gateway this script does not control cannot be
# scripted, which is every paid answer below. So the mode is printed and a non-loopback base
# URL is refused rather than quietly turning the run into something weaker.
case "$GATEWAY_URL" in
  http://127.0.0.1:*|http://localhost:*) GATEWAY_MODE="stub (scripts/stub-qris.py)" ;;
  *) fail "$GATEWAY_URL is not loopback. This run scripts the gateway's answers through the \
stub's /_control routes — the bridge never pushes, so a paid answer has to be produced on \
demand — and a gateway this script does not control cannot be scripted. Point QRIS_BASE_URL \
at scripts/stub-qris.py, which is what 'just smoke-server-phase13' does." ;;
esac
# The stub's port is the base URL's own port, so a run with a different one starts the stub
# where the server is already looking.
if [ -z "$GATEWAY_PORT" ]; then
  GATEWAY_PORT="$(printf '%s' "$GATEWAY_URL" | sed -E 's#^https?://[^:/]+:([0-9]+).*$#\1#')"
  case "$GATEWAY_PORT" in
    ''|*[!0-9]*) GATEWAY_PORT=8200 ;;
  esac
fi
printf 'gateway: %s at %s | merchant %s | invoice ttl %s min | reconcile window %s h\n' \
  "$GATEWAY_MODE" "$GATEWAY_URL" "$MERCHANT_ID" "$TTL_MINUTES" "$RECONCILE_HOURS"

# --- the stub gateway ------------------------------------------------------------------------
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

say "the stub QRIS gateway"
STUB_LOG="${STUB_LOG:-var/stub-phase13-requests.jsonl}"
mkdir -p "$(dirname "$STUB_LOG")"
rm -f "$STUB_LOG"
if nc -z 127.0.0.1 "$GATEWAY_PORT" 2>/dev/null; then
  printf 'a listener is already on %s; using it and starting none (its own log, if any, is not this run'"'"'s)\n' \
    "$GATEWAY_PORT"
else
  python3 scripts/stub-qris.py --port "$GATEWAY_PORT" --log "$STUB_LOG" \
    > "$TMP/stub.log" 2>&1 &
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

# --- the gateway, and the two ways this run makes it answer ---------------------------------
#
# The bridge never pushes, so every outcome asserted below is one this run scripted and then
# caused the server to *read*: `script_status` is the only way a payment becomes paid.
gw_requests() { curl -fsS "$GATEWAY_URL/_control/requests"; }

gw_asked() { # $1 = gateway payment id -> how many times the gateway was asked about it
  gw_requests | python3 -c "
import json, sys
print(sum(1 for r in json.load(sys.stdin)['requests'] if r.get('path') == '/payment/' + sys.argv[1]))" "$1"
}

script_status() { # $1 = gateway payment id, $2 = the status word, $3 = extra JSON fields ("" for none)
  python3 - "$TMP/status.json" "$1" "$2" "$3" <<'PY'
import json, sys
out, payment_id, status, extra = sys.argv[1:5]
body = {"payment_id": payment_id, "status": status}
body.update(json.loads(extra) if extra else {})
json.dump(body, open(out, "w"))
PY
  curl -fsS -X POST "$GATEWAY_URL/_control/status" -H 'content-type: application/json' \
    --data-binary "@$TMP/status.json" > /dev/null
}

script_paid() { # $1 = gateway payment id, $2 = the settlement instant, $3 = the amount that landed ("" = the invoice's own)
  local extra
  if [ -n "$3" ]; then
    extra="{\"paid_at\":\"$2\",\"uniqueAmount\":$3}"
  else
    extra="{\"paid_at\":\"$2\"}"
  fi
  script_status "$1" "paid" "$extra"
}

# The audit row the server keys an applied answer on: `gateway:<id>:<paid_at>`, with the
# settlement instant as chrono's `to_rfc3339` writes it (`+00:00`, not `Z`).
event_id() { # $1 = gateway payment id, $2 = the settlement instant
  python3 -c "
import datetime, sys
instant = datetime.datetime.fromisoformat(sys.argv[2].replace('Z', '+00:00'))
print(f'gateway:{sys.argv[1]}:{instant.isoformat()}')" "$1" "$2"
}

epoch() { python3 -c "
import datetime, sys
print(int(datetime.datetime.fromisoformat(sys.argv[1].replace('Z', '+00:00')).timestamp()))" "$1"; }

# **Second precision, deliberately**: the audit id is the settlement instant as the server
# serialises it, and a fractional second would be written with a different number of digits by
# Python and by chrono — making the id this script computes a different string from the id the
# server writes, for a reason that has nothing to do with the money path.
now_rfc3339() { python3 -c "
import datetime
print(datetime.datetime.now(datetime.UTC).replace(microsecond=0).isoformat().replace('+00:00','Z'))"; }

checkout() { # $1 = token, $2 = plan, $3 = period, $4 = output file
  curl -sS -o "$4" -w '%{http_code}' -X POST "$BASE/billing/checkout" \
    -H "authorization: Bearer $1" -H 'content-type: application/json' \
    -d "{\"plan_id\":\"$2\",\"billing_period\":\"$3\"}"
}

poll_payment() { # $1 = payment id, $2 = token, $3 = output file
  curl -sS -o "$3" -w '%{http_code}' "$BASE/billing/payments/$1" -H "authorization: Bearer $2"
}

# The gateway's id for our payment: the bridge mints its own, our order id travels as
# `reference`, and every status question is asked with the bridge's.
gw_payment_id() { db "select provider_payment_id from payments where id = '$1'"; }

# ==========================================================================================
# 1. A checkout
# ==========================================================================================
say "1. a checkout: a pending row, the config's price, the gateway's payable, a QR string, a $TTL_MINUTES-minute expiry"
CHECKOUT="$(signup checkout)"
CHECKOUT_ID="$(uid checkout)"
CHECKOUT_EMAIL="$(email_of checkout)"
STATUS="$(checkout "$CHECKOUT" pro monthly "$TMP/payment.json")"
[ "$STATUS" = "200" ] || fail "the checkout answered $STATUS: $(cat "$TMP/payment.json")"

PAYMENT_ID="$(field '["id"]' < "$TMP/payment.json")"
ORDER_ID="$(field '["order_id"]' < "$TMP/payment.json")"
AMOUNT="$(field '["amount_idr"]' < "$TMP/payment.json")"
PAYABLE="$(field '["payable_amount_idr"]' < "$TMP/payment.json")"
QR="$(field '["qr_string"]' < "$TMP/payment.json")"
EXPIRES="$(field '["expires_at"]' < "$TMP/payment.json")"
P_STATUS="$(field '["status"]' < "$TMP/payment.json")"
OFFSET=$((PAYABLE - AMOUNT))
printf 'checkout: status=%s amount_idr=%s payable_amount_idr=%s (the gateway offset is %s) expires_at=%s order_id=%s\n' \
  "$P_STATUS" "$AMOUNT" "$PAYABLE" "$OFFSET" "$EXPIRES" "$ORDER_ID"

[ "$P_STATUS" = "pending" ] || fail "a fresh checkout is pending, got $P_STATUS"
[ "$AMOUNT" = "$PRO_MONTHLY" ] || fail "the amount is $AMOUNT; $PLANS says pro monthly is $PRO_MONTHLY"
# **The payable is the gateway's**, and it is the assertion this phase exists for: it is the
# price plus an offset in 1..999. A payable equal to the price means either the gateway
# assigned no offset or the server dropped it — and the second would make every real
# settlement a mismatch.
[ "$OFFSET" -ge 1 ] && [ "$OFFSET" -le 999 ] \
  || fail "the payable is $PAYABLE against a price of $AMOUNT: the offset must be in 1..999"
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
# The stored row is what the settlement is checked against, so the response and the row are
# compared — including the payable, which is the figure the QR encodes.
STORED="$(db "select status || '|' || amount_idr || '|' || coalesce(payable_amount_idr, 0) || '|' || coalesce(qr_string,'') from payments where id = '$PAYMENT_ID'")"
[ "$STORED" = "pending|$AMOUNT|$PAYABLE|$QR" ] || fail "the stored row disagrees with the response: $STORED"

# **What we sent.** The stub derives the invoice from the request, so a QR string that is the
# gateway's own derivation is proof the invoice request carried this order id, this price and
# this lifetime — and that the payable it assigned is the number the code encodes.
GW_ID="$(gw_payment_id "$PAYMENT_ID")"
[ -n "$GW_ID" ] || fail "the payment row carries no gateway id: no invoice was minted"
printf 'the gateway minted %s for this checkout\n' "$GW_ID"
[ "$QR" = "STUBQRIS:$ORDER_ID:$PAYABLE" ] \
  || fail "the QR string is $QR, not the stub's derivation from this request"
python3 - "$(gw_requests)" "$ORDER_ID" "$AMOUNT" "$TTL_MINUTES" <<'PY'
import json, sys
requests = json.loads(sys.argv[1])["requests"]
order_id, amount, minutes = sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
sent = [r for r in requests if r["method"] == "POST" and r["path"] == "/payments"]
assert sent, f"the gateway received no invoice request: {requests}"
last = sent[-1]
assert last["body"] == {"amount": amount, "reference": order_id, "minutes": minutes}, last
print(f"the gateway received POST /payments {json.dumps(last['body'])}")
PY
printf 'the gateway received the invoice request, and the QR string is its own derivation of the payable\n'

# ==========================================================================================
# 2. A status word the bridge never sends
# ==========================================================================================
#
# The stub echoes a scripted status verbatim instead of inventing one, so this is the server's
# own `Invalid` — the failure a typo in the run should produce — and not a stub that silently
# answers something the server accepts. The poll route swallows a bridge failure into "the row
# stays pending" (a bridge that is down must not turn the checkout screen into an error page),
# so the row not moving is what a refused answer looks like from here.
say "2. a status word the bridge never sends: the server refuses it, and the row does not move"
script_status "$GW_ID" "settled" ""
STATUS="$(poll_payment "$PAYMENT_ID" "$CHECKOUT" "$TMP/poll-bogus.json")"
[ "$STATUS" = "200" ] || fail "the poll answered $STATUS: $(cat "$TMP/poll-bogus.json")"
BOGUS="$(field '["status"]' < "$TMP/poll-bogus.json")"
STORED_BOGUS="$(db "select status from payments where id = '$PAYMENT_ID'")"
printf 'the gateway answering "settled": the poll reads %s, the row is %s\n' "$BOGUS" "$STORED_BOGUS"
[ "$BOGUS" = "pending" ] || fail "an answer the server cannot read must not move the row, it reads $BOGUS"
[ "$STORED_BOGUS" = "pending" ] || fail "an unreadable answer wrote $STORED_BOGUS to the row"

# ==========================================================================================
# 3. The client's poll activates the payment
# ==========================================================================================
say "3. the client's poll activates the payment: one subscription, one grant, one audit row"
SUBS_BEFORE="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_BEFORE="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
[ "$SUBS_BEFORE" = "0" ] || fail "a fresh account already has $SUBS_BEFORE subscription(s)"

PAID_AT="$(now_rfc3339)"
script_paid "$GW_ID" "$PAID_AT" ""
STATUS="$(poll_payment "$PAYMENT_ID" "$CHECKOUT" "$TMP/poll-paid.json")"
[ "$STATUS" = "200" ] || fail "the poll answered $STATUS: $(cat "$TMP/poll-paid.json")"
PAID_STATUS="$(field '["status"]' < "$TMP/poll-paid.json")"
printf 'the client polled; the gateway reported paid at %s, and the response reads %s\n' \
  "$PAID_AT" "$PAID_STATUS"
[ "$PAID_STATUS" = "paid" ] || fail "the client's own poll must apply the paid answer, it reads $PAID_STATUS"

# The settlement instant on the row is the gateway's, not ours: `paid_at` is what the period
# rules and the margin report are computed from.
STORED_PAID_AT="$(db "select extract(epoch from paid_at)::bigint from payments where id = '$PAYMENT_ID'")"
[ "$STORED_PAID_AT" = "$(epoch "$PAID_AT")" ] \
  || fail "the row's paid_at is not the settlement instant the gateway reported: $STORED_PAID_AT vs $(epoch "$PAID_AT")"
SUBS_AFTER="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID' and status = 'active'")"
LEDGER_AFTER="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
GRANTED="$(db "select coalesce(sum(delta),0) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
PERIOD_END="$(db "select current_period_end from subscriptions where user_id = '$CHECKOUT_ID' and status = 'active'")"
GATEWAY_EVENTS="$(db "select count(*) from gateway_events where event_id = '$(event_id "$GW_ID" "$PAID_AT")'")"
printf 'applied: subscriptions %s -> %s | sub: grants %s -> %s (delta %s) | gateway_events %s | period_end %s\n' \
  "$SUBS_BEFORE" "$SUBS_AFTER" "$LEDGER_BEFORE" "$LEDGER_AFTER" "$GRANTED" "$GATEWAY_EVENTS" "$PERIOD_END"
[ "$SUBS_AFTER" = "1" ] || fail "the poll wrote $SUBS_AFTER active subscriptions, wanted 1"
[ "$LEDGER_AFTER" = "1" ] || fail "the poll wrote $LEDGER_AFTER subscription grants, wanted 1"
[ "$GRANTED" = "$PRO_CREDITS" ] || fail "the grant is $GRANTED credits; the matrix says $PRO_CREDITS"
# The audit row is keyed on the settlement, so the same answer read twice collides on this key
# (A293) — which is what section 4 turns on.
[ "$GATEWAY_EVENTS" = "1" ] || fail "the activation wrote $GATEWAY_EVENTS audit rows, wanted 1"
python3 - "$PAID_AT" "$PERIOD_END" <<'PY'
import datetime, sys
paid = datetime.datetime.fromisoformat(sys.argv[1].replace("Z", "+00:00"))
end = datetime.datetime.fromisoformat(sys.argv[2].replace("Z", "+00:00"))
days = (end - paid).total_seconds() / 86400
assert 29.9 < days < 30.1, f"a monthly period is 30 days, this one is {days:.2f}"
print(f"the period is {days:.2f} days: a fresh 30-day period from paid_at (A296)")
PY

# The plan, on the device that did not pay. The plan is not a JWT claim (A22): entitlements
# are loaded per request, so a token minted after the payment sees it.
SECOND="$(login_second_device "$CHECKOUT_EMAIL" "web")"
curl -fsS "$BASE/usage" -H "authorization: Bearer $SECOND" > "$TMP/usage.json"
PLAN="$(field '["plan"]' < "$TMP/usage.json")"
SUB_STATUS="$(field '["subscription"]["status"]' < "$TMP/usage.json")"
printf 'the second device: plan=%s subscription=%s period_end=%s\n' \
  "$PLAN" "$SUB_STATUS" "$(field '["subscription"]["period_end"]' < "$TMP/usage.json")"
[ "$PLAN" = "pro" ] || fail "the second device sees $PLAN, not the paid plan"
[ "$SUB_STATUS" = "active" ] || fail "the second device sees subscription $SUB_STATUS"

# ==========================================================================================
# 4. The replay
# ==========================================================================================
#
# **Defence one**, `gateway_events`' `unique (provider, event_id)` with `on conflict do
# nothing`: the id is `gateway:<id>:<paid_at>`, so the *same* answer read twice — the client's
# poll and the five-minute sweep can both be in flight — collides and the second read applies
# nothing. This section drives exactly that: the same settlement instant, scripted again, and
# the poll run again.
#
# **Defence two, the guarded `status = 'pending'` transition, has no subject in this run.**
# The callback ingress that used to deliver a *second answer* for one payment is gone (the
# bridge never pushes), and both readers left select pending rows *before* they ask the
# gateway — `poll::refresh_pending` returns without a call once the row is not `pending`, and
# `poll::poll_pending`'s query is `where status = 'pending'`. So a payment that is already
# `paid` is never asked about again, and the transition can only fire for two readers genuinely
# in flight with two different answers — a race a scripted run cannot force deterministically.
# `crates/server/tests/payment_jobs.rs::a_replayed_answer_activates_once` is where that outcome
# is asserted. What this run asserts instead is the stronger, observable half: the replay is
# not merely harmless, it **never reaches the gateway** (the `asked` count below).
say "4. the replay: the same answer again, and nothing moves"
PAID_BEFORE="$(db "select paid_at from payments where id = '$PAYMENT_ID'")"
ASKED_REPLAY_BEFORE="$(gw_asked "$GW_ID")"
script_paid "$GW_ID" "$PAID_AT" ""
STATUS="$(poll_payment "$PAYMENT_ID" "$CHECKOUT" "$TMP/poll-again.json")"
[ "$STATUS" = "200" ] || fail "the replay's poll answered $STATUS: $(cat "$TMP/poll-again.json")"
SUBS_NOW="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_NOW="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID' and period_key like 'sub:%'")"
PAID_AFTER="$(db "select paid_at from payments where id = '$PAYMENT_ID'")"
EVENTS_NOW="$(db "select count(*) from gateway_events where event_id = '$(event_id "$GW_ID" "$PAID_AT")'")"
ASKED_REPLAY_AFTER="$(gw_asked "$GW_ID")"
printf 'the same answer again: subscriptions=%s grants=%s paid_at unchanged=%s audit rows for it=%s asked %s -> %s\n' \
  "$SUBS_NOW" "$LEDGER_NOW" "$([ "$PAID_BEFORE" = "$PAID_AFTER" ] && echo yes || echo no)" \
  "$EVENTS_NOW" "$ASKED_REPLAY_BEFORE" "$ASKED_REPLAY_AFTER"
[ "$SUBS_NOW" = "1" ] || fail "a replay wrote a second subscription: $SUBS_NOW"
[ "$LEDGER_NOW" = "1" ] || fail "a replay wrote a second grant: $LEDGER_NOW"
[ "$PAID_BEFORE" = "$PAID_AFTER" ] || fail "a replay moved paid_at"
[ "$EVENTS_NOW" = "1" ] || fail "a replay wrote a second audit row for the same answer: $EVENTS_NOW"
[ "$ASKED_REPLAY_AFTER" = "$ASKED_REPLAY_BEFORE" ] \
  || fail "the poll asked the gateway about a payment that is no longer pending"

# ==========================================================================================
# 5. A settlement for the bare price
# ==========================================================================================
#
# The gateway's answer says what landed, and the server checks it against
# `payments.payable_amount_idr` — not against the price. A buyer who transfers the price
# instead of the payable is the case the two numbers exist for: the payment is `paid` (the
# money moved) and `review` (a human decides), with both figures written down.
say "5. a settlement for the bare price: review, with both numbers written down"
MISMATCH="$(signup mismatch)"
MISMATCH_ID="$(uid mismatch)"
STATUS="$(checkout "$MISMATCH" pro monthly "$TMP/payment-2.json")"
[ "$STATUS" = "200" ] || fail "the second checkout answered $STATUS: $(cat "$TMP/payment-2.json")"
PAYMENT_2="$(field '["id"]' < "$TMP/payment-2.json")"
AMOUNT_2="$(field '["amount_idr"]' < "$TMP/payment-2.json")"
PAYABLE_2="$(field '["payable_amount_idr"]' < "$TMP/payment-2.json")"
GW_ID_2="$(gw_payment_id "$PAYMENT_2")"
[ "$AMOUNT_2" = "$PRO_MONTHLY" ] || fail "the second checkout's amount is $AMOUNT_2, wanted $PRO_MONTHLY"
[ "$PAYABLE_2" -gt "$AMOUNT_2" ] || fail "the second checkout's payable is not above its price: $PAYABLE_2"
script_paid "$GW_ID_2" "$(now_rfc3339)" "$AMOUNT_2"
STATUS="$(poll_payment "$PAYMENT_2" "$MISMATCH" "$TMP/poll-mismatch.json")"
[ "$STATUS" = "200" ] || fail "the mismatched poll answered $STATUS: $(cat "$TMP/poll-mismatch.json")"
M_STATUS="$(field '["status"]' < "$TMP/poll-mismatch.json")"
printf 'a %s-IDR settlement against a %s-IDR invoice: the response reads %s\n' \
  "$AMOUNT_2" "$PAYABLE_2" "$M_STATUS"
[ "$M_STATUS" = "review" ] || fail "a settlement for the bare price must be review, got $M_STATUS"
REVIEW="$(db "select status || ' :: ' || coalesce(review_reason,'') from payments where id = '$PAYMENT_2'")"
printf 'the row: %s\n' "$REVIEW"
case "$REVIEW" in review*) ;; *) fail "the payment did not land in review: $REVIEW" ;; esac
python3 - "$REVIEW" "$AMOUNT_2" "$PAYABLE_2" <<'PY'
import sys
row, amount, payable = sys.argv[1], sys.argv[2], sys.argv[3]
assert f"{amount} IDR" in row, f"the settlement's own number must be written down: {row}"
assert f"{payable} IDR" in row, f"the payable it was checked against must be written down: {row}"
print("both numbers are in the reason, so a human can see which two figures disagree")
PY
[ "$(db "select count(*) from subscriptions where user_id = '$MISMATCH_ID'")" = "0" ] \
  || fail "a mismatched amount activated a subscription"
[ "$(db "select count(*) from credit_ledger where user_id = '$MISMATCH_ID' and period_key like 'sub:%'")" = "0" ] \
  || fail "a mismatched amount granted credits"

# ==========================================================================================
# 6. An expired invoice
# ==========================================================================================
#
# The poll route re-checks expiry itself and **persists** it, so a student whose clock is wrong
# cannot extend their own invoice by not noticing: a pending row past its expiry reads
# `expired` and the row follows. The run drives the expiry by moving the row's `expires_at`
# into the past, which is what a student who waited too long leaves behind.
#
# Not asserted here any more: **a payment made after the code died landing in `review`**. The
# callback ingress that used to deliver it is gone (the bridge never pushes), the poll route
# terminalises a row past its expiry *before* it asks the gateway, and the sweep's window is
# `expires_at > now()` — so the only answer that reaches that rule is one whose settlement
# instant is beyond an expiry that has not passed yet, which no honest scripted run produces.
# The rule is covered by crates/server/tests/payment_jobs.rs.
say "6. an expired invoice: the poll route reads and persists expired"
EXPIRED="$(signup expired)"
STATUS="$(checkout "$EXPIRED" pro yearly "$TMP/payment-3.json")"
[ "$STATUS" = "200" ] || fail "the third checkout answered $STATUS: $(cat "$TMP/payment-3.json")"
PAYMENT_3="$(field '["id"]' < "$TMP/payment-3.json")"
[ "$(field '["amount_idr"]' < "$TMP/payment-3.json")" = "$PRO_YEARLY" ] \
  || fail "the yearly amount is not the config's $PRO_YEARLY"

db "update payments set expires_at = now() - interval '1 minute' where id = '$PAYMENT_3'" > /dev/null
STATUS="$(poll_payment "$PAYMENT_3" "$EXPIRED" "$TMP/poll-expired.json")"
[ "$STATUS" = "200" ] || fail "the expired poll answered $STATUS: $(cat "$TMP/poll-expired.json")"
READ_BACK="$(field '["status"]' < "$TMP/poll-expired.json")"
STORED_3="$(db "select status from payments where id = '$PAYMENT_3'")"
printf 'the poll route on a pending-but-expired invoice: reads %s, the row is %s\n' "$READ_BACK" "$STORED_3"
[ "$READ_BACK" = "expired" ] || fail "an expired invoice must read expired, got $READ_BACK"
[ "$STORED_3" = "expired" ] || fail "the poll must persist the expiry, the row is $STORED_3"

# ==========================================================================================
# 7. The poller
# ==========================================================================================
#
# Two halves, and the first is the one that makes the second mean something.
#
#   (a) **After the client's poll, the poller changes nothing — structurally.** The proof is
#       not that the numbers are unchanged but that the gateway is **never asked**: the pass
#       selects `status = 'pending'` rows, and a paid row is not one. Asserting only the
#       numbers would pass against a poller that asked and then did nothing.
#   (b) **A pending payment the gateway reports paid is activated**, by the same `apply_paid`
#       the client's poll calls, and a second pass over the now-paid row changes nothing.
#       Without this half, (a) is what a poller that never ran would also report.
say "7. the poller: nothing for a payment the client's poll already applied (never asked), everything for one it has not"

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

SUBS_SNAPSHOT="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_SNAPSHOT="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID'")"
ASKED_BEFORE="$(gw_asked "$GW_ID")"
run_job poll_payment '{}'
ASKED_AFTER="$(gw_asked "$GW_ID")"
SUBS_NOW="$(db "select count(*) from subscriptions where user_id = '$CHECKOUT_ID'")"
LEDGER_NOW="$(db "select count(*) from credit_ledger where user_id = '$CHECKOUT_ID'")"
printf '  the paid payment: asked %s -> %s time(s) | subscriptions %s -> %s | ledger rows %s -> %s\n' \
  "$ASKED_BEFORE" "$ASKED_AFTER" "$SUBS_SNAPSHOT" "$SUBS_NOW" "$LEDGER_SNAPSHOT" "$LEDGER_NOW"
[ "$ASKED_AFTER" = "$ASKED_BEFORE" ] \
  || fail "the poller asked the gateway about a payment that is no longer pending"
[ "$SUBS_NOW" = "$SUBS_SNAPSHOT" ] || fail "the poller changed the subscription count"
[ "$LEDGER_NOW" = "$LEDGER_SNAPSHOT" ] || fail "the poller wrote ledger rows"

POLLED="$(signup polled)"
POLLED_ID="$(uid polled)"
STATUS="$(checkout "$POLLED" pro monthly "$TMP/payment-4.json")"
[ "$STATUS" = "200" ] || fail "the fourth checkout answered $STATUS: $(cat "$TMP/payment-4.json")"
PAYMENT_4="$(field '["id"]' < "$TMP/payment-4.json")"
GW_ID_4="$(gw_payment_id "$PAYMENT_4")"
PAID_AT_4="$(now_rfc3339)"
script_paid "$GW_ID_4" "$PAID_AT_4" ""
run_job poll_payment '{}'
POLLED_STATUS="$(db "select status from payments where id = '$PAYMENT_4'")"
POLLED_SUBS="$(db "select count(*) from subscriptions where user_id = '$POLLED_ID' and status = 'active'")"
POLLED_GRANTS="$(db "select count(*) from credit_ledger where user_id = '$POLLED_ID' and period_key like 'sub:%'")"
printf '  a paid answer the client never polled: payment=%s subscriptions=%s grants=%s\n' \
  "$POLLED_STATUS" "$POLLED_SUBS" "$POLLED_GRANTS"
[ "$POLLED_STATUS" = "paid" ] || fail "the poller did not apply a paid answer: $POLLED_STATUS"
[ "$POLLED_SUBS" = "1" ] || fail "the poller wrote $POLLED_SUBS subscriptions, wanted 1"
[ "$POLLED_GRANTS" = "1" ] || fail "the poller wrote $POLLED_GRANTS grants, wanted 1"

# And a second pass over the now-paid row changes nothing: the same idempotency the client's
# poll has, because it is the same function.
run_job poll_payment '{}'
POLLED_SUBS_2="$(db "select count(*) from subscriptions where user_id = '$POLLED_ID' and status = 'active'")"
POLLED_GRANTS_2="$(db "select count(*) from credit_ledger where user_id = '$POLLED_ID' and period_key like 'sub:%'")"
printf '  the same poll a second time: subscriptions %s -> %s | grants %s -> %s\n' \
  "$POLLED_SUBS" "$POLLED_SUBS_2" "$POLLED_GRANTS" "$POLLED_GRANTS_2"
[ "$POLLED_SUBS_2" = "$POLLED_SUBS" ] || fail "a second poll wrote a second subscription"
[ "$POLLED_GRANTS_2" = "$POLLED_GRANTS" ] || fail "a second poll wrote a second grant"

# ==========================================================================================
# 8. The reconciliation sweep
# ==========================================================================================
#
# **The join is on the amount.** The bridge's feed carries the *provider's* order id and never
# sends our `reference` back, so the only field that identifies one of our payments is the
# figure that landed — the same fact the bridge matches its own settlements by, and the reason
# `payments.payable_amount_idr` is stored beside the price.
#
# Three rows, and each says something different about the join:
#   * the paid payment, whose payable the feed carries -> left alone;
#   * a pending payment whose payable the feed also carries -> left alone, which is the join
#     *finding* an amount rather than failing to;
#   * a pending payment the feed says nothing about -> `review`, with its amount in the reason.
say "8. the reconciliation sweep: the feed's join is on the amount"
SWEPT="$(signup swept)"
STATUS="$(checkout "$SWEPT" pro monthly "$TMP/payment-5.json")"
[ "$STATUS" = "200" ] || fail "the fifth checkout answered $STATUS: $(cat "$TMP/payment-5.json")"
ROW_A="$(field '["id"]' < "$TMP/payment-5.json")"
PAYABLE_A="$(field '["payable_amount_idr"]' < "$TMP/payment-5.json")"
STATUS="$(checkout "$SWEPT" pro yearly "$TMP/payment-6.json")"
[ "$STATUS" = "200" ] || fail "the sixth checkout answered $STATUS: $(cat "$TMP/payment-6.json")"
ROW_B="$(field '["id"]' < "$TMP/payment-6.json")"
PAYABLE_B="$(field '["payable_amount_idr"]' < "$TMP/payment-6.json")"

# The sweep only considers rows older than ten minutes (a pending invoice created seconds ago
# is not a mismatch), so the rows this scenario is about are aged past that floor.
db "update payments set created_at = now() - interval '15 minutes' where id in ('$PAYMENT_ID', '$ROW_A', '$ROW_B')" > /dev/null

# The feed, from the stub's own ledger (the paid payments it minted) plus two settlements it did
# not mint: an amount the server holds a row for, and one no row of ours carries.
UNKNOWN=999999
python3 - "$TMP/extra.json" "$PAYABLE_A" "$UNKNOWN" <<'PY'
import datetime, json, sys
out, payable, unknown = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
instant = datetime.datetime.now(datetime.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")
json.dump(
    {
        "transactions": [
            {"id": "tx-extra-1", "orderId": "gopay-extra-1", "status": "settlement",
             "paymentType": "qris", "amount": payable, "realAmount": payable,
             "currency": "IDR", "time": instant},
            {"id": "tx-extra-2", "orderId": "gopay-extra-2", "status": "settlement",
             "paymentType": "qris", "amount": unknown, "realAmount": unknown,
             "currency": "IDR", "time": instant},
        ]
    },
    open(out, "w"),
)
PY
curl -fsS -X POST "$GATEWAY_URL/_control/transactions" -H 'content-type: application/json' \
  --data-binary "@$TMP/extra.json" > /dev/null

run_job reconcile_payments '{}'

python3 - "$(gw_requests)" "$MERCHANT_ID" "$((RECONCILE_HOURS * 60))" <<'PY'
import json, sys
requests = json.loads(sys.argv[1])["requests"]
merchant, minutes = sys.argv[2], sys.argv[3]
feeds = [r for r in requests if r["method"] == "GET" and r["path"] == "/transactions"]
assert feeds, f"the sweep never read the feed: {requests}"
last = feeds[-1]
assert last["query"].get("merchant_id") == merchant, last
assert last["query"].get("minutes") == minutes, last
print(f"the sweep read GET /transactions?merchant_id={merchant}&minutes={minutes}")
PY

PAID_ROW="$(db "select status || '|' || coalesce(review_reason,'') from payments where id = '$PAYMENT_ID'")"
ROW_A_STATE="$(db "select status || '|' || coalesce(review_reason,'') from payments where id = '$ROW_A'")"
ROW_B_STATE="$(db "select status || '|' || coalesce(review_reason,'') from payments where id = '$ROW_B'")"
printf '  the paid payment, whose payable the feed carries: %s\n' "$PAID_ROW"
printf '  a pending row the feed carries the amount of:       %s\n' "$ROW_A_STATE"
printf '  a pending row the feed says nothing about:          %s\n' "$ROW_B_STATE"
[ "$PAID_ROW" = "paid|" ] \
  || fail "the sweep flagged a paid payment whose amount the feed carries: $PAID_ROW"
[ "$ROW_A_STATE" = "pending|" ] \
  || fail "a feed entry for this row's payable must leave the row alone: $ROW_A_STATE"
case "$ROW_B_STATE" in
  "review|the gateway has no transaction for $PAYABLE_B IDR") ;;
  *) fail "a row the feed has no amount for must be flagged with that amount: $ROW_B_STATE" ;;
esac
[ "$(db "select count(*) from payments where coalesce(payable_amount_idr, amount_idr) = $UNKNOWN")" = "0" ] \
  || fail "the sweep wrote a row for money it cannot attribute"

# **The one half of this sweep the database does not carry.** Money the gateway took with no
# row of ours is counted and written to `tracing::error!` by `billing::poll::reconcile` — there
# is no row to flag, so the job's `flagged` count is a log line and nothing else. This run
# cannot read the server's log, so what it asserts is the feed the sweep read (above), that
# such a settlement leaves no row behind, and that the join by amount decided every row it
# could. That is the honest limit of a black-box run against a live process.
printf '  the %s-IDR settlement no row of ours carries: counted and logged, and no row written\n' "$UNKNOWN"

# ==========================================================================================
# 9. The margin report's split
# ==========================================================================================
#
# The admin flag is set in the database because the smoke run has no admin console: the route
# takes `AdminUser`, which reads `users.is_admin` per request, so the flag is what an operator
# would set once. The account is a **customer** in the report, and the report excludes admins
# from both sides — so the account that reads it is the one that pays, and the numbers below
# are its own.
say "9. the margin report: the free/paid split"
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
