#!/usr/bin/env bash
#
# Notarize and staple the macOS release: the ticket goes into **both** the `.app` and the
# `.dmg` (A333).
#
#   scripts/notarize-macos.sh <path/to/Stvdy_<version>_universal.dmg>
#   scripts/notarize-macos.sh --dry-run <path/to/...dmg>   # print the plan, touch nothing
#
# The `.dmg` Tauri's bundler produced is the argument. This script
#
#   1. mounts it and copies the volume out to a staging directory;
#   2. notarizes the `.app` inside it (`notarytool submit --wait`) and staples the ticket
#      into that app;
#   3. re-packs the dmg around the stapled app;
#   4. notarizes the dmg and staples the ticket into it.
#
# Two submissions, not one, and that is the whole reason this is a script rather than three
# workflow lines:
#
#   * `notarytool submit --wait` exits 0 for a submission that was **rejected**. The only
#     trustworthy signal is the `status` field of the JSON it prints, so a script that
#     trusted the exit code would staple nothing and call it a success. This script parses
#     `status: Accepted` and, when it is anything else, fails and prints the submission's
#     log id so the operator can run `xcrun notarytool log <id>`.
#
#   * A stapled dmg is not enough on its own. Gatekeeper checks the `.app` **inside** the
#     dmg, so a user who copies the app out of it onto a second machine and launches it with
#     no network gets "cannot be opened" unless the app itself carries the ticket. The
#     ticket is therefore stapled into the app **before the dmg is rebuilt** -- stapling the
#     app changes the dmg's bytes, so the dmg built around the stapled app is not the dmg
#     that was submitted first, and it has to be submitted and stapled in its own right.
#     That is the second submission, and it is not redundant: it is what makes
#     `xcrun stapler validate` pass on the file a user actually downloads.
#
# **Credentials.** One of two shapes, read from the environment:
#
#   APPLE_ID + APPLE_PASSWORD (an app-specific password) + APPLE_TEAM_ID
#   APPLE_API_KEY (a path to a `.p8`) + APPLE_API_KEY_ID + APPLE_API_ISSUER
#
# With neither complete the script **refuses** (exit 2) rather than submitting anonymously:
# an unauthenticated `notarytool submit` fails in a way that reads like a network problem.
#
# **It also refuses to run with `TAURI_SIGNING_PRIVATE_KEY` in its environment.** That key
# signs `latest.json` and is used in exactly one place (A332); a notarization helper that
# tolerates it is a second place, and the way a key leaks is a habit, not an attack. The
# release workflow scopes the secret to the signing step, so this refusal costs it nothing.
#
# Exit codes: 0 notarized and stapled; 1 a step failed; 2 refused to run (bad arguments, no
# credentials, or the updater signing key present).
set -euo pipefail

usage() {
  # The header comment, from line 2 to the first line that is not a comment.
  awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "$0"
}

die_usage() {
  echo "notarize-macos: $1" >&2
  echo "run with --help for the usage" >&2
  exit 2
}

refuse() {
  echo "notarize-macos: refusing to run: $1" >&2
  exit 2
}

dmg=""
dry_run=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    -*)
      die_usage "unknown argument $1"
      ;;
    *)
      [ -z "$dmg" ] || die_usage "one dmg at a time (already have $dmg)"
      dmg="$1"
      ;;
  esac
  shift
done

[ -n "$dmg" ] || die_usage "a .dmg path is required"

