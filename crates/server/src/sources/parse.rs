//! Text extraction: bytes and a media type in, `(page, text)` pairs out.
//!
//! PDF through `pdfium-render`, whose bindings are process-wide: see
//! `integrations::documents` for why the handle is bound once at boot and shared
//! rather than per document. A deployment with no `PDFIUM_LIB_PATH` gets a
//! readable error on the ingest job instead of a silently empty document.
//!
//! DOCX and PPTX are both zip + OOXML, so they share one opener and one set of
//! guards. **The guards are the reason there is no dedicated extractor crate**
//! (the crate-list deviation in `docs/ASSUMPTIONS.md`): a document we are handed
//! is untrusted, and a zip is the one format where reading it naively is itself
//! the vulnerability. Every member path is checked against zip-slip and the
//! decompressed total is bounded, *before* anything is extracted.
//!
//! Page numbers are the one thing the two formats disagree about, and the
//! disagreement is kept: a DOCX has no pages, so its runs carry `None`, while a
//! PPTX's slide number is the only way a student finds slide 7, so it is kept
//! exactly.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;

use pdfium_render::prelude::Pdfium;
use quick_xml::Reader;
use quick_xml::events::Event;
use zip::ZipArchive;

/// The decompressed-size ceiling for one OOXML document.
///
/// A zip bomb's whole trick is a small archive that expands past what the reader
/// can hold, so the *sum of the declared uncompressed sizes* is checked before a
/// single member is read. 64 MiB is generous for a slide deck and an order of
/// magnitude below the smallest plan's storage quota, so a legitimate document is
/// never refused and a bomb always is.
const MAX_DECOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;

/// The member-count ceiling.
///
/// A second bomb shape: a million tiny members, each below the size ceiling,
/// whose central directory alone exhausts memory.
const MAX_ZIP_MEMBERS: usize = 4096;

/// The per-member ceiling for the XML part actually read.
const MAX_XML_BYTES: u64 = 32 * 1024 * 1024;

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

/// Opens an OOXML package after checking every member.
///
/// Both guards run here, together, because they are the same decision: this
/// archive is not allowed to be hostile. A member whose path escapes the archive
/// is refused even though we never write a file, because the same check is what
/// makes the *next* person's extraction safe, and a path is not something to
/// trust just because today's reader happens not to use it.
fn open_ooxml(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("opening the document: {error}"))?;

    if archive.len() > MAX_ZIP_MEMBERS {
        return Err(format!(
            "the document has {} parts, which is more than the {MAX_ZIP_MEMBERS} a document may have",
            archive.len()
        ));
    }

    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("reading the document's directory: {error}"))?;

        // The zip-slip check: a path that escapes the archive, or that is absolute,
        // is refused outright rather than sanitized. `enclosed_name` is the zip
        // crate's own validator, so this agrees with what an extractor would do
        // rather than with a hand-rolled `contains("..")`.
        if entry.enclosed_name().is_none() {
            return Err(format!(
                "the document contains an unsafe member path: {}",
                entry
                    .name()
                    .map(|name| name.into_owned())
                    .unwrap_or_else(|_| "<undecodable>".to_string())
            ));
        }

        total = total.saturating_add(entry.size());
    }

    if total > MAX_DECOMPRESSED_BYTES {
        return Err(format!(
            "the document expands to {total} bytes, more than the {MAX_DECOMPRESSED_BYTES} allowed"
        ));
    }

    Ok(archive)
}

