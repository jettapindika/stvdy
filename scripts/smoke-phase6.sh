#!/usr/bin/env bash
# Phase 6 end-to-end smoke test: the report (outline → section → .md export) and the
# cheatsheet (blocks → HTML → A4 PDF).
#
# Drives the real server over HTTP against the local Postgres, with a STUB model server
# (scripts/stub-llm.py) that speaks real HTTP and answers three different replies in
# order: the outline JSON, a report section in Markdown, and a cheatsheet's blocks JSON.
# The stub is deliberate: the Rust suite replaces the provider at the trait boundary, so
# it cannot catch a wire-format mistake in the generation request, and it cannot catch a
# template or a browser problem at all. This run exercises the actual request bodies, the
# actual reply parses, and a real Chromium print.
#
# The PDF half needs a browser. Start the server with CHROME_PATH set (the same value this
# script uses for its own check), or the run reports that the print is unavailable and
# fails — which is the honest outcome, because "the PDF route answered 409" is not a pass.
#
# Not a test suite (that is crates/server/tests): this proves the wired process works.
set -euo pipefail

BASE="${BASE:-http://localhost:8099}"
# The port the SERVER was started with (LLM_BASE_URL), not a private one: the server is
# what talks to the stub, so the stub the script watches must be the stub the server
# calls. Start the server with
#   LLM_BASE_URL=http://127.0.0.1:8100/v1
# and leave this at its default, or set both to the same value.
STUB_PORT="${STUB_PORT:-8100}"
STUB_URL="http://127.0.0.1:$STUB_PORT"
# This must match the SERVER's `MAIL_DIR`, because the server is what writes the
# verification mail: the script only reads the newest `.eml` from it.
MAIL_DIR="${MAIL_DIR:-var/mail}"
# Where Chromium lives. The server needs the same value in its own environment; this is
# used to say so plainly when the print is missing.
CHROME_PATH="${CHROME_PATH:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
TMP="$(mktemp -d)"
EMAIL="phase6-$(date +%s)@example.test"
PASSWORD="correct horse battery"

# The three replies the stub serves, in the order the server asks for them: the report
# outline, a report section in Markdown, and the cheatsheet's blocks. The file is a
# committed fixture rather than a heredoc here, because `just smoke-llm-phase6` starts the
# stub separately and must serve exactly the same three answers — a copy in each place is a
# copy that drifts, and the symptom would be a parse failure that looks like a server bug.
ANSWERS="${ANSWERS:-scripts/fixtures/phase6-answers.json}"
[ -f "$ANSWERS" ] || fail "no answers fixture at $ANSWERS"

