//! The Phase 6 acceptance test: a cheatsheet renders to a six-page A4 PDF.
//!
//! From the roadmap, and it is the assertion about `$` that matters. A broken KaTeX asset
//! leaves the page rendering with literal `$x$` where the maths should be: nothing errors,
//! the PDF is produced, and the defect is only visible to the student who prints it. That is
//! the failure this test exists to catch, and it is why the test is not "does a PDF come
//! out" but "does the PDF contain the maths".
//!
//! It needs Chromium, so it is `#[ignore]`d by default and run with `--ignored`. CI runs it
//! **inside the image**, which is the phase's stated risk: a pipeline that works on a laptop
//! and fails on deploy is the thing this test is supposed to prevent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use uuid::Uuid;

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::render::blocks::parse_blocks;
use stvdy_core::render::{Cover, cheatsheet_v1, render_html};
use stvdy_server::render::chrome::{RenderError, Renderer, find_executable, pdf_options};

/// Where Chromium lives. The image sets `CHROME_PATH` to its pinned build; a developer's
/// machine usually has Chrome in the standard place.
///
/// The lookup itself is the server's (`find_executable`), not a copy of it: the test must
/// resolve the same binary the renderer does, or it proves something about the test.
fn chrome_path() -> Option<String> {
    find_executable()
}

/// Serves the print assets on a loopback port and returns the origin they answer on.
///
/// The acceptance test renders the **real** template, whose asset references the renderer
/// rewrites to `<base>/assets/…` (A339) — so the test needs something at that base. Serving the
/// server's own route on an ephemeral port is the honest way to get one: it exercises the real
/// handler and the real embedded bytes, it needs no database and no network, and it fails for
/// an asset reason rather than for "the server is not up yet".
///
/// The runtime is moved to its own thread and never shut down, which is what a `#[test]` that
/// drives a blocking browser needs: the server must outlive the render.
fn serve_assets() -> String {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the asset server");
    let listener = runtime.block_on(async {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port")
    });
    let port = listener.local_addr().expect("the bound address").port();

    let app = axum::Router::new().route(
        "/assets/{*path}",
        axum::routing::get(stvdy_server::assets::serve),
    );
    std::thread::spawn(move || {
        let _ = runtime.block_on(async { axum::serve(listener, app).await });
    });

    format!("http://127.0.0.1:{port}")
}

fn labels() -> ContextLabels {
    ContextLabels::new(
        vec![LabeledChunk {
            label: 1,
            chunk_id: Uuid::from_u128(11),
            source_id: Uuid::from_u128(1),
            page: Some(3),
        }],
        vec![],
    )
}

/// A document sized to six pages: a cover, a table of contents, and enough chapters to fill
/// four more. The page count is the assertion, so the content has to actually be that long.
fn sample_blocks() -> stvdy_core::render::Blocks {
    // Two chapters of two sections, which is the four content pages plus the cover and the
    // table of contents the roadmap's acceptance test counts.
    let mut chapters: Vec<String> = Vec::new();
    for chapter in 1..=2 {
        let mut sections: Vec<String> = Vec::new();
        for section in 1..=2 {
            let mut blocks: Vec<String> = Vec::new();
            // Enough prose per section to reach a page, plus a formula so the KaTeX path is
            // exercised on every section rather than once.
            for paragraph in 1..=4 {
                blocks.push(format!(
                    r#"{{"kind":"text","text":"Bab {chapter} bagian {section} paragraf {paragraph}.  Model OSI membagi komunikasi menjadi tujuh lapisan sedangkan TCP/IP  menyederhanakannya menjadi empat, dan pemetaan keduanya sering keluar di  soal ujian karena mahasiswa tertukar antara lapisan Transport dan  Session.","labels":[1]}}"#
                ));
            }
            blocks.push(
                r#"{"kind":"formula","latex":"\\eta = \\frac{L}{L + H} \\times 100\\%","caption":"Efisiensi pengiriman","legend":[{"symbol":"L","meaning":"Ukuran payload","unit":"byte"},{"symbol":"H","meaning":"Total overhead header","unit":"byte"},{"symbol":"\\eta","meaning":"Efisiensi pengiriman","unit":"%"}]}"#
                    .to_string(),
            );
            blocks.push(
                r#"{"kind":"concept","term":"Enkapsulasi","text":"Proses menambahkan header pada data saat turun dari lapisan atas ke lapisan bawah.","labels":[1]}"#
                    .to_string(),
            );
            sections.push(format!(
                r#"{{"title":"Bagian {section}","blocks":[{}]}}"#,
                blocks.join(",")
            ));
        }
        chapters.push(format!(
            r#"{{"title":"Bab {chapter}","sections":[{}]}}"#,
            sections.join(",")
        ));
    }
    let chapters = chapters.join(",");

    let raw = format!(
        r#"{{"title":"Cheatsheet UTS Jaringan Komputer","subtitle":"Ringkasan materi ujian","course":"Jaringan Komputer","exam":"UTS","chapters":[{chapters}]}}"#
    );
    parse_blocks(&raw).expect("the sample document parses")
}

