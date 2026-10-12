#!/usr/bin/env bash
#
# The machine-readable release gate. It answers one question — "may this artifact be
# published?" — with an exit code, so a release job cannot publish an unsigned installer, an
# unstapled dmg or a manifest nothing verifies.
#
#   scripts/verify-release-artifacts.sh --macos   --dir src-tauri/target/universal-apple-darwin/release/bundle
#   scripts/verify-release-artifacts.sh --windows --dir src-tauri/target/release/bundle
#   scripts/verify-release-artifacts.sh --manifest --manifest-dir dist/release
#   scripts/verify-release-artifacts.sh --version --tag v0.1.0
#   scripts/verify-release-artifacts.sh --all --manifest-url https://updates.stvdy.app/…
#
# **It refuses to run with the updater's signing key in its environment.**
#
# That refusal is the reason this script exists in the shape it does rather than being
# politeness. The property being established is "the published artifact was signed by the
# holder of the key whose public half is compiled into the app". A verification tool that
# *can* sign cannot establish it: given `TAURI_SIGNING_PRIVATE_KEY`, "the signature verifies"
# and "this machine's key produced the signature" are indistinguishable, and the check
# collapses into a tautology that passes on any artifact the operator's own key signed —
# including one they signed by accident. Worse, the key's whole defence is that it is used in
# exactly one place (A332); a verification tool that tolerates the key in its environment is a
# second place, and the way a key leaks is a habit, not an attack.
#
# So: exit 2, before any check runs, and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` counts too —
# with the password exported, the key itself is one `env` assignment away, and the habit is
# the thing being prevented. **The consequence for the release workflow is a real
# requirement**: the signing secret must be scoped to the signing *step*, never to the job,
# or this script refuses and the release fails. That is deliberate — a job-wide secret is how
# the key ends up in the environment of every later step, which is exactly the drift A332
# forbids.
#
# What each mode checks:
#
#   --macos     codesign --verify --deep --strict, and the signature's authority is a
#               Developer ID Application certificate (an ad-hoc signature passes
#               `codesign --verify`); spctl -a -vvv -t install; xcrun stapler validate;
#               lipo -archs names both arm64 and x86_64 (A333).
#   --windows   osslsigncode verify, the PE digest algorithm is SHA-256, and an Authenticode
#               or RFC 3161 timestamp is present in the counter-signature (A334).
#   --manifest  the committed `plugins.updater.pubkey` verifies the published `latest.json`,
#               **and a one-byte mutation of it does not** — the negative case is what makes
#               the positive one mean anything — and every artifact the manifest names
#               answers 2xx at the URL it names (a manifest pointing at a 404 is a channel
#               that reports "up to date" forever).
#   --version   the tag, the workspace `Cargo.toml` version and `tauri.conf.json`'s version
#               agree (A353; a `server-v…` tag names the server alone).
#
# Fail-closed everywhere: a check that cannot read what it needs to read is a FAIL, never a
# skip. A release gate that passes because it inspected nothing is worse than no gate, because
# it reads as coverage.
#
# **What this script cannot check, and does not pretend to.** It verifies signatures against
# the key the configuration commits; it cannot tell you whether that key is the operator's
# real one rather than the committed placeholder — a placeholder fails every verification, so
# the failure it produces is loud, and that is the whole of the guarantee. And it never
# reaches the network except to fetch the manifest and probe the artifact URLs, both of which
# are named on the command line.
set -euo pipefail

usage() {
  # The header comment, from line 2 to the first line that is not a comment.
  awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "$0"
}

die_usage() {
  echo "verify-release-artifacts: $1" >&2
  echo "run with --help for the modes" >&2
  exit 2
}

mode=""
dir=""
tag=""
manifest_dir=""
manifest_url=""
config="src-tauri/tauri.conf.json"
cargo_toml="Cargo.toml"
dry_run=0