say() { printf '\n=== %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
field() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

PSQL="$(command -v psql || true)"
if [ -z "$PSQL" ]; then
  for candidate in /opt/homebrew/opt/postgresql@*/bin/psql /usr/local/opt/postgresql@*/bin/psql; do
    if [ -x "$candidate" ]; then PSQL="$candidate"; break; fi
  done
fi

# --- the stub model server -------------------------------------------------
STUB_PID=""
cleanup() {
  [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
}
trap cleanup EXIT

say "stub model server"
STUB_LOG="${STUB_LOG:-var/stub-phase6-requests.jsonl}"
rm -f "$STUB_LOG"

if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
  # Already listening: reuse it. It must have been started with the three answers above,
  # in that order, or generation will not parse.
  echo "a stub is already listening on $STUB_PORT; reusing it"
  echo "it must have been started with --answers-file $ANSWERS"
else
  python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" \
    --answers-file "$ANSWERS" > "$TMP/stub.log" 2>&1 &
  STUB_PID=$!
  for _ in $(seq 1 50); do
    if nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null; then
      break
    fi
    sleep 0.2
  done
  cat "$TMP/stub.log"
fi
nc -z 127.0.0.1 "$STUB_PORT" 2>/dev/null || fail "no stub model server on $STUB_PORT"

say "health"
curl -fsS "$BASE/health" > "$TMP/health.json" 2>/dev/null \
  || fail "no server at $BASE. Start the stub with 'just smoke-llm-phase6' and the server with 'just smoke-server-phase6'."
field '["status"]' < "$TMP/health.json" | grep -qx ok || fail "health is not ok"

say "signup and verify ($EMAIL)"
curl -fsS -X POST "$BASE/auth/signup" -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"device\":{\"name\":\"Smoke laptop\",\"platform\":\"macos\"}}" \
  > "$TMP/signup.json"
ACCESS=$(field '["access_token"]' < "$TMP/signup.json")
# Newest first by name: the dev sink writes `<unix-millis>-<local part>.eml`.
MAIL=$(find "$MAIL_DIR" -name '*.eml' -type f | sort | tail -1)
TOKEN=$(grep -o 'token=[A-Za-z0-9_-]*' "$MAIL" | head -1 | cut -d= -f2)
curl -fsS -X POST "$BASE/auth/verify-email" -H 'content-type: application/json' \
  -d "{\"token\":\"$TOKEN\"}" > /dev/null

say "workspace with an Indonesian source"
curl -fsS -X POST "$BASE/workspaces" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"name":"Jaringan Komputer"}' > "$TMP/ws.json"
WS=$(field '["workspace"]["id"]' < "$TMP/ws.json")

DOKUMEN="Three-way handshake adalah proses membuka koneksi TCP antara klien dan server. Klien mengirim paket SYN, server membalas dengan SYN-ACK, lalu klien mengirim ACK. UDP tidak memakai handshake sehingga lebih cepat tetapi tidak menjamin urutan paket. Efisiensi pengiriman dihitung dengan rumus eta = L / (L + H) * 100 persen."
curl -fsS -X POST "$BASE/workspaces/$WS/sources" -H "authorization: Bearer $ACCESS" \
  -F kind=text -F title="Catatan Jaringan" -F "text=$DOKUMEN" > "$TMP/source.json"
SRC=$(field '["source"]["id"]' < "$TMP/source.json")

say "poll to ready"
STATUS=unknown
for _ in $(seq 1 120); do
  curl -fsS "$BASE/sources/$SRC/status" -H "authorization: Bearer $ACCESS" > "$TMP/status.json"
  STATUS=$(field '["status"]' < "$TMP/status.json")
  case "$STATUS" in ready|failed) break ;; esac
  sleep 1
done
cat "$TMP/status.json"
[ "$STATUS" = "ready" ] || fail "the Indonesian source did not reach ready"

# --- the report ------------------------------------------------------------

say "propose an outline (synchronous, 3 credits)"
curl -fsS -X POST "$BASE/workspaces/$WS/reports/outline" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"topic\":\"Jaringan Komputer\",\"style\":\"revisi ujian\",\"length\":\"sedang\",\"source_ids\":[\"$SRC\"]}" \
  > "$TMP/report.json"
REPORT=$(field '["report"]["id"]' < "$TMP/report.json")
[ "$(field '["report"]["status"]' < "$TMP/report.json")" = "draft" ] \
  || fail "an outline must land as draft: $(cat "$TMP/report.json")"
[ "$(field '["report"]["outline"]["sections"][1]["gaps"]' < "$TMP/report.json")" = "True" ] \
  || fail "the gap flag must survive the round trip: $(cat "$TMP/report.json")"
[ "$(field '["report"]["sections"][0]["body_md"]' < "$TMP/report.json")" = "None" ] \
  || fail "a fresh outline has no section text yet"

say "generate section 0 (a job, 6 credits)"
curl -fsS -X POST "$BASE/reports/$REPORT/generate" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{"index":0}' > "$TMP/generate.json"
[ "$(field '["report"]["sections"][0]["generating"]' < "$TMP/generate.json")" = "True" ] \
  || fail "the section must report itself in flight: $(cat "$TMP/generate.json")"

