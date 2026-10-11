//! The Chromium renderer: HTML in, A4 PDF out.
//!
//! This is the only part of the document pipeline that needs a browser, and it is the
//! phase's stated risk: Chromium must be **in the image**, pinned, or the pipeline works on
//! a laptop and fails on deploy. Everything upstream of this module is a pure function, so
//! the browser is reached only here and only for the last step.
//!
//! # The header and the footer are CDP's, not the template's
//!
//! `templates/cheatsheet.v1.html` carries `@top-center` and `@bottom-center` rules, and
//! **Chromium does not implement CSS Paged Media margin boxes**, so in Chrome they produce
//! no header and no footer at all. Both are supplied here through `displayHeaderFooter` plus
//! `headerTemplate`/`footerTemplate`, which is the only path that works in Chrome. The
//! declarative rules stay in the template for Prince and WeasyPrint.
//!
//! One consequence worth stating because it decides what the acceptance test asserts: CDP's
//! templates apply to **every** page, including the first, so the cover carries the running
//! header and the `Halaman N dari M` footer, and `M` counts the cover.

use std::sync::Arc;
use std::time::Duration;

use headless_chrome::types::PrintToPdfOptions;
use headless_chrome::{Browser, LaunchOptions};

/// How long one render may take before it is abandoned. A pooled browser that never
/// answers must not hold a job forever: the job retries, and a second attempt is cheaper
/// than a stuck worker.
const RENDER_TIMEOUT: Duration = Duration::from_secs(60);

/// The A4 page, in inches, which is what `printToPDF` takes. The template sets A4 in
/// `@page`, and `prefer_css_page_size` makes that authoritative, but the explicit size is
/// what a renderer without the CSS rule would use.
const A4_WIDTH_IN: f64 = 8.27;
const A4_HEIGHT_IN: f64 = 11.69;

/// The margins, matching the template's `@page { margin: 18mm 16mm }` so the CDP header and
/// footer sit inside the same frame the CSS reserves.
const MARGIN_TOP_IN: f64 = 0.71;
const MARGIN_BOTTOM_IN: f64 = 0.71;
const MARGIN_SIDE_IN: f64 = 0.63;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("could not launch the browser: {0}")]
    Launch(String),
    #[error("could not open a page: {0}")]
    Page(String),
    #[error("the page did not finish loading: {0}")]
    Load(String),
    #[error("the browser did not produce a PDF: {0}")]
    Print(String),
}

/// A rendered PDF and the facts about it the rest of the pipeline stores.
#[derive(Debug, Clone)]
pub struct Pdf {
    pub bytes: Vec<u8>,
    /// What the footer's `Halaman N dari M` reports, and therefore what
    /// `cheatsheets.page_count` holds.
    pub page_count: u32,
}

/// The pooled browser.
///
/// One process, reused across renders. Launching Chromium per document would cost more than
/// the render, and the phase's risk is exactly the kind of resource management that only
/// shows up under load.
pub struct Renderer {
    browser: Arc<Browser>,
}

impl Renderer {
    /// Launches the browser.
    ///
    /// `executable` is where Chromium lives: the pinned build in the image, or a locally
    /// installed Chrome in dev. `None` lets the crate find one on `PATH`.
    ///
    /// `sandbox` is off only when explicitly asked for, because a container that cannot use
    /// the namespace sandbox is a deployment fact, not a default. Outside dev the renderer
    /// must not run unsandboxed: it is rendering model-influenced HTML.
    pub fn launch(executable: Option<&str>, allow_no_sandbox: bool) -> Result<Self, RenderError> {
        let options = LaunchOptions::default_builder()
            .path(executable.map(std::path::PathBuf::from))
            .headless(true)
            .sandbox(!allow_no_sandbox)
            .build()
            .map_err(|error| RenderError::Launch(error.to_string()))?;

        let browser =
            Browser::new(options).map_err(|error| RenderError::Launch(error.to_string()))?;
        Ok(Self {
            browser: Arc::new(browser),
        })
    }