while [ $# -gt 0 ]; do
  case "$1" in
    --macos | --windows | --manifest | --version)
      [ -z "$mode" ] || die_usage "one mode at a time (already have $mode)"
      mode="${1#--}"
      ;;
    --all)
      [ -z "$mode" ] || die_usage "one mode at a time (already have $mode)"
      mode="all"
      ;;
    --dir)
      shift
      [ $# -gt 0 ] || die_usage "--dir needs a directory"
      dir="$1"
      ;;
    --tag)
      shift
      [ $# -gt 0 ] || die_usage "--tag needs a value"
      tag="$1"
      ;;
    --manifest-dir)
      shift
      [ $# -gt 0 ] || die_usage "--manifest-dir needs a directory"
      manifest_dir="$1"
      ;;
    --manifest-url)
      shift
      [ $# -gt 0 ] || die_usage "--manifest-url needs a URL"
      manifest_url="$1"
      ;;
    --config)
      shift
      [ $# -gt 0 ] || die_usage "--config needs a path"
      config="$1"
      ;;
    --cargo-toml)
      shift
      [ $# -gt 0 ] || die_usage "--cargo-toml needs a path"
      cargo_toml="$1"
      ;;
    --dry-run)
      dry_run=1
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      die_usage "unknown argument $1"
      ;;
  esac
  shift
done

[ -n "$mode" ] || die_usage "a mode is required"

