//! Text extraction: bytes and a media type in, `(page, text)` pairs out.
//!
//! PDF through `pdfium-render`, whose bindings are process-wide: see
//! `integrations::documents` for why the handle is bound once at boot and shared
//! rather than per document. A deployment with no `PDFIUM_LIB_PATH` gets a
//! readable error on the ingest job instead of a silently empty document.

use pdfium_render::prelude::Pdfium;

/// Extracts the text of each PDF page. Page numbers are 1-based, which is what
/// the citation UI shows and what `chunks.page` stores.
pub fn pdf_pages(
    pdfium: &Pdfium,
    bytes: &[u8],
    title: &str,
) -> Result<Vec<(Option<i32>, String)>, String> {
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, Some(title))
        .map_err(|error| format!("opening the PDF: {error}"))?;

    let mut pages = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let text = page
            .text()
            .map_err(|error| format!("reading page {}: {error}", index + 1))?
            .all();
        pages.push((Some(index as i32 + 1), text));
    }

    if pages.is_empty() {
        return Err("the PDF has no pages".to_string());
    }
    Ok(pages)
}
