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

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use headless_chrome::types::PrintToPdfOptions;
use headless_chrome::{Browser, LaunchOptions};

/// How long one render may take before it is abandoned. A pooled browser that never
/// answers must not hold a job forever: the job retries, and a second attempt is cheaper
/// than a stuck worker.
const RENDER_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the renderer waits for the template's `data-math-ready` sentinel before it
/// gives up on the document.
///
/// Bounded on purpose. The template sets the attribute on `load`, so the normal case
/// resolves in milliseconds; a template that never sets it (a renamed attribute, a
/// `render()` that throws before it marks anything, a `<body>` that no longer exists)
/// must **fail the job** rather than hang the worker on a promise that will never settle.
/// It is well inside [`RENDER_TIMEOUT`], so the deadline the caller sees is this one.
const MATH_READY_TIMEOUT_MS: u64 = 30_000;

/// Where Chromium is looked for when `CHROME_PATH` is unset.
///
/// The image sets `CHROME_PATH` to its pinned build (see `docs/RELEASE.md`), so this list
/// exists for a developer's machine, where Chrome is installed by the vendor's installer
/// and not on `PATH` at all.
const CHROME_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", // macOS
    "/usr/bin/google-chrome",                                       // Debian/Ubuntu
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
];

/// The Chromium this process should use: `CHROME_PATH` first, then the usual install
/// locations.
///
/// One function so the server, the acceptance test and the smoke run resolve the same
/// binary: a test that found Chrome by a different rule than the server would prove the
/// wrong thing.
pub fn find_executable() -> Option<String> {
    if let Ok(path) = std::env::var("CHROME_PATH")
        && !path.is_empty()
    {
        return Some(path);
    }
    CHROME_CANDIDATES
        .iter()
        .find(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
}

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
    /// The template's `data-math-ready` sentinel never reached `"1"`.
    ///
    /// Its own variant rather than a `Load` failure because the remedy is different and
    /// the message has to say which: the page loaded, and what failed is the maths. A
    /// PDF taken anyway would print literal `$x$` with no error anywhere.
    #[error("the document did not report its maths as ready: {0}")]
    MathNotReady(String),
    #[error("the browser did not produce a PDF: {0}")]
    Print(String),
}

impl RenderError {
    /// True when the failure is the browser *connection* rather than the document.
    ///
    /// This is what decides whether the pool is relaunched. `MathNotReady` is deliberately
    /// excluded: the page loaded and answered, and what failed is the document's maths, so a
    /// second browser would produce the same failure — and retrying it would hide the defect
    /// the guard exists to surface.
    pub fn is_connection(&self) -> bool {
        matches!(
            self,
            RenderError::Launch(_) | RenderError::Page(_) | RenderError::Load(_)
        )
    }
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
///
/// **The pool is replaceable, and that is not a nicety.** A browser process can exit between
/// two renders — it idles out, the container reclaims it, a page crashes the tab — and a
/// renderer that held one `Arc<Browser>` forever would then fail every subsequent print with
/// `Unable to make method calls because underlying connection is closed`, permanently, until
/// the server restarted. That is the state the first end-to-end smoke run found: the print
/// job failed on a server that had been idle for a minute, while the same code passed its
/// acceptance test because the test renders immediately after launching. So a render that
/// fails on the *connection* relaunches the browser and tries once more, and only a document
/// failure is reported as a document failure.
pub struct Renderer {
    /// The launch options, kept so the browser can be replaced with the same ones. Stored as
    /// the executable path rather than as `LaunchOptions` because that type is not `Clone`.
    executable: Option<String>,
    allow_no_sandbox: bool,
    browser: std::sync::Mutex<Arc<Browser>>,
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
        let browser = Self::launch_browser(executable, allow_no_sandbox)?;
        Ok(Self {
            executable: executable.map(str::to_string),
            allow_no_sandbox,
            browser: std::sync::Mutex::new(browser),
        })
    }