# ---------------------------------------------------------------------------
# The refusal. Before any check, and before --dry-run, because the point is that no checking
# process runs with the key present — not merely that this one does not use it. `--help` is
# answered above it on purpose: an operator who has just been refused needs a way to read why
# without unsetting anything.
# ---------------------------------------------------------------------------
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || [ -n "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
  echo "refusing to run with the updater signing key in the environment" >&2
  echo "  set: ${TAURI_SIGNING_PRIVATE_KEY:+TAURI_SIGNING_PRIVATE_KEY}${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:+ TAURI_SIGNING_PRIVATE_KEY_PASSWORD}" >&2
  echo "  verification must not sign: with the key present, \"the signature verifies\" and" >&2
  echo "  \"this key signed it\" are the same observation, which is the one thing this gate" >&2
  echo "  exists to separate (A332). Unset it, or run the verifier in a step that does not" >&2
  echo "  carry the secret." >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
# Resolve what the caller named against the repository, so the script works from any working
# directory (a release job's is not the repository root).
for name in dir config cargo_toml manifest_dir; do
  value="${!name}"
  case "$value" in
    "") continue ;;
    /*) continue ;;
  esac
  printf -v "$name" '%s' "$root/$value"
done
[ -n "$dir" ] || dir="$root"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failures=0
checked=0

ok() {
  checked=$((checked + 1))
  printf 'ok   %s\n' "$1"
}

fail() {
  checked=$((checked + 1))
  failures=$((failures + 1))
  printf 'FAIL %s\n' "$1" >&2
}

# A check whose precondition is missing is a failure, not a skip. The message says which
# command was not there, because "the gate passed" and "the gate could not look" must not
# print the same thing.
need_tool() {
  if command -v "$1" >/dev/null 2>&1; then
    return 0
  fi
  fail "$2: $1 is not installed, so this could not be checked"
  return 1
}

# JSON reads, in python3 rather than in a jq pipeline: one implementation instead of two that
# have to agree, and python3 is on both the macOS and the Ubuntu runners the release uses.
# `json_get` walks a dotted path (a key may be bracketed when it contains a dash);
# `json_keys` prints an object's keys, one per line.
json_get() {
  python3 "$work/stvdy-json.py" get "$1" "$2"
}

json_keys() {
  python3 "$work/stvdy-json.py" keys "$1" "$2"
}

# Not `json.py`: a module of that name in the script's own directory shadows the standard
# library's, and the failure is an AttributeError rather than a missing-file message.
cat >"$work/stvdy-json.py" <<'PY'
import json, sys

mode, path, expr = sys.argv[1], sys.argv[2], sys.argv[3]
value = json.load(open(path))

def walk(node, parts):
    for part in parts:
        if part == "":
            continue
        if part.startswith("[") and part.endswith("]"):
            node = node[part[1:-1]]
        else:
            node = node[part]
    return node

parts = expr.replace("][", "].[").split(".")
if mode == "get":
    try:
        value = walk(value, parts)
    except (KeyError, IndexError, TypeError):
        value = ""
    print("" if value is None else (value if isinstance(value, str) else json.dumps(value)))
elif mode == "keys":
    try:
        node = walk(value, parts)
    except (KeyError, IndexError, TypeError):
        node = {}
    for key in node if isinstance(node, dict) else []:
        print(key)
PY

# An `openssl` that can do Ed25519 over raw bytes. macOS ships LibreSSL at /usr/bin/openssl,
# whose `pkeyutl` has no `-rawin`, so the probe is not theoretical: it is the difference
# between verifying and appearing to.
find_openssl() {
  local candidate
  for candidate in "${OPENSSL:-}" openssl /opt/homebrew/bin/openssl /usr/local/bin/openssl; do
    [ -n "$candidate" ] || continue
    command -v "$candidate" >/dev/null 2>&1 || continue
    if "$candidate" pkeyutl -help 2>&1 | grep -q -- '-rawin'; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  return 1
}

# ---------------------------------------------------------------------------
# minisign verification, the way `tauri-plugin-updater` 2.13.3 does it
# (`updater.rs::verify_signature`): base64 → utf8 → `minisign_verify::{PublicKey,
# Signature}::decode` → `verify(bytes, signature, allow_legacy = true)`, which checks the
# message signature *and* the global signature over the trusted comment. Reproduced here
# rather than approximated, because the trusted comment carries the version the plugin
# compares against the endpoint's — a verifier that skipped the global signature would accept
# a manifest whose comment an attacker rewrote.
#
# The public key is `base64(<minisign.pub>)`, and the inner text's second line is
# `base64(alg[2] || key_id[8] || key[32])`. The signature is
# `base64(<untrusted comment>\n<base64(alg[2] || key_id[8] || sig[64])>\n<trusted comment>\n
# <base64(global sig[64])>)`.
# ---------------------------------------------------------------------------
minisign_verify_file() {
  local message="$1" signature_b64="$2" pubkey_b64="$3" openssl="$4" label="$5"

  # Outer base64 → text, for both halves.
  printf '%s' "$pubkey_b64" | "$openssl" base64 -d -A >"$work/pubkey.txt" 2>/dev/null || {
    fail "$label: the committed public key is not base64"
    return 1
  }
  printf '%s' "$signature_b64" | "$openssl" base64 -d -A >"$work/signature.txt" 2>/dev/null || {
    fail "$label: the published signature is not base64"
    return 1
  }

  # Inner base64 → the 42-byte public key and the 74-byte signature blob.
  sed -n '2p' "$work/pubkey.txt" | "$openssl" base64 -d -A >"$work/pubkey.bin" 2>/dev/null || true
  sed -n '2p' "$work/signature.txt" | "$openssl" base64 -d -A >"$work/signature.bin" 2>/dev/null || true
  sed -n '4p' "$work/signature.txt" | "$openssl" base64 -d -A >"$work/global.bin" 2>/dev/null || true

  local pubkey_len signature_len
  pubkey_len="$(wc -c <"$work/pubkey.bin" | tr -d ' ')"
  signature_len="$(wc -c <"$work/signature.bin" | tr -d ' ')"
  if [ "$pubkey_len" != "42" ]; then
    fail "$label: the committed public key is $pubkey_len bytes, not the 42 a minisign key has"
    return 1
  fi
  if [ "$signature_len" != "74" ]; then
    fail "$label: the signature blob is $signature_len bytes, not the 74 a minisign signature has"
    return 1
  fi

  # The algorithm tag, and the key id that binds the signature to this key. `Ed` is raw
  # Ed25519 over the message; `ED` is Ed25519 over its BLAKE2b-512 digest. Both are accepted
  # because the plugin passes `allow_legacy = true`; a signature for a *different* key is
  # rejected here rather than by a verification failure that would read as a tampering.
  # `od` rather than `xxd`: xxd is not in POSIX and is not installed everywhere the release
  # runs, and a gate that dies on a missing hex dumper is a gate nobody trusts.
  local pubkey_alg signature_alg pubkey_id signature_id
  pubkey_alg="$(dd if="$work/pubkey.bin" bs=1 count=2 2>/dev/null | od -An -tx1 | tr -d ' \n')"
  signature_alg="$(dd if="$work/signature.bin" bs=1 count=2 2>/dev/null | od -An -tx1 | tr -d ' \n')"
  pubkey_id="$(dd if="$work/pubkey.bin" bs=1 skip=2 count=8 2>/dev/null | od -An -tx1 | tr -d ' \n')"
  signature_id="$(dd if="$work/signature.bin" bs=1 skip=2 count=8 2>/dev/null | od -An -tx1 | tr -d ' \n')"

  if [ "$pubkey_id" != "$signature_id" ]; then
    fail "$label: the signature's key id ($signature_id) is not the committed key's ($pubkey_id)"
    return 1
  fi

  # The raw 32-byte Ed25519 key, wrapped in the fixed SubjectPublicKeyInfo header: SEQUENCE
  # (42) / SEQUENCE (5) / OID 1.3.101.112 / BIT STRING (33). The BIT STRING's leading `00` is
  # the "no unused bits" octet, and leaving it out produces a key OpenSSL refuses to decode
  # rather than a wrong verification — which is the failure mode you want, but not one to
  # debug twice.
  # Written with `printf` escapes rather than a hex dumper for the same reason as above.
  {
    printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'
    dd if="$work/pubkey.bin" bs=1 skip=10 count=32 2>/dev/null
  } >"$work/pubkey.der"
  if ! "$openssl" pkey -inform DER -pubin -in "$work/pubkey.der" -out "$work/pubkey.pem" 2>/dev/null; then
    fail "$label: the committed public key is not a usable Ed25519 key"
    return 1
  fi

  local signed="$message"
  if [ "$signature_alg" = "4544" ]; then
    # Pre-hashed: the signature covers BLAKE2b-512 of the bytes, not the bytes.
    "$openssl" dgst -blake2b512 -binary -out "$work/hashed.bin" "$message"
    signed="$work/hashed.bin"
  elif [ "$signature_alg" != "4564" ]; then
    fail "$label: unsupported signature algorithm $signature_alg"
    return 1
  fi
  if [ "$pubkey_alg" != "4564" ] && [ "$pubkey_alg" != "4544" ]; then
    fail "$label: unsupported public key algorithm $pubkey_alg"
    return 1
  fi

  dd if="$work/signature.bin" bs=1 skip=10 count=64 of="$work/message.sig" 2>/dev/null
  if ! "$openssl" pkeyutl -verify -pubin -inkey "$work/pubkey.pem" -rawin \
    -in "$signed" -sigfile "$work/message.sig" >/dev/null 2>&1; then
    return 1
  fi

  # The global signature, over `signature || trusted_comment`. This is the half that makes the
  # trusted comment trustworthy, and the plugin checks it on every update.
  #
  # The comment is the third line *minus* its `trusted comment: ` prefix and minus its
  # newline: `minisign_verify` slices the string it read, and a trailing newline here would
  # make every global signature fail against a manifest that is perfectly good.
  if ! sed -n '3p' "$work/signature.txt" | grep -q '^trusted comment: '; then
    fail "$label: the signature has no trusted comment line"
    return 1
  fi
  sed -n '3p' "$work/signature.txt" | sed 's/^trusted comment: //' | tr -d '\n' >"$work/comment.txt"
  cat "$work/message.sig" "$work/comment.txt" >"$work/global.input"
  if ! "$openssl" pkeyutl -verify -pubin -inkey "$work/pubkey.pem" -rawin \
    -in "$work/global.input" -sigfile "$work/global.bin" >/dev/null 2>&1; then
    fail "$label: the signature's trusted comment is not covered by its global signature"
    return 1
  fi

  return 0
}

# ---------------------------------------------------------------------------
# The checks
# ---------------------------------------------------------------------------

# The first `.app` (or the first `.dmg`, if there is no `.app`) under the bundle directory.
find_macos_app() {
  find "$dir" -maxdepth 4 -name '*.app' -type d 2>/dev/null | head -1
}

check_macos() {
  local app
  app="$(find_macos_app)"
  if [ -z "$app" ]; then
    fail "macos: no .app bundle under $dir, so nothing was checked"
    return 0
  fi
  echo "# macOS: $app"

  if need_tool codesign "macos: signature"; then
    if codesign --verify --deep --strict "$app" >/dev/null 2>&1; then
      ok "macos: codesign --verify --deep --strict passes"
    else
      fail "macos: codesign --verify --deep --strict rejects $app"
    fi
    # An ad-hoc signature passes `codesign --verify`, so the authority is the fact that
    # matters: a bundle signed with `-` is signed by nobody and Gatekeeper blocks it on every
    # machine but the one that built it (A333).
    local authority
    authority="$(codesign -dv --verbose=4 "$app" 2>&1 | grep '^Authority=' | head -1 || true)"
    case "$authority" in
      *"Developer ID Application"*)
        ok "macos: the signing authority is a Developer ID Application certificate"
        ;;
      "")
        fail "macos: no signing authority in the signature (an ad-hoc signature passes codesign --verify)"
        ;;
      *)
        fail "macos: the signing authority is not a Developer ID Application certificate: $authority"
        ;;
    esac
  fi

  if need_tool spctl "macos: Gatekeeper assessment"; then
    if spctl -a -vvv -t install "$app" >/dev/null 2>&1; then
      ok "macos: spctl accepts the bundle for install"
    else
      fail "macos: spctl rejects $app for install ($(spctl -a -vvv -t install "$app" 2>&1 | tail -1))"
    fi
  fi

  if need_tool xcrun "macos: notarization staple"; then
    # The ticket is stapled into the bundle *and* the dmg, because a first launch with no
    # network must not need a round trip to Apple (A333). The dmg is checked when one exists.
    local staple_targets=("$app")
    local dmg
    dmg="$(find "$dir" -maxdepth 4 -name '*.dmg' 2>/dev/null | head -1)"
    if [ -n "$dmg" ]; then
      staple_targets+=("$dmg")
    fi
    local target
    for target in "${staple_targets[@]}"; do
      if xcrun stapler validate "$target" >/dev/null 2>&1; then
        ok "macos: the notarization ticket is stapled to $(basename "$target")"
      else
        fail "macos: no stapled notarization ticket on $(basename "$target")"
      fi
    done
  fi

  if need_tool lipo "macos: architectures"; then
    local binary archs
    binary="$(find "$app/Contents/MacOS" -maxdepth 1 -type f 2>/dev/null | head -1)"
    if [ -z "$binary" ]; then
      fail "macos: no executable under $app/Contents/MacOS"
    else
      archs="$(lipo -archs "$binary" 2>/dev/null || true)"
      case "$archs" in
        *arm64*x86_64* | *x86_64*arm64*)
          ok "macos: the binary is universal ($archs)"
          ;;
        *)
          fail "macos: the binary is not universal: lipo -archs says '$archs'"
          ;;
      esac
    fi
  fi
}

check_windows() {
  local installer
  installer="$(find "$dir" -maxdepth 4 -name '*.exe' -type f 2>/dev/null | head -1)"
  if [ -z "$installer" ]; then
    fail "windows: no .exe under $dir, so nothing was checked"
    return 0
  fi
  echo "# windows: $installer"

  if ! need_tool osslsigncode "windows: signature"; then
    return 0
  fi

  local output
  if output="$(osslsigncode verify -in "$installer" 2>&1)"; then
    ok "windows: osslsigncode verify passes"
  else
    fail "windows: osslsigncode verify rejects $(basename "$installer") ($(printf '%s' "$output" | tail -1))"
    return 0
  fi

  # SHA-256, not SHA-1. A SHA-1 signature is accepted by the verifier and refused by
  # SmartScreen's reputation rules, so "it verifies" and "it installs" are different claims
  # (A334). Fail closed when the line cannot be read: a gate that cannot see the digest
  # algorithm must not pass.
  local digest
  digest="$(printf '%s' "$output" | grep -i 'Message digest algorithm' | head -1 || true)"
  case "$digest" in
    *[Ss][Hh][Aa]256*) ok "windows: the PE digest algorithm is SHA-256" ;;
    "") fail "windows: osslsigncode reported no message digest algorithm" ;;
    *) fail "windows: the PE digest algorithm is not SHA-256: $digest" ;;
  esac

  # A timestamp is what keeps the installer installable after the certificate expires. The
  # counter-signature lives in the signerInfo's unauthenticated attributes, so it is read
  # structurally when the tool can extract the signature, and from the verifier's own report
  # otherwise — never assumed.
  local openssl stamp_oid=""
  openssl="$(find_openssl || true)"
  if [ -n "$openssl" ] && osslsigncode extract-signature -in "$installer" -out "$work/authenticode.p7" >/dev/null 2>&1; then
    local printed
    printed="$("$openssl" pkcs7 -inform DER -print -in "$work/authenticode.p7" 2>/dev/null || true)"
    # 1.3.6.1.4.1.311.3.3.1 is Authenticode's; 1.2.840.113549.1.9.16.2.14 is RFC 3161's.
    case "$printed" in
      *1.3.6.1.4.1.311.3.3.1*) stamp_oid="Authenticode" ;;
      *1.2.840.113549.1.9.16.2.14*) stamp_oid="RFC 3161" ;;
    esac
  fi
  if [ -n "$stamp_oid" ]; then
    ok "windows: the signature carries an $stamp_oid timestamp"
  elif printf '%s' "$output" | grep -qi 'timestamp'; then
    ok "windows: the signature is timestamped (per osslsigncode's report)"
  else
    fail "windows: the signature carries no timestamp, so it stops installing when the certificate expires"
  fi
}

# The published manifest and its signature. `--manifest-dir` reads them from disk;
# `--manifest-url` fetches them, which is the round trip Task 5's workflow makes.
manifest_paths() {
  if [ -n "$manifest_dir" ]; then
    manifest_file="$manifest_dir/latest.json"
    manifest_sig_file="$manifest_dir/latest.json.sig"
    return 0
  fi
  if [ -n "$manifest_url" ]; then
    manifest_file="$work/latest.json"
    manifest_sig_file="$work/latest.json.sig"
    curl -fsS -o "$manifest_file" "$manifest_url" || {
      fail "manifest: $manifest_url did not answer"
      return 1
    }
    curl -fsS -o "$manifest_sig_file" "$manifest_url.sig" || {
      fail "manifest: $manifest_url.sig did not answer — an unsigned manifest is a channel that trusts anything"
      return 1
    }
    return 0
  fi
  fail "manifest: neither --manifest-dir nor --manifest-url was given"
  return 1
}

check_manifest() {
  local manifest_file manifest_sig_file
  if ! manifest_paths; then
    return 0
  fi
  for file in "$manifest_file" "$manifest_sig_file"; do
    [ -f "$file" ] || {
      fail "manifest: $file does not exist"
      return 0
    }
  done
  echo "# manifest: $manifest_file"

  local openssl pubkey
  if ! openssl="$(find_openssl)"; then
    fail "manifest: no openssl with Ed25519 -rawin support, so the signature could not be checked"
    return 0
  fi
  pubkey="$(json_get "$config" 'plugins.updater.pubkey')" || {
    fail "manifest: $config could not be read"
    return 0
  }
  if [ -z "$pubkey" ]; then
    fail "manifest: $config has no plugins.updater.pubkey"
    return 0
  fi

  local signature
  signature="$(tr -d '\n' <"$manifest_sig_file")"

  # (1) The positive case. Without it the mutation below would pass against a verifier that
  # rejects everything.
  if minisign_verify_file "$manifest_file" "$signature" "$pubkey" "$openssl" "manifest"; then
    ok "manifest: the committed pubkey verifies the published latest.json"
  else
    fail "manifest: the committed pubkey does NOT verify the published latest.json"
  fi

  # (2) The negative case: one byte of the manifest flipped, signature untouched. A verifier
  # that checked only the signature's shape, or that read the trusted comment without the
  # global signature, passes (1) and fails here — which is the point of having both.
  local size offset
  size="$(wc -c <"$manifest_file" | tr -d ' ')"
  if [ "$size" -lt 4 ]; then
    fail "manifest: latest.json is $size bytes, which cannot be a manifest"
    return 0
  fi
  offset=$((size / 2))
  dd if="$manifest_file" of="$work/mutated.json" bs=1 count="$offset" 2>/dev/null
  printf 'X' >>"$work/mutated.json"
  dd if="$manifest_file" of="$work/tail.bin" bs=1 skip=$((offset + 1)) 2>/dev/null
  cat "$work/tail.bin" >>"$work/mutated.json"
  if cmp -s "$manifest_file" "$work/mutated.json"; then
    fail "manifest: the mutation did not change the bytes, so the negative case proved nothing"
  elif minisign_verify_file "$work/mutated.json" "$signature" "$pubkey" "$openssl" "manifest"; then
    fail "manifest: a one-byte mutation of latest.json still verifies"
  else
    ok "manifest: a one-byte mutation of latest.json does not verify"
  fi

  # (3) Every platform entry carries a signature, and (4) every URL it names is live. A
  # manifest that names a 404 makes the app report "up to date" forever, which is a failure
  # nobody sees until they wonder why no update ever arrives.
  local platforms platform url entry_signature version
  if ! platforms="$(json_keys "$manifest_file" 'platforms')"; then
    fail "manifest: latest.json has no .platforms map"
    return 0
  fi
  if [ -z "$platforms" ]; then
    fail "manifest: .platforms is empty, so no artifact is named"
    return 0
  fi
  while IFS= read -r platform; do
    [ -n "$platform" ] || continue
    url="$(json_get "$manifest_file" "platforms.[$platform].url")"
    entry_signature="$(json_get "$manifest_file" "platforms.[$platform].signature")"
    version="$(json_get "$manifest_file" "platforms.[$platform].version")"
    if [ -z "$url" ] || [ -z "$entry_signature" ]; then
      fail "manifest: $platform names no url or no signature"
      continue
    fi
    case "$url" in
      https://*) ;;
      *) fail "manifest: $platform's url is not HTTPS: $url" && continue ;;
    esac
    # A signature that does not decode is a manifest the app cannot install from, and it is
    # cheap to catch here rather than on a user's machine.
    if printf '%s' "$entry_signature" | "$openssl" base64 -d -A 2>/dev/null | sed -n '2p' | "$openssl" base64 -d -A 2>/dev/null | wc -c | grep -q '^ *74$'; then
      ok "manifest: $platform carries a well-formed signature (version $version)"
    else
      fail "manifest: $platform's signature is not a minisign signature"
    fi
    # HEAD first, then a one-byte ranged GET: some object stores and CDNs answer 405 to HEAD,
    # and treating that as "missing" would make this check fire on a working release.
    if curl -fsS -o /dev/null -I "$url" 2>/dev/null || curl -fsS -o /dev/null -r 0-0 "$url" 2>/dev/null; then
      ok "manifest: $platform's artifact answers at its URL"
    else
      fail "manifest: $platform's artifact is not reachable at $url"
    fi
  done <<<"$platforms"
}

check_version() {
  local tag_value="${tag:-${GITHUB_REF_NAME:-}}"
  if [ -z "$tag_value" ]; then
    tag_value="$(git -C "$root" describe --tags --exact-match 2>/dev/null || true)"
  fi
  if [ -z "$tag_value" ]; then
    fail "version: no tag to compare against (--tag, GITHUB_REF_NAME, or a checked-out tag)"
    return 0
  fi

  local workspace shell
  workspace="$(awk '/^\[workspace\.package\]/{found=1} found && /^version *=/{gsub(/[" ]/, "", $3); print $3; exit}' "$cargo_toml")"
  shell="$(json_get "$config" 'version')"
  if [ -z "$workspace" ]; then
    fail "version: no [workspace.package] version in $cargo_toml"
    return 0
  fi
  if [ -z "$shell" ]; then
    fail "version: no version in $config"
    return 0
  fi

  local tagged="${tag_value#v}"
  # A353: the server and the shell are versioned independently, and a server-only hotfix is
  # tagged `server-v…` and ships no client. So a `server-` tag is compared to the workspace
  # version alone — the shell's version is not part of that release.
  case "$tag_value" in
    server-v* | server-*)
      tagged="${tag_value#server-v}"
      tagged="${tagged#server-}"
      if [ "$tagged" = "$workspace" ]; then
        ok "version: $tag_value names the server, and the workspace version is $workspace (A353: no client in this release)"
      else
        fail "version: the tag says $tagged and $cargo_toml says $workspace"
      fi
      return 0
      ;;
  esac

  if [ "$tagged" = "$workspace" ] && [ "$tagged" = "$shell" ]; then
    ok "version: the tag ($tagged), $cargo_toml ($workspace) and $config ($shell) agree"
  else
    fail "version: the tag says '$tagged', $cargo_toml says '$workspace', $config says '$shell'"
  fi
}

# ---------------------------------------------------------------------------
# Dispatch. `--dry-run` prints what would be checked and touches nothing, which is the one
# path that is verifiable without a signed artifact in hand.
# ---------------------------------------------------------------------------
case "$mode" in
  macos) modes=(macos) ;;
  windows) modes=(windows) ;;
  manifest) modes=(manifest) ;;
  version) modes=(version) ;;
  all) modes=(version macos windows manifest) ;;
  *) die_usage "unknown mode $mode" ;;
esac

if [ "$dry_run" = "1" ]; then
  printf 'verify-release-artifacts: mode=%s dir=%s\n' "$mode" "$dir"
  for m in "${modes[@]}"; do
    case "$m" in
      version) echo "would check version: tag, $cargo_toml, $config (A353)" ;;
      macos) echo "would check macos: codesign --verify --deep --strict, the signing authority, spctl -a -t install, xcrun stapler validate, lipo -archs (A333)" ;;
      windows) echo "would check windows: osslsigncode verify, SHA-256, a timestamp (A334)" ;;
      manifest) echo "would check manifest: the committed pubkey verifies latest.json, a one-byte mutation does not, every named URL is live (A332, A345)" ;;
    esac
  done
  exit 0
fi

for m in "${modes[@]}"; do
  case "$m" in
    version) check_version ;;
    macos) check_macos ;;
    windows) check_windows ;;
    manifest) check_manifest ;;
  esac
done

echo
if [ "$failures" -gt 0 ]; then
  echo "$failures of $checked checks failed — do not publish this artifact" >&2
  exit 1
fi
echo "=== PASS ($checked checks)"
