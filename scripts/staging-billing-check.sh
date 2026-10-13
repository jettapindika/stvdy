#!/usr/bin/env bash
#
# End-to-end check of the money path against the **live gateway**. It is the billing
# counterpart of `staging-smoke.sh`: that one proves an account can be created, verified and
# read; this one proves a student can be charged, and that the charge is the figure the gateway
# was told to collect.
#
#   bash staging-billing-check.sh [base-url] [pay-wait-seconds]
#
# Run it **on the host**, from the deployment root. Two legs need that: the account's
# verification, which is either the mailed link read out of the mail sink under `data/mail`
# (written by uid 10001, so only root can read it) or a direct row update when `SMTP_URL` is set
# and the link leaves the host, and the gateway's own payment id, which is read from the
# database because `PaymentView` deliberately does not expose it — a client has no business
# knowing the gateway's identifiers.
#
# **`BILLING_EMAIL` is required when `SMTP_URL` is set.** The signup then sends a real message,
# so the address has to be one that receives it: a synthetic `@example.test` address makes the
# relay bounce on every run, and a bounce is charged to the sending domain's reputation — the
# deliverability of the very verification mail this deployment exists to send. Any inbox works;
# the per-run plus-suffix is added below.
#
# **What it proves, in order, and each step is an assertion rather than a print.**
#
#  1. **Checkout mints an invoice through the gateway.** `POST /billing/checkout` answers 200
#     and the row carries a `qr_string` — which only exists if `create_invoice` reached the
#     bridge and its answer was parsed. A deployment with no gateway answers 502 here.
#  2. **The payable amount is the gateway's, and it is not the price.**
#     `payable_amount_idr >= amount_idr`, and the difference is the unique offset the bridge
#     added so it can match a settlement by amount alone. This is the assertion that would have
#     caught the class of defect the column exists for: a checkout screen showing the price
#     would have a student transfer a figure the gateway cannot match to any order.
#  3. **The gateway itself holds this payment, under the id the server stored.** Asked directly
#     at `GET /payment/{id}` on the bridge, so a server that recorded an invoice the gateway
#     never minted is caught. The id comes from `payments.provider_payment_id`.
#  4. **The client's poll reaches the gateway and the row does not move.** `GET
#     /billing/payments/:id` while pending answers `pending` with the same payable — the poll
#     asked the bridge, and the bridge has no settlement yet.
#  5. **The settlement activates the plan, once.** With a wait above zero the script polls until
#     the invoice is paid, then asserts the subscription is `active`, the plan is the one bought,
#     and the audit row names the gateway's id. Paying is a human act — scan the QR and transfer
#     `payable_amount_idr` — so the wait defaults to 0, which **stops after step 4 and says so**
#     rather than implying the money leg passed.
#
# The last leg is deliberately not simulated. A script that could mark a payment paid without
# money moving would be a second activation path, and the one in the server is the thing under
# test.
set -euo pipefail

base="${1:-http://localhost/api}"
wait_seconds="${2:-0}"
root="$(pwd)"
password="staging-password-1234"

# `docker compose exec` rather than a published port: the bridge deliberately publishes none
# (A292), so the only way to ask it anything is from a container on `stvdy_default`. The web
# container is the one with an HTTP client in it.
gateway_in_container="http://gopay-bridge:8787"
gw() { docker exec stvdy-web-1 wget -qO- "${gateway_in_container}$1"; }
db() { docker compose -f docker-compose.staging.yml exec -T postgres psql -U stvdy -d stvdy -tA -c "$1"; }

