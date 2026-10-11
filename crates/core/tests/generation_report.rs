//! The report contracts: the outline the student edits, and one section of Markdown.
//!
//! The cases here are the ones that reach a student. An outline is shown for editing, so a
//! section silently lost in parsing is a section they never see; `gaps` is the flag that
//! tells them which sections their sources cannot support, so a defaulted `gaps` would make
//! an unsupported section look supported; and a section reply is Markdown that is stored
//! verbatim and exported as the report, so JSON stored in `body_md` is a document the
//! student only discovers is broken after the credits are spent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::generation::report::{MAX_REPORT_SECTIONS, parse_outline, parse_section};
use stvdy_core::generation::{GenerationError, is_retryable};

/// One section, with the two labels the outline contract carries.
fn section(heading: &str, gaps: bool) -> serde_json::Value {
    serde_json::json!({
        "heading": heading,
        "purpose": format!("establish {heading}"),
        "labels": [1, 4],
        "gaps": gaps,
    })
}

fn outline(sections: Vec<serde_json::Value>) -> String {
    serde_json::to_string(&serde_json::json!({
        "title": "Jaringan Komputer",
        "sections": sections,
    }))
    .unwrap()
}

/// The documented reply parses, and every field arrives with the value the model sent:
/// the heading and purpose are what the section call is given, and the labels are the
/// support the report screen shows against each section.
#[test]
fn the_documented_outline_parses() {
    let parsed = parse_outline(&outline(vec![
        section("Pendahuluan", false),
        section("Kesimpulan", true),
    ]))
    .unwrap();

    assert_eq!(parsed.title, "Jaringan Komputer");
    assert_eq!(parsed.sections.len(), 2);
    assert_eq!(parsed.sections[0].heading, "Pendahuluan");
    assert_eq!(parsed.sections[0].purpose, "establish Pendahuluan");
    assert_eq!(parsed.sections[0].labels, vec![1, 4]);
    assert!(!parsed.sections[0].gaps);
    assert!(parsed.sections[1].gaps);
}

/// `gaps` is required output, and it is read rather than defaulted. A default would make
/// "supported by the sources" the silent meaning of an absent flag, which is the one claim
/// the flag exists to make honestly: the report screen would show no gap note on a section
/// the sources cannot support.
#[test]
fn gaps_is_required_and_is_not_defaulted() {
    let missing = r#"{"title":"T","sections":[{"heading":"H","purpose":"P","labels":[1]}]}"#;
    let error = parse_outline(missing).unwrap_err();
    assert!(
        matches!(error, GenerationError::Schema(_)),
        "a missing flag is a schema failure, not a default: {error:?}"
    );
    assert!(
        error.to_string().contains("gaps"),
        "the message must name the field the retry has to add: {error}"
    );

    // And the two values are distinguished, so a parser that reads `gaps` into a constant
    // fails here.
    let true_reply = parse_outline(&outline(vec![section("H", true)])).unwrap();
    let false_reply = parse_outline(&outline(vec![section("H", false)])).unwrap();
    assert!(true_reply.sections[0].gaps);
    assert!(!false_reply.sections[0].gaps);
}

/// A provider occasionally adds a fence despite the contract.
#[test]
fn a_fenced_outline_parses() {
    let raw = format!("```json\n{}\n```", outline(vec![section("H", false)]));
    assert_eq!(parse_outline(&raw).unwrap().sections.len(), 1);
}

/// Prose around the object is not guessed at. The outline is the object the student edits,
/// so a parser that picked the JSON out of a sentence is a parser that can pick the wrong
/// object out of a sentence.
#[test]
fn prose_around_the_outline_is_not_guessed_at() {
    let raw = format!(
        "Here is your outline:\n{}",
        outline(vec![section("H", false)])
    );
    // serde classifies leading prose as a syntax error, not a schema error. What matters
    // for the caller is that both are retryable and neither is guessed at.
    let error = parse_outline(&raw).unwrap_err();
    assert!(matches!(error, GenerationError::Json(_)), "{error:?}");
    assert!(is_retryable(&error));
}