/// Reads one XML member as a string, bounded by the per-member ceiling.
///
/// `None` when the member is absent, which is a document that simply is not the
/// format it claims — the caller decides whether that is fatal.
fn read_member(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<String>, String> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(format!("reading {name}: {error}")),
    };

    if entry.size() > MAX_XML_BYTES {
        return Err(format!(
            "{name} is {} bytes, more than the {MAX_XML_BYTES} allowed",
            entry.size()
        ));
    }

    // `take` bounds the *actual* read as well as the declared size: a lying
    // central directory is exactly how a bomb with a small declared size works.
    let mut xml = Vec::with_capacity(entry.size().min(MAX_XML_BYTES) as usize);
    entry
        .by_ref()
        .take(MAX_XML_BYTES)
        .read_to_end(&mut xml)
        .map_err(|error| format!("reading {name}: {error}"))?;

    String::from_utf8(xml)
        .map(Some)
        .map_err(|_| format!("{name} is not valid UTF-8"))
}

/// The text of one OOXML part: every run of `run_tag` in order, one line per
/// paragraph.
///
/// `run_tag` is the local name of the element that carries text — `t` inside
/// `w:t` for a DOCX and inside `a:t` for a PPTX. Matching on the *local* name is
/// deliberate: the prefix is a namespace alias the document chooses, so `w:t` in
/// one file and `ns0:t` in another are the same element.
///
/// Whitespace is handled per **paragraph**, not per run or per text event, and that
/// is the whole subtlety:
///   * per event, the reader's `trim_text` would eat the space in `a &amp; b` (the
///     entity splits one text node into three events) and the leading space in a run
///     like ` membrane`;
///   * per run, trimming would join `Cell` and ` membrane` into `Cellmembrane`.
///
/// So runs are accumulated verbatim and the *paragraph* is trimmed and collapsed,
/// which drops Word's whitespace-only spacer runs while preserving every space that
/// carries meaning.
fn part_text(xml: &str, run_tag: &str, paragraph_tag: &str) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut in_run = false;
    let mut paragraph = String::new();

    loop {
        match reader
            .read_event()
            .map_err(|error| format!("parsing the document XML: {error}"))?
        {
            Event::Eof => break,
            Event::Start(element) => {
                if element.local_name().into_inner() == run_tag {
                    in_run = true;
                }
            }
            Event::End(element) => {
                if element.local_name().into_inner() == run_tag {
                    in_run = false;
                }
                if element.local_name().into_inner() == paragraph_tag {
                    let text = collapse_whitespace(&paragraph);
                    if !text.is_empty() {
                        out.push_str(&text);
                        out.push('\n');
                    }
                    paragraph.clear();
                }
            }
            Event::Text(text) if in_run => {
                paragraph.push_str(&text.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            // An entity is its own event in this reader, so `&amp;` arrives as a
            // reference rather than as text. Dropping it would silently delete a
            // character from the document; an unresolvable one is kept as written,
            // which is what the reader would show a human opening the file.
            Event::GeneralRef(reference) if in_run => {
                let raw = reference.into_inner();
                match resolve_reference(raw.as_ref()) {
                    Some(resolved) => paragraph.push_str(&resolved),
                    None => {
                        paragraph.push('&');
                        paragraph.push_str(raw.as_ref());
                        paragraph.push(';');
                    }
                }
            }
            _ => {}
        }
    }

    Ok(out)
}

/// Resolves a `GeneralRef`'s content: a predefined entity name, or a numeric
/// character reference.
fn resolve_reference(reference: &str) -> Option<String> {
    if let Some(digits) = reference.strip_prefix('#') {
        let codepoint = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse::<u32>().ok()?,
        };
        return char::from_u32(codepoint).map(String::from);
    }
    quick_xml::escape::resolve_predefined_entity(reference).map(String::from)
}

/// Trims a paragraph and collapses its internal whitespace runs to one space.
///
/// A paragraph is a unit of meaning, so its edges are not part of it; the runs
/// inside it are, so the spaces between them survive.
fn collapse_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A DOCX is zip + OOXML: `word/document.xml`, every `<w:t>` run in order, and no
/// page numbers because a DOCX has none.
pub fn docx_runs(bytes: &[u8]) -> Result<Vec<(Option<i32>, String)>, String> {
    let mut archive = open_ooxml(bytes)?;
    let xml = read_member(&mut archive, "word/document.xml")?
        .ok_or_else(|| "the file has no word/document.xml: it is not a DOCX".to_string())?;

    let text = part_text(&xml, "t", "p")?;
    if text.trim().is_empty() {
        return Err("the document has no text".to_string());
    }
    Ok(vec![(None, text)])
}

