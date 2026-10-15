//! The report's print path: Markdown in, A4 PDF out.
//!
//! A report is the one document in this phase that is **not** a typed-block document. The
//! section prompt returns Markdown (`prompts/report_section.v1.md`), the body is stored as
//! Markdown, and the `.md` export is that text verbatim. The PDF export therefore needs a
//! Markdown step the cheatsheet does not have, and this module is it.
//!
//! # Why the conversion is a subset, and why it escapes first
//!
//! The report is generated from a student's own documents, so its text is untrusted input
//! that ends up in a document they print. Every character is escaped **before** any markup
//! is inserted, and the only tags in the output are the ones this module writes: a source
//! containing `<script>` is shown literally, exactly as `stvdy_core::render` guarantees for
//! a cheatsheet.
//!
//! What is understood is what the prompt asks for — headings, paragraphs, ordered and
//! unordered lists, block quotes, inline `code`, bold, italic, and the `[n]` chunk labels.
//! Anything else is text. That is deliberately not a Markdown implementation: a full one
//! is a parser, a CommonMark conformance suite and a sanitiser, and this phase needs a
//! printable report, not a second Markdown engine to keep in step with the client's.
//!
//! # The template
//!
//! `templates/report.v1.html` is compiled in with `include_str!`, so a missing or renamed
//! file is a build error rather than a 500 in front of a student. It is a **separate**
//! template from the cheatsheet's: one is a print layout for typed blocks, the other for
//! prose, and merging them would mean one file with two unrelated rule sets.

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The template, compiled in.
const REPORT_V1: &str = include_str!("../../../../templates/report.v1.html");

/// The version stored in a log line when a report is printed. A report's PDF is produced
/// on demand and not stored, so this exists for the log rather than for a column.
pub const REPORT_V1_VERSION: &str = "report.v1";

/// The document's language. Indonesian is the product's second language and the one the
/// roadmap's gate 6 exercises, so the `lang` attribute is not decoration: it decides
/// hyphenation and the screen reader's voice for a printed-and-read-back document.
const DEFAULT_LANG: &str = "id";

/// Renders the report's Markdown into the template's HTML.
///
/// Pure: no clock, no network, no randomness, so the same body produces the same document
/// twice. That is what lets the export be re-run for free.
pub fn render_report_html(title: &str, body_md: &str) -> String {
    let escaped_title = stvdy_core::render::escape(title);
    REPORT_V1
        .replace("{{lang}}", DEFAULT_LANG)
        .replace("{{doc_title}}", &escaped_title)
        .replace("{{body}}", &markdown_to_html(body_md))
}

/// The body as HTML.
///
/// Block by block: a heading, a list, a block quote or a paragraph. A paragraph ends at a
/// blank line, and its lines are joined with a space, which is Markdown's own rule and the
/// reason a model that wraps its prose is not punished for it.
pub fn markdown_to_html(markdown: &str) -> String {
    let mut html = String::with_capacity(markdown.len() + markdown.len() / 4);
    let mut paragraph: Vec<&str> = Vec::new();
    // The open list, if any: `None`, `ul` or `ol`. Closed when a non-item line arrives,
    // because a list that runs into a paragraph is a rendering bug in every reader.
    let mut list: Option<&str> = None;

    for line in markdown.lines() {
        let trimmed = line.trim_end();

        if let Some((level, heading)) = heading_of(trimmed) {
            close_paragraph(&mut paragraph, &mut html);
            close_list(&mut list, &mut html);
            html.push_str(&format!("<h{level}>{}</h{level}>", inline(heading)));
            continue;
        }

        if let Some(item) = list_item_of(trimmed) {
            close_paragraph(&mut paragraph, &mut html);
            if list != Some(item.0) {
                close_list(&mut list, &mut html);
                html.push_str(&format!("<{}>", item.0));
                list = Some(item.0);
            }
            html.push_str(&format!("<li>{}</li>", inline(item.1)));
            continue;
        }

        if let Some(quote) = trimmed.strip_prefix("> ") {
            close_paragraph(&mut paragraph, &mut html);
            close_list(&mut list, &mut html);
            html.push_str(&format!("<blockquote>{}</blockquote>", inline(quote)));
            continue;
        }

        if trimmed.trim().is_empty() {
            close_paragraph(&mut paragraph, &mut html);
            close_list(&mut list, &mut html);
            continue;
        }

        close_list(&mut list, &mut html);
        paragraph.push(trimmed);
    }

    close_paragraph(&mut paragraph, &mut html);
    close_list(&mut list, &mut html);
    html
}