say "poll until the section settles"
SECTION=unknown
for _ in $(seq 1 120); do
  curl -fsS "$BASE/reports/$REPORT" -H "authorization: Bearer $ACCESS" > "$TMP/report-state.json"
  SECTION=$(field '["report"]["sections"][0]["body_md"]' < "$TMP/report-state.json")
  case "$SECTION" in None) ;; *) break ;; esac
  sleep 1
done
[ "$SECTION" != "None" ] || fail "the section never got text: $(cat "$TMP/report-state.json")"
[ "$(field '["report"]["sections"][0]["generating"]' < "$TMP/report-state.json")" = "False" ] \
  || fail "the section must stop reporting itself in flight once it is written"
[ "$(field '["report"]["sections"][1]["body_md"]' < "$TMP/report-state.json")" = "None" ] \
  || fail "generating section 0 must not write section 1"

say "the document is the section under its outline heading"
python3 - "$TMP/report-state.json" <<'PY'
import json, sys

report = json.load(open(sys.argv[1]))["report"]
body = report["body_md"]
assert body, "the report must have a document once a section is written"
assert body.startswith("## Pendahuluan"), body
assert "three-way handshake" in body, body
# The label the model cited is kept verbatim in the Markdown: resolving `[n]` is the
# client's business for a report, and the `.md` export is the document as stored.
assert "[1]" in body, body
assert report["status"] == "draft", report["status"]
print(f"document: {len(body)} characters, first heading {body.splitlines()[0]!r}")
PY

say "export the report as .md"
curl -fsS "$BASE/reports/$REPORT/export?format=md" -H "authorization: Bearer $ACCESS" \
  -o "$TMP/report.md" -D "$TMP/report-md-headers.txt"
grep -qi 'content-type: text/markdown' "$TMP/report-md-headers.txt" \
  || fail "the md export must be Markdown, not JSON: $(cat "$TMP/report-md-headers.txt")"
grep -q 'three-way handshake' "$TMP/report.md" || fail "the exported document is not the report"
# Gate 6, the data half: the document's own sentences are Indonesian, not just the
# source's words. A prompt that dropped `output_language`, or a section composed from an
# English template, leaves the student a report in the wrong language while every English
# test stays green — which is the failure this gate exists for.
grep -q 'UDP tidak memakai handshake sehingga lebih cepat' "$TMP/report.md" \
  || fail "the exported report is not the Indonesian section the model wrote: $(head -c 300 "$TMP/report.md")"
# The plan gate: free gets [md] only, so `pdf` is refused with the plan that has it.
curl -sS "$BASE/reports/$REPORT/export?format=pdf" -H "authorization: Bearer $ACCESS" \
  -o "$TMP/report-pdf.json" -w '%{http_code}' > "$TMP/report-pdf-status.txt"
[ "$(cat "$TMP/report-pdf-status.txt")" = "403" ] \
  || fail "pdf must be plan-gated on free: $(cat "$TMP/report-pdf.json")"
[ "$(field '["code"]' < "$TMP/report-pdf.json")" = "plan_required" ] \
  || fail "the refusal must be plan_required: $(cat "$TMP/report-pdf.json")"
[ "$(field '["min_plan"]' < "$TMP/report-pdf.json")" = "pro" ] \
  || fail "the refusal must name the plan that has pdf: $(cat "$TMP/report-pdf.json")"
printf 'exported %s bytes as .md; pdf refused with min_plan=pro\n' "$(wc -c < "$TMP/report.md" | tr -d ' ')"

# --- the cheatsheet --------------------------------------------------------

say "create a cheatsheet from the Indonesian source"
curl -fsS -X POST "$BASE/workspaces/$WS/cheatsheets" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' \
  -d "{\"scope\":\"UTS Jaringan Komputer\",\"exam_date\":\"2026-11-01\",\"depth\":\"normal\",\"source_ids\":[\"$SRC\"]}" \
  > "$TMP/cheatsheet.json"
