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
  report flow needs no schema change.
- `migrations/0016_cheatsheets.sql`: `cheatsheets` with `scope`, `exam_date`, `depth`,
  `source_ids`, `status`, `error`, `blocks_key`, `template_version`, `html_key`, `pdf_key`,
  `page_count`. **The whole artifact pipeline is already modelled**, including the
  re-render contract's `blocks_key` and `template_version`. No schema change is needed
  except the generation columns Phase 4/5 taught (`credits_spent`, `usage_event_id`,
  `requested_*`), which a later migration adds.
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
   The table does not exist yet; this phase adds it, with the losing body recoverable.
9. **Chromium is pinned and the renderer is a pooled child process**, because this is the
   phase's stated risk: it must be in the Docker image, not just on a laptop. Locally it
   uses the installed Chrome; in the image it uses the pinned build. The renderer refuses
   to run unsandboxed outside dev.
10. **The PDF's page count is read back from the renderer, not assumed.** `page_count` is
    what the footer's `Halaman N dari M` depends on, so it is a value the renderer reports
    (Chromium's page count for the print), not a number the template guesses.
11. **Export formats come from the plan matrix.** `GET /reports/:id/export?format=` and
    `GET /cheatsheets/:id/pdf` both check `export_formats` and answer 403
    `plan_required` with `min_plan` when the format is not in the plan, so the mobile
    build cannot be told about a purchase it must not offer.

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

- [ ] **Step 1: Write the failing tests**

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

- [ ] **Step 2: Run them and watch them fail**

`cargo test -p stvdy-core --test render` → unresolved import.

- [ ] **Step 3: Write the block model**

`blocks.rs`: `Blocks { title, subtitle, course: Option<String>, exam: Option<String>,
chapters: Vec<Chapter> }`, `Chapter { title, sections }`, `Section { title, blocks }`, and
`Block` as an enum with one variant per kind the prompt documents (`Text`, `Bullets`,
`Concept`, `Formula`, `QuickFormula`, `Analogy`, `Trap`, `Memorize`, `Table`, `Question`,
`Diagram`, `Svg`), each `deny_unknown_fields`, each carrying `labels: Vec<u32>`.

`parse_blocks` strips a code fence (reuse `generation::strip_code_fence`), parses with
`deny_unknown_fields` so a drifted prompt fails loudly, and bounds the document
(`MAX_CHAPTERS`, `MAX_SECTIONS_PER_CHAPTER`, `MAX_BLOCKS_PER_SECTION`) so a model that
ignores the structure rules cannot produce an unrenderable document.

- [ ] **Step 4: Write the renderer**

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

- [ ] **Step 5: Run the tests, then commit**

```bash
cargo test -p stvdy-core --test render
git add crates/core/src/render crates/core/src/lib.rs crates/core/tests/render.rs
git commit -m "feat(core): the pure renderer, the typed blocks, and the SVG allowlist"
```

---

## Task 2: The report contracts, and the report job

**Files:**
- Create: `crates/core/src/generation/report.rs`
- Modify: `crates/core/src/prompts/registry.rs` (+ three modules), `crates/core/src/generation/mod.rs`
- Create: `crates/server/src/reports/{mod.rs,routes.rs,generate.rs}`
- Create: `crates/server/src/jobs/handlers/report.rs`
- Create: `migrations/0023_report_revisions.sql`, `migrations/0024_report_cheatsheet_generation.sql`
- Test: `crates/server/tests/reports.rs`, `crates/core/tests/generation_report.rs`

- [ ] **Step 1: The outline contract and its tests**

`ReportOutline { title, sections: Vec<OutlineSection { heading, summary, source_ids }> }`,
`deny_unknown_fields`, bounded (`MAX_REPORT_SECTIONS`). The outline is **editable**, so the
route accepts a replacement outline and validates it against the same struct — an edit that
the generator could not have produced is still valid input if it parses.

- [ ] **Step 2: The generation columns, and the revisions table**

`0024` adds the generation columns the two earlier phases taught (`status`, `error`,
`credits_spent`, `usage_event_id`) to `reports`, and to `cheatsheets` if any are missing
(they are present, so verify rather than assume). `0023` creates:

```sql
-- The roadmap's conflict rule for long text: keep the latest body and store the losing
-- one as a recoverable copy, so a two-device edit cannot silently destroy work.
create table report_revisions (
  id uuid primary key default gen_random_uuid(),
  report_id uuid not null references reports on delete cascade,
  user_id uuid not null references users on delete cascade,
  body_md text not null,
  created_at timestamptz not null default now()
);
create index on report_revisions (report_id, created_at desc);
```

`report_revisions` is deliberately **not** `+sync`: it is a server-side recovery log, not
user-facing state a device needs, and adding it to the trigger array would ship every
revision to every device.

- [ ] **Step 3: The job**

`generate_report_outline` and `generate_report_section`, mirroring
`quizzes::generate` exactly: compose through `compose_task`, one retry on a parse failure
with the violation appended, record the prompt version and the provider cost on the usage
row, claim the row with a guarded status flip, and refund once on a permanent failure.

Section generation writes **only that section's** Markdown into `body_md`, replacing any
previous text for that index, so regenerating one section does not cost the whole report.

- [ ] **Step 4: The routes**

`POST /workspaces/:id/reports` (outline), `PATCH /reports/:id` (edit the outline),
`POST /reports/:id/generate` (sections), `GET /reports/:id`, and
`GET /reports/:id/export?format=md`. The export checks `export_formats`; `pdf` arrives in
Task 4. Each write that can race another device's write stores the losing body as a
revision (decision 8).

- [ ] **Step 5: Tests, then commit**

The tests mirror `tests/quizzes.rs`: generation with a scripted model, the retry naming
the violation, the refund, the monthly cap, and another account's report being a 404.

---

## Task 3: The cheatsheet contract and the generation job

**Files:**
- Create: `crates/core/src/generation/cheatsheet.rs`
- Create: `crates/server/src/cheatsheets/{mod.rs,routes.rs,generate.rs}`
- Create: `crates/server/src/jobs/handlers/cheatsheet.rs`
- Test: `crates/server/tests/cheatsheets.rs`

- [ ] **Step 1: The contract** — the twelve block kinds, reusing `render::blocks::Block`
      so the parser and the renderer cannot disagree about the shape. The generation
      contract *is* the renderer's input type; a second definition would be a second thing
      to keep in step.

- [ ] **Step 2: The job** — `generate_cheatsheet`, same shape. On success it writes the
      blocks to object storage at `blocks_key`, records `template_version`, and enqueues
      `render_cheatsheet_pdf` (Task 4). If the renderer is unavailable the cheatsheet is
      still `ready` with its blocks stored: the document exists, only its PDF does not, and
      that is a retryable render, not a failed generation.

- [ ] **Step 3: The routes** — `POST /workspaces/:id/cheatsheets`,
      `GET /cheatsheets/:id`, `GET /cheatsheets/:id/html`, `POST /cheatsheets/:id/re-render`.
      Re-render reads `blocks_key`, re-runs the pure renderer with the **current** template,
      and updates `html_key`/`template_version`. It never calls the model, so it is free.

- [ ] **Step 4: Tests, then commit.**

---

## Task 4: The Chromium renderer and the PDF job

**Files:**
- Create: `crates/server/src/render/chrome.rs`, `crates/server/src/jobs/handlers/render_pdf.rs`
- Modify: `crates/server/Cargo.toml` (+ `headless_chrome`, pinned), `docker-compose.yml`,
  the server `Dockerfile`, `.github/workflows/ci.yml`
- Test: `crates/server/tests/pdf.rs` (ignored by default; the phase's acceptance test)

**This is the phase's stated risk: the native dependency must be in the image, not just on
a laptop.**

- [ ] **Step 1: The renderer**

A pooled `headless_chrome` browser, one process, pages reused, with the flags the roadmap
names: `print_background`, `prefer_css_page_size`, `generate_document_outline`,
`generate_tagged_pdf`. It returns `(pdf_bytes, page_count)`, and `page_count` is what the
footer's `Halaman N dari M` depends on.

- [ ] **Step 2: The acceptance test**

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

- [ ] **Step 3: The image**

The `Dockerfile` installs a **pinned** Chromium; the CI job builds the image and runs the
acceptance test inside it, not on the host. Without that, the pipeline works on a laptop
and fails on deploy, which is the risk the roadmap names.

- [ ] **Step 4: The PDF routes** — `GET /cheatsheets/:id/pdf` and
      `GET /reports/:id/export?format=pdf`, both gated on `export_formats`.

---

## Task 5: The SPA — the Report panel, the report screen, the cheatsheet preview

**Files:**
- Create: `app/src/api/queries/reports.ts`, `app/src/api/queries/cheatsheets.ts`,
  `app/src/features/studio/ReportPanel.tsx`, `app/src/features/reports/ReportScreen.tsx`
- Modify: `app/src/features/workspaces/WorkspaceScreen.tsx` (the Studio sub-tab),
  `app/src/router.tsx`

- [ ] **Step 1: The panel and the screen.** The Studio gains a Report sub-tab beside
      Flashcards and Quiz, using the same roving-tabindex strip. The report screen edits the
      outline, generates sections one at a time with visible progress, and offers the export
      formats the plan allows.

- [ ] **Step 2: The cheatsheet preview.** `GET /cheatsheets/:id/html` renders in a sandboxed
      frame (the document is generated HTML; the frame is what keeps it away from the app's
      own DOM), with a "Download PDF" action shown only when the plan allows it.

- [ ] **Step 3: Tests, then commit.** Mirror `QuizScreen.test.tsx`: the export action is
      absent when the plan does not allow the format, and the preview frame is `sandbox`ed.

---

## Task 6: Docs, both gates, and the smoke script

- [ ] **Step 1: `docs/API.md`** — the `## Reports` and `## Cheatsheets` sections.
- [ ] **Step 2: `docs/ARCHITECTURE.md`** — the content-versus-layout split, the purity
      property, and the renderer's deployment story.
- [ ] **Step 3: `docs/RELEASE.md`** — the pinned Chromium in the image.
- [ ] **Step 4: `docs/ASSUMPTIONS.md`** — one row per decision above, from the next free
      number.
- [ ] **Step 5: Roadmap gate 5** on the report screen and the cheatsheet preview, performed
      and recorded, with the same five checks the earlier phases used.
- [ ] **Step 6: Roadmap gate 6** — an Indonesian source produces an Indonesian cheatsheet
      and the rendered PDF's text is Indonesian.
- [ ] **Step 7: `scripts/smoke-phase6.sh`** — generate a report and a cheatsheet from an
      Indonesian source, export both, and assert the PDF's page count and that its text
      contains no `$`.
- [ ] **Step 8: The full gate**, then commit.

---

## Phase 6 exit checklist

- [ ] `crates/core/src/render/` is pure, deterministic, and unit-tested without Chromium.
- [ ] Generation produces typed blocks, never markup; a tag in a field is escaped.
- [ ] The SVG allowlist is closed; an element outside it drops the block and records why.
- [ ] SVG ids are prefixed per block, so two diagrams cannot collide.
- [ ] An editable outline generates per-section with citations, and regenerating one
      section does not regenerate the others.
- [ ] A long body edited on two devices keeps the latest and stores the loser as a
      revision.
- [ ] A cheatsheet renders to A4 PDF with the running header, `Halaman N dari M`, and no
      raw `$`.
- [ ] `POST /cheatsheets/:id/re-render` re-renders from stored blocks with no model call.
- [ ] Chromium is pinned in the Docker image and CI runs the acceptance test inside it.
- [ ] PDF export is plan-gated, and the mobile build is never told about a purchase.
- [ ] Roadmap gate 5 performed and recorded.
- [ ] Roadmap gate 6 performed and recorded.
- [ ] Docs updated in the same commit as the code.
- [ ] The full quality gate is green.

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
