//! The cheatsheet reply contract, through the generation entry point.
//!
//! The block model itself — the per-kind rules, the SVG allowlist, escaping — is
//! `tests/render.rs`, which is where the renderer's own contracts live. What is tested here
//! is the boundary the *job* crosses: a reply arrives as text from a model, is parsed into
//! the renderer's input type, and the failure that reaches the student is a document that
//! never renders or renders with a field missing. A provider that fences its reply, a model
//! that answers a drifted schema, and a reply over the cap are the three shapes that
//! actually happen, and each of them is a retry decision rather than a silent loss.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::generation::cheatsheet::{Blocks, MAX_CHAPTERS, parse_blocks};
use stvdy_core::generation::{GenerationError, is_retryable};
use stvdy_core::render::{Cover, cheatsheet_v1, render_html};
use uuid::Uuid;

/// A document with one chapter, one section and the given blocks.
fn document(blocks: &str) -> String {
    format!(
        r#"{{"title":"Jaringan","subtitle":"Ringkasan","chapters":[
             {{"title":"Lapisan","sections":[{{"title":"Model","blocks":[{blocks}]}}]}}]}}"#
    )
}

fn text(text: &str) -> String {
    format!(r#"{{"kind":"text","text":{text:?},"labels":[1]}}"#)
}

fn labels() -> ContextLabels {
    ContextLabels::new(
        vec![LabeledChunk {
            label: 1,
            chunk_id: Uuid::from_u128(11),
            source_id: Uuid::from_u128(1),
            page: Some(3),
        }],
        vec![],
    )
}

fn cover() -> Cover {
    Cover {
        course: "Jaringan Komputer".to_string(),
        exam: "UTS".to_string(),
        exam_date: "2026-11-01".to_string(),
        source_count: 1,
        generated_at: "2026-10-12 07:00".to_string(),
        lang: "id".to_string(),
    }
}

/// The generation contract *is* the renderer's input type, so the job's chain composes
/// without a conversion step. This asserts the chain the job runs — parse, then render —
/// against the type the renderer takes, so a second block definition introduced here would
/// have to fail a test rather than be noticed at the first render.
#[test]
fn a_parsed_reply_renders_through_the_renderer_without_a_conversion() {
    let blocks: Blocks = parse_blocks(&document(&text("TCP memakai handshake."))).unwrap();

    let rendered = render_html(&blocks, &cheatsheet_v1(), &labels(), &cover()).unwrap();

    assert!(rendered.html.contains("Lapisan"), "{}", rendered.html);
    assert!(
        rendered.html.contains("TCP memakai handshake."),
        "the block's text must reach the document"
    );
    assert!(
        rendered.dropped.is_empty(),
        "a document the contract accepted must render whole: {:?}",
        rendered.dropped
    );
}

/// A provider occasionally adds a fence despite the contract, and a cheatsheet reply is the
/// largest JSON object in the app, which is where a fence is most likely to appear.
#[test]
fn a_fenced_reply_parses() {
    let raw = format!("```json\n{}\n```", document(&text("a")));
    assert_eq!(parse_blocks(&raw).unwrap().chapters.len(), 1);
}

/// Prose around the object is not guessed at: guessing which part of a reply is the JSON is
/// how a half-parsed document ships, and a cheatsheet is stored as the validated blocks, so
/// a half-parsed one is what re-render would keep reproducing.
#[test]
fn prose_around_the_document_is_not_guessed_at() {
    let raw = format!("Here is your cheatsheet:\n{}", document(&text("a")));
    let error = parse_blocks(&raw).unwrap_err();
    assert!(matches!(error, GenerationError::Json(_)), "{error:?}");
    assert!(is_retryable(&error));
}

/// A required field the module documents and the model omits is refused by name, because
/// the retry's corrective text quotes the message: a bare "invalid JSON" leaves the model
/// guessing which field to add.
#[test]
fn a_missing_required_field_is_refused_by_name() {
    let titleless = r#"{"subtitle":"S","chapters":[{"title":"B","sections":[{"title":"S","blocks":[{"kind":"text","text":"a","labels":[]}]}]}]}"#;
    let error = parse_blocks(titleless).unwrap_err();
    assert!(matches!(error, GenerationError::Schema(_)), "{error:?}");
    assert!(error.to_string().contains("title"), "{error}");

    let textless = parse_blocks(&document(r#"{"kind":"text","labels":[]}"#)).unwrap_err();
    assert!(error_names(&textless, "text"), "{textless}");

    let sectionless =
        parse_blocks(r#"{"title":"T","subtitle":"S","chapters":[{"title":"B"}]}"#).unwrap_err();
    assert!(error_names(&sectionless, "sections"), "{sectionless}");
}

/// An empty section list and an empty block list are both refused with the path to the
/// place in the document. A chapter with no sections renders as a heading with nothing
/// under it, and a student's printed page with a gap in it is worse than a retry.
#[test]
fn a_chapter_with_no_sections_or_a_section_with_no_blocks_is_refused() {
    let no_sections =
        parse_blocks(r#"{"title":"T","subtitle":"S","chapters":[{"title":"B","sections":[]}]}"#)
            .unwrap_err();
    assert!(
        error_names(&no_sections, "chapter 0 has no sections"),
        "{no_sections}"
    );

    let no_blocks = parse_blocks(
        r#"{"title":"T","subtitle":"S","chapters":[{"title":"B","sections":[{"title":"S","blocks":[]}]}]}"#,
    )
    .unwrap_err();
    assert!(
        error_names(&no_blocks, "chapter 0 section 0 has no blocks"),
        "{no_blocks}"
    );
}

/// A drifted prompt fails loudly rather than dropping a field the model invented. The
/// nested case matters as much as the top-level one: a `legend` row that grows a field is
/// the shape a formula's symbols come in, and a silently ignored key there would print a
/// legend that does not match the formula.
#[test]
fn an_unknown_field_is_refused_by_name() {
    let document_level = parse_blocks(
        r#"{"title":"T","subtitle":"S","subject":"Jaringan","chapters":[{"title":"B","sections":[{"title":"S","blocks":[{"kind":"text","text":"a","labels":[]}]}]}]}"#,
    )
    .unwrap_err();
    assert!(error_names(&document_level, "subject"), "{document_level}");

    let nested = parse_blocks(&document(
        r#"{"kind":"formula","latex":"x","legend":[{"symbol":"x","meaning":"a","unit":null,"si":"m"}],"labels":[]}"#,
    ))
    .unwrap_err();
    assert!(error_names(&nested, "si"), "{nested}");
}

/// Pins both sides of the cap: the limit itself is accepted, one chapter more is refused.
/// A truncating implementation would return `Ok` with fewer chapters, so the passing case
/// asserts the count too — a cheatsheet that silently lost its last chapter is a document
/// the student studies from and fails the exam with.
#[test]
fn a_reply_at_the_cap_parses_and_one_over_is_refused() {
    let chapter = |index: usize| {
        serde_json::json!({
            "title": format!("Bab {index}"),
            "sections": [{"title": "S", "blocks": [{"kind": "text", "text": "a", "labels": []}]}],
        })
    };
    let at_cap = serde_json::to_string(&serde_json::json!({
        "title": "T",
        "subtitle": "S",
        "chapters": (0..MAX_CHAPTERS).map(chapter).collect::<Vec<_>>(),
    }))
    .unwrap();
    assert_eq!(parse_blocks(&at_cap).unwrap().chapters.len(), MAX_CHAPTERS);

    let over_cap = serde_json::to_string(&serde_json::json!({
        "title": "T",
        "subtitle": "S",
        "chapters": (0..=MAX_CHAPTERS).map(chapter).collect::<Vec<_>>(),
    }))
    .unwrap();
    let error = parse_blocks(&over_cap).unwrap_err();
    assert!(matches!(error, GenerationError::Schema(_)), "{error:?}");
    assert!(
        is_retryable(&error),
        "a retry is what fixes a drifted count"
    );
}

/// Whether the error's message names the thing that was wrong.
fn error_names(error: &GenerationError, expected: &str) -> bool {
    error.to_string().contains(expected)
}
