//! The cheatsheet renderer: typed blocks in, HTML out.
//!
//! **This module is pure.** It reads no clock, opens no socket, and uses no randomness, so
//! `render(blocks, template)` is a function of its arguments and a re-render of an existing
//! document is byte-identical. That is the property `POST /cheatsheets/:id/re-render`
//! exists for: a template fix re-renders every existing document for free, without
//! re-invoking the model, which would cost credits and produce *different content* for a
//! document the student may already have printed.
//!
//! It is also why this module has no Chromium dependency. Printing the HTML to PDF is a
//! separate step, so everything here is unit-testable without a browser.
//!
//! Two rules the template's own comments state as this renderer's contract:
//!
//! - **The document's structure is the renderer's, not the model's.** The model returns
//!   typed blocks; every tag, class and attribute comes from here. A field containing `<`
//!   is escaped and shown literally.
//! - **SVG ids are prefixed per block.** See [`svg::check_and_prefix`].

pub mod blocks;
pub mod svg;

use crate::citations::ContextLabels;

pub use blocks::{Block, Blocks};

/// The print template. `version` is what lands in `cheatsheets.template_version`, so an
/// admin can list documents rendered by an older one.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    pub text: &'static str,
    pub version: &'static str,
}

/// The template compiled into the binary, so a missing or renamed file is a build error
/// rather than a 500 in front of a student.
pub const CHEATSHEET_V1: &str = include_str!("../../../../templates/cheatsheet.v1.html");

pub fn cheatsheet_v1() -> Template {
    Template {
        text: CHEATSHEET_V1,
        version: "cheatsheet.v1",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("the template is missing its {0} marker")]
    MissingMarker(&'static str),
    #[error("the template has no cover section")]
    MissingCover,
}

/// A block that was refused, with the reason. Recorded rather than dropped silently: a
/// climbing rate means the prompt or the model changed, and the student is owed the
/// knowledge that their document is missing something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedBlock {
    /// `chapter 1 section 2 block 3`, so the reason points at a place in the document.
    pub path: String,
    pub kind: &'static str,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Rendered {
    pub html: String,
    pub dropped: Vec<DroppedBlock>,
}

/// What fills the cover. Every field is either from the blocks or from the request, and
/// none of them is a clock reading: `generated_at` is passed in by the caller so this
/// function stays pure, and the caller is the one that knows what "now" means.
#[derive(Debug, Clone)]
pub struct Cover {
    pub course: String,
    pub exam: String,
    pub exam_date: String,
    pub source_count: usize,
    pub generated_at: String,
    pub lang: String,
}

/// Escapes text for an HTML text node or a quoted attribute value.
///
/// Applied to **every** field that came from the model, without exception, so a source
/// document containing `<script>` cannot become markup in a document the student prints.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Renders the document.
pub fn render_html(
    blocks: &Blocks,
    template: &Template,
    labels: &ContextLabels,
    cover: &Cover,
) -> Result<Rendered, RenderError> {
    let mut dropped = Vec::new();
    let content = render_chapters(blocks, labels, &mut dropped);

    let toc_marker = "<!-- {{toc}} -->";
    let content_marker = "<!-- {{content}} -->";
    if !template.text.contains(toc_marker) {
        return Err(RenderError::MissingMarker("toc"));
    }
    if !template.text.contains(content_marker) {
        return Err(RenderError::MissingMarker("content"));
    }

    let mut html = template.text.replace(toc_marker, &render_toc(blocks));
    html = html.replace(content_marker, &content);

    // The cover's placeholders. `{{braces}}` is the template's own escape for the literal
    // `{{`/`}}` it documents in its header comment, and is replaced last so a cover value
    // containing braces cannot be mistaken for a placeholder.
    for (name, value) in [
        ("{{doc_title}}", escape(&blocks.title)),
        ("{{doc_subtitle}}", escape(&blocks.subtitle)),
        ("{{cover_eyebrow}}", escape(&cover.exam)),
        ("{{course}}", escape(&cover.course)),
        ("{{exam}}", escape(&cover.exam)),
        ("{{exam_date}}", escape(&cover.exam_date)),
        ("{{source_count}}", cover.source_count.to_string()),
        ("{{generated_at}}", escape(&cover.generated_at)),
        ("{{lang}}", escape(&cover.lang)),
        ("{{braces}}", "{{".to_string() + "}}"),
    ] {
        html = html.replace(name, &value);
    }

    Ok(Rendered { html, dropped })
}

