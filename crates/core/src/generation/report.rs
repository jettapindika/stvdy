//! The report contracts (`prompts/report_outline.v1.md`, `prompts/report_section.v1.md`),
//! parsed.
//!
//! A report is generated in two steps and they are two different contracts. The outline is
//! one JSON object, and it is the object the *student* edits before anything is written, so
//! it is validated here rather than trusted: a route that accepted a replacement outline
//! without parsing it would store a shape the section call cannot read. The sections are
//! Markdown, one call per section, so the second contract is what keeps a section reply from
//! being stored when it is empty or when the model answered the outline's schema instead of
//! the section's.
//!
//! Neither contract resolves `[n]` labels. The label set is request-scoped, and a section's
//! labels are the ones the composer sent for *that* call; the server resolves them (A17).

use crate::generation::{GenerationError, strip_code_fence};

/// The most sections one outline may carry.
///
/// `prompts/report_outline.v1.md` asks for 4-8, and this sits above that range on purpose:
/// a model that splits one section in two is not making a mistake worth a retry, and the
/// student may add sections to the outline by hand. What it does stop is a reply that
/// ignores the instruction and returns an unbounded list, which the report screen would
/// then render as an unbounded number of section calls.
pub const MAX_REPORT_SECTIONS: usize = 12;

/// The outline reply, exactly as `prompts/report_outline.v1.md` documents it:
/// `{"title":"string","sections":[{"heading":"string","purpose":"string","labels":[1,4],
/// "gaps":false}]}`.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that invents a
/// field produces a parse error and a retry rather than an outline with a silently dropped
/// field. The field names are the prompt's, not a paraphrase, so the schema the model reads
/// and the struct the server stores are the same document.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportOutline {
    pub title: String,
    pub sections: Vec<OutlineSection>,
}

/// One section of the outline.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutlineSection {
    pub heading: String,
    /// What the section will establish, not what it is about: the prompt asks for the
    /// former because that is what the section call is given.
    pub purpose: String,
    /// The chunk labels that will support this section, as shown inside `<sources>`.
    pub labels: Vec<u32>,
    /// Required output, not decoration: the roadmap's exit gate and the spec's `## Reports`
    /// both read it, and the report screen marks these sections for the student. A struct
    /// without the field could not represent the contract, and defaulting it would silently
    /// claim every section is fully supported by the sources, which is the one claim the
    /// flag exists to make honestly.
    pub gaps: bool,
}

/// The section reply: Markdown, with the `[n]` labels and the inline
/// `(Not covered in your sources: …)` note the prompt asks for.
///
/// Not a JSON reply, so there is nothing to deserialise and no schema to check against: the
/// model returns prose and this wraps it so the job passes a named type rather than a bare
/// `String`. `body_md` is the field name the API and `reports.body_md` use.
#[derive(Debug, Clone)]
pub struct ReportSection {
    pub body_md: String,
}

/// Parses a model reply into an outline.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite the
/// contract. It does **not** tolerate prose around the object: guessing which part is the
/// JSON is how a half-parsed outline ships, and this object is shown to the student for
/// editing, so a section silently lost here is a section they never see.
pub fn parse_outline(raw: &str) -> Result<ReportOutline, GenerationError> {
    let text = strip_code_fence(raw);
    let outline: ReportOutline = serde_json::from_str(text).map_err(|error| {
        // `serde_json` reports both "not JSON" and "wrong shape" through the same error
        // type; the message distinguishes them, and the caller only needs to know a retry
        // is worth one attempt. A missing or mistyped field lands in `Schema`, and serde
        // names the field in the message, which is what the retry's corrective text quotes.
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    if outline.sections.is_empty() {
        // The model answered the question it was asked and the answer was "no sections".
        // Retrying would spend a second call to be told the same thing, and an outline the
        // student can edit is not something to invent on their behalf.
        return Err(GenerationError::Empty);
    }
    if outline.sections.len() > MAX_REPORT_SECTIONS {
        return Err(GenerationError::Schema(format!(
            "the model returned {} sections, more than the {MAX_REPORT_SECTIONS} allowed",
            outline.sections.len()
        )));
    }

    if outline.title.trim().is_empty() {
        return Err(GenerationError::Schema(
            "the outline has no title".to_string(),
        ));
    }

    for (index, section) in outline.sections.iter().enumerate() {
        if section.heading.trim().is_empty() {
            return Err(GenerationError::Schema(format!(
                "section {index}: the heading is empty"
            )));
        }
        if section.purpose.trim().is_empty() {
            return Err(GenerationError::Schema(format!(
                "section {index}: the purpose is empty, so the section call would have \
                 nothing to establish"
            )));
        }
    }

    Ok(outline)
}

/// Parses a model reply into a report section.
///
/// Markdown, so the fence is stripped the same way a JSON reply's is (a provider that adds
/// ```` ```markdown ```` around a section body is the same provider behaviour), and what is
/// left is stored as the model wrote it apart from the surrounding whitespace the shared
/// stripper trims: the `[n]` labels are resolved by the server against the labels this
/// request sent, and rewriting them here would lose the only link a claim has.
pub fn parse_section(raw: &str) -> Result<ReportSection, GenerationError> {
    let body_md = strip_code_fence(raw);

    if body_md.trim().is_empty() {
        // Nothing to store. `Empty` rather than a schema failure for the same reason as an
        // empty outline: the model answered, and the answer was "nothing".
        return Err(GenerationError::Empty);
    }

    if is_json_container(body_md) {
        // The model answered the outline's schema, or wrapped the Markdown in
        // `{"body_md": …}`, or returned the blocks as a list of strings. Storing that would
        // put JSON in `reports.body_md`, which the student reads as the report and the
        // Markdown export writes out as-is: the failure is visible only after the credits
        // are spent. A schema failure is retryable, so the second attempt is told what went
        // wrong.
        return Err(GenerationError::Schema(
            "the section reply is JSON, not Markdown; the module asks for the section's \
             Markdown with its [n] labels"
                .to_string(),
        ));
    }

    Ok(ReportSection {
        body_md: body_md.to_string(),
    })
}

/// True when the text is one JSON object or array and nothing else.
///
/// Deliberately not "starts with `{`": a section that opens with a brace is legitimate
/// Markdown (a code block, set notation), and only text that parses whole as a JSON
/// container is the model answering the wrong schema. Scalars are left alone on purpose —
/// a one-line section that is a quoted sentence is valid JSON *and* valid Markdown, so
/// refusing it would refuse a section the student can read.
fn is_json_container(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text.trim())
        .map(|value| value.is_object() || value.is_array())
        .unwrap_or(false)
}
