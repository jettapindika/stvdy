//! Prep progress: the goal's true components, and nothing else.
//!
//! Spec § Honesty rules: no composite "readiness" and no grade prediction. Every field is
//! either a count the app computed or `None` when the data to compute it does not exist —
//! `0%` is a claim the app cannot make from no attempts.
//!
//! Pure: no clock, no IO. The 14-day window is the caller's, applied before the rows arrive.

use super::model::Topic;

/// A topic whose accuracy is below this is flagged weak. A constant with a doc comment, not a
/// tunable, because it is a display threshold rather than a planning parameter.
pub const WEAK_TOPIC_ACCURACY: f64 = 0.6;

/// What one topic's attempts looked like, as the server reads them.
#[derive(Debug, Clone, PartialEq)]
pub struct TopicAttempts {
    pub topic: String,
    pub attempts: i64,
    pub correct: i64,
}

/// The goal's true components, and nothing else (spec § Honesty rules: no composite
/// "readiness" and no grade prediction).
#[derive(Debug, Clone, PartialEq)]
pub struct PrepProgress {
    /// The share of the goal's topics that have at least one review or quiz attempt.
    pub topic_coverage_pct: Option<i64>,
    /// Over the last 14 days, or `None` — never 0 when there are no attempts.
    pub quiz_accuracy_pct: Option<i64>,
    pub cards_due: i64,
    pub weak_topics: Vec<String>,
}

/// Builds the four honest numbers.
///
/// `weak_topics` is the topics whose accuracy is below [`WEAK_TOPIC_ACCURACY`], ordered by
/// accuracy ascending then by name. A topic with no accuracy (`None`) is not weak: missing
/// data is not evidence of weakness (A258).
pub fn prep_progress(topics: &[Topic], attempts: &[TopicAttempts], cards_due: i64) -> PrepProgress {
    let coverage = if topics.is_empty() {
        None
    } else {
        let covered = topics
            .iter()
            .filter(|topic| {
                attempts
                    .iter()
                    .any(|attempt| attempt.topic == topic.name && attempt.attempts > 0)
            })
            .count();
        Some(round_pct(covered as i64, topics.len() as i64))
    };

    let total_attempts: i64 = attempts.iter().map(|attempt| attempt.attempts).sum();
    let total_correct: i64 = attempts.iter().map(|attempt| attempt.correct).sum();
    let accuracy = (total_attempts > 0).then(|| round_pct(total_correct, total_attempts));

    let mut weak: Vec<&Topic> = topics
        .iter()
        .filter(|topic| matches!(topic.accuracy, Some(accuracy) if accuracy < WEAK_TOPIC_ACCURACY))
        .collect();
    weak.sort_by(|a, b| {
        let left = a.accuracy.unwrap_or(1.0);
        let right = b.accuracy.unwrap_or(1.0);
        left.total_cmp(&right).then_with(|| a.name.cmp(&b.name))
    });

    PrepProgress {
        topic_coverage_pct: coverage,
        quiz_accuracy_pct: accuracy,
        cards_due,
        weak_topics: weak.into_iter().map(|topic| topic.name.clone()).collect(),
    }
}

fn round_pct(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        return 0;
    }
    let scaled = part as f64 * 100.0 / whole as f64;
    scaled.round() as i64
}
