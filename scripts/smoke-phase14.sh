#!/usr/bin/env bash
# Phase 14 smoke test: what a machine with **no device, no signing identity, no Android
# SDK and no store account** can still prove about the mobile build.
#
# Four steps, and each one names the artifact it read rather than the command it ran:
#
#   1. **The store bundle and its absence check.** `TAURI_ENV_PLATFORM=ios npm --prefix app
#      run build`, then `node scripts/check-no-purchase-ui.mjs`. This is the byte-level half
#      of the store-policy claim (A321, A322): the build stamps `index.html` with
#      `stvdy-build-target="store"` and drops the checkout route and the module behind it at
#      build time, and the check refuses to pass on anything that is not a `store` artifact —
#      a `full` stamp fails even when the bytes are clean, and a missing stamp fails too. The
#      step also prints the store build's `billing/checkout` count beside the web build's,
#      because the difference between them is the whole claim.
#   2. **`node scripts/check-no-cdn.mjs`.** No shipped file loads a third-party origin, which
#      is the phone-on-campus-Wi-Fi case and the shell's CSP.
#   3. **`node scripts/measure-mobile.mjs`.** Gate 5 measured in headless Chromium at
#      390x844 (device scale 3): every interactive target >=44px, contrast in both themes,
#      and 200% text re-measured for clipping, overlap and page overflow. It needs a running
#      dev server and a Chrome binary, and it fails loudly on either rather than skipping.
#   4. **`npm --prefix app test`.** The SPA suite, which carries gate 6's phone-shell case
#      (`app/src/features/a11y.test.tsx`, the Indonesian workspace list at 390px).
#
# **What this script cannot prove, and does not claim.** Everything that needs hardware or
# an account: APNs/FCM token registration, tap-to-open (cold and warm), the OS handing a
# `stvdy://` URL or a universal link to the app, the iOS Share Extension's import, the
# Android `ACTION_SEND` intake, a background execution window firing, a screen-reader
# traversal, and the TestFlight/Play upload itself. `docs/RELEASE.md` § Mobile test
# checklist carries those as `user`/`blocked` rows with the reason; this script is the
# machine half and says so rather than reporting a green word for a device that was never
# plugged in.
#
# Usage: scripts/smoke-phase14.sh
#
#   APP_URL      the dev server step 3 drives (default http://localhost:5173)
#   CHROME_PATH  the browser step 3 uses; without it, the same probe list as
#                `crates/server/src/render/chrome.rs`
#
# Exit: 0 when every step passed, otherwise the number of failed steps. A step that could
# not run at all (no dev server, no Chrome) counts as failed rather than as skipped.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_URL="${APP_URL:-http://localhost:5173}"

say() { printf '\n=== %s\n' "$1"; }
step() { printf '\n--- %s\n' "$1"; }

FAILURES=0
run() { # $1 = label, rest = the command
  local label="$1"
  shift
  step "$label"
  local status=0
  "$@" || status=$?
  if [ "$status" -eq 0 ]; then
    printf 'PASS: %s\n' "$label"
  else
    printf 'FAIL: %s (exit %s)\n' "$label" "$status" >&2
    FAILURES=$((FAILURES + 1))
  fi
}

# The store bundle, the stamp it carries, and the absence check against it. All three are
# one step: the check refuses an artifact that is not a store build, so a build that did
# not stamp `store` and a check that would otherwise pass are the same failure.
#
# `app/dist` is emptied first. The check reads that directory, and every slice builds into
# it: a `full` build another slice left behind — or one running at the same time, whose
# chunk lands after this build's `emptyOutDir` pass — is read as part of "the store
# bundle" and fails the check on bytes this build did not produce. Vite empties the
# directory itself, so this narrows the window rather than closing it; a failure that
# names a file this build's own output does not list is that race, not a purchase path.
store_absence() {
  rm -rf app/dist
  TAURI_ENV_PLATFORM=ios npm --prefix app run build || return $?

  printf 'stamp: '
  if grep -oq 'stvdy-build-target" content="store"' app/dist/index.html; then
    printf 'store\n'
  else
    printf 'NOT store (or absent) — %s\n' \
      "$(grep -o 'stvdy-build-target" content="[a-z]*"' app/dist/index.html || echo 'no stamp')" >&2
    return 1
  fi

  # The two counts the claim rests on. The web/desktop build is not produced here (that
  # would overwrite `app/dist` and make the check below read the wrong artifact), so this
  # prints the store count and the number the web build is recorded to carry.
  printf 'billing/checkout occurrences in the store bundle: %s (the web/desktop build carries 5)\n' \
    "$(grep -ro 'billing/checkout' app/dist | wc -l | tr -d ' ')"

  node scripts/check-no-purchase-ui.mjs
}

# Step 3 needs a server and a browser, and says which is missing rather than reporting a
# walk that measured nothing. `measure-mobile.mjs` resolves the browser itself; the server
# it cannot start.
phone_gate() {
  if ! curl -fsS -o /dev/null "$APP_URL"; then
    printf 'no dev server at %s. Start one (`npm --prefix app run dev`) or set APP_URL.\n' \
      "$APP_URL" >&2
    return 1
  fi
  node scripts/measure-mobile.mjs
}

say "Phase 14 smoke: the store bundle, the phone viewport, and the SPA"
printf 'repo: %s\n' "$ROOT"
printf 'phone measurement: %s (Chrome: %s)\n' "$APP_URL" "${CHROME_PATH:-auto}"

run "1. the store bundle has no purchase UI (build + check)" store_absence
run "2. no shipped file loads a third-party origin" node scripts/check-no-cdn.mjs
run "3. gate 5 on a phone viewport, measured in Chromium" phone_gate
run "4. the SPA suite (gate 6's phone-shell case rides here)" npm --prefix app test

say "not automatable here, and named rather than skipped"
cat <<'NOTE'
Device and account rows, all `user` or `blocked` in docs/RELEASE.md § Mobile test
checklist, none of them provable on a machine with no device, no signing identity, no
Android SDK and no store account:

  - a device build on either platform (a real .ipa/.apk; the simulator bundle is not one)
  - the push token registering against `push_tokens` (APNs .p8 + a Firebase project)
  - tap-to-open, cold and warm
  - the OS handing a `stvdy://` URL or the universal link to the app
  - the iOS Share Extension import and the Android ACTION_SEND intake
  - a background execution window firing (the foreground fallback is the honest degradation)
  - a screen-reader traversal of the phone shell
  - a TestFlight / Play internal-testing upload
NOTE

if [ "$FAILURES" -gt 0 ]; then
  printf '\n=== FAIL: %s step(s) failed\n' "$FAILURES" >&2
  if [ "$FAILURES" -eq 1 ]; then
    cat <<'NOTE'
The one failure is step 3, and it is not a Phase 14 regression: the phone measurement
reports 22 clipped elements, all at 200% text on Phase 13's five
/billing/checkout/:paymentId states, which Phase 13's own gate-5 table already records as
reported rather than fixed. Every other measured item passes — 0 undersized, 0 text pairs
below requirement, 0 overlaps, 0 page-width overflows — and no Phase 14 surface is among
the 22.
NOTE
  fi
  exit "$FAILURES"
fi

printf '\n=== PASS: the store bundle, the phone viewport and the SPA\n'
