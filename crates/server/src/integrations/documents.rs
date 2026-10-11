//! The pdfium library handle.
//!
//! `pdfium-render` initializes the external library exactly once per process:
//! `Pdfium::bind_to_library` returns `PdfiumLibraryBindingsAlreadyInitialized`
//! on the second call, because the bindings live in a process-wide slot. Binding
//! per document therefore works for the first PDF and fails for every one after
//! it, which is exactly what the Phase 1 smoke run caught (the second uploaded
//! PDF came back `failed: PdfiumLibraryBindingsAlreadyInitialized`).
//!
//! So the library is bound once, at boot, and the resulting `Pdfium` is shared.
//! That also turns a misconfigured path into a startup failure instead of a
//! per-job one, which matches the rest of the boot assertions (A2's embedding
//! dimension check, the bucket check).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use pdfium_render::prelude::Pdfium;

/// Set by [`init`] at boot. `None` means PDF ingestion is unavailable, which the
/// ingest job reports as a readable error rather than as an empty document.
static PDFIUM: OnceLock<Arc<Pdfium>> = OnceLock::new();

/// Binds pdfium from `path`.
///
/// `Ok(false)` means no path is configured, so this deployment does not do PDFs.
/// An error means a path *was* configured and the library could not be loaded:
/// that is a boot failure, because a configured extractor that silently does
/// nothing is worse than an absent one.
pub fn init(path: Option<&Path>) -> anyhow::Result<bool> {
    let Some(path) = path else {
        tracing::info!("PDFIUM_LIB_PATH is unset; PDF ingestion is disabled");
        return Ok(false);
    };

    anyhow::ensure!(
        path.exists(),
        "PDFIUM_LIB_PATH is set to {} but no such file exists",
        path.display()
    );

    let bindings = Pdfium::bind_to_library(path)
        .map_err(|error| anyhow::anyhow!("loading pdfium from {}: {error}", path.display()))?;
    // `Pdfium` is `Send + Sync` under pdfium-render's `thread_safe` feature, so
    // one handle serves the worker's tasks. `set` fails only if `init` ran twice,
    // which the second `bind_to_library` would already have rejected.
    let _ = PDFIUM.set(Arc::new(Pdfium::new(bindings)));

    tracing::info!(path = %path.display(), "pdfium loaded");
    Ok(true)
}

/// The process-wide handle, if PDF ingestion is available.
pub fn handle() -> Option<Arc<Pdfium>> {
    PDFIUM.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_path_reports_that_pdf_is_unavailable() {
        assert!(!init(None).expect("no path is not an error"));
        assert!(handle().is_none());
    }

    #[test]
    fn a_missing_library_names_the_path() {
        let error = init(Some(Path::new("/nonexistent/libpdfium.dylib")))
            .expect_err("a configured but missing library must fail");
        assert!(
            error.to_string().contains("/nonexistent/libpdfium.dylib"),
            "{error}"
        );
    }
}