/// The table of contents, generated from the chapter and section titles so it always
/// matches the document.
///
/// The anchors are the same ones the chapters emit (`#ch1`, `#ch1-1`), which is why the
/// numbering is derived here and in `render_chapters` from the same index rather than
/// stored.
fn render_toc(blocks: &Blocks) -> String {
    let mut out = String::from(
        "<nav class=\"toc\">\n  <h2 class=\"toc__title\" style=\"border:none;\">Daftar Isi</h2>\n  <ol>\n",
    );
    for (chapter_index, chapter) in blocks.chapters.iter().enumerate() {
        let number = chapter_index + 1;
        out.push_str(&format!(
            "    <li>\n      <a href=\"#ch{number}\"><span class=\"toc__num\">{number}</span>\
             <span>{}</span></a>\n      <ol>\n",
            escape(&chapter.title)
        ));
        for (section_index, section) in chapter.sections.iter().enumerate() {
            out.push_str(&format!(
                "        <li class=\"toc--l2\"><a href=\"#ch{number}-{}\">\
                 <span class=\"toc__num\">{number}.{}</span><span>{}</span></a></li>\n",
                section_index + 1,
                section_index + 1,
                escape(&section.title)
            ));
        }
        out.push_str("      </ol>\n    </li>\n");
    }
    out.push_str("  </ol>\n</nav>");
    out
}

/// The chapters, their sections and their blocks.
fn render_chapters(
    blocks: &Blocks,
    labels: &ContextLabels,
    dropped: &mut Vec<DroppedBlock>,
) -> String {
    let mut out = String::new();
    for (chapter_index, chapter) in blocks.chapters.iter().enumerate() {
        let number = chapter_index + 1;
        out.push_str(&format!(
            "<section class=\"chapter\">\n  <h1 id=\"ch{number}\">\
             <span class=\"chapter-label\">BAB {number}</span>{}</h1>\n",
            escape(&chapter.title)
        ));
        for (section_index, section) in chapter.sections.iter().enumerate() {
            out.push_str(&format!(
                "  <h2 id=\"ch{number}-{}\">{number}.{} {}</h2>\n",
                section_index + 1,
                section_index + 1,
                escape(&section.title)
            ));
            for (block_index, block) in section.blocks.iter().enumerate() {
                let path = format!(
                    "chapter {} section {} block {}",
                    chapter_index + 1,
                    section_index + 1,
                    block_index + 1
                );
                // The SVG id prefix is the document path, not the block index: two
                // sections that each hold a diagram at position 1 would both be `b1`.
                let prefix = format!(
                    "b{}-{}-{}",
                    chapter_index + 1,
                    section_index + 1,
                    block_index + 1
                );
                match render_block(block, labels, &prefix) {
                    Ok(markup) => out.push_str(&markup),
                    Err(reason) => dropped.push(DroppedBlock {
                        path,
                        kind: block.kind(),
                        reason,
                    }),
                }
            }
        }
        out.push_str("</section>\n");
    }
    out
}

/// The citation markers a block's labels resolve to.
///
/// A label that was never sent is dropped rather than rendered as a dangling reference
/// (A17): the block keeps its text, and only the marker is lost.
fn citations(labels: &[u32], map: &ContextLabels) -> String {
    let mut out = String::new();
    for label in labels {
        if map.chunk(*label).is_some() {
            out.push_str(&format!(" <span class=\"src\">[{label}]</span>"));
        }
    }
    out
}

