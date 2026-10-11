//! Answering a quiz: grading, and the weak-topic summary.
//!
//! Two things live here, and both are pure functions so they can be tested without a
//! database, a model or a clock.
//!
//! **Grading is the server's, for an MCQ.** The submitted answer is compared to the
//! question's stored answer after normalisation. A client cannot raise its own score by
//! claiming an attempt was correct, because the claim is not what is read.
//!
//! **The score is derived, never stored.** [`summarize`] computes it from the attempts
//! every time it is asked. That is what makes attempts from several devices
//! conflict-free: there is no stored score for two devices to disagree about, and a
//! replayed attempt cannot double-count because it is the same row.
//!
//! A short answer is self-assessed by the student, which is a deliberate limit rather
//! than an omission: no string comparison can judge free prose, and a second model call
//! per answer would cost more than generating the quiz did. See A90.

use uuid::Uuid;

/// The comparison form of an answer: trimmed, case-folded, and with internal runs of
/// whitespace collapsed to one space.
///
/// Applied to **both** sides of every comparison, so a client's stray newline or a
/// double space is not a wrong answer. It is deliberately not more forgiving than
/// that: accepting a letter (`"B"`) or an index (`"1"`) would mean guessing which
/// option the client meant, and a wrong guess is a silently wrong score.
pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether a submitted MCQ answer matches the question's answer.
pub fn grade_mcq(answer: &str, submitted: &str) -> bool {
    normalize(answer) == normalize(submitted)
}

/// One question's outcome, as the summary needs it.
///
/// The caller builds one of these per question from the stored attempts: `attempted`
/// from whether any attempt exists, and `correct` from the student's grade. The two are
/// separate fields because `Some(false)` and `None` are different facts. One is a wrong
/// answer; the other is an answer nobody has judged yet, and collapsing them would make
/// a student's score fall as they worked through the quiz.
#[derive(Debug, Clone)]
pub struct AnswerRecord {
    pub kind: String,
    /// The question's chunk's source, or `None` when the model's label never resolved.
    pub source_id: Option<Uuid>,
    pub source_title: Option<String>,
    pub page: Option<i32>,
    pub attempted: bool,
    pub correct: Option<bool>,
}

/// One row of the weak-topic summary.
#[derive(Debug, Clone, PartialEq)]
pub struct TopicRow {
    pub source_id: Option<Uuid>,
    pub source_title: Option<String>,
    pub page: Option<i32>,
    /// Questions in this topic the student has answered.
    pub answered: i64,
    /// How many of them have been judged. The denominator of this topic's correctness,
    /// for the same reason [`QuizSummary::score`] uses `graded`: an answer nobody has
    /// marked yet is not a wrong answer, and counting it as one would make a topic look
    /// weak purely because the student has not finished it.
    pub graded: i64,
    pub correct: i64,
}

/// The score and the weak-topic summary for one quiz.
#[derive(Debug, Clone, PartialEq)]
pub struct QuizSummary {
    /// How many questions the quiz has.
    pub total: i64,
    /// How many have an answer submitted.
    pub answered: i64,
    /// How many have been graded. Equal to `answered` for MCQ, lower while a short
    /// answer is waiting to be self-assessed.
    pub graded: i64,
    pub correct: i64,
    /// `correct / graded`, or `None` when nothing is graded yet.
    ///
    /// `None` rather than `0.0`: "nothing marked yet" and "everything wrong" are
    /// different facts, and a client that renders them the same is lying.
    pub score: Option<f64>,
    /// Weakest first. See [`summarize`] for the order and for why unassessed topics
    /// sort last.
    pub topics: Vec<TopicRow>,
}

/// The topic key: a question's source and page, or the unlinked bucket.
///
/// The title is carried for display but is not part of the key: a topic is a place in a
/// document, and two questions from page 12 of one source are the same topic whatever
/// the title string says.
type TopicKey = (Option<Uuid>, Option<i32>);

/// Grades and groups one quiz's questions.
///
/// Topics are keyed by `(source_id, page)` and sorted **weakest first**, by
/// `correct / answered` ascending. A topic with no answers sorts after every assessed
/// topic rather than before them: a topic nobody has attempted is not weak, it is
/// unknown, and putting it at the top of a list headed "weakest" would tell the student
/// something false. The order is total (ratio, then title, page, id), so two runs over
/// the same data agree.
///
/// Questions whose label never resolved are grouped in an explicit unlinked bucket
/// rather than dropped. A summary that silently omitted what it could not attribute
/// would look better and mean less, and the bucket is the visible symptom of a prompt
/// or label-map regression.
pub fn summarize(records: &[AnswerRecord]) -> QuizSummary {
    let total = records.len() as i64;
    let answered = records.iter().filter(|r| r.attempted).count() as i64;
    let graded = records.iter().filter(|r| r.correct.is_some()).count() as i64;
    let correct = records.iter().filter(|r| r.correct == Some(true)).count() as i64;

    let score = if graded > 0 {
        Some(correct as f64 / graded as f64)
    } else {
        None
    };

    // Insertion order is not the iteration order, so the rows are collected first and
    // sorted once at the end.
    let mut topics: Vec<TopicRow> = Vec::new();
    for record in records {
        if !record.attempted {
            continue;
        }
        let key: TopicKey = (record.source_id, record.page);
        let index = match topics
            .iter()
            .position(|row| (row.source_id, row.page) == key)
        {
            Some(index) => index,
            None => {
                topics.push(TopicRow {
                    source_id: record.source_id,
                    source_title: record.source_title.clone(),
                    page: record.page,
                    answered: 0,
                    graded: 0,
                    correct: 0,
                });
                // The row was pushed on the line above, so this is the row just added.
                topics.len() - 1
            }
        };
        let row = &mut topics[index];
        row.answered += 1;
        if let Some(correct) = record.correct {
            row.graded += 1;
            if correct {
                row.correct += 1;
            }
        }
    }

    topics.sort_by(|a, b| {
        ratio(a)
            .partial_cmp(&ratio(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.source_title.cmp(&b.source_title))
            .then_with(|| a.page.cmp(&b.page))
            .then_with(|| a.source_id.cmp(&b.source_id))
    });

    QuizSummary {
        total,
        answered,
        graded,
        correct,
        score,
        topics,
    }
}

/// A topic's correctness, for the weakest-first order.
///
/// `INFINITY` when nothing in the topic has been judged, which sorts it after every
/// assessed topic including a perfect one. The denominator is `graded`, not `answered`:
/// a topic the student has answered but not yet marked is unknown, not zero.
fn ratio(row: &TopicRow) -> f64 {
    if row.graded == 0 {
        f64::INFINITY
    } else {
        row.correct as f64 / row.graded as f64
    }
}
