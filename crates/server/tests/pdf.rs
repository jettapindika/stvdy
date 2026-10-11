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
use stvdy_server::render::chrome::{Renderer, pdf_options};

/// Where Chromium lives. The image sets `CHROME_PATH` to its pinned build; a developer's
/// machine usually has Chrome in the standard place.
fn chrome_path() -> Option<String> {
    if let Ok(path) = std::env::var("CHROME_PATH")
        && !path.is_empty()
    {
        return Some(path);
    }
    let candidates = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ];
    candidates
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .map(|path| (*path).to_string())
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

    let renderer = Renderer::launch(Some(&path), false).expect("the browser launches");
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
    assert!(
        text.contains("Halaman"),
        "the footer text is in the PDF, so the CDP template reached it"
    );
    assert!(
        text.contains("Cheatsheet UTS Jaringan Komputer"),
        "the running header is in the PDF"
    );
    assert!(
        text.contains("Bab 1") || text.contains("Bab 1".to_uppercase().as_str()),
        "the chapter titles are in the PDF"
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