SHEET=$(field '["cheatsheet"]["id"]' < "$TMP/cheatsheet.json")
[ "$(field '["cheatsheet"]["status"]' < "$TMP/cheatsheet.json")" = "generating" ] \
  || fail "a fresh cheatsheet must be generating: $(cat "$TMP/cheatsheet.json")"

say "poll until the cheatsheet settles"
SHEET_STATUS=unknown
for _ in $(seq 1 120); do
  curl -fsS "$BASE/cheatsheets/$SHEET" -H "authorization: Bearer $ACCESS" > "$TMP/sheet-state.json"
  SHEET_STATUS=$(field '["cheatsheet"]["status"]' < "$TMP/sheet-state.json")
  case "$SHEET_STATUS" in ready|failed) break ;; esac
  sleep 1
done
cat "$TMP/sheet-state.json"
[ "$SHEET_STATUS" = "ready" ] \
  || fail "the cheatsheet did not reach ready (status $SHEET_STATUS): $(field '["cheatsheet"]["error"]' < "$TMP/sheet-state.json")"
[ "$(field '["cheatsheet"]["template_version"]' < "$TMP/sheet-state.json")" = "cheatsheet.v1" ] \
  || fail "the template version must be recorded: $(cat "$TMP/sheet-state.json")"

say "the HTML document is served as a document"
curl -fsS "$BASE/cheatsheets/$SHEET/html" -H "authorization: Bearer $ACCESS" \
  -o "$TMP/sheet.html" -D "$TMP/sheet-html-headers.txt"
grep -qi 'content-type: text/html' "$TMP/sheet-html-headers.txt" \
  || fail "the html route must answer a document: $(cat "$TMP/sheet-html-headers.txt")"
grep -q 'Cheatsheet UTS Jaringan Komputer' "$TMP/sheet.html" \
  || fail "the document does not carry its title"
grep -q 'three-way handshake' "$TMP/sheet.html" || fail "the document does not carry its content"
# Gate 6, the data half, on the document before the print: the cheatsheet's *own* text
# (the model's words) and the template's chrome (the fixed labels the renderer writes) are
# both Indonesian. The chrome is the half that catches a template rendered in the wrong
# language — the student's document would say "Table of Contents" while the UI around it
# stayed Indonesian. HTML comments are stripped first, because the template's own comments
# are English prose about the template (`Chapter page break: chapters start on …`) and a
# grep for the word would fail on a comment rather than on the document.
python3 - "$TMP/sheet.html" <<'PY'
import re, sys

html = open(sys.argv[1], encoding="utf-8").read()
document = re.sub(r"<!--.*?-->", "", html, flags=re.S)
for phrase in (
    "Ringkasan materi ujian",   # the cover subtitle, from the model's JSON
    "Daftar Isi",               # the template's table-of-contents heading
    "Ukuran payload",           # the formula legend, from the model's JSON
    "Efisiensi pengiriman",     # the formula's caption, from the model's JSON
    "SYN dari klien",           # a bullet, from the model's JSON
    "belum tercakup",           # the template's honesty note under the cover
    "Halaman",                  # the running footer's own words
):
    assert phrase in document, f"the document is not Indonesian: {phrase!r} is missing"
# The footer's page numbers are a CSS counter (`counter(pages)`) that only the print
# resolves, so the HTML is asserted for the counter and the PDF for the number it became.
assert "counter(pages)" in document, "the footer lost its page counter"
for english in ("Table of Contents", "Page 1 of", "Summary"):
    assert english not in document, f"the document carries English chrome: {english!r}"
print("the HTML document's text and chrome are Indonesian")
PY
# The formula reached the page as LaTeX for KaTeX to render, and the template's own
# markers are filled.
grep -q 'frac' "$TMP/sheet.html" || fail "the formula is not on the page"
for marker in '{{doc_title}}' '{{content}}' '{{toc}}' '{{cover_eyebrow}}'; do
  grep -q "$marker" "$TMP/sheet.html" && fail "the template marker $marker is unfilled"
