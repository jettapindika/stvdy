//! Grading and the weak-topic summary.
//!
//! Both are pure functions, so these tests need no database and no model. The summary's
//! order and its denominator are the two things a later refactor breaks silently, so
//! they are pinned here rather than left to the route test.

use uuid::Uuid;

use stvdy_core::quiz::{AnswerRecord, grade_mcq, normalize, summarize};

fn source(n: u128) -> Option<Uuid> {
    Some(Uuid::from_u128(n))
}

/// One question's outcome. `correct: None` means "not judged yet", which is not the
/// same as `Some(false)`.
fn rec(
    kind: &str,
    source_id: Option<Uuid>,
    page: Option<i32>,
    attempted: bool,
    correct: Option<bool>,
) -> AnswerRecord {
    AnswerRecord {
        kind: kind.to_string(),
        source_id,
        source_title: source_id.map(|_| "Jaringan Komputer".to_string()),
        page,
        attempted,
        correct,
    }
}

/// Grading is a pure function of the two strings, and it is the server's answer, not
/// the client's: a device that claimed `correct: true` for a wrong MCQ must not be able
/// to raise its own score.
#[test]
fn mcq_grading_normalises_but_does_not_forgive() {
    assert!(grade_mcq("Membuka koneksi TCP.", "  membuka   koneksi tcp. "));
    assert!(grade_mcq("Membuka koneksi TCP.", "Membuka koneksi TCP."));
    assert!(!grade_mcq("Membuka koneksi TCP.", "Menutup koneksi TCP."));

    // A letter or an index is not an answer. Accepting one would mean guessing which
    // option the client meant, and a wrong guess is a silently wrong score.
    assert!(!grade_mcq("Membuka koneksi TCP.", "B"));
    assert!(!grade_mcq("Membuka koneksi TCP.", "1"));

    // A partial answer is not an answer either.
    assert!(!grade_mcq("Membuka koneksi TCP.", "Membuka koneksi"));
}

/// The comparison form, stated directly, because both sides of every MCQ comparison
/// depend on it.
#[test]
fn normalisation_folds_case_and_collapses_whitespace_only() {
    assert_eq!(normalize("  A  B \n"), "a b");
    assert_eq!(normalize(""), "");
    // Not a general "loose" comparison: punctuation still matters, because two options
    // that differ only in punctuation are different options.
    assert_ne!(normalize("TCP/IP"), normalize("TCPIP"));
}

/// The denominator is the *graded* questions, not every question: a short answer the
/// student has not marked yet is not a wrong answer, and counting it as one would make
/// the score fall as they work through the quiz.
#[test]
fn an_ungraded_answer_is_excluded_from_the_denominator() {
    let records = vec![
        rec("mcq", source(1), Some(12), true, Some(true)),
        rec("mcq", source(1), Some(12), true, Some(false)),
        // Answered, revealed, not marked yet.
        rec("short", source(1), Some(30), true, None),
    ];
    let summary = summarize(&records);

    assert_eq!(summary.total, 3);
    assert_eq!(summary.answered, 3, "all three have an answer submitted");
    assert_eq!(summary.graded, 2, "only two have been judged");
    assert_eq!(summary.correct, 1);
    assert_eq!(summary.score, Some(0.5));
}

/// A question nobody has attempted is neither answered nor graded, and it is not a
/// topic yet: it must not appear in a list headed "weakest".
#[test]
fn an_unattempted_question_is_counted_but_is_not_a_topic() {
    let records = vec![
        rec("mcq", source(1), Some(12), true, Some(false)),
        rec("mcq", source(2), Some(4), false, None),
    ];
    let summary = summarize(&records);

    assert_eq!(summary.total, 2);
    assert_eq!(summary.answered, 1);
    assert_eq!(summary.graded, 1);
    assert_eq!(summary.score, Some(0.0));

    assert_eq!(summary.topics.len(), 1, "the unattempted topic is not listed");
    assert_eq!(summary.topics[0].page, Some(12));
}

/// Weakest first, and the questions whose label never resolved get their own bucket
/// rather than being dropped: a summary that omits what it cannot attribute looks
/// better and means less.
#[test]
fn topics_are_grouped_by_source_and_page_weakest_first_with_an_unlinked_bucket() {
    let records = vec![
        rec("mcq", source(1), Some(12), true, Some(true)),
        rec("mcq", source(1), Some(12), true, Some(false)),
        rec("mcq", source(2), Some(4), true, Some(true)),
        rec("short", None, None, true, Some(false)),
    ];
    let summary = summarize(&records);

    let linked: Vec<_> = summary
        .topics
        .iter()
        .filter(|topic| topic.source_id.is_some())
        .collect();
    assert_eq!(linked.len(), 2, "page 12 and page 4 are two topics");
    assert_eq!(
        linked[0].page,
        Some(12),
        "the weakest topic comes first: 1 of 2"
    );
    assert_eq!(linked[0].answered, 2);
    assert_eq!(linked[0].graded, 2);
    assert_eq!(linked[0].correct, 1);
    assert_eq!(linked[1].page, Some(4), "then the stronger one: 1 of 1");
    assert_eq!(linked[1].correct, 1);

    let unlinked = summary
        .topics
        .iter()
        .find(|topic| topic.source_id.is_none())
        .expect("an unlinked question gets its own bucket, it is not dropped");
    assert_eq!(unlinked.answered, 1);
    assert_eq!(unlinked.correct, 0);
    assert_eq!(unlinked.source_title, None);
}

/// A topic with nothing graded sorts after every assessed topic, including a perfect
/// one. "Nobody has tried this" is not the same as "this is the weakest", and a list
/// headed "weakest" that put it first would tell the student something false.
#[test]
fn a_topic_with_nothing_graded_sorts_last() {
    let records = vec![
        rec("mcq", source(1), Some(1), true, None),
        rec("mcq", source(2), Some(2), true, Some(false)),
    ];
    let summary = summarize(&records);

    assert_eq!(
        summary.topics[0].page,
        Some(2),
        "the graded-and-wrong topic is the weakest"
    );
    assert_eq!(summary.topics[1].page, Some(1));
    assert_eq!(
        summary.topics[1].graded, 0,
        "it is last because nothing in it is judged, not because it is perfect"
    );
}

/// No graded questions is `None`, not `0.0`: "nothing marked yet" and "everything
/// wrong" are different facts.
#[test]
fn a_quiz_with_nothing_graded_has_no_score_rather_than_zero() {
    assert_eq!(summarize(&[]).score, None);
    assert_eq!(summarize(&[]).total, 0);
    assert!(summarize(&[]).topics.is_empty());

    let nothing_marked = vec![rec("short", source(1), Some(3), true, None)];
    assert_eq!(summarize(&nothing_marked).score, None);
}