    /// Renders one HTML document to an A4 PDF.
    ///
    /// `running_header` is the text CDP puts at the top of every page, and the footer is
    /// built here from `page_number`/`total_pages` so the numbering is CDP's own rather than
    /// something the template would have to know.
    pub fn render(&self, html: &str, running_header: &str) -> Result<Pdf, RenderError> {
        let tab = self
            .browser
            .new_tab()
            .map_err(|error| RenderError::Page(error.to_string()))?;

        tab.set_default_timeout(RENDER_TIMEOUT);

        // `load` rather than `domcontentloaded`: the template's KaTeX renderer waits for
        // `load` because it needs the deferred scripts to have executed, and a PDF taken
        // before that has every formula as literal `$...$`.
        // Written into the page rather than navigated to: a real cheatsheet is far past any
        // URL length limit, and `document.write` is what a document with no URL needs.
        let script = format!(
            "document.open(); document.write({}); document.close();",
            serde_json::to_string(html).map_err(|error| RenderError::Page(error.to_string()))?
        );
        tab.evaluate(&script, false)
            .map_err(|error| RenderError::Page(error.to_string()))?;

        // The template reports its own KaTeX outcome, which is what makes a broken asset a
        // visible fact rather than a silent one. Waiting for it is what keeps the PDF from
        // being taken mid-render.
        // The template's KaTeX runs on `load` and marks its outcome; awaiting the promise
        // is what keeps the PDF from being taken before the maths is rendered.
        tab.evaluate(
            "new Promise((resolve) => { if (document.readyState === 'complete') resolve(true); \
             else window.addEventListener('load', () => resolve(true)); })",
            true,
        )
        .map_err(|error| RenderError::Load(error.to_string()))?;

        let bytes = tab
            .print_to_pdf(Some(pdf_options(running_header)))
            .map_err(|error| RenderError::Print(error.to_string()))?;

        let page_count = count_pages(&bytes);
        Ok(Pdf { bytes, page_count })
    }
}

/// The running header, on every page.
///
/// The `#pageNumber`/`#totalPages` spans are CDP's own placeholders and are **not** in the
/// allowlist of anything: they are literal class names the protocol substitutes. The
/// `.header`/`.footer` classes and the margin reset are CDP's own defaults, which the
/// protocol applies only when the template carries them.
fn header_template(header: &str) -> String {
    format!(
        "<div style=\"font-family: 'Inter', Arial, sans-serif; font-size: 8pt; color: #4b5563; \
         width: 100%; margin: 0 16mm; padding-bottom: 3mm; border-bottom: 0.4pt solid #e5e7eb;\">\
         {header}</div>"
    )
}

/// The footer, with the page number CDP substitutes.
fn footer_template() -> String {
    "<div style=\"font-family: 'Inter', Arial, sans-serif; font-size: 8pt; color: #4b5563; \
     width: 100%; margin: 0 16mm; padding-top: 3mm; border-top: 0.4pt solid #e5e7eb; \
     text-align: center;\">Halaman <span class=\"pageNumber\"></span> dari \
     <span class=\"totalPages\"></span></div>"
        .to_string()
}

/// Escapes text going into the CDP header template, which is HTML.
fn escape_template_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The number of pages in a PDF, read from the document itself.
///
/// Counted rather than assumed: it is what the footer's `Halaman N dari M` reports, so the
/// two must agree. A PDF's page tree is a tree of `/Kids`, so the count is the sum over the
/// leaves, and the scan is bounded because a malformed document must not spin.
fn count_pages(bytes: &[u8]) -> u32 {
    // `/Type /Page` (not `/Pages`) marks a leaf. Counting the marker is enough for the
    // documents this renderer produces, and is deliberately not a full parser: a real one
    // belongs in a crate, not here.
    let needle = b"/Type /Page";
    let mut count = 0u32;
    let mut index = 0usize;
    while let Some(at) = find(&bytes[index..], needle) {
        let after = index + at + needle.len();
        // `/Pages` shares the prefix; the character after it tells them apart.
        let next = bytes.get(after).copied().unwrap_or(b'\n');
        if next != b's' {
            count += 1;
        }
        index = after;
    }
    count.max(1)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The print options, as a pure function of the running header.
///
/// Extracted so the acceptance test can assert the options the renderer actually uses
/// (A4, `display_header_footer`, the footer's page placeholders) without launching a
/// browser, and so a change to them is a change to one function rather than two.
pub fn pdf_options(running_header: &str) -> PrintToPdfOptions {
    PrintToPdfOptions {
        landscape: Some(false),
        display_header_footer: Some(true),
        print_background: Some(true),
        scale: None,
        paper_width: Some(A4_WIDTH_IN),
        paper_height: Some(A4_HEIGHT_IN),
        margin_top: Some(MARGIN_TOP_IN),
        margin_bottom: Some(MARGIN_BOTTOM_IN),
        margin_left: Some(MARGIN_SIDE_IN),
        margin_right: Some(MARGIN_SIDE_IN),
        page_ranges: None,
        ignore_invalid_page_ranges: None,
        header_template: Some(header_template(&escape_template_text(running_header))),
        footer_template: Some(footer_template()),
        prefer_css_page_size: Some(true),
        transfer_mode: None,
        generate_document_outline: Some(true),
        generate_tagged_pdf: Some(true),
    }
}