/// A PPTX keeps its slide numbers: `ppt/slides/slideN.xml`, `<a:t>` runs, and the
/// page is N.
///
/// The number is not decoration. A citation for slide 7 has to say 7, because that
/// is the only way a student finds the slide the answer came from.
pub fn pptx_slides(bytes: &[u8]) -> Result<Vec<(Option<i32>, String)>, String> {
    let mut archive = open_ooxml(bytes)?;

    // Sorted by slide number, because a zip's member order is whatever the writer
    // chose and a deck read out of order would chunk backwards.
    let mut slides: BTreeMap<i32, String> = BTreeMap::new();
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .and_then(|entry| entry.name().ok().map(|name| name.into_owned()))
        })
        .filter_map(|name| slide_number(&name).map(|number| (number, name)))
        .map(|(_, name)| name)
        .collect();

    for name in names {
        let Some(number) = slide_number(&name) else {
            continue;
        };
        let xml = read_member(&mut archive, &name)?
            .ok_or_else(|| format!("{name} disappeared from the archive"))?;
        let text = part_text(&xml, "t", "p")?;
        if !text.trim().is_empty() {
            slides.insert(number, text);
        }
    }

    if slides.is_empty() {
        return Err("the presentation has no slides with text".to_string());
    }
    Ok(slides
        .into_iter()
        .map(|(n, text)| (Some(n), text))
        .collect())
}

/// The slide number of `ppt/slides/slideN.xml`, or `None` for any other member.
///
/// Exact rather than a suffix match: `ppt/slides/_rels/slide7.xml.rels` also ends
/// in `.xml.rels` and would otherwise be read as slide 7's content, and
/// `ppt/notesSlides/slide7.xml` is a different part with the same number.
fn slide_number(name: &str) -> Option<i32> {
    let rest = name.strip_prefix("ppt/slides/slide")?;
    let digits = rest.strip_suffix(".xml")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i32>().ok()
}

