//! The cheatsheet's typed blocks.
//!
//! Generation produces **data**, never markup: the model returns these structs and the
//! renderer owns every tag. A field containing `<` is escaped and shown literally, so a
//! prompt-injected `<script>` in a source document cannot become markup in a document the
//! student prints.
//!
//! The block set is the one `prompts/cheatsheet.v1.md` documents, and `deny_unknown_fields`
//! is what keeps the two in step: a model that invents a field produces a parse error and a
//! retry rather than a block with a silently dropped field.

use crate::generation::{GenerationError, strip_code_fence};

/// The most chapters one document may carry. The prompt asks for 2-6; this is what makes
/// the request enforceable, and it is above the prompt's range so a model that adds one
/// chapter is not punished for it.
pub const MAX_CHAPTERS: usize = 8;
pub const MAX_SECTIONS_PER_CHAPTER: usize = 8;
pub const MAX_BLOCKS_PER_SECTION: usize = 12;
/// The most table columns, because the template's `.data` table is a print layout and a
/// twelve-column table cannot fit an A4 page whatever the font size.
pub const MAX_TABLE_COLUMNS: usize = 8;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocks {
    pub title: String,
    pub subtitle: String,
    #[serde(default)]
    pub course: Option<String>,
    #[serde(default)]
    pub exam: Option<String>,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chapter {
    pub title: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
    pub blocks: Vec<Block>,
}

/// One block. The `kind` tag is the discriminator, and each variant's fields are the
/// module's own schema.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Block {
    Text(Text),
    Bullets(Bullets),
    Concept(Concept),
    Formula(Formula),
    QuickFormula(QuickFormula),
    Analogy(Analogy),
    Trap(Trap),
    Memorize(Memorize),
    Table(Table),
    Question(Question),
    Diagram(Diagram),
    Svg(Svg),
}