fn cover() -> Cover {
    Cover {
        course: "Jaringan Komputer".to_string(),
        exam: "UTS".to_string(),
        exam_date: "2026-11-01".to_string(),
        source_count: 3,
        generated_at: "2026-10-12 07:00".to_string(),
        lang: "id".to_string(),
    }
}

/// The PDF options the renderer actually uses, asserted without a browser.
///
/// Cheap and always-run: it pins A4, the header/footer switch, and the page-number
/// placeholders the `Halaman N dari M` footer depends on, so a change to any of them fails
/// here rather than silently producing a document with no page numbers.
#[test]
fn the_print_options_are_a4_with_a_page_numbered_footer() {
    let options = pdf_options("Cheatsheet UTS Jaringan Komputer");

    assert_eq!(options.paper_width, Some(8.27), "A4 width in inches");
    assert_eq!(options.paper_height, Some(11.69), "A4 height in inches");
    assert_eq!(
        options.display_header_footer,
        Some(true),
        "Chromium ignores @page margin boxes, so the header and footer must come from CDP"
    );
    assert_eq!(options.print_background, Some(true), "the template paints");

    let footer = options.footer_template.expect("a footer template");
    assert!(
        footer.contains("Halaman"),
        "the footer is the one the acceptance test reads: {footer}"
    );
    assert!(
        footer.contains("class=\"pageNumber\""),
        "CDP substitutes the page number here: {footer}"
    );
    assert!(
        footer.contains("class=\"totalPages\""),
        "and the total here: {footer}"
    );

    let header = options.header_template.expect("a header template");
    assert!(
        header.contains("Cheatsheet UTS Jaringan Komputer"),
        "the running header carries the exam: {header}"
    );
}

/// The header template is HTML, so the running header is escaped: a course name containing
/// `<` must not become markup in the frame of every page.
#[test]
fn the_running_header_is_escaped() {
    let options = pdf_options("<script>alert(1)</script>");
    let header = options.header_template.expect("a header template");

    assert!(!header.contains("<script>"), "{header}");
    assert!(header.contains("&lt;script&gt;"), "{header}");
}

/// The phase's acceptance test. Needs Chromium; run with `--ignored`.
#[test]
#[ignore = "needs Chromium; run with --ignored"]
fn the_cheatsheet_renders_to_a_six_page_a4_pdf() {
    let Some(path) = chrome_path() else {
        panic!("no Chromium found; set CHROME_PATH");
    };

    let blocks = sample_blocks();
    let html = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover())
        .expect("the document renders")
        .html;

    // The template's KaTeX and font references are rewritten to this server before the page is
    // written, so the render depends on the assets being served rather than on a CDN being
    // reachable. That is what makes the `no raw $` assertion below a statement about the print
    // assets: if a stylesheet or a script 404s, `renderMathInElement` is undefined, the template
    // reports `unavailable`, and the render fails rather than printing literal LaTeX.
    let base = serve_assets();

    let renderer = Renderer::launch(Some(&path), false, Some(&base)).expect("the browser launches");
    let pdf = renderer
        .render(&html, "Cheatsheet UTS Jaringan Komputer")
        .expect("the PDF renders");

    assert!(!pdf.bytes.is_empty(), "a PDF was produced");
    assert!(
        pdf.bytes.starts_with(b"%PDF"),
        "and it is a PDF: {:?}",
        &pdf.bytes[..4.min(pdf.bytes.len())]
    );
    assert_eq!(
        pdf.page_count, 6,
        "the sample document is sized to six pages; the footer's `dari 6` depends on this"
    );

    let text = extract_text(&pdf.bytes);

    // The footer and the running header are matched with **all** whitespace removed from both
    // sides, and that is not a weakened assertion — it is the same one against a lossy reader.
    //
    // `pdf-extract` reconstructs text from glyph positions, and at the footer's 8pt it splits
    // words at a kerning boundary: the Phase 15 image work extracted
    // `H alaman 2 dari 6` and `C heatsheet UTS  Jaringan K omputer` from a document whose
    // footer and header render correctly, and only once the typefaces resolved (with the CDN
    // stylesheet unreachable the footer fell back to Arial and the same extractor read it
    // cleanly). The property this assertion exists for is "CDP's footer and header templates
    // reached the PDF", and a space inside a word is the extractor's, not the document's.
    // Stripping whitespace keeps the word sequence — `CheatsheetUTSJaringanKomputer` — so a
    // footer that was absent or carried different words still fails.
    //
    // The `no raw $` assertion below is deliberately **not** run against this: a `$` is a
    // character rather than a spacing artefact, and no normalisation can hide one.
    let unspaced: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        unspaced.contains("Halaman"),
        "the footer text is in the PDF, so the CDP template reached it: {text}"
    );
    assert!(
        unspaced.contains("CheatsheetUTSJaringanKomputer"),
        "the running header is in the PDF: {text}"
    );
    assert!(
        unspaced.contains("Bab1") || unspaced.contains("BAB1"),
        "the chapter titles are in the PDF: {text}"
    );

    // The assertion that matters. A broken KaTeX asset leaves the maths as literal `$...$`:
    // no error, a plausible-looking PDF, and the defect only visible on paper.
    assert!(
        !text.contains('$'),
        "KaTeX did not render; the maths is literal text: {text}"
    );
}

