#!/usr/bin/env bash
#
# Vendors the cheatsheet's print assets into `assets/`.
#
# Why the bytes are committed rather than fetched at build time: the template used
# to load KaTeX and the two typefaces from a third-party CDN, so a render depended
# on that CDN being reachable. A blocked or slow CDN does not fail the render — it
# leaves every formula as literal `$…$` and the text in a fallback face, which is
# exactly the silent defect `crates/server/tests/pdf.rs` asserts against. The
# renderer now rewrites those references to this server (A339); this script is how
# the bytes get into the repository.
#
# The same reasoning as `scripts/fetch-fonts.mjs`, which does it for the SPA: a
# build that reaches the network for a font fails on the day the CDN is slow, and
# it fails as a missing glyph rather than as an error.
#
# Every file is verified against the SHA-256 in `assets/checksums.txt` before it is
# written, so a re-run against a CDN that changed its bytes fails loudly instead of
# silently swapping a shipped asset for a different one. `assets/fonts.css` is
# generated from the Google Fonts stylesheet, and every woff2 it names must already
# be in that list: an unlisted face is a hard failure, not a download.
#
#   bash scripts/fetch-print-assets.sh
#
# Regenerating after a version bump is a deliberate two-step, because a checksum
# list that updates itself verifies nothing:
#
#   1. edit the URL(s) in `assets/checksums.txt`,
#   2. run `bash scripts/fetch-print-assets.sh --update` to record the new hashes,
#   3. read the diff: a changed hash is a changed asset, and that is the review.
#
# `--update` is the only mode that writes a hash it did not verify against a
# committed one.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
assets="$root/assets"
checksums="$assets/checksums.txt"
update=""
if [ "${1:-}" = "--update" ]; then
  update="1"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

# A modern browser UA, because the Google Fonts API answers with ttf for anything
# else and a ttf is not what the stylesheet the browser gets asks for.
UA="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/124.0 Safari/537.36"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fetch() {
  # `-A "$UA"` is not decoration: the Google Fonts API answers a stylesheet with
  # `ttf` sources and no subset annotations to anything it does not recognise as a
  # modern browser, and this script reads the subset annotations.
  curl -fsSL --retry 3 --max-time 120 -A "$UA" -o "$2" "$1" ||
    { echo "fetch failed: $1" >&2; return 1; }
}

# Looks up a URL in the checksum list and prints "<sha256> <dest>".
lookup() {
  awk -v url="$1" '$2 == url { print $1, $3; found = 1 } END { exit found ? 0 : 1 }' "$checksums"
}

# --- 1. KaTeX, and the woff2 files its stylesheet names ------------------------
#
# Every row whose destination is under `katex/` is a KaTeX file. The list is the
# authority for which files exist; the stylesheet is checked against it below.