done
printf 'document: %s bytes, %s\n' "$(wc -c < "$TMP/sheet.html" | tr -d ' ')" \
  "$(grep -o '<title>[^<]*</title>' "$TMP/sheet.html" | head -1)"

say "re-render from the stored blocks (free, no model call)"
CALLS_BEFORE=$(wc -l < "$STUB_LOG" 2>/dev/null | tr -d ' ' || echo 0)
curl -fsS -X POST "$BASE/cheatsheets/$SHEET/re-render" -H "authorization: Bearer $ACCESS" \
  -H 'content-type: application/json' -d '{}' > "$TMP/rerender.json"
curl -fsS "$BASE/cheatsheets/$SHEET/html" -H "authorization: Bearer $ACCESS" -o "$TMP/sheet-again.html"
cmp -s "$TMP/sheet.html" "$TMP/sheet-again.html" \
  || fail "a re-render must be byte-identical: the content did not change"
CALLS_AFTER=$(wc -l < "$STUB_LOG" 2>/dev/null | tr -d ' ' || echo 0)
[ "$CALLS_BEFORE" = "$CALLS_AFTER" ] \
  || fail "a re-render called the model: $CALLS_BEFORE -> $CALLS_AFTER requests"
printf 're-render identical, model calls unchanged at %s\n' "$CALLS_AFTER"

say "print the cheatsheet as PDF"
# The PDF is plan-gated, so this asserts the refusal first and then, when the server has a
# renderer, the document itself. A run whose plan allows pdf but whose server has no
# Chromium is a failure, not a skip: that is exactly the "works on a laptop, fails on
# deploy" risk the phase names.
curl -sS "$BASE/cheatsheets/$SHEET/pdf" -H "authorization: Bearer $ACCESS" \
  -o "$TMP/sheet-pdf-refused.json" -w '%{http_code}' > "$TMP/sheet-pdf-status.txt"