/// Renders a PPTX or DOCX to PDF with LibreOffice, for the uniform viewer.
///
/// `soffice` is an external process, so this is the one extractor that can fail
/// for a reason that is nobody's fault: the binary may be absent (A158 — the
/// source still ingests and the preview is simply absent), may take longer than a
/// test is willing to wait, or may refuse a document it cannot lay out. Every one
/// of those is a `String` the caller logs, never a panic.
///
/// The conversion is `--headless --convert-to pdf` into a scratch directory, which
/// is why the input has to exist as a file: LibreOffice's CLI takes a path, not a
/// stream. The directory is removed on every exit path, success or not.
pub fn preview_pdf(soffice: &Path, kind: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let extension = match kind {
        "docx" => "docx",
        "pptx" => "pptx",
        other => return Err(format!("{other} has no PDF preview")),
    };

    let scratch = std::env::temp_dir().join(format!("stvdy-preview-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&scratch)
        .map_err(|error| format!("creating {}: {error}", scratch.display()))?;

    // One closure for the work so the cleanup below runs on every path, including
    // the `?`s, without a guard type or a second copy of the removal.
    let result = (|| -> Result<Vec<u8>, String> {
        let input = scratch.join(format!("source.{extension}"));
        std::fs::write(&input, bytes).map_err(|error| format!("writing the input: {error}"))?;

        let output = Command::new(soffice)
            .arg("--headless")
            .arg("--norestore")
            .arg("--convert-to")
            .arg("pdf")
            .arg("--outdir")
            .arg(&scratch)
            .arg(&input)
            .output()
            .map_err(|error| format!("running {}: {error}", soffice.display()))?;

        if !output.status.success() {
            return Err(format!(
                "{} exited {}: {}",
                soffice.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let rendered = scratch.join("source.pdf");
        std::fs::read(&rendered).map_err(|error| {
            format!(
                "{} produced no PDF at {}: {error}",
                soffice.display(),
                rendered.display()
            )
        })
    })();

    // Best-effort: a leftover scratch directory is a cleanup job's problem, not a
    // reason to fail an ingest that otherwise succeeded.
    if let Err(error) = std::fs::remove_dir_all(&scratch) {
        tracing::warn!(dir = %scratch.display(), %error, "removing the preview scratch directory failed");
    }

    let pdf = result?;
    if pdf.is_empty() {
        return Err(format!("{} produced an empty PDF", soffice.display()));
    }
    Ok(pdf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slide_number_is_read_only_from_a_slide_part() {
        assert_eq!(slide_number("ppt/slides/slide1.xml"), Some(1));
        assert_eq!(slide_number("ppt/slides/slide17.xml"), Some(17));
        // A relationship file is not a slide, and neither are the notes.
        assert_eq!(slide_number("ppt/slides/_rels/slide7.xml.rels"), None);
        assert_eq!(slide_number("ppt/notesSlides/slide7.xml"), None);
        assert_eq!(slide_number("ppt/slides/slide.xml"), None);
        assert_eq!(slide_number("word/document.xml"), None);
    }

    #[test]
    fn runs_are_read_in_order_and_paragraphs_break_lines() {
        let xml = r#"<?xml version="1.0"?>
            <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
              <w:body>
                <w:p><w:r><w:t>Cell</w:t></w:r><w:r><w:t>membrane</w:t></w:r></w:p>
                <w:p><w:r><w:t>Mitochondria</w:t></w:r></w:p>
              </w:body>
            </w:document>"#;
        let text = part_text(xml, "t", "p").expect("the document parses");
        assert_eq!(text.trim(), "Cellmembrane\nMitochondria");
    }

    #[test]
    fn an_entity_in_a_run_is_unescaped() {
        let xml = r#"<w:p xmlns:w="w"><w:r><w:t>a &amp; b</w:t></w:r></w:p>"#;
        let text = part_text(xml, "t", "p").expect("the document parses");
        assert_eq!(text.trim(), "a & b");
    }

    #[test]
    fn the_spaces_around_an_entity_and_between_runs_survive() {
        // The entity splits one text node into three events, and a run may carry its
        // own leading space; a reader that trimmed per event would eat both, which is
        // the difference between "a & b" and "ab".
        let xml = r#"<w:p xmlns:w="w"><w:r><w:t>a &amp; b</w:t></w:r></w:p>"#;
        assert_eq!(part_text(xml, "t", "p").unwrap().trim(), "a & b");

        let split =
            r#"<w:p xmlns:w="w"><w:r><w:t>Cell</w:t></w:r><w:r><w:t> membrane</w:t></w:r></w:p>"#;
        assert_eq!(
            part_text(split, "t", "p").unwrap().trim(),
            "Cell membrane",
            "a run's own leading space is content"
        );
    }

    #[test]
    fn a_whitespace_only_paragraph_contributes_nothing() {
        // Word emits these for spacing, and an empty line in the extracted text would
        // become a chunk boundary for no reason.
        let xml = r#"<w:body xmlns:w="w">
            <w:p><w:r><w:t>First</w:t></w:r></w:p>
            <w:p><w:r><w:t>   </w:t></w:r></w:p>
            <w:p><w:r><w:t>Second</w:t></w:r></w:p>
        </w:body>"#;
        assert_eq!(part_text(xml, "t", "p").unwrap(), "First\nSecond\n");
    }

    #[test]
    fn a_numeric_character_reference_resolves() {
        let xml = r#"<w:p xmlns:w="w"><w:r><w:t>&#38;</w:t></w:r></w:p>"#;
        assert_eq!(part_text(xml, "t", "p").unwrap().trim(), "&");
    }
}