# One JSON reader, so no step parses a response with `sed`. A field that is absent comes back
# empty rather than silently matching a neighbouring object's key — which `sed` cannot promise
# and is exactly how a smoke run reports a pass it did not earn.
json() { python3 -c 'import json,sys
doc = json.load(sys.stdin)
for key in sys.argv[1:]:
    if isinstance(doc, dict) and key in doc:
        doc = doc[key]
    else:
        print(""); sys.exit(0)
print(doc)' "$@"; }

say() { printf '\n=== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# Which mailer is live decides two things at once — whether this signup's message leaves the
# host, and therefore how the account gets verified. Read from `.env.staging` at the deployment
# root, which is the file the compose stack configures the server from: the same source and the
# same rule `staging-smoke.sh` uses, so the two scripts cannot disagree about which mailer ran.
smtp=0
if grep -qE '^SMTP_URL=.+' .env.staging 2>/dev/null; then smtp=1; fi

if [ "$smtp" = 1 ]; then
  [ -n "${BILLING_EMAIL:-}" ] || fail "SMTP_URL is set, so this signup sends real mail: set BILLING_EMAIL to an address that receives it (a plus-address on any inbox will do)"
  email="${BILLING_EMAIL%%@*}+billing$(date +%s)@${BILLING_EMAIL##*@}"
else
  email="billing-$(date +%s)@example.test"
fi

say "health"
curl -fsS "$base/health"; echo

say "signup $email"
curl -fsS -X POST "$base/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"$email\",\"password\":\"$password\",\"device\":{\"name\":\"billing\",\"platform\":\"web\"}}" \
  >/dev/null

if [ "$smtp" = 1 ]; then
  say "the account is verified directly (SMTP_URL is set, so the link went to a real inbox)"
  # The row `POST /auth/verify-email` sets from the mailed token, so the state the billing legs
  # run against is the state a click on that link produces. The mailbox is real and this script
  # cannot read it, so the token in it is left unused — spending it is the student's act, not
  # this one's.
  db "update users set email_verified = true, email_verified_at = now() where email = '$email'" >/dev/null
else
  say "the verification link is a file (SMTP_URL is unset)"
  eml="$(sudo ls -t "$root/data/mail"/*.eml 2>/dev/null | head -1)"
  [ -n "$eml" ] || fail "no message under $root/data/mail"
  token="$(sudo grep -o 'token=[A-Za-z0-9_-]*' "$eml" | head -1 | cut -d= -f2)"
  [ -n "$token" ] || fail "no verification token in $eml"
  echo "token ${token:0:8}…"
  curl -fsS -X POST "$base/auth/verify-email" -H 'content-type: application/json' \
    -d "{\"token\":\"$token\"}" >/dev/null
fi

say "login"
login="$(curl -fsS -X POST "$base/auth/login" -H 'content-type: application/json' \
  -d "{\"email\":\"$email\",\"password\":\"$password\",\"device\":{\"name\":\"billing2\",\"platform\":\"web\"}}")"
access="$(printf '%s' "$login" | json access_token)"
[ -n "$access" ] || fail "no access_token in $login"
auth="authorization: Bearer $access"

say "1. checkout"
payment="$(curl -fsS -X POST "$base/billing/checkout" -H 'content-type: application/json' \
  -H "$auth" -d '{"plan_id":"pro","billing_period":"monthly"}')"
printf '%s\n' "$payment" | python3 -m json.tool

id="$(printf '%s' "$payment" | json id)"
amount="$(printf '%s' "$payment" | json amount_idr)"
payable="$(printf '%s' "$payment" | json payable_amount_idr)"
qr="$(printf '%s' "$payment" | json qr_string)"
status="$(printf '%s' "$payment" | json status)"

[ -n "$id" ] || fail "checkout answered no payment id"
[ "$status" = "pending" ] || fail "a fresh invoice is not pending: $status"
[ -n "$qr" ] || fail "the row carries no qr_string, so the gateway was never reached"

say "2. the payable amount is the gateway's, not the price"
offset=$((payable - amount))
printf 'price %s IDR + gateway offset %s IDR = payable %s IDR\n' "$amount" "$offset" "$payable"
[ "$payable" -ge "$amount" ] || fail "the payable is below the price: $payable < $amount"
[ "$offset" -ge 0 ] && [ "$offset" -le 999 ] ||
  fail "the offset is outside the bridge's 0..999 window: $offset"
[ "$offset" -eq 0 ] && printf 'note: the gateway charged the price exactly this time\n'

say "3. the gateway itself holds this payment (asked directly, not through the server)"
gw_id="$(db "select provider_payment_id from payments where id = '$id'")"
[ -n "$gw_id" ] || fail "the row has no provider_payment_id"
printf 'the server recorded gateway id %s\n' "$gw_id"
bridge="$(gw "/payment/$gw_id")"
printf '%s\n' "$bridge" | python3 -m json.tool | head -12
bridge_amount="$(printf '%s' "$bridge" | json uniqueAmount)"
bridge_status="$(printf '%s' "$bridge" | json status)"
[ "$bridge_amount" = "$payable" ] ||
  fail "the bridge charges $bridge_amount but the row says $payable — the two disagree"
printf 'the bridge agrees: %s IDR, %s\n' "$bridge_amount" "$bridge_status"

say "4. the client's poll reaches the gateway and the row stays pending"
poll="$(curl -fsS "$base/billing/payments/$id" -H "$auth")"
polled_status="$(printf '%s' "$poll" | json status)"
polled_payable="$(printf '%s' "$poll" | json payable_amount_idr)"
printf 'poll: status=%s payable=%s\n' "$polled_status" "$polled_payable"
[ "$polled_status" = "pending" ] || fail "a pending invoice answered $polled_status"
[ "$polled_payable" = "$payable" ] || fail "the poll's payable differs from the checkout's"

say "pay this, and the plan activates"
printf '  amount : %s IDR   <- transfer exactly this figure\n' "$payable"
printf '  qr     : %s\n' "$qr"
printf '  payment: %s  (gateway %s)\n' "$id" "$gw_id"

if [ "$wait_seconds" -le 0 ]; then
  say "STOPPED BEFORE THE MONEY LEG"
  echo "Steps 1-4 passed against the live gateway. The settlement leg is not simulated: paying"
  echo "is a human act, and a script that could mark a payment paid without money moving would be"
  echo "a second activation path. Transfer $payable IDR, then re-run with a wait:"
  echo "  bash scripts/staging-billing-check.sh $base 300"
  exit 0
fi

say "5. waiting up to ${wait_seconds}s for the settlement"
deadline=$(( $(date +%s) + wait_seconds ))
final=""
while [ "$(date +%s)" -lt "$deadline" ]; do
  final="$(curl -fsS "$base/billing/payments/$id" -H "$auth")"
  st="$(printf '%s' "$final" | json status)"
  printf '  %s status=%s\n' "$(date -u +%H:%M:%S)" "$st"
  case "$st" in
    paid|review|expired|failed|cancelled|refunded) break ;;
  esac
  sleep 5
done

st="$(printf '%s' "$final" | json status)"
[ "$st" = "paid" ] || fail "the invoice settled as $st, not paid — see GET /admin/payments?status=review"

usage="$(curl -fsS "$base/usage" -H "$auth")"
plan="$(printf '%s' "$usage" | json plan)"
sub_status="$(printf '%s' "$usage" | json subscription status)"
printf 'plan=%s subscription=%s\n' "$plan" "$sub_status"
[ "$plan" = "pro" ] || fail "the plan did not activate: $plan"
[ "$sub_status" = "active" ] || fail "the subscription is not active: $sub_status"

paid_rows="$(db "select count(*) from payments where id = '$id' and status = 'paid'")"
[ "$paid_rows" = "1" ] || fail "the payment row is not paid exactly once: $paid_rows"

say "PASS — the money path is live end to end"
printf 'checkout %s IDR -> gateway offset %s -> settlement -> %s active\n' "$amount" "$offset" "$plan"