/// `#`, `##` or `###` and the text after it.
///
/// Bounded to three levels because that is what the report body uses — `## ` is written by
/// `compose_body_md` for each section, and the model's own subheadings are `### `. A
/// deeper run of `#` is text, not a heading, which is the safe direction: it prints as the
/// characters the model wrote rather than as a heading it did not.
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    for level in (1..=3usize).rev() {
        let marker = "#".repeat(level);
        if let Some(rest) = trimmed.strip_prefix(&marker)
            && let Some(text) = rest.strip_prefix(' ')
        {
            let text = text.trim_end_matches('#').trim();
            if !text.is_empty() {
                return Some((level, text));
            }
        }
    }
    None
}

/// A list item: its tag and its text.
///
/// `- `, `* ` and `+ ` open an unordered list; `1. ` (any number) opens an ordered one. A
/// `-` with no space is a dash, not a list marker.
fn list_item_of(line: &str) -> Option<(&'static str, &str)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(marker) {
            return Some(("ul", text.trim()));
        }
    }

    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if let Some(text) = rest.strip_prefix(". ") {
            return Some(("ol", text.trim()));
        }
    }
    None
}

fn close_paragraph(paragraph: &mut Vec<&str>, html: &mut String) {
    if paragraph.is_empty() {
        return;
    }
    let text = paragraph.join(" ");
    let text = text.trim();
    if text.is_empty() {
        paragraph.clear();
        return;
    }
    // The prompt's honesty note, marked rather than hidden: a student reading the printed
    // report has to be able to see which parts their sources did not cover.
    let class = if text.starts_with("(Not covered in your sources") {
        " class=\"gap\""
    } else {
        ""
    };
    html.push_str(&format!("<p{class}>{}</p>", inline(text)));
    paragraph.clear();
}

fn close_list(list: &mut Option<&'static str>, html: &mut String) {
    if let Some(tag) = list.take() {
        html.push_str(&format!("</{tag}>"));
    }
}

/// One line's inline markup, with everything escaped first.
///
/// The order is the security property, not a detail: `escape` runs before any tag is
/// written, so the only `<` in the output is one this function put there.
fn inline(text: &str) -> String {
    let escaped = stvdy_core::render::escape(text);
    let with_citations = citations(&escaped);
    emphasis(&with_citations)
}

/// `[3]` and `[1, 4]` become superscript citation markers.
///
/// Only digits, commas and spaces inside the brackets: the section prompt asks the model to
/// cite with labels copied from the sources, and anything else in brackets — a Markdown
/// link, a set, a bracket in prose — is left exactly as written. A label that no source
/// carried is the server's to resolve (A17), and a report's labels are resolved against the
/// chunks the section call sent.
fn citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'['
            && let Some(close) = text[index + 1..].find(']')
        {
            let end = index + 1 + close;
            let inner = &text[index + 1..end];
            let is_labels = !inner.is_empty()
                && inner
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ',' || c == ' ');
            if is_labels {
                out.push_str("<sup class=\"cite\">[");
                out.push_str(inner);
                out.push_str("]</sup>");
                index = end + 1;
                continue;
            }
        }

        // Advance one character, not one byte: the text is Indonesian as often as English
        // and a byte-wise copy would split a multi-byte character.
        let character = text[index..].chars().next().unwrap_or(']');
        out.push(character);
        index += character.len_utf8();
    }

    out
}

