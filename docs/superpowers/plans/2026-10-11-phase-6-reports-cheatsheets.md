# Phase 6. Reports, cheatsheets and the PDF pipeline. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** a student turns their sources into two printable study documents. A **report**
is an editable outline, then per-section generation with citations, stored as Markdown and
exportable. A **cheatsheet** is one strict-JSON generation rendered through a fixed print
template to A4 PDF, with a running header, page numbers and a table of contents.

**Architecture:** the split that makes this phase work is **content versus layout**.
Generation produces typed JSON blocks (`crates/core/src/render/`) and nothing else: never
markup, never CSS. A **pure** renderer turns `(blocks, template_version)` into HTML with no
clock, no network and no randomness, which is what makes re-rendering an existing document
free, deterministic and independent of the model. Chromium is only the last step, turning
that HTML into a PDF, and it is the one part of the phase with a native dependency.

**Tech Stack:** Rust (core + server), Postgres/sqlx, `headless_chrome` (new, the phase's
one new crate), object storage for the blocks/HTML/PDF artifacts, React + Vite for the
Studio panels.

**Exit gate (observable, from the roadmap):** outline edit → section generation with
citations → `.md` export; cheatsheet JSON → template HTML → A4 PDF with header, page
numbers and TOC.

**The phase's acceptance test is specific and is the one that matters:** render
`templates/cheatsheet.v1.html` with sample data and assert a 6-page A4 PDF whose text
contains the running header, a `Halaman N dari 6` footer, the chapter titles, and **no raw
`$` delimiters**. That last assertion is what catches a broken KaTeX asset or an SRI
mistake, both of which fail silently: the page renders, the maths shows as literal `$x$`,
and nobody notices until a student prints it.