# ---------------------------------------------------------------------------
# The refusal, before any check and before --dry-run: no process that can staple runs with
# the updater's key in its environment (A332). --help is answered above it on purpose.
# ---------------------------------------------------------------------------
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || [ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
  echo "notarize-macos: refusing to run with the updater signing key in the environment" >&2
  echo "  set: ${TAURI_SIGNING_PRIVATE_KEY:+TAURI_SIGNING_PRIVATE_KEY}${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:+ TAURI_SIGNING_PRIVATE_KEY_PASSWORD}" >&2
  echo "  that key signs latest.json and is used in exactly one place (A332). This script" >&2
  echo "  notarizes and staples; it never signs, so the key has no business here. Unset it," >&2
  echo "  or run this in a step that does not carry the secret." >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# The credential shape. Both are accepted; the release workflow sets the app-specific
# password pair. A dry run reports which one it found and touches nothing.
# ---------------------------------------------------------------------------
api_shape=0
id_shape=0
if [ -n "${APPLE_API_KEY:-}" ] || [ -n "${APPLE_API_KEY_ID:-}" ] || [ -n "${APPLE_API_ISSUER:-}" ]; then
  api_shape=1
fi
if [ -n "${APPLE_ID:-}" ] || [ -n "${APPLE_PASSWORD:-}" ] || [ -n "${APPLE_TEAM_ID:-}" ]; then
  id_shape=1
fi

if [ "$dry_run" = "1" ]; then
  echo "notarize-macos --dry-run: nothing is mounted, submitted or stapled"
  echo "  dmg: $dmg"
  [ -e "$dmg" ] || echo "  note: $dmg does not exist"
  if [ "$api_shape" = "1" ]; then
    echo "  credentials: the App Store Connect API key shape"
  elif [ "$id_shape" = "1" ]; then
    echo "  credentials: the Apple ID shape"
  else
    echo "  credentials: none -- the real run would refuse (exit 2)"
  fi
  echo "  would: mount the dmg, notarize and staple the .app inside it, re-pack the dmg,"
  echo "         then notarize and staple the dmg"
  exit 0
fi

auth=()
if [ "$api_shape" = "1" ]; then
  missing=""
  [ -n "${APPLE_API_KEY:-}" ] || missing="$missing APPLE_API_KEY"
  [ -n "${APPLE_API_KEY_ID:-}" ] || missing="$missing APPLE_API_KEY_ID"
  [ -n "${APPLE_API_ISSUER:-}" ] || missing="$missing APPLE_API_ISSUER"
  [ -z "$missing" ] || refuse "the App Store Connect API key shape is incomplete:$missing is not set"
  auth=(--key "$APPLE_API_KEY" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER")
elif [ "$id_shape" = "1" ]; then
  missing=""
  [ -n "${APPLE_ID:-}" ] || missing="$missing APPLE_ID"
  [ -n "${APPLE_PASSWORD:-}" ] || missing="$missing APPLE_PASSWORD"
  [ -n "${APPLE_TEAM_ID:-}" ] || missing="$missing APPLE_TEAM_ID"
  [ -z "$missing" ] || refuse "the Apple ID credential shape is incomplete:$missing is not set"
  auth=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
else
  echo "notarize-macos: refusing to notarize without credentials" >&2
  echo "  set either APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID (an app-specific password)," >&2
  echo "  or APPLE_API_KEY + APPLE_API_KEY_ID + APPLE_API_ISSUER (an App Store Connect" >&2
  echo "  API key). Neither shape is set. An unauthenticated 'notarytool submit' fails in" >&2
  echo "  a way that reads like a network problem, so this script stops here (A333)." >&2
  exit 2
fi

[ -f "$dmg" ] || die_usage "no such file: $dmg"
dmg="$(cd "$(dirname "$dmg")" && pwd)/$(basename "$dmg")"

for tool in xcrun hdiutil ditto diskutil python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "notarize-macos: $tool is not installed" >&2; exit 1; }
done

work="$(mktemp -d)"
mount="$work/mnt"
mounted=0
cleanup() {
  if [ "$mounted" = "1" ]; then
    hdiutil detach "$mount" >/dev/null 2>&1 || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

failures=0

ok() {
  printf 'ok   %s\n' "$1"
}

fail() {
  failures=$((failures + 1))
  printf 'FAIL %s\n' "$1" >&2
}

die() {
  fail "$1"
  exit 1
}

# One field out of the JSON `notarytool` prints. `json.loads` first, because a single
# `--output-format json` object is the normal shape; the regex fallback covers a build that
# printed one object per poll, where the last `status` is the final one.
json_field() {
  printf '%s' "$1" | python3 -c '
import json, re, sys
field = sys.argv[1]
raw = sys.stdin.read()
try:
    value = json.loads(raw).get(field, "")
except Exception:
    found = re.findall(r"\"" + re.escape(field) + r"\"\s*:\s*\"([^\"]*)\"", raw)
    value = found[-1] if found else ""
print("" if value is None else value)
' "$2"
}

# Submit one artifact and wait. The exit code is not the answer: `--wait` returns 0 for a
# submission the notary **rejected**, so the `status` field is the answer, and a rejection
# prints the log id that names why.
submit_and_wait() {
  local artifact="$1" what="$2" output status id
  echo "# submitting $what to the notary"
  if ! output="$(xcrun notarytool submit "$artifact" --wait --output-format json "${auth[@]}")"; then
    die "notarytool submit failed for $what (see the output above)"
  fi
  status="$(json_field "$output" status)"
  id="$(json_field "$output" id)"
  if [ "$status" != "Accepted" ]; then
    fail "the notary did not accept $what (status: ${status:-<none>})"
    echo "  submission log id: ${id:-<none>}" >&2
    echo "  read why with: xcrun notarytool log ${id:-<id>} <credentials>" >&2
    exit 1
  fi
  ok "the notary accepted $what (submission $id)"
}

# ---------------------------------------------------------------------------
# The work.
# ---------------------------------------------------------------------------
mkdir -p "$mount"
echo "# mounting $dmg"
if ! hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mount" >/dev/null; then
  die "could not mount $dmg (is it a disk image?)"
fi
mounted=1

# The volume name, so the rebuilt dmg keeps the one the user sees in Finder.
volume_name="$(diskutil info "$mount" 2>/dev/null | awk -F': *' '/Volume Name/ { print $2; exit }')"
[ -n "$volume_name" ] || volume_name="$(basename "$dmg" .dmg)"

# Copy the whole volume -- the app, the Applications symlink, the background, the .DS_Store
# -- so the rebuild reproduces the layout Tauri created rather than a bare app.
ditto "$mount" "$work/volume"
hdiutil detach "$mount" >/dev/null
mounted=0

app="$(find "$work/volume" -maxdepth 1 -name '*.app' -type d -print -quit)"
[ -n "$app" ] || die "no .app inside $dmg"

# `notarytool` accepts a zip, a dmg or a pkg, not a bundle directory.
ditto -c -k --keepParent "$app" "$work/app.zip"
submit_and_wait "$work/app.zip" "the .app"

echo "# stapling the ticket into the .app (Gatekeeper checks the app, not the dmg)"
if ! xcrun stapler staple "$app" >/dev/null; then
  die "could not staple the .app"
fi
ok "stapled the ticket into the .app"

# The app Tauri left beside the dmg is the same bundle, and the release job validates *that*
# path with `xcrun stapler validate`; staple it too so the check reads the ticket.
sibling="$(cd "$(dirname "$dmg")/../macos" 2>/dev/null && pwd || true)"
if [ -n "$sibling" ]; then
  for a in "$sibling"/*.app; do
    [ -d "$a" ] || continue
    if xcrun stapler staple "$a" >/dev/null; then
      ok "stapled the ticket into $(basename "$a") (the app beside the dmg)"
    else
      fail "could not staple $a"
    fi
  done
fi

echo "# re-packing $dmg around the stapled .app"
rm -f "$dmg"
if ! hdiutil create -volname "$volume_name" -srcfolder "$work/volume" -ov -format UDZO "$dmg" >/dev/null; then
  die "could not re-pack $dmg"
fi

submit_and_wait "$dmg" "the .dmg"

echo "# stapling the ticket into the .dmg"
if ! xcrun stapler staple "$dmg" >/dev/null; then
  die "could not staple the .dmg"
fi
ok "stapled the ticket into the .dmg"

if [ "$failures" -gt 0 ]; then
  echo "=== FAIL ($failures)" >&2
  exit 1
fi
echo "=== notarized and stapled: $dmg"