/// An outline with no sections is the model saying it has nothing, which is `Empty` and is
/// not retried: a second call would be told the same thing, and there is no outline to show
/// the student either way.
#[test]
fn an_outline_with_no_sections_is_empty_and_is_not_retried() {
    let error = parse_outline(r#"{"title":"T","sections":[]}"#).unwrap_err();
    assert!(matches!(error, GenerationError::Empty), "{error:?}");
    assert!(!is_retryable(&error));
}

/// Pins both sides of the cap: the limit itself is accepted, one section more is refused.
/// A truncating implementation would return `Ok` with fewer sections, so the passing case
/// asserts the count too.
#[test]
fn an_outline_at_the_cap_parses_and_one_over_is_refused() {
    let at_cap = outline(
        (0..MAX_REPORT_SECTIONS)
            .map(|index| section(&format!("S{index}"), false))
            .collect(),
    );
    assert_eq!(
        parse_outline(&at_cap).unwrap().sections.len(),
        MAX_REPORT_SECTIONS
    );

    let over_cap = outline(
        (0..=MAX_REPORT_SECTIONS)
            .map(|index| section(&format!("S{index}"), false))
            .collect(),
    );
    let error = parse_outline(&over_cap).unwrap_err();
    assert!(matches!(error, GenerationError::Schema(_)), "{error:?}");
    assert!(
        is_retryable(&error),
        "a retry is what fixes a drifted count"
    );
}

/// An empty title, heading or purpose is refused with the index, because the section call
/// is composed from exactly these three: a blank purpose would ask the model to write a
/// section that establishes nothing, and the student would have paid for it.
#[test]
fn an_empty_title_heading_or_purpose_is_refused_by_index() {
    let titleless =
        r#"{"title":"  ","sections":[{"heading":"H","purpose":"P","labels":[],"gaps":false}]}"#;
    let error = parse_outline(titleless).unwrap_err();
    assert!(error.to_string().contains("title"), "{error}");

    let headingless = outline(vec![
        section("H", false),
        serde_json::json!({"heading":" ","purpose":"P","labels":[1],"gaps":false}),
    ]);
    let error = parse_outline(&headingless).unwrap_err();
    assert!(
        error.to_string().contains("section 1") && error.to_string().contains("heading"),
        "the message must name the section and the field: {error}"
    );

    let purposeless = outline(vec![
        serde_json::json!({"heading":"H","purpose":"","labels":[1],"gaps":false}),
    ]);
    let error = parse_outline(&purposeless).unwrap_err();
    assert!(error.to_string().contains("purpose"), "{error}");
}

/// A drifted prompt fails loudly rather than dropping a field the model invented: the
/// outline is stored and edited, so an unknown key that was ignored would be an edit the
/// student made that the server cannot read back.
#[test]
fn an_unknown_field_is_refused_by_name() {
    let extra = r#"{"title":"T","summary":"x","sections":[{"heading":"H","purpose":"P","labels":[],"gaps":false}]}"#;
    let error = parse_outline(extra).unwrap_err();
    assert!(matches!(error, GenerationError::Schema(_)), "{error:?}");
    assert!(error.to_string().contains("summary"), "{error}");

    let nested = outline(vec![serde_json::json!({
        "heading": "H",
        "purpose": "P",
        "labels": [1],
        "gaps": false,
        "citations": [1],
    })]);
    let error = parse_outline(&nested).unwrap_err();
    assert!(error.to_string().contains("citations"), "{error}");
}

/// A section reply is Markdown and is stored as the model wrote it: the `[n]` labels are
/// resolved by the server against the labels this request sent (A17), and the gap note the
/// prompt asks for is text, not a field. Rewriting either here would lose the only link a
/// claim has. A label that was never sent is kept too — dropping it is the renderer's
/// decision, and it happens against the request's own label map, not against this text.
///
/// Only the surrounding whitespace goes, which is what `strip_code_fence` trims for every
/// contract in this module: the stored body should not begin with a blank line.
#[test]
fn a_valid_section_is_kept_verbatim_including_labels_never_sent() {
    let body = "Enkapsulasi menambah header [1].\n\n(Not covered in your sources: QoS [99]).\n";
    let parsed = parse_section(body).unwrap();
    assert_eq!(
        parsed.body_md,
        "Enkapsulasi menambah header [1].\n\n(Not covered in your sources: QoS [99])."
    );
}

/// A provider that fences a Markdown reply is the same provider that fences a JSON one,
/// and the fence must not be stored: it would print in the report and in the `.md` export.
#[test]
fn a_fenced_section_is_stripped() {
    let raw = "```markdown\n# Pendahuluan\n\nTeks [2].\n```";
    assert_eq!(
        parse_section(raw).unwrap().body_md,
        "# Pendahuluan\n\nTeks [2]."
    );
}

/// A section that is JSON is the model answering the outline's schema, or wrapping the
/// Markdown in `{"body_md": …}`, or returning a list of strings. Stored, it would be the
/// report the student reads and the `.md` they export, and the failure is only visible
/// after the call is paid for. The violation is a schema failure so the retry's corrective
/// text can say what was wrong.
#[test]
fn a_section_that_is_a_json_container_is_refused() {
    for raw in [
        r##"{"body_md":"# Pendahuluan"}"##,
        r#"{"title":"Jaringan","sections":[]}"#,
        r#"["Pendahuluan","Isi"]"#,
    ] {
        let error = parse_section(raw).unwrap_err();
        assert!(matches!(error, GenerationError::Schema(_)), "{error:?}");
        assert!(
            error.to_string().contains("Markdown"),
            "the message must say what was expected: {error}"
        );
        assert!(is_retryable(&error));
    }
}

/// The other half of that rule: Markdown that merely contains braces is Markdown. A body
/// that opens with set notation, that quotes a JSON example inside a fenced block, or that
/// is a single quoted sentence is a legitimate report section, and refusing it would make
/// some material impossible to write about at all.
#[test]
fn markdown_containing_json_is_not_mistaken_for_a_json_reply() {
    for body in [
        "# Himpunan\n\nHimpunan kosong ditulis { }.\n\n```json\n{\"a\":1}\n```\n",
        // Valid JSON as a scalar, and a perfectly readable line of Markdown.
        "\"Enkapsulasi menambah header.\"\n",
    ] {
        assert_eq!(parse_section(body).unwrap().body_md, body.trim());
    }
}

/// An empty body is `Empty`, not an empty success: there is nothing to store in
/// `reports.body_md`, and `Empty` is not retried because the model already answered.
#[test]
fn an_empty_section_body_is_empty_and_is_not_retried() {
    for raw in ["", "   \n\t ", "```markdown\n\n```"] {
        let error = parse_section(raw).unwrap_err();
        assert!(
            matches!(error, GenerationError::Empty),
            "{raw:?}: {error:?}"
        );
        assert!(!is_retryable(&error));
    }
}