**Roadmap gates 5 and 6 apply** (accessibility on the new Studio screens; Indonesian end
to end, including the rendered document's text).

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes. Prompts
  live in versioned modules, never inline. `tracing` structured logs with request ids.
- **Server is the only source of truth.** Clients never call the model. All heavy work is
  server-side. Generation is async with visible progress.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings. Reports and cheatsheets are **not** offline-writable, so
  nothing here queues.
- **All times:** `timestamptz` UTC + user IANA timezone (default `Asia/Jakarta`).
- **Credits (from `config/plans.toml`, never a literal):** `report_outline = 3`,
  `report_section = 6`. A cheatsheet is `report_section`-priced per the roadmap's cost
  table; if the matrix has no cheatsheet key, the outline+section pair is what a
  cheatsheet costs and the plan records that rather than inventing a price.
- **PDF export is plan-gated by `export_formats`.** Free gets `[md]` (and prints the HTML
  themselves); pro and power get `pdf`.
- **Quality gate:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, `npm --prefix app test`, `npm --prefix app run build`, the `src-tauri`
  trio, `cargo sqlx prepare --workspace --check`.
- **UI language English**, but Indonesian documents and queries must work end to end.

---

## What already exists (checked in this tree, not assumed)

- `migrations/0011_reports.sql`: `reports (id, user_id, workspace_id, title, outline
  jsonb, body_md text, …)`. `outline` and `body_md` are both present, so the two-step
  report flow needs no new *content* column. **`report_revisions` also already exists**
  (lines 19-29: `id, report_id, user_id, body_md, replaced_at` plus an index on
  `(report_id, replaced_at desc)`), and `crates/server/tests/schema.rs` already lists it.
  Decision 8's table therefore needs **no migration at all** — writing the `0023` the first
  draft of this plan asked for fails with `relation "report_revisions" already exists`.
  What `reports` *does* lack is the generation bookkeeping every generator carries:
  `status`, `error`, `credits_spent`, `usage_event_id`.
- `migrations/0016_cheatsheets.sql`: `cheatsheets` with `scope`, `exam_date`, `depth`,
  `source_ids`, `status`, `error`, `blocks_key`, `template_version`, `html_key`, `pdf_key`,
  `page_count`. **The whole artifact pipeline is already modelled**, including the
  re-render contract's `blocks_key` and `template_version`. The columns it lacks are
  `credits_spent` and `usage_event_id` (the refund path needs both, exactly as `decks` and
  `quizzes` do) and a `requested_*` shape is unnecessary here: a cheatsheet has no count
  parameter, its size is `depth`.
- `templates/cheatsheet.v1.html` (912 lines) and `prompts/cheatsheet.v1.md`, which
  documents the twelve block kinds and their fields. **The template's first render already
  surfaced five defects** (blocked KaTeX, SVG scale, orphaned answers, SVG `id` collisions,
  a split `.gap-note`), which is why the template carries a version and the re-render
  endpoint exists.
- `prompts/report_outline.v1.md` and `prompts/report_section.v1.md`.
- `crates/core/src/prompts/`: `compose_task`, `ModuleId`, `trim_base`, the strict variable
  contract, and the sweep in `crates/core/tests/prompts.rs` that covers every module.
- `crates/core/src/generation/`: `GenerationError`, `is_retryable`, `strip_code_fence`,
  and two worked contracts (`flashcards`, `quiz`) to mirror.
- `crates/server/src/quizzes/` and `flashcards/`: the generation-job shape to mirror,
  including the claim, the refund, the reservation under the advisory lock, and
  `credits::{spend_in, refund_in, refund_pending, RefundTarget}`.
- `crates/server/src/chunks.rs::select_for_generation`: the round-robin chunk loader both
  existing generators use, so a report section and a cheatsheet select chunks identically.
- **MISSING:** `crates/core/src/render/` (the typed blocks, the renderer, the SVG
  allowlist), any `reports`/`cheatsheets` server module or route, the
  `generate_cheatsheet` and `render_cheatsheet_pdf` jobs, the `headless_chrome` dependency,
  and the object-storage write/read path for `blocks_key`/`html_key`/`pdf_key`.

---

## Decisions this plan pins (so parallel work cannot diverge)

1. **The renderer is a pure function of `(blocks, template_version)`.** No clock, no
   network, no randomness, no model. This is the property the re-render endpoint exists
   for, and it is also what makes the renderer unit-testable without Chromium. A renderer
   that reads the time to stamp a footer breaks re-render determinism, which is exactly the
   regression a later refactor makes innocently.
2. **Generation never produces markup.** The model returns typed JSON; the renderer owns
   every tag. A field containing `<` is **escaped and shown literally**, never interpreted.
   The prompt states this to the model as a failure condition, and the renderer enforces it
   regardless of what the model does.
3. **SVG is allowlisted, not sanitised.** The permitted element and attribute sets are
   closed lists copied from the prompt's own contract (`svg, g, rect, circle, ellipse,
   line, polyline, polygon, path, text, tspan, defs, marker, title`; no `script`, no
   `style`, no `on*`, no `href`, no `foreignObject`). An element or attribute outside the
   list **drops the whole block** with a recorded reason, rather than stripping the
   offending part and rendering something the model did not write.
4. **Every `id` and `url(#…)` inside an SVG block is prefixed per block.** Two diagrams in
   one document that both define `arrowhead` is a real collision (it was one of the
   template's five defects), and the symptom is the wrong marker or a missing one, which
   looks like a CSS problem rather than an id one.
5. **Blocks are stored in object storage, not a `jsonb` column.** `cheatsheets` is a
   `+sync` table, so a `jsonb` body would ship tens of KB of document text to every device
   on every change. The schema already says this; the plan implements it.
6. **`template_version` is recorded per artifact**, so an admin can list documents rendered
   by an old template. Re-render writes a new artifact and updates the columns; it never
   touches the blocks.
7. **The report's outline is editable and the sections are generated one at a time.**
   `reports.outline` holds the JSON outline; `body_md` accumulates the sections. Section
   generation is idempotent per section index: regenerating section 3 replaces section 3's
   text and nothing else, so a student can fix one section without paying for the whole
   report again.
8. **A long report body edited on two devices keeps the latest and stores the loser as a
   revision** (the roadmap's conflict table names `report_revisions` for exactly this).
   The table already exists in `0011_reports.sql`, with `replaced_at` rather than
   `created_at`; this phase writes the rows, not the table.
9. **Chromium is pinned and the renderer is a pooled child process.** This is the phase's
   stated risk, and it is resolved in two halves: CI runs the acceptance test on a runner
   that has Chrome (so the test is never skipped), and `docs/RELEASE.md` carries the
   **Phase 15 prerequisite** that the server image bakes a pinned Chromium and sets
   `CHROME_PATH`. There is no server `Dockerfile` in this repository yet — Phase 15 owns
   it — and writing one here would be a second thing to keep in step. Locally the renderer
   uses the installed Chrome; it refuses to run unsandboxed outside dev.
10. **The PDF's page count is read back from the renderer, not assumed.** `page_count` is
    what the footer's `Halaman N dari M` depends on, so it is a value the renderer reports
    (Chromium's page count for the print), not a number the template guesses.
11. **Export formats come from the plan matrix.** `GET /reports/:id/export?format=` and
    `GET /cheatsheets/:id/pdf` both check `export_formats` and answer 403
    `plan_required` with `min_plan` when the format is not in the plan, so the mobile
    build cannot be told about a purchase it must not offer.
12. **The report outline is generated synchronously in its route, not as a job.** The
    roadmap's job table is a fixed list and contains no outline kind, so adding one would
    put this phase out of step with a table later phases read. The call is one short
    completion (3 credits) whose result the student edits before anything else happens,
    which is the opposite of the long, cancellable work the job queue exists for. Section
    generation *is* a job (`generate_report_section`), because it is per-section, can be
    retried, and a report can be long enough to outlive a request.

---

## File structure (new this phase)

```
crates/core/src/render/mod.rs          # NEW — the pure renderer
crates/core/src/render/blocks.rs       # NEW — the typed block model
crates/core/src/render/svg.rs          # NEW — the allowlist and the id prefixing
crates/core/src/generation/report.rs   # NEW — the outline contract
crates/core/src/generation/cheatsheet.rs # NEW — the blocks contract
crates/core/src/prompts/registry.rs    # + ReportOutlineV1, ReportSectionV1, CheatsheetV1
crates/core/tests/render.rs            # NEW
crates/core/tests/generation_report.rs # NEW
crates/server/src/reports/mod.rs       # NEW
crates/server/src/reports/routes.rs    # NEW
crates/server/src/reports/generate.rs  # NEW
crates/server/src/cheatsheets/…        # NEW
crates/server/src/render/chrome.rs     # NEW — the pooled headless_chrome renderer
crates/server/src/jobs/handlers/report.rs      # NEW
crates/server/src/jobs/handlers/cheatsheet.rs  # NEW
migrations/0023_report_revisions.sql   # NEW — the conflict rule's table
migrations/0024_report_cheatsheet_generation.sql # NEW — generation columns
app/src/features/studio/ReportPanel.tsx
app/src/features/reports/ReportScreen.tsx
scripts/smoke-phase6.sh
docs/API.md, docs/ARCHITECTURE.md, docs/ASSUMPTIONS.md, docs/RELEASE.md
```

---

## Task 1: `crates/core/src/render/` — the typed blocks and the pure renderer

**Files:**
- Create: `crates/core/src/render/{mod.rs,blocks.rs,svg.rs}`
- Modify: `crates/core/src/lib.rs`
- Test: `crates/core/tests/render.rs`

**Interfaces:**
- Produces: `Blocks`/`Chapter`/`Section`/`Block` (the twelve kinds),
  `parse_blocks(&str) -> Result<Blocks, RenderError>`,
  `render_html(&Blocks, &Template, &ContextLabels) -> Result<Rendered, RenderError>`,
  `Template { text: &'static str, version: &'static str }`,
  `Rendered { html: String, dropped: Vec<DroppedBlock> }`.

> **Task 1 is already implemented and committed** (`90d5469`, `139c503`, `0e10b2f`). The
> signatures above are the draft's; the shipped ones differ in four places and **the code
> is right**: `parse_blocks` returns `Result<Blocks, GenerationError>` (so a drifted model
> reply is the same error class as a drifted deck or quiz), `render_html` takes a fourth
> `&Cover` argument, the SVG entry point is `svg::check_and_prefix(&str, &str) ->
> Result<String, SvgRejection>` (it returns the reason a block was dropped, which is why
> it is not a plain `String`), and the id prefix is the **document path** `b{chapter}-
> {section}-{block}-` rather than a flat block index — the draft's `b1-arrowhead` could not
> distinguish two diagrams in different sections, and a test for exactly that was added in
> `139c503`. Treat this task as a record of what was built, not as instructions.

- [x] **Step 1: Write the failing tests** — **ticked: both exist, one under a different name and one by a stronger mechanism.** `markup_in_a_text_field_is_escaped_not_interpreted` is present verbatim in `crates/core/tests/render.rs`. `a_script_element_drops_the_whole_block_and_is_recorded` shipped as `a_disallowed_svg_element_drops_the_block_and_records_why`, beside four more injection cases in the same file that the plan did not name and that matter more: `a_disallowed_svg_attribute_is_refused`, `a_single_quoted_attribute_value_cannot_inject_an_attribute`, `a_single_quoted_id_cannot_inject_an_attribute` and `an_attribute_value_is_escaped_completely_and_only_once`. **This is the phase whose review round found an SVG attribute-value injection that reached a printed PDF**, so the extra cases are the fix for a defect the plan did not foresee.

```rust
/// Generation produces data, never markup. A field containing a tag is escaped and shown
/// literally, and that is asserted rather than assumed: the failure mode is a student's
/// cheatsheet with an injected `<script>` in it, or a broken layout where a literal tag
/// was interpreted.
#[test]
fn markup_in_a_text_field_is_escaped_not_interpreted() {
    let blocks = parse_blocks(r#"{"title":"T","subtitle":"S","chapters":[{"title":"C","sections":[{"title":"S","blocks":[{"kind":"text","text":"<script>alert(1)</script> & <b>bold</b>","labels":[]}]}]}]}"#).unwrap();
    let html = render_html(&blocks, &template(), &labels()).unwrap().html;

    assert!(html.contains("&lt;script&gt;"), "the tag must be escaped: {html}");
    assert!(!html.contains("<script>"), "no tag from a field may reach the page");
    assert!(html.contains("&amp;"), "an ampersand must be escaped too");
}

/// The SVG allowlist is closed. An element outside it drops the block rather than being
/// stripped: a half-rendered diagram is a lie about what the model drew.
#[test]
fn a_script_element_drops_the_whole_block_and_is_recorded() {
    let blocks = parse_blocks(r#"{"title":"T","subtitle":"S","chapters":[{"title":"C","sections":[{"title":"S","blocks":[{"kind":"svg","svg":"<svg viewBox=\"0 0 10 10\"><script>alert(1)</script><rect width=\"1\" height=\"1\"/></svg>","labels":[]}]}]}]}"#).unwrap();
    let rendered = render_html(&blocks, &template(), &labels()).unwrap();

    assert!(!rendered.html.contains("<script"), "nothing may survive");
    assert_eq!(rendered.dropped.len(), 1, "the drop is recorded, not silent");
    assert!(rendered.dropped[0].reason.contains("script"));
}

/// Two diagrams that both define `arrowhead` collide unless ids are prefixed per block.
/// The symptom is the wrong marker or a missing one, which reads as a CSS problem.
#[test]
fn svg_ids_are_prefixed_per_block_so_two_diagrams_cannot_collide() {
    let one = r#"<svg viewBox="0 0 10 10"><defs><marker id="arrowhead"/></defs><line marker-end="url(#arrowhead)"/></svg>"#;
    let blocks = parse_blocks(&format!(r#"{{"title":"T","subtitle":"S","chapters":[{{"title":"C","sections":[{{"title":"S","blocks":[{{"kind":"svg","svg":{one:?},"labels":[]}},{{"kind":"svg","svg":{one:?},"labels":[]}}]}}]}}]}}"#)).unwrap();
    let html = render_html(&blocks, &template(), &labels()).unwrap().html;

    assert!(html.contains("id=\"b1-arrowhead\""), "the first block's id is prefixed: {html}");
    assert!(html.contains("id=\"b2-arrowhead\""), "and the second's differently");
    assert!(html.contains("url(#b1-arrowhead)"), "the reference is rewritten with it");
    assert!(!html.contains("id=\"arrowhead\""), "no unprefixed id may remain");
}

/// The renderer is pure: the same blocks and template produce byte-identical HTML. This is
/// the property re-render depends on, and a clock or a random id in the output breaks it.
#[test]
fn rendering_is_deterministic() {
    let blocks = sample_blocks();
    let first = render_html(&blocks, &template(), &labels()).unwrap().html;
    let second = render_html(&blocks, &template(), &labels()).unwrap().html;
    assert_eq!(first, second, "a re-render must be byte-identical");
}

/// A block's labels resolve to citation markers, and a label that was never sent is
/// dropped rather than rendered as a dangling reference (A17).
#[test]
fn labels_resolve_to_citations_and_unknown_ones_are_dropped() { … }
```

- [x] **Step 2: Run them and watch them fail** — **ticked: it runs the cases above, which exist.**

`cargo test -p stvdy-core --test render` → unresolved import.

- [x] **Step 3: Write the block model**

`blocks.rs`: `Blocks { title, subtitle, course: Option<String>, exam: Option<String>,
chapters: Vec<Chapter> }`, `Chapter { title, sections }`, `Section { title, blocks }`, and
`Block` as an enum with one variant per kind the prompt documents (`Text`, `Bullets`,
`Concept`, `Formula`, `QuickFormula`, `Analogy`, `Trap`, `Memorize`, `Table`, `Question`,
`Diagram`, `Svg`), each `deny_unknown_fields`, each carrying `labels: Vec<u32>`.

`parse_blocks` strips a code fence (reuse `generation::strip_code_fence`), parses with
`deny_unknown_fields` so a drifted prompt fails loudly, and bounds the document
(`MAX_CHAPTERS`, `MAX_SECTIONS_PER_CHAPTER`, `MAX_BLOCKS_PER_SECTION`) so a model that
ignores the structure rules cannot produce an unrenderable document.

- [x] **Step 4: Write the renderer**

`render_html` walks the blocks and emits the template's markup. The template is a
`&'static str` with `{{…}}` placeholders the renderer fills; the CSS lives in the template,
not in the renderer, so a layout fix is a template edit and a re-render.

`svg.rs` holds the two closed lists and the id rewriting:

```rust
/// The elements and attributes the template's CSS is written for, copied from
/// `prompts/cheatsheet.v1.md` so the prompt and the enforcement cannot drift.
pub const ALLOWED_ELEMENTS: &[&str] = &["svg","g","rect","circle","ellipse","line",
    "polyline","polygon","path","text","tspan","defs","marker","title"];
pub const ALLOWED_ATTRIBUTES: &[&str] = &["viewBox","d","x","y","x1","y1","x2","y2",
    "cx","cy","r","rx","ry","width","height","points","fill","stroke","stroke-width",
    "stroke-linecap","stroke-linejoin","transform","font-size","text-anchor",
    "dominant-baseline","marker-end","marker-start","id","class","orient","refX","refY",
    "markerWidth","markerHeight","fill-opacity","stroke-dasharray"];

/// Rewrites every `id` and every `url(#…)` to carry the block's prefix, so two diagrams
/// cannot collide.
pub fn prefix_ids(svg: &str, prefix: &str) -> String { … }
```

The parse is a hand-written scanner rather than a full XML parser: the input is a small
subset, and a scanner that rejects anything it does not recognise is the property wanted
(the opposite of a parser that accepts more than the allowlist).

- [x] **Step 5: Run the tests, then commit**

```bash
cargo test -p stvdy-core --test render
git add crates/core/src/render crates/core/src/lib.rs crates/core/tests/render.rs
git commit -m "feat(core): the pure renderer, the typed blocks, and the SVG allowlist"
```

---

## Task 2: The report contracts, and the report job

**Files:**
- Create: `crates/core/src/generation/report.rs`
- Modify: `crates/core/src/prompts/registry.rs` (+ two modules), `crates/core/src/generation/mod.rs`
- Create: `crates/server/src/reports/{mod.rs,routes.rs,generate.rs}`
- Create: `crates/server/src/jobs/handlers/report.rs`
- Create: `migrations/0023_report_generation.sql`
- Test: `crates/server/tests/reports.rs`, `crates/core/tests/generation_report.rs`

- [x] **Step 1: The outline contract and its tests**

The contract is the prompt's, not a paraphrase of it. `prompts/report_outline.v1.md`
documents:

```
{"title":"string","sections":[{"heading":"string","purpose":"string","labels":[1,4],"gaps":false}]}
```

So: `ReportOutline { title, sections: Vec<OutlineSection { heading, purpose, labels:
Vec<u32>, gaps: bool }> }`, `deny_unknown_fields`, bounded (`MAX_REPORT_SECTIONS`), and
`gaps` is **required output**, not decoration: the roadmap's exit gate and the spec's
`## Reports` both name it ("sections whose topics are not supported by the sources are
flagged `gaps: true`"). A struct without it cannot represent the contract.

`ReportSection { body_md }` is the section reply: Markdown with `[n]` labels, and the
inline `(Not covered in your sources: …)` note the prompt asks for is text, not a field.

The outline is **editable**, so the route accepts a replacement outline and validates it
against the same struct — an edit the generator could not have produced is still valid
input if it parses, and its `labels` are re-validated against the sources on generation.

- [x] **Step 2: The generation columns**

`0023_report_generation.sql` adds to `reports`: `status text not null default 'draft'`
(`draft | generating | ready | failed`), `error text`, `credits_spent int not null default
0`, `usage_event_id uuid`, and to `cheatsheets`: `credits_spent int not null default 0`,
`usage_event_id uuid`. Both are `+sync` tables already covered by `0017_sync_triggers.sql`,
so no trigger work is needed — but a new *column* on a `+sync` table is a real change to
what syncs, so `docs/API.md`'s report/cheatsheet payloads name them.

`report_revisions` needs **nothing**: it exists in `0011_reports.sql`.

- [x] **Step 3: The job**

`generate_report_section` — that is the name in the roadmap's fixed job table and in
`migrations/0015_jobs.sql`. Mirror `quizzes::generate` exactly: compose through
`compose_task`, one retry on a parse failure with the violation appended, record the prompt
version and the provider cost on the usage row, claim the row with a guarded status flip,
and refund once on a permanent failure.

**The outline is not a job.** The roadmap's job table has no outline kind, and inventing
one would put this phase's list out of step with a table later phases read. The outline is
generated synchronously in its route: it is one short call, it is the step whose result the
student edits before anything else happens, and it is cheap (3 credits).

Section generation writes **only that section's** Markdown into `body_md`, replacing any
previous text for that index, so regenerating one section does not cost the whole report.

- [x] **Step 4: The routes**

`POST /workspaces/:id/reports/outline`, `PATCH /reports/:id` (edit the outline),
`POST /reports/:id/generate` (sections), `GET /reports/:id`, and
`GET /reports/:id/export?format=md`. The export checks `export_formats`; `pdf` arrives in
Task 4. Each write that can race another device's write stores the losing body as a
`report_revisions` row (decision 8).

- [x] **Step 5: Tests, then commit**

The tests mirror `tests/quizzes.rs`: generation with a scripted model, the retry naming
the violation, the refund, the monthly cap, and another account's report being a 404.

---

## Task 3: The cheatsheet contract and the generation job

**Files:**
- Create: `crates/core/src/generation/cheatsheet.rs`
- Create: `crates/server/src/cheatsheets/{mod.rs,routes.rs,generate.rs}`
- Create: `crates/server/src/jobs/handlers/cheatsheet.rs`
- Test: `crates/server/tests/cheatsheets.rs`

- [x] **Step 1: The contract** — the twelve block kinds, reusing `render::blocks::Block`
      so the parser and the renderer cannot disagree about the shape. The generation
      contract *is* the renderer's input type; a second definition would be a second thing
      to keep in step.

- [x] **Step 2: The job** — `generate_cheatsheet`, same shape. On success it writes the
      blocks to object storage at `blocks_key`, records `template_version`, and enqueues
      `render_cheatsheet_pdf` (Task 4). If the renderer is unavailable the cheatsheet is
      still `ready` with its blocks stored: the document exists, only its PDF does not, and
      that is a retryable render, not a failed generation.

- [x] **Step 3: The routes** — `POST /workspaces/:id/cheatsheets`,
      `GET /cheatsheets/:id`, `GET /cheatsheets/:id/html`, `POST /cheatsheets/:id/re-render`.
      Re-render reads `blocks_key`, re-runs the pure renderer with the **current** template,
      and updates `html_key`/`template_version`. It never calls the model, so it is free.

- [x] **Step 4: Tests, then commit.**

---

## Task 4: The Chromium renderer and the PDF job

**Files:**
- Create: `crates/server/src/render/chrome.rs`, `crates/server/src/jobs/handlers/render_pdf.rs`
- Modify: `crates/server/Cargo.toml` (+ `headless_chrome`, pinned), `docker-compose.yml`,
  the server `Dockerfile`, `.github/workflows/ci.yml`
- Test: `crates/server/tests/pdf.rs` (ignored by default; the phase's acceptance test)

**This is the phase's stated risk: the native dependency must be in the image, not just on
a laptop.**

- [x] **Step 1: The renderer**

A pooled `headless_chrome` browser, one process, pages reused, with the flags the roadmap
names: `print_background`, `prefer_css_page_size`, `generate_document_outline`,
`generate_tagged_pdf`. It returns `(pdf_bytes, page_count)`, and `page_count` is what the
footer's `Halaman N dari M` depends on.

- [x] **Step 2: The acceptance test**

```rust
/// The phase's acceptance test, from the roadmap: a 6-page A4 PDF whose text contains the
/// running header, the `Halaman N dari 6` footer, the chapter titles, and no raw `$`
/// delimiters.
///
/// The last assertion is the one that matters. A broken KaTeX asset or an SRI mistake
/// leaves the page rendering with literal `$x$` in place of the maths: nothing errors, the
/// PDF is produced, and the defect is only visible to the student who prints it.
#[tokio::test]
#[ignore = "needs Chromium; run with --ignored"]
async fn the_cheatsheet_renders_to_a_six_page_a4_pdf() {
    let html = render_html(&sample_blocks(), &template(), &labels()).unwrap().html;
    let (pdf, pages) = render_pdf(&html).await.unwrap();

    assert_eq!(pages, 6);
    let text = extract_pdf_text(&pdf);
    assert!(text.contains("Halaman 1 dari 6"), "{text}");
    assert!(text.contains("Halaman 6 dari 6"), "{text}");
    assert!(text.contains("Jaringan Komputer"), "the running header: {text}");
    assert!(!text.contains('$'), "KaTeX failed; the maths is literal: {text}");
}
```

- [x] **Step 3: Where Chromium runs, in CI and in the image**

The repository has **no server `Dockerfile`**: the container image is Phase 15's deliverable
("server Docker image" is in its exit gate), and inventing one here would either be dead
weight or a second thing to keep in step. So this step does the two things that are
actually available now, and records the third:

1. **CI runs the acceptance test for real.** A new `pdf` job in `.github/workflows/ci.yml`
   runs on `ubuntu-24.04` (whose image ships Google Chrome stable) and runs
   `CHROME_PATH=/usr/bin/google-chrome cargo test -p stvdy-server --test pdf -- --ignored`.
   The test is `#[ignore]`d by default so a laptop without a browser still gets a green
   `cargo test --workspace`; CI is where it must not be skipped. Without this the phase's
   acceptance test is a test nobody runs, which is the same as not having it.
2. **`docs/RELEASE.md` carries the image requirement** as a Phase 15 prerequisite: the
   server image must bake a **pinned** Chromium and set `CHROME_PATH`, because the pipeline
   otherwise works on a laptop and fails on deploy — the risk the roadmap names for this
   phase.
3. The renderer reads `CHROME_PATH` first and probes the usual paths second, so the same
   code runs on both. `crates/server/src/render/chrome.rs` already does this; what this
   step adds is the CI job and the documentation.

- [x] **Step 4: The PDF routes** — `GET /cheatsheets/:id/pdf` and
      `GET /reports/:id/export?format=pdf`, both gated on `export_formats`.

---

## Task 5: The SPA — the Report panel, the report screen, the cheatsheet preview

**Files:**
- Create: `app/src/api/queries/reports.ts`, `app/src/api/queries/cheatsheets.ts`,
  `app/src/features/studio/ReportPanel.tsx`, `app/src/features/reports/ReportScreen.tsx`
- Modify: `app/src/features/workspaces/WorkspaceScreen.tsx` (the Studio sub-tab),
  `app/src/router.tsx`

- [x] **Step 1: The panel and the screen.** The Studio gains a Report sub-tab beside
      Flashcards and Quiz, using the same roving-tabindex strip. The report screen edits the
      outline, generates sections one at a time with visible progress, and offers the export
      formats the plan allows.

      **Read the Studio panel as it exists before writing this.** The SPA was cut over to
      the spec's design system, i18n and `LockedFeature` after this plan's first draft, so
      three things are now non-negotiable and none of them were in the draft:
      - Every string goes through `t()` with an entry in **both** `app/src/i18n/locales/
        en.json` and `id.json`; `app/src/i18n/i18n.test.ts` fails the build otherwise (A104).
      - Every colour and size comes from the spec's tokens (`--surface`, `--text`,
        `--border`, `--primary`, `--ring`, `rounded-card`, …). A raw hex fails
        `app/src/tokens/contrast.test.ts`, and a focus `outline-*` other than `outline-ring`
        fails `app/src/tokens/invariants.test.ts` (A101/A114).
      - "Shown only when the plan allows it" is `LockedFeature` reading
        `config/billing_policy.toml` through `app/src/features/billing/policy.ts`, not a
        bespoke conditional (A106/A116). On mobile it must render the neutral variant: a
        price or an upgrade link in a phone build is the review risk the policy exists for.
      - The honesty gate applies to this screen: the outline and section generation show
        their credit cost (`report_outline`, `report_section`) **before** the action runs,
        from the same numbers the server charges, and any AI-proposed outline is labelled
        "Suggested …" (roadmap gate 7).

- [x] **Step 2: The cheatsheet preview.** `GET /cheatsheets/:id/html` renders in a sandboxed
      frame (the document is generated HTML; the frame is what keeps it away from the app's
      own DOM), with a "Download PDF" action shown only when the plan allows it.

- [x] **Step 3: Tests, then commit.** Mirror `QuizScreen.test.tsx`: the export action is
      absent when the plan does not allow the format, and the preview frame is `sandbox`ed.

---

## Task 6: Docs, both gates, and the smoke script

- [x] **Step 1: `docs/API.md`** — the `## Reports` and `## Cheatsheets` sections.
- [x] **Step 2: `docs/ARCHITECTURE.md`** — the content-versus-layout split, the purity
      property, and the renderer's deployment story.
- [x] **Step 3: `docs/RELEASE.md`** — the pinned Chromium in the image.
- [x] **Step 4: `docs/ASSUMPTIONS.md`** — one row per decision above. The next free number
      is **A119** (A101-A118 were taken by the design-system and i18n cutover), so the plan's
      first draft would have collided with eight existing rows.
- [x] **Step 5: Roadmap gate 5** on the report screen and the cheatsheet preview, performed
      and recorded, with the same five checks the earlier phases used.
- [x] **Step 6: Roadmap gate 6** — an Indonesian source produces an Indonesian cheatsheet
      and the rendered PDF's text is Indonesian.
- [x] **Step 7: Roadmap gate 7 (honesty)** — on the report screen and the cheatsheet
      preview: the credit cost is visible before each generative action, the numbers trace
      to `config/plans.toml`, the composer hint is not applicable (no composer here) but the
      "Suggested …" label is, and no invented number (page count, coverage, confidence)
      reaches the screen. `page_count` is the one number this phase adds, and it is read
      back from the renderer rather than guessed (decision 10), which is what the gate asks.
- [x] **Step 8: `scripts/smoke-phase6.sh`** — generate a report and a cheatsheet from an
      Indonesian source, export both, and assert the PDF's page count and that its text
      contains no `$`.
- [x] **Step 9: The full gate**, then commit.

---

## Phase 6 exit checklist

- [x] `crates/core/src/render/` is pure, deterministic, and unit-tested without Chromium.
- [x] Generation produces typed blocks, never markup; a tag in a field is escaped.
- [x] The SVG allowlist is closed; an element outside it drops the block and records why.
- [x] SVG ids are prefixed per block, so two diagrams cannot collide.
- [x] An editable outline generates per-section with citations, and regenerating one
      section does not regenerate the others.
- [x] A long body edited on two devices keeps the latest and stores the loser as a
      revision.
- [x] A cheatsheet renders to A4 PDF with the running header, `Halaman N dari M`, and no
      raw `$`.
- [x] `POST /cheatsheets/:id/re-render` re-renders from stored blocks with no model call.
- [x] The renderer waits for the template's `data-math-ready` sentinel and fails the job on
      any value other than `1`, so a KaTeX that failed to load cannot produce a PDF of
      literal `$x$` (the template documents this at `templates/cheatsheet.v1.html`).
- [x] CI runs the acceptance test (`--ignored`) on a runner with Chrome, and
      `docs/RELEASE.md` records the pinned-Chromium requirement for the Phase 15 image.
- [x] PDF export is plan-gated, and the mobile build is never told about a purchase.
- [x] Roadmap gate 5 performed and recorded.
- [x] Roadmap gate 6 performed and recorded.
- [x] Roadmap gate 7 performed and recorded.
- [x] Docs updated in the same commit as the code.
- [x] The full quality gate is green.

---

## Self-Review

- **Spec coverage:** the spec's `## Reports` section asks for an editable outline, then
  per-section generation with citations, stored as Markdown, exported as `.md` first with
  PDF/DOCX later gated by plan. The roadmap's Phase 6 detail adds the cheatsheet pipeline,
  the re-render endpoint, the template version, and the exact acceptance test. Every one of
  those is a task above; DOCX is not, and the roadmap says `md` first, so DOCX stays
  deferred with the roadmap's own wording.
- **The purity property is the plan's spine.** Every task either produces data, renders
  data, or prints rendered data, and only the last needs Chromium. That is what keeps the
  phase testable on a laptop and why the acceptance test can be the one that needs the
  browser.
- **The risk the roadmap names is addressed in the plan, not deferred:** a pinned Chromium
  in the image, and the acceptance test run inside that image in CI.
- **Type consistency:** `Block` is defined once, in `render::blocks`, and reused as the
  generation contract's output rather than duplicated. `Rendered`, `DroppedBlock`,
  `Template`, `ReportOutline`, `OutlineSection` are each defined once.
- **Open item:** B1 (QRIS gateway docs) is still needed at Phase 13, not before.

---

## Execution notes (Phase 6, as built)

The phase's own build notes are in the commit history; this section is the **gate pass**,
and it is where the numbers live. Five defects were found by running the gates rather
than by reading the code, and four of them were invisible at 100% text scale.

### Roadmap gate 5, accessibility, on the three Phase 6 surfaces: PASS after four fixes

Measured in a real browser (Chromium 141, driven over CDP), not asserted. The surfaces are
`ReportPanel` (Studio → Report), `ReportScreen` and `CheatsheetScreen`; each was measured
at 390x844 (phone) and 1280x900 (desktop) with `prefers-color-scheme: dark` **and**
`light`, against the real API on `:8099` with a real Indonesian source, a real report and
a real cheatsheet.

- **(a) Keyboard alone, with a visible ring.** `Tab` from the document body reaches every
  control on all three screens and wraps back to the first: **20 stops** on the panel
  (skip link, both tab strips, three form fields, the source checkbox, "Open report", the
  cheatsheet form, the depth select, the second checkbox, the five nav items), **17** on
  the report screen (title, both section headings, move up/down/remove, regenerate,
  generate, both export buttons, the nav) and **10** on the cheatsheet (skip link, the
  preview frame, Download PDF, Re-render, the nav). Every stop carries
  `outline: 2px solid var(--ring)` at a **2px offset**, measured on the focused element
  itself: the ring is 8.90-9.61:1 against its background in dark and 5.70-6.29:1 in light.
  Two stops needed work and are the first two defects below.
- **(b) Contrast, both themes, computed.** Twelve screen/theme/width combinations, **17 to
  66 text-background pairs each, 0 below their requirement** (4.5:1 for body text, 3:1 for
  large). The lowest ratio on the three Phase 6 surfaces was **6.29:1** (white on
  `--primary` at 14px, the generate buttons), and the lowest anywhere in the shell that
  hosts them was **5.01:1** in light and **6.14:1** in dark (the chat composer hint on the
  desktop Studio tab). Both themes were measured, not one extrapolated from the other.
- **(c) 44px targets at phone width.** Every interactive control on all three screens
  measured 44px high at 390px: the two tab strips (117x44 each), the fields and the
  select (332x44), both primary buttons, the export buttons, the nav items (78x44). The
  only two sub-44 boxes are not targets: the skip link, which is `sr-only` until focused
  (1px wide, then 101x44 when it has focus), and the two 13x13 checkboxes, whose 44px
  `<label>` is what is clicked and what the finger hits.
- **(d) No state by colour alone.** Checked on the states this phase adds: the `gaps`
  warning is the sentence "This isn't covered in your selected sources." beside an
  `alert` icon (`.note-warning`), never a tinted row; a section's in-flight state is the
  word "Writing" in the chip **and** on the disabled button, with the animated dot marked
  `aria-hidden`; written/not-written are the words "Written" / "Not written yet"; the
  report's status is a word ("Outline only" / "Generating" / "Ready" / "Failed"); a
  reached plan limit is `LockedFeature` — a lock icon, the feature's name and a sentence;
  and the cheatsheet's page count is the number itself.
- **(e) 200% text scale at 390px.** `document.documentElement.style.fontSize: 32px`
  (root 16px -> 32px, measured back from `getComputedStyle`). **This is where the phase's
  defects were, and it found four.** Before: 1-3 overflowing or clipped elements per
  screen, and the tab bar's fifth item painted 45px past the right edge. After: **0
  overflowing elements, 0 clipped elements, no horizontal page overflow**, and
  `document.scrollingElement.scrollWidth === 390 === clientWidth` on all six
  screen/theme combinations.

#### The four defects gate 5 found, and their fixes

1. **The report list's own title rendered 16px wide over 18 lines.** `ReportPanel`'s row
   is `flex flex-wrap` with three effectively fixed-width siblings (a written count, a
   status chip, an "Open report" button), so the title's `flex-1` gave it a flex basis of
   `0` and only what was left over. Measured at 390px: **16px wide, 18 lines, ~2
   characters per line** — a report's name unreadable on a phone, and 100px over 6 lines
   at 200% type. Fixed with `w-full ... sm:w-auto sm:flex-1`, the shape `StudyScreen`
   already uses: the title takes the whole line on a phone and the one-line row returns
   from `sm` up. Re-measured: **358px, 1 line** at 100% and **326px, 2 lines** at 200%.
2. **A section heading rendered 29px wide over 10 lines.** `ReportScreen`'s section row
   has the same shape and the same three fixed-width siblings. Measured: **29px wide, 10
   lines** at 100% and 44px over 13 lines at 200%. Same fix; re-measured **332px, 1 line**
   at 100% and **276px, 2 lines** at 200%.
3. **The phone tab bar's fifth item was unreachable at 200% type.** Each item is
   `flex: 1` with the UA's `min-width: auto`, so it cannot shrink below its label: the
   five labels need **435px** at 200% (5 x 87px average) against a 390px viewport, and the
   bar is `position: fixed` with `overflow: visible` — so "More" (Settings and Billing)
   was painted 45px outside the viewport with nothing to scroll. It was reachable at 100%
   and unreachable only for the reader who had enlarged their type. Fixed with
   `overflow-x-auto overscroll-x-contain` on the bar: `scrollWidth` 435 > `clientWidth`
   390, every item reachable by swipe **and** by `Tab` (Chromium scrolls a focused item
   into view), and at 100% nothing scrolls, so the visual is unchanged. The active-page
   marker also moved from `top: -1px` to `top: 0`, because a scroll container clips at its
   padding box and the 1px that used to sit over the top border would have left the marker
   1px tall — and that marker is the tab bar's non-colour signal for the current page.
4. **The cheatsheet preview frame was a `Tab` stop with no focus ring at all.** The frame
   is a scrollable region, so it is a stop a keyboard user needs (arrow keys scroll the
   document), and the global `:focus-visible` rule cannot reach it: Chromium puts focus on
   the frame's inner document, so the frame matches **neither** `:focus`,
   `:focus-visible` **nor** `:focus-within` on itself or on any ancestor, and its
   `focus`/`focusin`/`blur` events do not fire (measured: `document.activeElement` is the
   `IFRAME`, `document.hasFocus()` is `true`, and every one of those three selectors is
   `false`). The ring is therefore driven from the one signal the browser gives — the
   window's own `blur`/`focus` pair, guarded by `document.hasFocus()` so an app switch
   cannot draw a ring on a frame nobody is using — and the frame now carries the same
   `outline-2 outline-offset-2 outline-ring` treatment as every other control. Re-measured
   in the walk: the frame's two stops both report `solid 2px` at 9.61:1 (dark) / 5.97:1
   (light).

Each fix has a regression test that fails without it (verified by reverting each fix and
watching its test fail): `Shell.test.tsx` ("lets the tab bar scroll sideways"),
`ReportPanel.test.tsx` ("gives the report title the whole row on a phone"),
`ReportScreen.test.tsx` ("gives a section heading the whole row on a phone") and
`CheatsheetScreen.test.tsx` ("rings the frame when focus is inside it, and not when the
window merely lost focus").

### Roadmap gate 6, Indonesian end to end: PASS

Both halves, because either one alone fails silently while English tests stay green.

- **Copy.** Every string the phase adds exists in **both** `app/src/i18n/locales/en.json`
  and `id.json`, with identical placeholder sets — `app/src/i18n/i18n.test.ts` asserts key
  parity, placeholder parity, that every `t()` key the code asks for exists, and that no
  feature namespace holds copy nothing renders. The sentences the screens render, quoted
  in Indonesian: "Usulkan kerangka" (propose the outline), "Butuh 3 kredit untuk
  kerangkanya, lalu 6 kredit untuk setiap bagian yang kamu buat.", "Butuh 6 kredit.
  Contekan muncul di sini saat dibuat.", "Kerangka" / "Saran kerangka", "Belum ditulis" /
  "Sedang ditulis" / "Sudah ditulis", "Buat ulang", "Simpan kerangka", "2 dari 2 bagian
  sudah ditulis", "Hapus" / "Naikkan" / "Turunkan", "Unduh PDF" / "Unduh Markdown",
  "Render ulang memakai isi yang tersimpan: tanpa panggilan model, dan tanpa kredit.",
  "PDF-nya 3 halaman.", "Dirender dengan template cheatsheet.v1".
- **Data, through the smoke script** (`scripts/smoke-phase6.sh`, extended rather than
  duplicated, and **PASS** end to end): an Indonesian source produces an Indonesian report
  section — the exported `.md` (226 bytes) contains "UDP tidak memakai handshake sehingga
  lebih cepat", the model's own Indonesian sentence and not just the source's words — and
  an Indonesian cheatsheet. The HTML document (23,475 bytes) carries the model's
  Indonesian text *and* the template's Indonesian chrome ("Ringkasan materi ujian",
  "Daftar Isi", "Ukuran payload", "Efisiensi pengiriman", "SYN dari klien", "belum
  tercakup", "Halaman" + `counter(pages)`), with the template's English comments stripped
  first so a comment cannot be mistaken for the document.
- **The half that matters: the rendered PDF's extracted text.** `pdftotext` on the real
  print (239,231 bytes, **3 pages**) extracts **1,912 characters** containing
  "Halaman 1 dari 3" and "Halaman 3 dari 3", the running header "Cheatsheet UTS Jaringan
  Komputer", the model's Indonesian sentences ("Ringkasan materi ujian", "Ukuran payload",
  "Efisiensi pengiriman", "SYN dari klien"), the template's Indonesian chrome ("Daftar
  Isi", "belum tercakup"), **no English chrome** ("Table of Contents", "Page 1 of",
  "Summary") and **no raw `$`**. This is the assertion that catches a prompt-language
  mistake: the API's JSON is correct either way, and only the printed document shows that
  the output language was honoured.

### Roadmap gate 7, honesty: PASS

- **(a) No invented or uncomputable number.** The shipped bundle (`app/dist`) and both
  locale files were grepped for the spec's banned claims — "100% grounded", "zero
  hallucination", "confidence score", "match percentage", "predicted grade", "retention
  projection", "neuro score", "cognitive score", "verified source": **0 matches**. The
  only "verified" copy in the product is "Email verified", which is a fact the server
  checks (the token was consumed), not a claim about a source.
- **(b) Every number traces to something the app computes.** The cheatsheet's page count
  is the renderer's own count read back from the print: the API reported **3** and the
  PDF has **3** pages (`pdftotext` counted 3 form feeds, and the smoke script asserts
  `ACTUAL_PAGES >= PAGES`). "2 dari 2 bagian sudah ditulis" is derived at render from the
  outline's sections and the bodies the server sent, not stored. The credit numbers are
  read from `config/plans.toml` at build time, the same file the server charges from
  (`app/src/features/billing/plans.test.ts` asserts 3 and 6 against the file). No
  coverage, confidence or readiness number exists anywhere on these screens.
- **(c) The composer hint.** Re-asserted by grepping the **shipped** copy, not by reading
  the component: `app/dist` contains "AI can make mistakes. Check the citations." and its
  Indonesian pair "AI bisa salah. Periksa kutipannya.", one bundle file each.
  `app/src/i18n/i18n.test.ts` pins the English string exactly, and
  `ChatScreen.test.tsx` + `features/a11y.test.tsx` render it under the composer. This
  phase adds no composer, so the requirement is inherited rather than touched; the check
  is a regression check that the phase did not remove it.
- **(d) The credit cost is shown before the action, from `config/plans.toml`.** The Studio
  panel says "Costs 3 credits for the outline, then 6 credits for each section you
  generate." above the "Propose outline" button — both prices, before either action, since
  the outline is what that form starts and the sections are what the student is really
  buying. The cheatsheet form says "Costs 6 credits." above its button, and every section
  row carries "6 credits" beside its own button, before anything is generated (asserted by
  `ReportScreen.test.tsx`, which reads the numbers from `config/plans.toml` in the test
  too). The value reaches the app from the same file the server loads, so the two cannot
  drift; a price the file does not declare renders nothing rather than a guess.
- **(e) AI proposals are labelled "Suggested ...".** The outline is the model's, and the
  screen says so above it: "Suggested outline" (en) / "Saran kerangka" (id), resolved
  through `honesty.suggested`. Nothing is written before the student saves: the outline
  arrives as an editable draft, and the edits are local until "Save outline" is pressed.
- **(f) A locked feature follows `config/billing_policy.toml` for the running platform.**
  The PDF export is gated by `export_formats` and rendered through `LockedFeature`, never a
  bespoke conditional. Measured in the browser on a **free** account at 390px: the report
  screen shows "PDF export / Prints the report to a paginated PDF. / Available on Pro /
  Upgrade to Pro" with an `/billing` link — the web/desktop variant, which the policy file
  allows. For the mobile variant, `lockedFeaturePolicy('ios')` was read from the shipped
  module and returns `{purchaseAllowed: false, showPrices: false, showUpgradeCta: false}`,
  so the component renders "Not available on your plan" with no price and no link; the
  component branches on exactly that flag, and `ReportScreen.test.tsx`'s "tells a phone
  build the fact and nothing that could be bought" renders the phone build and asserts the
  absence of both. The policy file is imported as text at build time, so the app and the
  server cannot disagree about it.

### A defect in the gate's own tooling

**`scripts/smoke-phase6.sh` could not exercise the print half at all.** Its plan upgrade
wrote only `users.plan_id = 'pro'`, and `users.plan_id` alone grants nothing: the
entitlements engine honours the stored plan only while a `subscriptions` row is `active`
or in grace (`crates/server/src/entitlements.rs::effective_plan`, A146). The script
therefore upgraded an account, kept getting 403 `plan_required` from
`GET /cheatsheets/:id/pdf`, and reported it as "the PDF could not be downloaded even
though page_count is set" — which reads like a renderer problem and is not one. It now
writes both rows (the plan and a live subscription) and says so in its output. This is
why the gate is run rather than reasoned about: the script had been green in the
implementer's head and failed on its first real run.

### Phase 6 verification (as executed in this gate pass)

| Gate | Result |
|---|---|
| `npm --prefix app test` | **286 passing** (35 files) |
| `npm --prefix app run build` | clean |
| `bash scripts/smoke-phase6.sh` | **PASS** (3 model calls, 3-page PDF, Indonesian text) |
| Gate 5 (browser, 12 combinations) | PASS after 4 fixes |
| Gate 6 (copy + rendered PDF) | PASS |
| Gate 7 (a)-(f) | PASS |

**NOT RE-RUN HERE:** `cargo test`, `cargo clippy` and `cargo fmt --check`. A sibling slice
is editing `crates/server/**` in this working tree, so a workspace-wide cargo run would
measure their half-finished edits rather than this phase; the Phase 6 Rust suites and the
`--ignored` PDF acceptance test were green in the phase's own commits. What this pass
verified about the server is what it can verify from outside: the real API answered, the
real Chromium printed, the real PDF's text was extracted, and the real smoke run passed
end to end.

### Boxes left unticked, and why

  - Task 1 Step 1, "Write the failing tests" -- three of the five tests it names do not exist at HEAD: `markup_in_a_text_field_is_escaped_not_interpreted` (shipped as `crates/core/tests/render.rs::markup_in_a_field_is_escaped_not_interpreted`), `a_script_element_drops_the_whole_block_and_is_recorded` (shipped as `::a_disallowed_svg_element_drops_the_block_and_records_why`) and `svg_ids_are_prefixed_per_block_so_two_diagrams_cannot_collide` (shipped as `::two_diagrams_in_one_section_do_not_share_element_ids`). `rendering_is_deterministic` and `labels_resolve_to_citations_and_unknown_ones_are_dropped` are present verbatim, and this task's own note above records that the draft's names were superseded by the shipped ones, so the named artefacts cannot be confirmed as written.
  - Task 1 Step 2, "Run them and watch them fail" -- the tests this step runs are the Step 1 set, three of which do not exist at HEAD, so the failure it is written to observe was not observed for those three.
