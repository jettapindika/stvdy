#!/usr/bin/env bash
#
# Fails when the web assets a mobile build ships can buy something.
#
# This script is a *wrapper*, not a second check. Phase 14's
# `scripts/check-no-purchase-ui.mjs` holds the rule and is the only
# implementation of it: it holds the purchase denylist, it holds the rule that a
# scan of nothing is a failure, and it re-asserts `config/billing_policy.toml`
# for `ios` and `android`. A second copy of any of that would drift from the
# first, and a reviewer would then have two answers to one question. So this
# script runs *that* check, unchanged, against the tree a mobile build actually
# ships.
#
# Why a wrapper exists. Phase 14's check reads `<repo>/app/dist`, the build
# output. The bytes a store reviewer sees are the ones inside the packaged
# artifact, and "the build output was clean" and "the artifact is clean" are two
# claims: a stale `dist/`, or anything that writes into the artifact after the
# build, is invisible to the first. This script points the same check at the
# packaged tree — `app/dist`, or an `.apk`/`.aab`/`.ipa` extracted with `unzip`
# — and says what it inspected and what it did not.
#
# How it redirects the check, and why that is not a trick. The shared check
# resolves the tree it reads as `<its own directory>/../app/dist`, and the policy
# file as `<its own directory>/../config/billing_policy.toml`. This script
# copies it into a scratch root whose `app/dist` is the artifact and whose
# `config/` is this repository's, so the check's own walker, denylist and policy
# reader run unchanged against the artifact. That layout is an assumption about
# another file, so it is asserted on every run rather than trusted:
# `probe_scratch_root` plants a priced file and fails the run if the check does
# not report it. A refactor that stopped honouring the layout therefore breaks
# this script loudly, instead of leaving it green while it inspected the wrong
# directory — the exact failure mode it exists to prevent.
#
# Usage: scripts/check-mobile-no-purchase.sh [TARGET]
#
#   TARGET  the shipped web assets to check: a directory (default: app/dist), or
#           an .apk/.aab/.ipa/.zip archive to extract first.
#
# Exit codes: 0 the artifact is clean; 1 it is not, or nothing was inspected;
# 2 this script could not check at all (usage, missing `unzip`, an unreadable
# success line). A 2 is never a pass.
#
# What it does not inspect, and no part of this output should be read as
# covering: the native layer (`lib*.so`, `classes.dex`, `Payload/*.app/*`), the
# store metadata, the review notes, or the account-deletion screen's server
# behaviour. Those are checklist rows in `docs/RELEASE.md` § Store submission
# checklists, not this script's claim.

set -euo pipefail

EXIT_PASS=0
EXIT_FAIL=1
EXIT_CANNOT_CHECK=2

self=$(basename "${BASH_SOURCE[0]}")
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
shared="$script_dir/check-no-purchase-ui.mjs"
policy="$repo_root/config/billing_policy.toml"
spa_dist="$repo_root/app/dist"

usage() {
  cat <<'USAGE'
Usage: scripts/check-mobile-no-purchase.sh [TARGET]

Fails when the web assets a mobile build ships can buy something, by running
Phase 14's scripts/check-no-purchase-ui.mjs against them.

  TARGET  a directory of shipped web assets (default: app/dist), or an
          .apk/.aab/.ipa/.zip archive to extract and check.

Exit codes: 0 clean, 1 offenders or nothing inspected, 2 could not check.
USAGE
}

fail() {
  printf '%s: %s\n' "$self" "$1" >&2
  exit "$EXIT_FAIL"
}

cannot_check() {
  printf '%s: %s\n' "$self" "$1" >&2
  exit "$EXIT_CANNOT_CHECK"
}

[ -f "$shared" ] || cannot_check "missing $shared — the shared check is what this script runs"
[ -f "$policy" ] || cannot_check "missing $policy — the policy assertion cannot be made"
command -v node >/dev/null 2>&1 || cannot_check "node is not on PATH; the shared check is a Node program"

target_arg=
if [ "$#" -gt 1 ]; then
  usage >&2
  cannot_check "at most one TARGET"
fi
if [ "$#" -eq 1 ]; then
  case "$1" in
    -h | --help)
      usage
      exit "$EXIT_PASS"
      ;;
    -*)
      usage >&2
      cannot_check "unknown option: $1"
      ;;
    *) target_arg=$1 ;;
  esac
fi

if [ -z "$target_arg" ]; then
  target="$spa_dist"
  label="app/dist"
else
  [ -e "$target_arg" ] || fail "TARGET does not exist: $target_arg (a path that is not there inspects nothing)"
  target="$(cd "$(dirname "$target_arg")" && pwd)/$(basename "$target_arg")"
  label=$target_arg
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/check-mobile-no-purchase.XXXXXX") || cannot_check "mktemp failed"
# Resolved physically, not left as `mktemp` printed it: on macOS `$TMPDIR` is
# under `/var`, a symlink to `/private/var`, and Node resolves symlinks in
# module URLs. A script path that goes in as `/var/...` comes back out of
# `import.meta.url` as `/private/var/...`, the shared check's own
# `isCli()` test fails, and it exits 0 having checked nothing. The probe below
# is what found that; this line is the fix.
scratch=$(cd "$scratch" && pwd -P)
# Single-quoted on purpose: `$scratch` is expanded when the trap fires, so the
# scratch root (and the extraction under it) is removed on every exit path,
# including the `cannot_check` exits below.
trap 'rm -rf "$scratch"' EXIT