/// The field every block carries: the chunk labels its claims came from.
///
/// Written out on each struct rather than shared, because serde's `#[serde(flatten)]`
/// cannot be combined with an internally tagged enum's `deny_unknown_fields`, and the loud
/// failure on an unknown field is worth the repetition: it is what turns a drifted prompt
/// into a retry instead of a block with a silently dropped field.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub text: String,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bullets {
    pub items: Vec<String>,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Concept {
    pub term: String,
    pub text: String,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Formula {
    /// The formula in LaTeX, without delimiters: the renderer adds `$$…$$`.
    pub latex: String,
    #[serde(default)]
    pub caption: Option<String>,
    /// What each symbol means, which is what makes a formula usable under exam pressure.
    pub legend: Vec<LegendRow>,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegendRow {
    pub symbol: String,
    pub meaning: String,
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickFormula {
    /// The LaTeX, without delimiters.
    pub latex: String,
    pub text: String,
    /// When the shortcut is valid. Required, because a mnemonic rule with no stated
    /// limits is the one a student misapplies.
    pub valid_when: String,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Analogy {
    pub text: String,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trap {
    pub text: String,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memorize {
    pub items: Vec<String>,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    #[serde(default)]
    pub caption: Option<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// The prompt asks for this when there are five or more columns, so the template can
    /// shrink the font.
    #[serde(default)]
    pub wide: bool,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub prompt: String,
    /// `high | medium | low`: the model's estimate of how likely this appears.
    pub likelihood: String,
    pub answer: Answer,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    /// The reasoning steps, in order.
    pub steps: Vec<String>,
    /// The final answer, on its own so the template can set it apart.
    ///
    /// Renamed because `final` is a reserved keyword: the JSON field is `final`, which is
    /// what the prompt's schema documents.
    #[serde(rename = "final")]
    pub final_answer: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagram {
    /// An ASCII diagram in a monospace block. The prompt caps it at 60 characters wide so
    /// it cannot overflow the page; the renderer does not enforce that, because a diagram
    /// one character over is still readable and dropping it would lose the content.
    pub text: String,
    #[serde(default)]
    pub caption: Option<String>,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Svg {
    /// The SVG source. Validated against a closed allowlist by `render::svg`; an element
    /// or attribute outside it drops the whole block rather than being stripped.
    pub svg: String,
    #[serde(default)]
    pub caption: Option<String>,
    /// The chunk labels this block's claims came from, copied from `<sources>`.
    #[serde(default)]
    pub labels: Vec<u32>,
}

impl Block {
    /// The labels this block claims, whatever its kind.
    pub fn labels(&self) -> &[u32] {
        match self {
            Block::Text(block) => &block.labels,
            Block::Bullets(block) => &block.labels,
            Block::Concept(block) => &block.labels,
            Block::Formula(block) => &block.labels,
            Block::QuickFormula(block) => &block.labels,
            Block::Analogy(block) => &block.labels,
            Block::Trap(block) => &block.labels,
            Block::Memorize(block) => &block.labels,
            Block::Table(block) => &block.labels,
            Block::Question(block) => &block.labels,
            Block::Diagram(block) => &block.labels,
            Block::Svg(block) => &block.labels,
        }
    }

    /// The kind, for a log line or a drop reason.
    pub fn kind(&self) -> &'static str {
        match self {
            Block::Text(_) => "text",
            Block::Bullets(_) => "bullets",
            Block::Concept(_) => "concept",
            Block::Formula(_) => "formula",
            Block::QuickFormula(_) => "quickformula",
            Block::Analogy(_) => "analogy",
            Block::Trap(_) => "trap",
            Block::Memorize(_) => "memorize",
            Block::Table(_) => "table",
            Block::Question(_) => "question",
            Block::Diagram(_) => "diagram",
            Block::Svg(_) => "svg",
        }
    }
}

/// Parses a model reply into blocks.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite the
/// contract. It does not tolerate prose around the object: guessing which part is the JSON
/// is how a half-parsed document ships.
pub fn parse_blocks(raw: &str) -> Result<Blocks, GenerationError> {
    let text = strip_code_fence(raw);
    let blocks: Blocks = serde_json::from_str(text).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    if blocks.chapters.is_empty() {
        return Err(GenerationError::Empty);
    }
    if blocks.chapters.len() > MAX_CHAPTERS {
        return Err(GenerationError::Schema(format!(
            "the model returned {} chapters, more than the {MAX_CHAPTERS} allowed",
            blocks.chapters.len()
        )));
    }

    for (index, chapter) in blocks.chapters.iter().enumerate() {
        if chapter.sections.is_empty() {
            return Err(GenerationError::Schema(format!(
                "chapter {index} has no sections"
            )));
        }
        if chapter.sections.len() > MAX_SECTIONS_PER_CHAPTER {
            return Err(GenerationError::Schema(format!(
                "chapter {index} has {} sections, more than the {MAX_SECTIONS_PER_CHAPTER} allowed",
                chapter.sections.len()
            )));
        }
        if chapter.title.trim().is_empty() {
            return Err(GenerationError::Schema(format!(
                "chapter {index} has an empty title"
            )));
        }

        for (section_index, section) in chapter.sections.iter().enumerate() {
            if section.blocks.is_empty() {
                return Err(GenerationError::Schema(format!(
                    "chapter {index} section {section_index} has no blocks"
                )));
            }
            if section.blocks.len() > MAX_BLOCKS_PER_SECTION {
                return Err(GenerationError::Schema(format!(
                    "chapter {index} section {section_index} has {} blocks, more than the \
                     {MAX_BLOCKS_PER_SECTION} allowed",
                    section.blocks.len()
                )));
            }
            if section.title.trim().is_empty() {
                return Err(GenerationError::Schema(format!(
                    "chapter {index} section {section_index} has an empty title"
                )));
            }
            for (block_index, block) in section.blocks.iter().enumerate() {
                validate_block(index, section_index, block_index, block)?;
            }
        }
    }

    Ok(blocks)
}

/// The per-kind rules the type cannot express.
///
/// The index path is in every message so the retry's corrective text can name the block
/// that was wrong, which is the difference between a model fixing it and guessing again.
fn validate_block(
    chapter: usize,
    section: usize,
    index: usize,
    block: &Block,
) -> Result<(), GenerationError> {
    let at = format!("chapter {chapter} section {section} block {index}");
    let bad = |detail: String| GenerationError::Schema(format!("{at}: {detail}"));

    match block {
        Block::Text(block) if block.text.trim().is_empty() => {
            return Err(bad("a text block has no text".to_string()));
        }
        Block::Bullets(block) if block.items.is_empty() => {
            return Err(bad("a bullets block has no items".to_string()));
        }
        Block::Bullets(block) if block.items.iter().any(|item| item.trim().is_empty()) => {
            return Err(bad("a bullets block has an empty item".to_string()));
        }
        Block::Concept(block) if block.term.trim().is_empty() => {
            return Err(bad("a concept block has no term".to_string()));
        }
        Block::Formula(block) if block.latex.trim().is_empty() => {
            return Err(bad("a formula block has no latex".to_string()));
        }
        // A formula without its legend is the one a student cannot use under pressure,
        // which is the whole reason the kind exists.
        Block::Formula(block) if block.legend.is_empty() => {
            return Err(bad(
                "a formula block has no legend; the symbols are what make it usable".to_string(),
            ));
        }
        Block::QuickFormula(block) if block.valid_when.trim().is_empty() => {
            return Err(bad(
                "a quickformula block does not say when it is valid".to_string()
            ));
        }
        Block::Memorize(block) if block.items.is_empty() => {
            return Err(bad("a memorize block has no items".to_string()));
        }
        Block::Table(block) => {
            if block.headers.is_empty() {
                return Err(bad("a table has no headers".to_string()));
            }
            if block.headers.len() > MAX_TABLE_COLUMNS {
                return Err(bad(format!(
                    "a table has {} columns, more than the {MAX_TABLE_COLUMNS} that fit a page",
                    block.headers.len()
                )));
            }
            // A row of the wrong width is a mis-set table rather than a missing one, and
            // the template cannot recover from it.
            for (row_index, row) in block.rows.iter().enumerate() {
                if row.len() != block.headers.len() {
                    return Err(bad(format!(
                        "table row {row_index} has {} cells against {} headers",
                        row.len(),
                        block.headers.len()
                    )));
                }
            }
        }
        Block::Question(block) => {
            if !matches!(block.likelihood.as_str(), "high" | "medium" | "low") {
                return Err(bad(format!(
                    "a question's likelihood must be high, medium or low, not {:?}",
                    block.likelihood
                )));
            }
            if block.answer.steps.is_empty() {
                return Err(bad(
                    "a question's answer has no steps; the reasoning is what is being taught"
                        .to_string(),
                ));
            }
            if block.answer.final_answer.trim().is_empty() {
                return Err(bad("a question's answer has no final line".to_string()));
            }
        }
        Block::Diagram(block) if block.text.trim().is_empty() => {
            return Err(bad("a diagram block has no text".to_string()));
        }
        Block::Svg(block) if block.svg.trim().is_empty() => {
            return Err(bad("an svg block has no svg".to_string()));
        }
        _ => {}
    }
    Ok(())
}
