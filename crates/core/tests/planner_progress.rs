//! Prep progress: the goal's true components and nothing else.
//!
//! Spec § Honesty rules: no composite "readiness" and no grade prediction. The two cases here
//! are the two ways a progress card can lie: reporting `0%` when there are no attempts (a
//! number the app cannot compute), and growing a fifth field that quietly becomes a score.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::planner::Topic;
use stvdy_core::planner::progress::{PrepProgress, TopicAttempts, prep_progress};

fn topic(name: &str, accuracy: Option<f64>) -> Topic {
    Topic {
        source_id: None,
        name: name.to_string(),
        remaining_min: 100,
        accuracy,
    }
}

fn attempts(topic: &str, attempts: i64, correct: i64) -> TopicAttempts {
    TopicAttempts {
        topic: topic.to_string(),
        attempts,
        correct,
    }
}

/// The spec is explicit: "quiz accuracy (last 14 days, or 'no quizzes yet')". `None`, never
/// 0 — 0% is a claim the app cannot make from no data.
#[test]
fn prep_progress_reports_null_accuracy_when_there_are_no_attempts() {
    let topics = vec![topic("Jaringan", Some(0.5))];
    let progress = prep_progress(&topics, &[], 4);
    assert_eq!(
        progress.quiz_accuracy_pct, None,
        "no attempts is null, not zero"
    );
    assert_eq!(progress.cards_due, 4);
    assert_eq!(
        progress.topic_coverage_pct,
        Some(0),
        "no topic has an attempt yet"
    );
    assert_eq!(progress.weak_topics, vec!["Jaringan".to_string()]);
}

/// `PrepProgress` has exactly the four fields the spec names, and no `readiness`, `grade` or
/// `score`.
///
/// Asserted by destructuring exhaustively: adding a fifth field fails to compile here rather
/// than passing silently into a progress card that has become a prediction.
#[test]
fn prep_progress_has_no_composite_score() {
    let topics = vec![topic("Algoritma", Some(0.3)), topic("Jaringan", Some(0.9))];
    let attempts = vec![attempts("Algoritma", 10, 3), attempts("Jaringan", 10, 9)];
    let progress = prep_progress(&topics, &attempts, 5);

    let PrepProgress {
        topic_coverage_pct,
        quiz_accuracy_pct,
        cards_due,
        weak_topics,
    } = progress;

    assert_eq!(topic_coverage_pct, Some(100), "both topics have attempts");
    assert_eq!(quiz_accuracy_pct, Some(60), "12 of 20 correct");
    assert_eq!(cards_due, 5);
    assert_eq!(
        weak_topics,
        vec!["Algoritma".to_string()],
        "only the below-threshold topic is weak, ordered by accuracy"
    );
}