/// `**bold**`, `*italic*` and `` `code` ``.
///
/// Hand-written rather than pulled from a crate, for the same reason the block scanner is:
/// three markers over already-escaped text is smaller than a dependency, and it cannot
/// accept anything this function does not name.
fn emphasis(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;

    while !rest.is_empty() {
        if let Some(body) = delimited(rest, "**", "**") {
            out.push_str(&format!("<strong>{}</strong>", emphasis_inline(body)));
            rest = &rest[body.len() + 4..];
            continue;
        }
        if let Some(body) = delimited(rest, "`", "`") {
            out.push_str(&format!("<code>{}</code>", emphasis_inline(body)));
            rest = &rest[body.len() + 2..];
            continue;
        }
        if let Some(body) = delimited(rest, "*", "*") {
            out.push_str(&format!("<em>{}</em>", emphasis_inline(body)));
            rest = &rest[body.len() + 2..];
            continue;
        }

        let character = rest.chars().next().unwrap_or(']');
        out.push(character);
        rest = &rest[character.len_utf8()..];
    }

    out
}

/// Nested emphasis inside an already-matched run: bold and code only, so the scan cannot
/// recurse forever on a line that is nothing but asterisks.
fn emphasis_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while !rest.is_empty() {
        if let Some(body) = delimited(rest, "**", "**") {
            out.push_str(&format!("<strong>{body}</strong>"));
            rest = &rest[body.len() + 4..];
            continue;
        }
        if let Some(body) = delimited(rest, "`", "`") {
            out.push_str(&format!("<code>{body}</code>"));
            rest = &rest[body.len() + 2..];
            continue;
        }
        let character = rest.chars().next().unwrap_or(']');
        out.push(character);
        rest = &rest[character.len_utf8()..];
    }

    out
}

/// The text between a pair of delimiters, when there is a closing one with something
/// inside it.
///
/// An empty run is refused, which is what stops `**` and `*` from matching each other and
/// swallowing the rest of the line.
fn delimited<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(open)?;
    let end = rest.find(close)?;
    let body = &rest[..end];
    if body.is_empty() {
        return None;
    }
    Some(body)
}