/// One block's markup.
///
/// `prefix` is the block's path through the document (`b1-2-3`), used as the SVG id prefix.
/// It has to be the whole path rather than the block's index within its section, because
/// that index restarts per section and two sections holding a diagram at the same position
/// would emit the same ids.
fn render_block(block: &Block, labels: &ContextLabels, prefix: &str) -> Result<String, String> {
    let cited = |block_labels: &[u32]| citations(block_labels, labels);
    Ok(match block {
        Block::Text(block) => {
            format!("  <p>{}{}</p>\n", escape(&block.text), cited(&block.labels))
        }
        Block::Bullets(block) => {
            let mut out = String::from("  <ul>\n");
            for item in &block.items {
                out.push_str(&format!("    <li>{}</li>\n", escape(item)));
            }
            out.push_str(&format!("  </ul>{}\n", cited(&block.labels)));
            out
        }
        Block::Concept(block) => format!(
            "  <div class=\"box konsep\">\n    <p class=\"box__title\">Konsep</p>\n    \
             <p><span class=\"konsep__term\">{}</span> {}{}</p>\n  </div>\n",
            escape(&block.term),
            escape(&block.text),
            cited(&block.labels)
        ),
        Block::Formula(block) => {
            let mut out = String::from(
                "  <div class=\"box rumus\">\n    <p class=\"box__title\">Rumus</p>\n",
            );
            if let Some(caption) = &block.caption {
                out.push_str(&format!(
                    "    <p class=\"rumus__caption\">{}</p>\n",
                    escape(caption)
                ));
            }
            // The delimiters are the renderer's: the prompt asks for the LaTeX alone, so a
            // model that wraps it in `$$` itself would produce `$$$$`.
            out.push_str(&format!(
                "    <div class=\"rumus__body\">$${}$$</div>\n",
                escape(block.latex.trim_matches('$'))
            ));
            out.push_str(
                "    <table class=\"rumus__legend\">\n      <tr><th>Simbol</th><th>Arti</th>\
                 <th>Satuan</th></tr>\n",
            );
            for row in &block.legend {
                out.push_str(&format!(
                    "      <tr><td class=\"sym\">{}</td><td>{}</td><td>{}</td></tr>\n",
                    escape(&row.symbol),
                    escape(&row.meaning),
                    escape(row.unit.as_deref().unwrap_or("&mdash;"))
                ));
            }
            out.push_str(&format!("    </table>{}\n  </div>\n", cited(&block.labels)));
            out
        }
        Block::QuickFormula(block) => format!(
            "  <div class=\"box trik\">\n    <p class=\"box__title\">\u{26a1} Rumus Cepat</p>\n    \
             <p>${}$ {}</p>\n    <p class=\"trik__note\">Valid hanya jika {}</p>\n  </div>\n",
            escape(block.latex.trim_matches('$')),
            escape(&block.text),
            escape(&block.valid_when)
        ),
        Block::Analogy(block) => format!(
            "  <div class=\"box analogi\">\n    <p class=\"box__title\">\u{1f4e6} Perumpamaan</p>\n    \
             <p>{}{}</p>\n  </div>\n",
            escape(&block.text),
            cited(&block.labels)
        ),
        Block::Trap(block) => format!(
            "  <div class=\"box jebakan\">\n    <p class=\"box__title\">\u{26a0}\u{fe0f} Jebakan</p>\n    \
             <p>{}{}</p>\n  </div>\n",
            escape(&block.text),
            cited(&block.labels)
        ),
        Block::Memorize(block) => {
            let mut out = String::from(
                "  <div class=\"box hafalan\">\n    <p class=\"box__title\">\u{1f3af} Kata Kunci Hafalan</p>\n    <ul>\n",
            );
            for item in &block.items {
                out.push_str(&format!("      <li>{}</li>\n", escape(item)));
            }
            out.push_str(&format!("    </ul>{}\n  </div>\n", cited(&block.labels)));
            out
        }
        Block::Table(block) => {
            let class = if block.wide {
                "data data--wide"
            } else {
                "data"
            };
            let mut out = format!("  <table class=\"{class}\">\n");
            if let Some(caption) = &block.caption {
                out.push_str(&format!("    <caption>{}</caption>\n", escape(caption)));
            }
            out.push_str("    <thead>\n      <tr>");
            for header in &block.headers {
                out.push_str(&format!("<th>{}</th>", escape(header)));
            }
            out.push_str("</tr>\n    </thead>\n    <tbody>\n");
            for row in &block.rows {
                out.push_str("      <tr>");
                for cell in row {
                    out.push_str(&format!("<td>{}</td>", escape(cell)));
                }
                out.push_str("</tr>\n");
            }
            out.push_str(&format!(
                "    </tbody>{}\n  </table>\n",
                cited(&block.labels)
            ));
            out
        }
        Block::Question(block) => {
            let badge = match block.likelihood.as_str() {
                "high" => "Tinggi",
                "medium" => "Sedang",
                _ => "Rendah",
            };
            let mut out = format!(
                "  <div class=\"qa\">\n    <div class=\"soal\">\n      <div class=\"soal__header\">\n        \
                 <span>Soal</span>\n        <span class=\"badge badge--{}\">{badge}</span>\n      </div>\n      \
                 <div class=\"soal__prompt\">\n        <p>{}</p>\n      </div>\n    </div>\n\n    \
                 <div class=\"jawaban\">\n      <p class=\"jawaban__title\">Jawaban</p>\n      <ol>\n",
                escape(&block.likelihood),
                escape(&block.prompt)
            );
            for step in &block.answer.steps {
                out.push_str(&format!("        <li>{}</li>\n", escape(step)));
            }
            out.push_str(&format!(
                "      </ol>\n      <p class=\"final\"><span class=\"final__label\">Jawaban akhir</span> {}</p>{}\n    </div>\n  </div>\n",
                escape(&block.answer.final_answer),
                cited(&block.labels)
            ));
            out
        }
        Block::Diagram(block) => {
            let mut out = format!(
                "  <figure>\n    <pre class=\"diagram\">{}</pre>\n",
                escape(&block.text)
            );
            if let Some(caption) = &block.caption {
                out.push_str(&format!(
                    "    <figcaption>{}</figcaption>\n",
                    escape(caption)
                ));
            }
            out.push_str("  </figure>\n");
            out
        }
        Block::Svg(block) => {
            let svg =
                svg::check_and_prefix(&block.svg, prefix).map_err(|rejection| rejection.reason)?;
            let mut out = format!("  <figure>\n    {svg}\n");
            if let Some(caption) = &block.caption {
                out.push_str(&format!(
                    "    <figcaption>{}</figcaption>\n",
                    escape(caption)
                ));
            }
            out.push_str("  </figure>\n");
            out
        }
    })
}