PDF_STATUS=$(cat "$TMP/sheet-pdf-status.txt")
if [ "$PDF_STATUS" = "403" ]; then
  [ "$(field '["code"]' < "$TMP/sheet-pdf-refused.json")" = "plan_required" ] \
    || fail "the pdf refusal must be plan_required: $(cat "$TMP/sheet-pdf-refused.json")"
  [ "$(field '["min_plan"]' < "$TMP/sheet-pdf-refused.json")" = "pro" ] \
    || fail "the pdf refusal must name min_plan=pro: $(cat "$TMP/sheet-pdf-refused.json")"
  echo "the free plan is refused a PDF, as documented (403 plan_required, min_plan=pro)"

  # Upgrade this account in place, so the print itself is exercised rather than skipped.
  #
  # **Both rows, and the subscription is the one that matters.** `users.plan_id` alone
  # grants nothing: the entitlements engine reads the plan only while the subscription is
  # `active` or in grace (`crates/server/src/entitlements.rs::effective_plan`, A146), so an
  # account whose `plan_id` was set without a live `subscriptions` row is free by that same
  # rule. Writing only the first row left this script upgrading an account and then
  # reporting the PDF route's 403 as a missing renderer — which is what it did until the
  # gate re-ran it.
  if [ -n "$PSQL" ]; then
    if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
      DATABASE_URL="$(grep -E '^DATABASE_URL=' .env | tail -1 | cut -d= -f2-)"
      export DATABASE_URL
    fi
    [ -n "${DATABASE_URL:-}" ] || fail "no DATABASE_URL: set one or provide it in .env"
    "$PSQL" "$DATABASE_URL" -tAc "update users set plan_id = 'pro' where email = '$EMAIL'" > /dev/null
    "$PSQL" "$DATABASE_URL" -tAc "insert into subscriptions
        (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
      select id, 'pro', 'active', 'monthly', now(), now() + interval '30 days'
        from users where email = '$EMAIL'" > /dev/null
    echo "upgraded $EMAIL to pro (plan_id + a live subscription row) so the print can be exercised"
  else
    fail "no psql on PATH, so the plan could not be raised and the print cannot be exercised"
  fi
elif [ "$PDF_STATUS" != "200" ]; then
  fail "GET /cheatsheets/:id/pdf answered $PDF_STATUS: $(cat "$TMP/sheet-pdf-refused.json")"
fi

# The print is its own job, and the cheatsheet is `ready` before it runs: polling on the
# status alone would fetch the PDF while the print is still in flight and report the 409 as
# a missing renderer. So this waits for the artifact the route actually serves — the page
# count is set by the same statement that writes `pdf_key`.
for _ in $(seq 1 120); do
  curl -fsS "$BASE/cheatsheets/$SHEET" -H "authorization: Bearer $ACCESS" > "$TMP/sheet-final.json"
  PAGES=$(field '["cheatsheet"]["page_count"]' < "$TMP/sheet-final.json")
  [ "$PAGES" != "None" ] && break
  sleep 1
done
PAGES=$(field '["cheatsheet"]["page_count"]' < "$TMP/sheet-final.json")
[ "$PAGES" != "None" ] \
  || fail "the print never produced a PDF (error: $(field '["cheatsheet"]["error"]' < "$TMP/sheet-final.json")). Is the server running with CHROME_PATH=$CHROME_PATH, and does it have a Chromium renderer?"

curl -fsS "$BASE/cheatsheets/$SHEET/pdf" -H "authorization: Bearer $ACCESS" -o "$TMP/sheet.pdf" \
  || fail "the PDF could not be downloaded even though page_count is set: $(cat "$TMP/sheet-final.json")"
[ "$(head -c 4 "$TMP/sheet.pdf")" = "%PDF" ] || fail "the download is not a PDF"

# The page count is the renderer's, read back and reported — never a number this pipeline
# guessed, because the footer's `Halaman N dari M` is printed from it (decision 10).
printf 'PDF: %s bytes, %s page(s)\n' "$(wc -c < "$TMP/sheet.pdf" | tr -d ' ')" "$PAGES"

say "the PDF's own text: the footer, the header, and no raw LaTeX"
if command -v pdftotext > /dev/null 2>&1; then
  pdftotext -layout "$TMP/sheet.pdf" "$TMP/sheet.txt"
  ACTUAL_PAGES=$(grep -c $'\f' "$TMP/sheet.txt" || true)
  [ "$ACTUAL_PAGES" -ge "$PAGES" ] \
    || fail "the PDF has $ACTUAL_PAGES pages but the API reports $PAGES"
  python3 - "$TMP/sheet.txt" "$PAGES" <<'PY'
import sys

text = open(sys.argv[1], encoding="utf-8").read()
pages = sys.argv[2]
assert text.strip(), "the extracted text is empty, so nothing was actually checked"
assert "Halaman" in text, f"the footer is not in the PDF: {text[:400]}"
assert f"dari {pages}" in text, f"the footer does not report {pages} pages: {text[:400]}"
assert "Cheatsheet UTS Jaringan Komputer" in text, f"the running header is missing: {text[:400]}"
# The assertion that matters. A broken KaTeX asset or an SRI mistake leaves the maths as
# literal `$...$`: no error, a plausible-looking PDF, and the defect only visible on paper.
assert "$" not in text, f"KaTeX did not render; the maths is literal text: {text[:600]}"

# Gate 6, the half that only the printed document can check. The UI can be Indonesian
# while the document comes back in English — a prompt that dropped `output_language`, or a
# template rendered with English chrome, produces exactly that, and the API's JSON looks
# correct either way. So the assertions are on the *rendered* text: the model's own
# sentences, the template's fixed labels, and the absence of their English counterparts.
for phrase in (
    "Ringkasan materi ujian",   # the cover subtitle, from the model's JSON
    "Daftar Isi",               # the template's table-of-contents heading
    "Ukuran payload",           # the formula legend, from the model's JSON
    "Efisiensi pengiriman",     # the formula's caption, from the model's JSON
    "SYN dari klien",           # a bullet, from the model's JSON
    "belum tercakup",           # the template's honesty note under the cover
):
    assert phrase in text, f"the printed document is not Indonesian: {phrase!r} is missing"
for english in ("Table of Contents", "Page 1 of", "Summary", "Chapter"):
    assert english not in text, f"the printed document carries English chrome: {english!r}"
print(
    f"extracted {len(text)} characters: the footer, the header, no raw $, "
    "and the document's own text and chrome are Indonesian"
)
PY
else
  fail "pdftotext is not installed, so the PDF's text cannot be checked; install poppler"
fi

say "the usage rows name the action, the prompt version and the cost"
if [ -n "$PSQL" ]; then
  ROWS=$("$PSQL" "$DATABASE_URL" -tAc "select ue.action || '|' || coalesce(ue.prompt_version, '') || '|' || case when ue.provider_cost_usd is null then 'no-cost' else 'cost' end from usage_events ue where ue.user_id = (select id from users where email = '$EMAIL') and ue.action in ('report_outline', 'report_section', 'generate_cheatsheet') order by ue.created_at")
  printf '%s\n' "$ROWS"
  printf '%s\n' "$ROWS" | grep -q '^report_outline|report_outline.v1|cost$' \
    || fail "no outline usage row with its prompt version"
  printf '%s\n' "$ROWS" | grep -q '^report_section|report_section.v1|cost$' \
    || fail "no section usage row with its prompt version"
  printf '%s\n' "$ROWS" | grep -q '^generate_cheatsheet|cheatsheet.v1|cost$' \
    || fail "no cheatsheet usage row with its prompt version"
else
  echo "no psql on PATH; skipped the usage-row assertion"
fi

say "the generation called the model with the composed prompt"
python3 - "$STUB_LOG" <<'PY'
import json, pathlib, sys

path = pathlib.Path(sys.argv[1])
if not path.exists():
    print("the stub did not record request bodies; skipping the prompt assertion")
    raise SystemExit(0)

bodies = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
assert len(bodies) >= 3, f"expected at least three model calls, saw {len(bodies)}"

for body in bodies:
    assert body.get("stream") is False, "generation must not stream"
    prompt = body["messages"][0]["content"]
    assert "{{" not in prompt, "no unfilled variable may reach the model"
    assert "<grounding_and_citations>" in prompt, "the grounding section is the guard rail"
    assert "<security_untrusted_content>" in prompt, "the security section is the guard rail"

# Each call carried the Indonesian source under label 1, and each named its own module's
# settings. A prompt rebuilt from a default rather than from the stored row is the failure
# these assertions exist for.
outline, section, cheatsheet = bodies[0], bodies[1], bodies[2]
for prompt in (outline, section, cheatsheet):
    assert "three-way handshake" in prompt["messages"][0]["content"], "the source's text"
    assert "[1]" in prompt["messages"][0]["content"], "the chunk's label"

assert "Jaringan Komputer" in outline["messages"][0]["content"], "the outline's topic"
assert "revisi ujian" in outline["messages"][0]["content"], "the outline's style"
assert "Perbandingan TCP dan UDP" in section["messages"][0]["content"] or \
    "Pendahuluan" in section["messages"][0]["content"], "the section's heading"
assert "UTS Jaringan Komputer" in cheatsheet["messages"][0]["content"], "the cheatsheet's scope"
assert "2026-11-01" in cheatsheet["messages"][0]["content"], "the cheatsheet's exam date"
print(f"{len(bodies)} prompt(s) carried the source, its labels, the settings and both guard rails")
PY

printf '\n=== PASS\n'
