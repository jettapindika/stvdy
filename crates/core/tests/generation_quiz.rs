//! The quiz reply contract.
//!
//! The cases here are the ones a model actually gets wrong: options on a short answer,
//! an MCQ with three options, an MCQ whose `answer` is not one of its options (which the
//! grader would silently mark every student wrong on), and an unknown field.

use uuid::Uuid;

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::generation::quiz::{
    MAX_QUESTIONS_PER_REPLY, MCQ, MCQ_OPTIONS, SHORT, parse_questions, resolve_questions,
};
use stvdy_core::generation::{GenerationError, is_retryable};

fn mcq(label: u32) -> String {
    format!(
        r#"{{"kind":"mcq","prompt":"Apa fungsi three-way handshake?","options":["Membuka koneksi TCP.","Menutup koneksi TCP.","Mengenkripsi data.","Mengompres data."],"answer":"Membuka koneksi TCP.","explanation":"Handshake menegosiasikan parameter sebelum data dikirim.","label":{label},"key_points":null}}"#
    )
}

fn short(label: u32) -> String {
    format!(
        r#"{{"kind":"short","prompt":"Jelaskan three-way handshake.","options":null,"answer":"Tiga pesan untuk membuka koneksi.","explanation":"SYN, SYN-ACK, ACK.","label":{label},"key_points":["SYN","SYN-ACK","ACK"]}}"#
    )
}