    /// The current browser handle.
    ///
    /// A poisoned lock is not worth failing a render over: the handle is replaced rather than
    /// mutated, so the worst a poisoned lock can mean is that another thread panicked while
    /// holding it, and the value behind it is still a usable `Arc`.
    fn browser(&self) -> Arc<Browser> {
        match self.browser.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Replaces the browser with a fresh process and returns it.
    fn relaunch(&self) -> Result<Arc<Browser>, RenderError> {
        let browser = Self::launch_browser(self.executable.as_deref(), self.allow_no_sandbox)?;
        match self.browser.lock() {
            Ok(mut guard) => *guard = browser.clone(),
            Err(poisoned) => *poisoned.into_inner() = browser.clone(),
        }
        Ok(browser)
    }

    /// One Chromium process, launched with the renderer's own options.
    fn launch_browser(
        executable: Option<&str>,
        allow_no_sandbox: bool,
    ) -> Result<Arc<Browser>, RenderError> {
        let options = LaunchOptions::default_builder()
            .path(executable.map(std::path::PathBuf::from))
            .headless(true)
            .sandbox(!allow_no_sandbox)
            .build()
            .map_err(|error| RenderError::Launch(error.to_string()))?;

        Browser::new(options)
            .map(Arc::new)
            .map_err(|error| RenderError::Launch(error.to_string()))
    }

    /// Renders one HTML document to an A4 PDF.
    ///
    /// `running_header` is the text CDP puts at the top of every page, and the footer is
    /// built here from `page_number`/`total_pages` so the numbering is CDP's own rather than
    /// something the template would have to know.
    ///
    /// This is **blocking**: it drives a browser over CDP. The job that calls it runs it on
    /// a blocking thread, so one document's render cannot stall the async worker.
    pub fn render(&self, html: &str, running_header: &str) -> Result<Pdf, RenderError> {
        self.render_within(html, running_header, MATH_READY_TIMEOUT_MS)
    }

    /// [`render`](Self::render) with an explicit sentinel deadline.
    ///
    /// Extracted so the guard's failure path is testable in a fraction of a second rather
    /// than in the production [`MATH_READY_TIMEOUT_MS`], and so the test proves the deadline
    /// is what ends the wait: a document that never sets the sentinel must fail *when the
    /// deadline passes*, not hang.
    pub fn render_within(
        &self,
        html: &str,
        running_header: &str,
        math_ready_timeout_ms: u64,
    ) -> Result<Pdf, RenderError> {
        match self.render_once(&self.browser(), html, running_header, math_ready_timeout_ms) {
            Ok(pdf) => Ok(pdf),
            Err(error) if error.is_connection() => {
                // The pooled process is gone rather than the document being wrong, so the
                // pool is replaced and the render is attempted once more. Bounded to one
                // retry: a second failure is either a real document problem or a machine that
                // cannot run Chromium at all, and neither is helped by a third launch.
                tracing::warn!(%error, "the browser connection was lost; relaunching it");
                let browser = self.relaunch()?;
                self.render_once(&browser, html, running_header, math_ready_timeout_ms)
            }
            Err(error) => Err(error),
        }
    }

    /// One render against a specific browser.
    fn render_once(
        &self,
        browser: &Browser,
        html: &str,
        running_header: &str,
        math_ready_timeout_ms: u64,
    ) -> Result<Pdf, RenderError> {
        let tab = browser
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

        // `load` first, because the sentinel is set from the template's `load` handler and
        // polling before it fires would read an attribute that is legitimately not there
        // yet. Awaiting the promise is what keeps the PDF from being taken mid-render.
        tab.evaluate(
            "new Promise((resolve) => { if (document.readyState === 'complete') resolve(true); \
             else window.addEventListener('load', () => resolve(true)); })",
            true,
        )
        .map_err(|error| RenderError::Load(error.to_string()))?;

        // Then the sentinel, which is what makes a broken KaTeX a visible fact rather than
        // a plausible-looking PDF full of LaTeX source. The template documents the contract
        // (`templates/cheatsheet.v1.html`): it sets `data-math-ready` to `"1"` when the
        // maths rendered or there was none to render, and to `unavailable`, `incomplete` or
        // `error` otherwise. Any value other than `"1"` fails the render, and so does a
        // template that never sets the attribute at all — the poll is bounded inside the
        // page, so a missing sentinel costs [`MATH_READY_TIMEOUT_MS`] and a failed job, not
        // a worker stuck on a promise that never settles.
        let probe = tab
            .evaluate(&math_ready_probe(math_ready_timeout_ms), true)
            .map_err(|error| RenderError::MathNotReady(error.to_string()))?;
        let reported = match probe.value {
            Some(serde_json::Value::String(raw)) => {
                serde_json::from_str::<MathReady>(&raw)
                    .map_err(|error| RenderError::MathNotReady(error.to_string()))?
                    .ready
            }
            // The probe resolves a JSON string, so anything else is the page refusing to
            // answer. Treated as not ready rather than assumed ready: the whole point of
            // the guard is that "we could not tell" must not become "it is fine".
            _ => None,
        };
        match reported.as_deref() {
            Some("1") => {}
            Some(other) => {
                return Err(RenderError::MathNotReady(format!(
                    "the template reported `{other}`, which means the maths did not render"
                )));
            }
            None => {
                return Err(RenderError::MathNotReady(format!(
                    "the template never set `data-math-ready` within {}s",
                    MATH_READY_TIMEOUT_MS / 1000
                )));
            }
        }

        let bytes = tab
            .print_to_pdf(Some(pdf_options(running_header)))
            .map_err(|error| RenderError::Print(error.to_string()))?;

        let page_count = count_pages(&bytes);
        Ok(Pdf { bytes, page_count })
    }
}

/// What the sentinel probe resolves: `{"ready": null}` when the attribute is absent,
/// `{"ready": "1"}` or `{"ready": "unavailable"}` when it is set.
///
/// An object rather than a bare value so the two states stay distinguishable: `undefined`
/// and `""` are different facts (never set versus set to nothing) and only one of them
/// means the template never ran.
#[derive(serde::Deserialize)]
struct MathReady {
    ready: Option<String>,
}

/// The in-page poll.
///
/// It resolves a **string** because `Tab::evaluate` asks CDP for a preview rather than a
/// by-value result, and parsing a preview's properties is exactly the kind of guess that
/// makes a guard unreliable. `JSON.stringify` puts the whole answer in the remote object's
/// `value`, where it can be read without heuristics.
fn math_ready_probe(timeout_ms: u64) -> String {
    format!(
        "new Promise((resolve) => {{\n\
         \x20 const deadline = Date.now() + {timeout_ms};\n\
         \x20 const check = () => {{\n\
         \x20   const value = document.documentElement.dataset.mathReady;\n\
         \x20   if (value !== undefined) {{ resolve(JSON.stringify({{ ready: value }})); return; }}\n\
         \x20   if (Date.now() >= deadline) {{ resolve(JSON.stringify({{ ready: null }})); return; }}\n\
         \x20   setTimeout(check, 100);\n\
         \x20 }};\n\
         \x20 check();\n\
         }})"
    )
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