/// Renders the report to a PDF through the pooled Chromium.
///
/// On demand, not as a job: the student is waiting for a download they asked for, and the
/// document is one page-per-section of prose rather than the multi-second, retryable render
/// the cheatsheet pipeline exists for. The browser is still the pooled one, so a report
/// print cannot launch a second Chromium.
pub async fn render_report_pdf(state: &AppState, title: &str, body_md: &str) -> AppResult<Vec<u8>> {
    let renderer = state.renderer.clone().ok_or_else(|| {
        AppError::Provider(
            "this server has no Chromium renderer configured, so PDF export is unavailable"
                .to_string(),
        )
    })?;

    let html = render_report_html(title, body_md);
    let running_header = title.to_string();

    // `Renderer::render` is blocking: it drives a browser over CDP, and running it on an
    // async worker thread would stall every other request that thread is holding.
    let pdf = tokio::task::spawn_blocking(move || renderer.render(&html, &running_header))
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("the render task failed: {error}")))?
        .map_err(|error| AppError::Provider(format!("rendering the report: {error}")))?;

    tracing::info!(
        template = REPORT_V1_VERSION,
        pages = pdf.page_count,
        "printed a report"
    );

    Ok(pdf.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report is generated from a student's documents, so a tag in the text is escaped
    /// and shown literally. This is the property the whole phase rests on, asserted rather
    /// than assumed: the failure it prevents is a `<script>` from a source document running
    /// in a page the app renders.
    #[test]
    fn markup_in_the_body_is_escaped_not_interpreted() {
        let html = markdown_to_html("Halo <script>alert(1)</script> & <b>tebal</b>");

        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&amp;"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
    }

    /// The tags the conversion writes are the only tags in the output, and each block kind
    /// maps to the element it means.
    #[test]
    fn the_blocks_the_prompt_asks_for_are_the_ones_rendered() {
        let markdown = "# Judul\n\n## Bagian\n\nParagraf pertama\nterus di baris berikutnya.\n\n\
                        - satu\n- dua\n\n1. pertama\n2. kedua\n\n> kutipan\n\n`kode` dan **tebal** dan *miring*";
        let html = markdown_to_html(markdown);

        assert!(html.contains("<h1>Judul</h1>"), "{html}");
        assert!(html.contains("<h2>Bagian</h2>"), "{html}");
        // Two lines of one paragraph are one paragraph, joined: a model that wraps its
        // prose must not produce two paragraphs.
        assert!(
            html.contains("<p>Paragraf pertama terus di baris berikutnya.</p>"),
            "{html}"
        );
        assert!(
            html.contains("<ul><li>satu</li><li>dua</li></ul>"),
            "{html}"
        );
        assert!(
            html.contains("<ol><li>pertama</li><li>kedua</li></ol>"),
            "{html}"
        );
        assert!(html.contains("<blockquote>kutipan</blockquote>"), "{html}");
        assert!(html.contains("<code>kode</code>"), "{html}");
        assert!(html.contains("<strong>tebal</strong>"), "{html}");
        assert!(html.contains("<em>miring</em>"), "{html}");
    }

    /// A citation label becomes a superscript marker, and a bracket that is not a label is
    /// left alone: a Markdown link or a set in prose must not be rewritten into a citation
    /// the student cannot open (A17).
    #[test]
    fn only_label_brackets_become_citations() {
        let html = markdown_to_html("Klaim pertama [1] dan kedua [2, 3].");

        assert!(html.contains("<sup class=\"cite\">[1]</sup>"), "{html}");
        assert!(html.contains("<sup class=\"cite\">[2, 3]</sup>"), "{html}");

        let html = markdown_to_html("Lihat [catatan](https://example.test) dan [x].");
        assert!(!html.contains("<sup"), "{html}");
        assert!(html.contains("[catatan]"), "{html}");
    }

    /// The prompt's honesty note is marked, so a printed report shows which parts the
    /// student's sources did not cover.
    #[test]
    fn a_not_covered_note_is_marked() {
        let html = markdown_to_html("(Not covered in your sources: tidak ada materi ini)");
        assert!(html.contains("<p class=\"gap\">"), "{html}");
    }

    /// The document is a function of its arguments: the same body renders the same bytes,
    /// which is what makes a repeated export free and identical.
    #[test]
    fn rendering_is_deterministic() {
        let body = "# A\n\n## B\n\nIsi [1].";
        assert_eq!(render_report_html("T", body), render_report_html("T", body));
    }

    /// The title reaches the document escaped, and the template's placeholders are all
    /// filled: a literal `{{…}}` in the printed page is the same class of bug as an
    /// unfilled prompt variable.
    #[test]
    fn the_template_is_filled_and_the_title_is_escaped() {
        let html = render_report_html("Laporan <UTS>", "Isi");

        assert!(html.contains("Laporan &lt;UTS&gt;"), "{html}");
        assert!(!html.contains("<UTS>"), "{html}");
        assert!(!html.contains("{{"), "no placeholder may survive: {html}");
        assert!(html.contains("lang=\"id\""), "{html}");
        // The sentinel the server's renderer waits for: without it every report print would
        // fail the guard, which is a real defect rather than a missing nicety.
        assert!(html.contains("dataset.mathReady"), "{html}");
    }

    /// A report's body is prose written by a model, so the scan must terminate and lose no
    /// text whatever the markers do. Two shapes matter: a marker with no partner at all, and
    /// a lone marker that happens to pair with a later one — the second is ordinary
    /// emphasis, and the property to hold is that the words are all still there.
    #[test]
    fn an_unpaired_marker_is_text_and_loses_nothing() {
        let html = markdown_to_html("* tidak ditutup sama sekali");
        assert!(html.contains("tidak ditutup sama sekali"), "{html}");
        assert!(!html.contains("<em>"), "{html}");
        assert!(!html.contains("<strong>"), "{html}");

        let html = markdown_to_html("** tidak ditutup dan * juga tidak");
        for word in ["tidak", "ditutup", "dan", "juga"] {
            assert!(html.contains(word), "no word may be lost: {html}");
        }
        assert!(
            !html.contains("<strong>"),
            "no partner means no bold: {html}"
        );
    }
}