# The physical path of a directory, so `app/dist` and a symlink to it compare
# equal and the artifact is not scanned twice.
physical() { (cd "$1" && pwd -P); }

# Lay out a scratch root whose `app/dist` is $2 and whose `config/` is this
# repository's. The shared check reads both relative to its own location, so the
# copy must live at `<root>/scripts/`.
prepare_root() {
  local root=$1 dist=$2
  mkdir -p "$root/scripts" "$root/config" "$root/app"
  cp "$shared" "$root/scripts/check-no-purchase-ui.mjs"
  ln -s "$policy" "$root/config/billing_policy.toml"
  ln -s "$dist" "$root/app/dist"
}

# The scratch-root redirect is an assumption about the shared check's own
# layout, so assert it every run: plant a price and fail if the check does not
# find it. This is what keeps "I ran the check against the artifact" from
# quietly meaning "I ran the check against the repository".
mkdir -p "$scratch/probe-dist"
printf 'const price = "Rp 10.000"\n' >"$scratch/probe-dist/probe.js"
prepare_root "$scratch/probe-root" "$scratch/probe-dist"
if probe_output=$(node "$scratch/probe-root/scripts/check-no-purchase-ui.mjs" 2>&1); then
  printf '%s\n' "$probe_output" >&2
  cannot_check "the scratch-root redirect no longer works: the shared check did not fail on a planted price under app/dist"
fi
case "$probe_output" in
  *probe.js*) ;;
  *)
    printf '%s\n' "$probe_output" >&2
    cannot_check "the scratch-root redirect is not reading app/dist: the shared check failed without naming the planted probe.js"
    ;;
esac

if [ -d "$target" ]; then
  dist="$target"
  inspected="the directory $label"
elif [ -f "$target" ]; then
  case "$target" in
    *.apk | *.aab | *.ipa | *.zip) ;;
    *) cannot_check "TARGET is a file but not an archive this script knows (.apk/.aab/.ipa/.zip): $label" ;;
  esac
  command -v unzip >/dev/null 2>&1 ||
    cannot_check "unzip is not on PATH, so the archive $label cannot be inspected (this is not a pass)"
  mkdir -p "$scratch/extracted"
  unzip -qq -o "$target" -d "$scratch/extracted" || fail "could not extract $label with unzip"
  members=$(find "$scratch/extracted" -type f | wc -l | tr -d ' ')
  dist="$scratch/extracted"
  inspected="the archive $label ($members member file(s), extracted; the paths below read app/dist/<member>)"
else
  fail "TARGET is neither a directory nor a file: $label"
fi

# 1/2 — the SPA build. This is the tree `tauri build` packages for every
# platform, and it is the one Phase 14's CI job already checks, so a failure
# here is a failure of the build rather than of the packaging.
printf '== the SPA build the mobile artifact packages: app/dist\n'
[ -d "$spa_dist" ] ||
  fail "app/dist is missing — run 'npm --prefix app run build' first (a missing build must not read as clean)"
prepare_root "$scratch/spa-root" "$spa_dist"
if spa_output=$(node "$scratch/spa-root/scripts/check-no-purchase-ui.mjs" 2>&1); then
  spa_status=$EXIT_PASS
else
  spa_status=$?
fi
printf '%s\n' "$spa_output"
[ "$spa_status" -eq "$EXIT_PASS" ] || fail "the SPA build carries a purchase affordance or a policy that permits one (see above)"

# 2/2 — the artifact, unless it is the same tree, in which case the scan above
# already is the artifact scan.
if [ "$(physical "$dist")" = "$(physical "$spa_dist")" ]; then
  printf '\n== TARGET is the SPA build itself, so one scan covers both\n'
  output=$spa_output
else
  printf '\n== the artifact target: %s\n' "$inspected"
  prepare_root "$scratch/artifact-root" "$dist"
  if artifact_output=$(node "$scratch/artifact-root/scripts/check-no-purchase-ui.mjs" 2>&1); then
    artifact_status=$EXIT_PASS
  else
    artifact_status=$?
  fi
  printf '%s\n' "$artifact_output"
  [ "$artifact_status" -eq "$EXIT_PASS" ] ||
    fail "the shipped web assets in $label carry a purchase affordance or a policy that permits one (see above)"
  output=$artifact_output
fi

# Report what was inspected. The count comes from the shared check's own success
# line rather than from a second walk here, because a second walk would need a
# second copy of the file-selection rule. If that line ever changes shape, this
# script says so and exits 2: an evidence script whose "what did you inspect"
# claim is unreadable is not evidence.
files=$(printf '%s\n' "$output" | sed -n 's/^no purchase UI in \([0-9][0-9]*\) shipped file(s)$/\1/p' | head -n 1)
[ -n "$files" ] ||
  cannot_check "the shared check passed but its success line no longer states a file count, so this script cannot report what it inspected"

printf '\n== summary\n'
printf 'inspected %s: %s shipped file(s), plus config/billing_policy.toml for ios and android\n' "$label" "$files"
printf 'not inspected: the native layer (lib*.so, classes.dex, Payload/*.app binaries), the store\n'
printf 'metadata, and the account-deletion behaviour — see docs/RELEASE.md store checklists\n'
printf 'no purchase UI ships in %s\n' "$label"
exit "$EXIT_PASS"