/// The text of a PDF.
///
/// Through a real extractor rather than a hand-rolled stream scanner: the first attempt
/// returned a single space, which would have let the `no raw $` assertion pass without
/// looking at the document at all. An assertion that cannot fail is worse than no
/// assertion, because it reads as coverage.
fn extract_text(bytes: &[u8]) -> String {
    pdf_extract::extract_text_from_mem(bytes).expect("the PDF's text is extractable")
}

/// A template that never sets `data-math-ready` fails the render, and fails it **when the
/// deadline passes** rather than hanging the worker.
///
/// This is the guard's other half. The acceptance test proves a document whose maths
/// rendered prints; this proves the document that never reported anything does not print
/// at all. Without it, a renamed attribute or a `render()` that throws before it marks
/// anything leaves the pipeline printing literal LaTeX with no error anywhere — the same
/// silent defect, reached by a different route.
///
/// The deadline is a few hundred milliseconds here so the test proves the bound instead of
/// waiting out the production one, which is the assertion that matters: a promise that
/// never settles must end in an error, not in a stuck job.
#[test]
#[ignore = "needs Chromium; run with --ignored"]
fn a_document_that_never_reports_its_maths_fails_the_render() {
    let Some(path) = chrome_path() else {
        panic!("no Chromium found; set CHROME_PATH");
    };

    let renderer = Renderer::launch(Some(&path), false, None).expect("the browser launches");
    // Complete, printable, and silent about its maths.
    let html = "<!DOCTYPE html><html lang=\"id\"><head><meta charset=\"utf-8\">\
                <title>T</title></head><body><p>$x$</p></body></html>";

    let started = std::time::Instant::now();
    let error = renderer
        .render_within(html, "Cheatsheet", 500)
        .expect_err("a document that never sets the sentinel must not produce a PDF");

    assert!(
        matches!(error, RenderError::MathNotReady(_)),
        "the failure must name the sentinel: {error}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the poll must end at its own deadline, not hang: {:?}",
        started.elapsed()
    );
}

/// A template that reports a failed KaTeX fails the render too, and says which value it
/// reported.
///
/// `unavailable` is what the template sets when `renderMathInElement` is not a function —
/// a blocked or missing KaTeX asset. This is the exact state the acceptance test's `no raw
/// $` assertion is written to catch at print time; catching it here is the point, because
/// here it is an error rather than a plausible-looking document.
#[test]
#[ignore = "needs Chromium; run with --ignored"]
fn a_template_that_reports_katex_unavailable_fails_the_render() {
    let Some(path) = chrome_path() else {
        panic!("no Chromium found; set CHROME_PATH");
    };

    let renderer = Renderer::launch(Some(&path), false, None).expect("the browser launches");
    let html = "<!DOCTYPE html><html lang=\"id\"><head><meta charset=\"utf-8\"><title>T</title>\
                </head><body><p>$x$</p><script>\
                document.documentElement.dataset.mathReady = 'unavailable';</script></body></html>";

    let error = renderer
        .render_within(html, "Cheatsheet", 5000)
        .expect_err("a document whose maths did not render must not produce a PDF");

    assert!(
        matches!(error, RenderError::MathNotReady(_)),
        "the failure must name the sentinel: {error}"
    );
    assert!(
        error.to_string().contains("unavailable"),
        "the message must carry the value the template reported: {error}"
    );
}
