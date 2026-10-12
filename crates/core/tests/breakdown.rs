//! The task-breakdown proposal contract.
//!
//! The cases here are the ones a model actually gets wrong: fewer than three subtasks (which
//! restates the parent), more than eight (which is a project plan), a total that disagrees
//! with its own subtasks, and a subtask that repeats the parent task it was asked to break
//! down. The order cases matter because a breakdown is a *sequence*, not a set: a sort would
//! silently reorder a student's plan.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::generation::breakdown::{MAX_SUBTASKS, MIN_SUBTASKS, Proposal, parse_proposal};
use stvdy_core::generation::{GenerationError, is_retryable};

const PARENT: &str = "Selesaikan tugas kripto";

/// One reply object, with the total equal to the sum of the estimates given — which is what
/// the module asks the model for, and what the contract checks.
fn file(subtasks: &[(&str, i64)]) -> String {
    let total: i64 = subtasks.iter().map(|(_, estimate)| estimate).sum();
    file_with_total(subtasks, total)
}

/// The same object with the total stated rather than summed, for the cases where the reply's
/// own arithmetic is the thing under test.
fn file_with_total(subtasks: &[(&str, i64)], total: i64) -> String {
    let items: Vec<String> = subtasks
        .iter()
        .map(|(title, estimate)| format!(r#"{{"title":"{title}","estimate_min":{estimate}}}"#))
        .collect();
    format!(
        r#"{{"subtasks":[{}],"total_estimate_min":{total}}}"#,
        items.join(",")
    )
}

/// `n` subtasks whose titles are the order this file asserts.
fn file_of(n: usize) -> String {
    let subtasks: Vec<(String, i64)> = (1..=n)
        .map(|index| (format!("Baca bab {index}"), 45))
        .collect();
    let borrowed: Vec<(&str, i64)> = subtasks
        .iter()
        .map(|(title, estimate)| (title.as_str(), *estimate))
        .collect();
    file(&borrowed)
}

fn parse(raw: &str) -> Result<Proposal, GenerationError> {
    parse_proposal(raw, PARENT)
}

/// The prompt asks for 3-8 subtasks, and the contract is what makes that a contract. Fewer
/// than three restates the parent; more than eight is a project plan. A violation is a schema
/// failure, so the one retry the server already performs is what fixes it.
#[test]
fn a_proposal_outside_three_to_eight_subtasks_is_a_schema_failure() {
    assert!(matches!(
        parse(&file_of(2)),
        Err(GenerationError::Schema(_))
    ));
    assert!(matches!(
        parse(&file_of(9)),
        Err(GenerationError::Schema(_))
    ));
    assert!(parse(&file_of(3)).is_ok());
    assert!(parse(&file_of(8)).is_ok());
    assert_eq!(MIN_SUBTASKS, 3);
    assert_eq!(MAX_SUBTASKS, 8);
}

/// The declared total must equal the sum, and the sum is what the caller uses. A total the
/// model wrote that disagrees with its own subtasks is a number that cannot be shown, so the
/// reply is rejected and retried rather than displayed with the sum silently substituted —
/// which would make the "Suggested …" region show a number the student never saw proposed.
#[test]
fn a_total_that_disagrees_with_its_own_subtasks_is_a_schema_failure() {
    let agreeing = file(&[("Baca bab 1", 30), ("Baca bab 2", 30), ("Baca bab 3", 30)]);
    assert!(parse(&agreeing).is_ok());

    let disagreeing = agreeing.replace("\"total_estimate_min\":90", "\"total_estimate_min\":120");
    let error = parse(&disagreeing).unwrap_err();
    assert!(matches!(error, GenerationError::Schema(_)));
    assert!(
        error.to_string().contains("120") || error.to_string().contains("90"),
        "the message must name the disagreement: {error}"
    );
    assert!(is_retryable(&error));
}

/// An estimate outside 15-90 minutes is a warning, not a failure: it is a judgement the
/// student can fix in the proposal list, and rejecting the whole reply over one number costs
/// a second model call for something a single keystroke fixes.
#[test]
fn an_out_of_range_estimate_is_warned_about_and_kept() {
    let raw = file(&[
        ("Baca bab 1", 45),
        ("Baca bab 2", 45),
        ("Baca bab 3", 240),
        ("Baca bab 4", 45),
    ]);
    let proposal = parse(&raw).expect("parses");
    assert_eq!(proposal.subtasks[2].estimate_min, 240);
    assert_eq!(proposal.warnings.len(), 1, "{:?}", proposal.warnings);
    assert!(
        proposal.warnings[0].contains("240"),
        "{:?}",
        proposal.warnings
    );
}

/// A subtask title that restates the parent task is the one thing the prompt explicitly
/// forbids and the one thing a model does anyway. It is dropped with a warning rather than
/// failing the reply, and dropping it cannot take the count below the minimum because the
/// minimum is checked *after* the drop.
///
/// The total is compared after the drop, so this reply's total is the sum of what remains:
/// the restatement is dropped from the list *and* from the number that describes it, which is
/// what makes the "Suggested …" total describe the subtasks the student is looking at.
#[test]
fn a_subtask_that_restates_its_parent_is_dropped_with_a_warning() {
    let raw = file_with_total(
        &[
            (PARENT, 45),
            ("Baca bab 1", 45),
            ("Baca bab 2", 45),
            ("Baca bab 3", 45),
        ],
        135,
    );
    let proposal = parse(&raw).expect("parses");
    assert_eq!(proposal.subtasks.len(), 3);
    assert_eq!(proposal.total_estimate_min, 135);
    assert_eq!(proposal.warnings.len(), 1, "{:?}", proposal.warnings);
    assert!(
        proposal.warnings[0].contains(PARENT),
        "{:?}",
        proposal.warnings
    );
    assert_eq!(proposal.subtasks[0].title, "Baca bab 1");
}

/// A total that counted the dropped restatement no longer describes the list, so it is
/// refused and retried rather than shown beside three subtasks that sum to less.
#[test]
fn a_total_that_counted_the_dropped_restatement_is_refused() {
    let raw = file(&[
        (PARENT, 45),
        ("Baca bab 1", 45),
        ("Baca bab 2", 45),
        ("Baca bab 3", 45),
    ]);
    assert!(matches!(parse(&raw), Err(GenerationError::Schema(_))));
}

/// The restatement is caught by the same normalisation the MCQ grader uses, so a model that
/// changes case or spacing around the parent's title does not smuggle the parent back in as
/// a subtask.
#[test]
fn a_restatement_is_caught_across_case_and_spacing() {
    let raw = file_with_total(
        &[
            ("  selesaikan   TUGAS Kripto ", 45),
            ("Baca bab 1", 45),
            ("Baca bab 2", 45),
            ("Baca bab 3", 45),
        ],
        135,
    );
    let proposal = parse(&raw).expect("parses");
    assert_eq!(proposal.subtasks.len(), 3);
    assert_eq!(proposal.warnings.len(), 1, "{:?}", proposal.warnings);
}

/// Dropping the restatement happens first, so a reply that only reaches the minimum by
/// counting the parent is refused rather than shipped as a two-item breakdown.
#[test]
fn a_proposal_that_reaches_the_minimum_only_with_its_restatement_is_refused() {
    let raw = file_with_total(&[(PARENT, 45), ("Baca bab 1", 45), ("Baca bab 2", 45)], 90);
    assert!(matches!(parse(&raw), Err(GenerationError::Schema(_))));
}

/// Duplicate titles are kept. Two subtasks can legitimately share a title ("Review"), and
/// dropping one would silently shorten the student's list.
#[test]
fn duplicate_subtask_titles_are_kept() {
    let raw = file(&[("Review", 30), ("Review", 30), ("Kirim", 20)]);
    let proposal = parse(&raw).expect("parses");
    assert_eq!(proposal.subtasks.len(), 3);
    assert!(proposal.warnings.is_empty(), "{:?}", proposal.warnings);
}

/// A subtask with no title is not a subtask: it would insert a row the Tasks screen cannot
/// render. Unlike a restatement there is nothing to drop and warn about, because the reply
/// did not answer the question at all.
#[test]
fn an_empty_subtask_title_is_a_schema_failure() {
    let raw = file(&[("   ", 30), ("Baca bab 1", 30), ("Baca bab 2", 30)]);
    assert!(matches!(parse(&raw), Err(GenerationError::Schema(_))));
}

/// The order is the model's, and it is the order the caller assigns `position` from: the
/// index in this vector is the position. A sort here would silently reorder a student's plan
/// (A196, Phase 5's A100), which is why the case is worth pinning.
#[test]
fn the_proposal_order_is_the_models_order() {
    let proposal = parse(&file_of(5)).unwrap();
    let titles: Vec<&str> = proposal
        .subtasks
        .iter()
        .map(|subtask| subtask.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "Baca bab 1",
            "Baca bab 2",
            "Baca bab 3",
            "Baca bab 4",
            "Baca bab 5"
        ]
    );
}

/// A fenced reply still parses, and prose around the object does not: guessing which part is
/// the JSON is how a half-parsed proposal ships.
#[test]
fn a_fenced_reply_parses_and_prose_does_not() {
    let raw = format!("```json\n{}\n```", file_of(3));
    assert!(parse(&raw).is_ok());

    let raw = format!("Here is the breakdown:\n{}", file_of(3));
    assert!(matches!(parse(&raw), Err(GenerationError::Json(_))));
}

/// The proposal is what the client renders inside the "Suggested …" region, so its field
/// names are a client contract.
#[test]
fn the_proposal_serialises_with_the_field_names_the_client_reads() {
    let proposal = parse(&file(&[
        ("Baca bab 1", 30),
        ("Baca bab 2", 30),
        ("Baca bab 3", 30),
    ]))
    .expect("parses");
    let json = serde_json::to_value(&proposal).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "subtasks": [
                {"title": "Baca bab 1", "estimate_min": 30},
                {"title": "Baca bab 2", "estimate_min": 30},
                {"title": "Baca bab 3", "estimate_min": 30},
            ],
            "total_estimate_min": 90,
            "warnings": [],
        })
    );
}

/// A reply with no subtasks at all is not worth a second call: the model answered the
/// question it was asked and the answer was "nothing".
#[test]
fn an_empty_proposal_is_not_retryable() {
    let error = parse(r#"{"subtasks":[],"total_estimate_min":0}"#).unwrap_err();
    assert!(matches!(error, GenerationError::Empty));
    assert!(!is_retryable(&error));
}
