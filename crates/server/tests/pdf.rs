//! The pdfium binding, exercised the way the server uses it.
//!
//! The regression this pins: `pdfium-render` keeps its bindings in a process-wide
//! slot and returns `PdfiumLibraryBindingsAlreadyInitialized` on a second
//! `bind_to_library`. A per-document bind therefore succeeded for the *first* PDF
//! and failed for every one after it, which is what the Phase 1 smoke run caught
//! when the second uploaded PDF came back `failed: PdfiumLibraryBindingsAlreadyInitialized`.
//!
//! The fix binds once at boot and shares the handle, so this test asserts the
//! property that fix exists for: **two documents, one process**. It needs the
//! shared library, which is a deployment artefact rather than a repository file,
//! so it skips with a printed reason when the path is absent. A skip that is
//! printed is not a silent pass.
//!
//! This is a separate test binary on purpose: `init` occupies the process-wide
//! slot, so a test that asserts the handle is *absent* could not coexist with it
//! in the same binary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

/// The fixture: a real PDF with extractable text, committed so the test needs no
/// `cupsfilter` or any other tool to produce one.
const FIXTURE: &[u8] = include_bytes!("fixtures/notes.pdf");

fn library_path() -> Option<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let configured = std::env::var("PDFIUM_LIB_PATH")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| root.join(value))
        .unwrap_or_else(|| root.join("var/pdfium/libpdfium.dylib"));

    if configured.exists() {
        Some(configured)
    } else {
        eprintln!(
            "SKIPPED: no pdfium library at {}. Fetch one with\n  \
             curl -sSL https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8086/\
             pdfium-mac-arm64.tgz | tar xz -C /tmp && cp /tmp/lib/libpdfium.dylib var/pdfium/",
            configured.display()
        );
        None
    }
}

/// Two documents through one bound handle. This is the exact case that failed
/// before the handle was shared.
#[test]
fn one_process_extracts_from_two_documents() {
    let Some(path) = library_path() else {
        return;
    };

    let bound =
        stvdy_server::integrations::documents::init(Some(&path)).expect("the library binds once");
    assert!(bound, "a path was given, so the binding must have happened");

    let handle = stvdy_server::integrations::documents::handle()
        .expect("the handle is available after init");

    for attempt in 1..=2 {
        let pages = stvdy_server::sources::parse::pdf_pages(&handle, FIXTURE, "notes")
            .unwrap_or_else(|error| {
                panic!(
                    "document {attempt} failed: {error}. Before the handle was shared this \
                     failed on the second document with PdfiumLibraryBindingsAlreadyInitialized"
                )
            });

        assert!(!pages.is_empty(), "document {attempt} produced no pages");
        let text: String = pages.iter().map(|(_, text)| text.as_str()).collect();
        assert!(
            text.contains("Mitochondria"),
            "document {attempt} extracted no usable text: {text:?}"
        );
        // The page number is 1-based, which is what the citation UI shows and
        // what `chunks.page` stores.
        assert_eq!(pages[0].0, Some(1));
    }
}

/// A file that is not a PDF is an error rather than a panic, because the message
/// becomes `sources.error` and the user reads it.
#[test]
fn a_non_pdf_is_a_readable_error() {
    let Some(path) = library_path() else {
        return;
    };

    stvdy_server::integrations::documents::init(Some(&path)).expect("the library binds");
    let handle = stvdy_server::integrations::documents::handle().expect("a handle");

    let error = stvdy_server::sources::parse::pdf_pages(&handle, b"not a pdf at all", "broken")
        .expect_err("garbage must not parse as a document");
    assert!(
        error.contains("opening the PDF"),
        "the error must say what failed: {error}"
    );
}

/// A second `init` is not a failure.
///
/// `pdfium-render` keeps its bindings in a process-wide slot, so the second
/// `bind_to_library` returns `AlreadyInitialized`. `init` treats that one error as
/// success, which is what makes it safe to call twice: two tests in one binary
/// (this file), a boot path that runs again, or a caller that just wants to be
/// sure. Without this the tests here are order-dependent and fail on whichever
/// thread binds second.
#[test]
fn init_is_idempotent() {
    let Some(path) = library_path() else {
        return;
    };

    assert!(stvdy_server::integrations::documents::init(Some(&path)).expect("the first bind"));
    assert!(
        stvdy_server::integrations::documents::init(Some(&path)).expect("the second bind reuses"),
        "a second init must succeed rather than report AlreadyInitialized"
    );

    let handle = stvdy_server::integrations::documents::handle().expect("a handle after both");
    assert!(
        !stvdy_server::sources::parse::pdf_pages(&handle, FIXTURE, "notes")
            .expect("the handle still works")
            .is_empty(),
        "the reused handle must be usable"
    );
}

/// An unset path means PDF ingestion is off, which is not an error: a deployment
/// that never ingests PDFs must still boot.
#[test]
fn an_unset_path_reports_pdf_as_unavailable() {
    // Deliberately does not touch the process-wide slot: this asserts the
    // contract of the call, not the state it leaves.
    assert!(
        !stvdy_server::integrations::documents::init(None).expect("no path is not an error"),
        "an unset path means PDF ingestion is unavailable, not that init failed"
    );
}