fn file(questions: &[String]) -> String {
    format!(r#"{{"questions":[{}]}}"#, questions.join(","))
}

/// The two kinds have different shapes, and the contract has to reject the mix-ups.
#[test]
fn an_mcq_must_carry_four_options_and_an_answer_among_them() {
    let bad = [
        // options null on an mcq
        r#"{"questions":[{"kind":"mcq","prompt":"p","options":null,"answer":"a","explanation":"e","label":1,"key_points":null}]}"#.to_string(),
        // three options
        r#"{"questions":[{"kind":"mcq","prompt":"p","options":["a","b","c"],"answer":"a","explanation":"e","label":1,"key_points":null}]}"#.to_string(),
        // five options
        r#"{"questions":[{"kind":"mcq","prompt":"p","options":["a","b","c","d","e"],"answer":"a","explanation":"e","label":1,"key_points":null}]}"#.to_string(),
        // the answer is not among the options: the server cannot grade this, and every
        // student would be marked wrong
        r#"{"questions":[{"kind":"mcq","prompt":"p","options":["a","b","c","d"],"answer":"z","explanation":"e","label":1,"key_points":null}]}"#.to_string(),
        // an empty option
        r#"{"questions":[{"kind":"mcq","prompt":"p","options":["a","","c","d"],"answer":"a","explanation":"e","label":1,"key_points":null}]}"#.to_string(),
    ];

    for raw in bad {
        assert!(
            matches!(parse_questions(&raw), Err(GenerationError::Schema(_))),
            "{raw} must be refused as a schema error, not stored"
        );
    }

    // And the well-formed one passes, asserted against the constants rather than
    // against the literals above: a cap changed in one place must change this too.
    let good = parse_questions(&file(&[mcq(1)])).expect("the fixture is valid");
    assert_eq!(good[0].kind, MCQ);
    assert_eq!(good[0].options.as_ref().unwrap().len(), MCQ_OPTIONS);
}

/// The answer is compared after normalisation, so a trailing space in a copied option
/// is not a rejection: the module asks for a verbatim copy and a model that adds
/// whitespace has still answered the question.
#[test]
fn an_mcq_answer_matches_its_option_after_normalisation() {
    let raw = r#"{"questions":[{"kind":"mcq","prompt":"p","options":["Membuka koneksi TCP.","b","c","d"],"answer":"  membuka   koneksi tcp. ","explanation":"e","label":1,"key_points":null}]}"#;
    assert!(parse_questions(raw).is_ok());
}

/// A short answer has no options and must carry key points, because a student cannot
/// self-assess against a bare model answer.
#[test]
fn a_short_answer_carries_key_points_and_no_options() {
    let parsed = parse_questions(&file(&[short(2)])).expect("valid");
    assert_eq!(parsed[0].kind, SHORT);
    assert!(parsed[0].options.is_none());
    assert_eq!(parsed[0].key_points.as_ref().unwrap().len(), 3);

    let with_options = r#"{"questions":[{"kind":"short","prompt":"p","options":["a"],"answer":"a","explanation":"e","label":1,"key_points":["k"]}]}"#;
    assert!(matches!(
        parse_questions(with_options),
        Err(GenerationError::Schema(_))
    ));

    let without_points = r#"{"questions":[{"kind":"short","prompt":"p","options":null,"answer":"a","explanation":"e","label":1,"key_points":null}]}"#;
    assert!(matches!(
        parse_questions(without_points),
        Err(GenerationError::Schema(_))
    ));
}

/// An unknown `kind` is refused by name rather than stored as a third kind the SPA has
/// no renderer for.
#[test]
fn an_unknown_kind_is_refused_by_name() {
    let raw = r#"{"questions":[{"kind":"essay","prompt":"p","options":null,"answer":"a","explanation":"e","label":1,"key_points":["k"]}]}"#;
    match parse_questions(raw) {
        Err(GenerationError::Schema(message)) => assert!(
            message.contains("essay"),
            "the message must name the kind it refused: {message}"
        ),
        other => panic!("expected a schema error, got {other:?}"),
    }
}

/// An empty quiz is the model saying the material cannot support one. That is `Empty`,
/// and `Empty` is not retried: a second call would be told the same thing.
#[test]
fn an_empty_quiz_is_empty_and_is_not_retried() {
    let error = parse_questions(r#"{"questions":[]}"#).unwrap_err();
    assert!(matches!(error, GenerationError::Empty));
    assert!(!is_retryable(&error));

    // A malformed reply *is* worth one retry.
    assert!(is_retryable(&parse_questions("not json").unwrap_err()));
}

/// A reply over the cap is a schema error, so a model that ignores `{{count}}` cannot
/// store an unbounded quiz.
#[test]
fn a_reply_over_the_cap_is_refused() {
    let questions: Vec<String> = (0..=MAX_QUESTIONS_PER_REPLY as u32)
        .map(|n| short(n + 1))
        .collect();
    assert!(matches!(
        parse_questions(&file(&questions)),
        Err(GenerationError::Schema(_))
    ));

    // Exactly the cap is fine: the bound is the cap, not one under it.
    let at_cap: Vec<String> = (0..MAX_QUESTIONS_PER_REPLY as u32)
        .map(|n| short(n + 1))
        .collect();
    assert!(parse_questions(&file(&at_cap)).is_ok());
}

/// A drifted prompt fails loudly rather than dropping a field the model invented.
#[test]
fn an_unknown_field_is_refused() {
    let raw = r#"{"questions":[{"kind":"mcq","prompt":"p","options":["a","b","c","d"],"answer":"a","explanation":"e","label":1,"key_points":null,"hint":"look here"}]}"#;
    assert!(matches!(
        parse_questions(raw),
        Err(GenerationError::Schema(_))
    ));
}

/// A provider occasionally adds a fence despite the contract.
#[test]
fn a_fenced_reply_parses() {
    let fenced = format!("```json\n{}\n```", file(&[mcq(1)]));
    assert_eq!(parse_questions(&fenced).unwrap().len(), 1);

    // Prose around the object is not tolerated: guessing which part is the JSON is how
    // a half-parsed quiz ships.
    let with_prose = format!("Here is your quiz:\n{}", file(&[mcq(1)]));
    assert!(matches!(
        parse_questions(&with_prose),
        Err(GenerationError::Json(_))
    ));
}

/// A label that was never sent costs the link, not the question, and the miss is
/// recorded once so a climbing rate is visible.
#[test]
fn a_label_that_was_never_sent_costs_the_link_not_the_question() {
    let labels = ContextLabels::new(
        vec![LabeledChunk {
            label: 1,
            chunk_id: Uuid::from_u128(0xabc),
            source_id: Uuid::from_u128(1),
            page: Some(12),
        }],
        vec![],
    );

    let parsed = parse_questions(&file(&[mcq(1), mcq(7), mcq(7)])).unwrap();
    let outcome = resolve_questions(parsed, &labels);

    assert_eq!(outcome.questions.len(), 3, "the question is kept");
    assert_eq!(
        outcome.questions[0].chunk_id,
        Some(Uuid::from_u128(0xabc)),
        "label 1 resolved"
    );
    assert_eq!(
        outcome.questions[1].chunk_id, None,
        "label 7 was never sent, so the link is dropped rather than guessed"
    );
    assert_eq!(
        outcome.dropped_labels,
        vec![7],
        "recorded once, not once per question"
    );
    assert_eq!(outcome.questions[1].label, 7, "the label itself is kept");
}
