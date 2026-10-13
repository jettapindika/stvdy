#!/usr/bin/env bash
#
# End-to-end smoke against a running server. It exercises what `/health` cannot: Postgres
# through the migrations, the argon2 signup path, the verification token's single-use
# predicate, and an authenticated read.
#
#   bash staging-smoke.sh [base-url] [mail-dir]
#
# **The mail leg follows whichever mailer is live** (`integrations/mailer.rs`, B3). The mode
# is read from `.env.staging` at the deployment root, which is the file the compose stack
# configures the server from:
#
#   * `SMTP_URL` empty. The server writes the message under `MAIL_DIR`, so this script reads
#     the link out of the newest file and posts it back. The whole flow is automatic, and the
#     signup address is synthetic because nothing is delivered anywhere.
#   * `SMTP_URL` set. The message leaves the host for a real inbox this script cannot read,
#     and the signup address has to be one that receives mail: a synthetic `@example.test`
#     address would make the relay bounce on every deploy. The account is therefore created
#     at `SMOKE_EMAIL`, with a per-run plus-address so a second deploy is not a duplicate
#     signup. The verification row is then set the way the mailed link would set it — the leg
#     this script exists to close is the authenticated read behind A359's gate, and an account
#     left unverified would 403 there and report a pass for a chain it never walked.
#
# With `SMTP_URL` set and `SMOKE_EMAIL` unset, the legs that need a mailbox are **skipped and
# said to be skipped** rather than reported as a failure — the pattern A76's smoke uses to
# state its own limits instead of implying coverage.
#
# The mail directory defaults to `data/mail` under the same root.
set -euo pipefail

# The origin nginx serves: the SPA at `/` and the API under `/api`. A raw `:8080` default would
# now find nothing — the `server` service publishes no port, so that address only ever answered
# on the instance whose firewall opened it by hand.
base="${1:-http://localhost/api}"
mail_dir="${2:-$(pwd)/data/mail}"
password="staging-password-1234"

if [ -n "${SMOKE_EMAIL:-}" ] || grep -qE '^SMTP_URL=.+' .env.staging 2>/dev/null; then
  smtp=1
  email="${SMOKE_EMAIL:-}"
  # A plus-address is appended rather than required of the caller: signup for an address that
  # already exists is a 400, and the point of this account is that it is new on every run.
  if [ -n "$email" ]; then
    email="${email%%@*}+staging$(date +%s)@${email##*@}"
  fi
else
  smtp=0
  email="smoke-$(date +%s)@example.test"
fi

echo "=== health"
curl -fsS "$base/health"; echo

if [ "$smtp" = 1 ] && [ -z "$email" ]; then
  echo "=== the mail legs are SKIPPED: SMTP_URL is set, so the verification link goes to a real"
  echo "    inbox this script cannot read, and the signup address must be one that receives mail."
  echo "    Set SMOKE_EMAIL to such an address — a plus-address on a verified destination is free"
  echo "    and needs no domain of its own — to run the signup, login and authenticated-read legs."
  echo "=== PASS (only /health was exercised)"
  exit 0
fi

echo "=== signup $email"
curl -fsS -X POST "$base/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"$email\",\"password\":\"$password\",\"device\":{\"name\":\"smoke\",\"platform\":\"web\"}}" \
  | head -c 120; echo

if [ "$smtp" = 1 ]; then
  echo "=== the verification link was mailed to $email; this script cannot read an inbox, so the"
  echo "    row is set the way that link would set it and the mailed token stays unused"
  # The same two columns `POST /auth/verify-email` writes from the token, so the state the
  # authenticated read below runs against is the state a click on that link produces. Reached
  # the way `staging-billing-check.sh` reaches it, which is the other script that needs a
  # verified account on a host whose mailer is real.
  docker compose -f docker-compose.staging.yml exec -T postgres \
    psql -U stvdy -d stvdy -tA \
    -c "update users set email_verified = true, email_verified_at = now() where email = '$email'" >/dev/null
else
  echo "=== the verification link is a file (SMTP_URL is unset)"
  # The message is written by the container's `stvdy` user (uid 10001), so reading it needs
  # root; `sudo` is the deployment root's own privilege, not a network one.
  eml="$(sudo ls -t "$mail_dir"/*.eml 2>/dev/null | head -1)"
  if [ -z "$eml" ]; then echo "no message under $mail_dir" >&2; exit 1; fi
  echo "$eml"
  token="$(sudo grep -o 'token=[A-Za-z0-9_-]*' "$eml" | head -1 | cut -d= -f2)"
  if [ -z "$token" ]; then echo "no verification token in $eml" >&2; exit 1; fi
  echo "token ${token:0:8}…"

  echo "=== verify-email"
  curl -fsS -X POST "$base/auth/verify-email" -H 'content-type: application/json' \
    -d "{\"token\":\"$token\"}"; echo
fi

echo "=== login"
login="$(curl -fsS -X POST "$base/auth/login" -H 'content-type: application/json' \
  -d "{\"email\":\"$email\",\"password\":\"$password\",\"device\":{\"name\":\"smoke2\",\"platform\":\"web\"}}")"
access="$(printf '%s' "$login" | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')"
if [ -z "$access" ]; then echo "no access_token in $login" >&2; exit 1; fi
echo "access_token ${access:0:16}…"

echo "=== /auth/me"
curl -fsS "$base/auth/me" -H "authorization: Bearer $access"; echo

echo "=== /workspaces"
curl -fsS "$base/workspaces" -H "authorization: Bearer $access" | head -c 200; echo

echo "=== PASS"