mkdir -p "$assets/katex/fonts" "$assets/katex/contrib"
katex_count=0
while read -r sha url dest; do
  case "$dest" in
  katex/*) ;;
  *) continue ;;
  esac
  fetch "$url" "$work/asset"
  got="$(sha256 "$work/asset")"
  if [ -n "$update" ]; then
    sha="$got"
  elif [ "$got" != "$sha" ]; then
    echo "checksum mismatch for $dest" >&2
    echo "  expected $sha" >&2
    echo "  got      $got" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$assets/$dest")"
  cp "$work/asset" "$assets/$dest"
  katex_count=$((katex_count + 1))
done <"$checksums"

# `katex.min.css` names its fonts relatively (`fonts/KaTeX_Main-Regular.woff2`),
# which resolves to `/assets/katex/fonts/…` because that is where the stylesheet
# is served from. A font the stylesheet names and the list does not carry would be
# a 404 at print time, so it is a failure here instead.
missing=0
for font in $(grep -o 'fonts/[A-Za-z0-9_-]*\.woff2' "$assets/katex/katex.min.css" | sort -u); do
  if [ ! -f "$assets/katex/$font" ]; then
    echo "katex.min.css names a font that is not vendored: $font" >&2
    missing=1
  fi
done
[ "$missing" = 0 ] || exit 1

# --- 2. The typefaces the template names, and the stylesheet that declares them -
#
# `assets/fonts.css` replaces the `fonts.googleapis.com` stylesheet the template
# used to link. Only latin and latin-ext are kept: latin-ext is not optional,
# because Indonesian source material carries characters outside basic latin.

for family in "Inter:wght@400;600;700" "JetBrains+Mono:wght@400;600"; do
  fetch "https://fonts.googleapis.com/css2?family=$family&display=swap" "$work/$family.css"
done

faces="$work/faces"
: >"$faces"
for family in "Inter:wght@400;600;700" "JetBrains+Mono:wght@400;600"; do
  awk '
    /^\/\* / { subset = $0; sub(/^\/\* /, "", subset); sub(/ \*\/$/, "", subset); next }
    /font-family:/ { fam = $0; sub(/^ *font-family: /, "", fam); sub(/;$/, "", fam); gsub(/'"'"'/, "", fam) }
    /font-weight:/ { weight = $2; sub(/;$/, "", weight) }
    /unicode-range:/ { range = $0; sub(/^ *unicode-range: /, "", range); sub(/;$/, "", range) }
    /url\(https:/ { url = $0; sub(/^.*url\(/, "", url); sub(/\).*$/, "", url) }
    /^}/ {
      if ((subset == "latin" || subset == "latin-ext") && url != "") {
        print fam "|" weight "|" subset "|" range "|" url
      }
      url = ""
    }
  ' "$work/$family.css" >>"$faces"
done

mkdir -p "$assets/fonts"
{
  cat <<'HEADER'
/* Bundled print typefaces. Generated by `scripts/fetch-print-assets.sh`; edit that
   script rather than this file.

   The cheatsheet template names Inter and JetBrains Mono. It used to load them
   from `fonts.googleapis.com`, which makes a printed document depend on a
   third-party origin: a blocked or slow CDN leaves the PDF in a fallback face.
   The renderer rewrites that stylesheet reference to this file (A339).

   latin-ext is kept: Indonesian source material carries characters outside basic
   latin, and dropping the subset renders them mid-sentence in a fallback face.

   Licences: both families are SIL Open Font License 1.1; see
   THIRD_PARTY_NOTICES.md. */
HEADER
} >"$assets/fonts.css"

while IFS='|' read -r fam weight subset range url; do
  [ -n "$url" ] || continue
  set +e
  found="$(lookup "$url")"
  status=$?
  set -e
  if [ "$status" != 0 ]; then
    echo "the Google Fonts stylesheet names a face that is not in assets/checksums.txt:" >&2
    echo "  $fam $weight $subset $url" >&2
    echo "add its row (url and destination) and re-run with --update" >&2
    exit 1
  fi
  sha="${found%% *}"
  dest="${found##* }"
  fetch "$url" "$work/font"
  got="$(sha256 "$work/font")"
  if [ -n "$update" ]; then
    sha="$got"
  elif [ "$got" != "$sha" ]; then
    echo "checksum mismatch for $dest" >&2
    echo "  expected $sha" >&2
    echo "  got      $got" >&2
    exit 1
  fi
  cp "$work/font" "$assets/$dest"
  {
    echo ""
    echo "@font-face {"
    echo "  font-family: '$fam';"
    echo "  font-style: normal;"
    echo "  font-weight: $weight;"
    echo "  font-display: swap;"
    echo "  src: url('$dest') format('woff2');"
    echo "  unicode-range: $range;"
    echo "}"
  } >>"$assets/fonts.css"
done <"$faces"

echo "verified $katex_count KaTeX files and $(wc -l <"$faces" | tr -d ' ') font faces"
echo "wrote $assets"
