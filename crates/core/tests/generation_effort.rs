//! The effort contract's own rules: one entry per requested topic, and a total that is the
//! sum of them.
//!
//! A reply that drops a topic is the case that matters. The planner schedules what it is
//! given, so a topic with no estimate is a topic the student is never scheduled to study,
//! and the plan looks complete while missing a third of the exam.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::generation::GenerationError;
use stvdy_core::generation::effort::{MAX_ESTIMATE_MIN, parse_effort, reconcile};

fn topics(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn a_reply_that_drops_a_topic_is_refused() {
    // The module says "Return exactly one entry per input topic, using the same topic
    // names." The caller retries once on this, which is why it must be an error rather than
    // a short list.
    let raw = r#"{"topics":[{"topic":"Graphs","estimate_min":120,"reason":"few sources"}],
                 "total_estimate_min":120}"#;
    let estimate = parse_effort(raw).unwrap();
    assert!(matches!(
        reconcile(&estimate, &topics(&["Graphs", "Trees"])),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_reply_that_invents_a_topic_is_refused() {
    let raw = r#"{"topics":[
        {"topic":"Graphs","estimate_min":120,"reason":"few sources"},
        {"topic":"Sorting","estimate_min":60,"reason":"well practised"}],
        "total_estimate_min":180}"#;
    let estimate = parse_effort(raw).unwrap();
    assert!(matches!(
        reconcile(&estimate, &topics(&["Graphs"])),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_duplicated_topic_is_refused() {
    // Two entries for one topic are two numbers for one demand, and which one the plan uses
    // would depend on the order the reply happened to use.
    let raw = r#"{"topics":[
        {"topic":"Graphs","estimate_min":120,"reason":"few sources"},
        {"topic":"Graphs","estimate_min":60,"reason":"actually easy"}],
        "total_estimate_min":180}"#;
    let estimate = parse_effort(raw).unwrap();
    assert!(matches!(
        reconcile(&estimate, &topics(&["Graphs"])),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_total_that_is_not_the_sum_is_refused() {
    let raw = r#"{"topics":[{"topic":"Graphs","estimate_min":120,"reason":"few sources"}],
                 "total_estimate_min":600}"#;
    let estimate = parse_effort(raw).unwrap();
    assert!(matches!(
        reconcile(&estimate, &topics(&["Graphs"])),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn an_estimate_outside_the_bound_is_refused() {
    for minutes in [0, -30, MAX_ESTIMATE_MIN + 1] {
        let raw = format!(
            r#"{{"topics":[{{"topic":"Graphs","estimate_min":{minutes},"reason":"x"}}],
                 "total_estimate_min":{minutes}}}"#
        );
        let estimate = parse_effort(&raw).unwrap();
        assert!(
            matches!(
                reconcile(&estimate, &topics(&["Graphs"])),
                Err(GenerationError::Schema(_))
            ),
            "{minutes} minutes must be refused"
        );
    }
}

#[test]
fn the_estimates_come_back_in_the_callers_order() {
    // The plan is built in the order the student's topics are stored. Taking the model's
    // order instead would make the same request produce two different plans depending on how
    // the model felt that day.
    let raw = r#"{"topics":[
        {"topic":"Trees","estimate_min":90,"reason":"well practised"},
        {"topic":"Graphs","estimate_min":120,"reason":"few sources"}],
        "total_estimate_min":210}"#;
    let estimate = parse_effort(raw).unwrap();
    assert_eq!(
        reconcile(&estimate, &topics(&["Graphs", "Trees"])).unwrap(),
        vec![("Graphs".to_string(), 120), ("Trees".to_string(), 90)]
    );
}

#[test]
fn a_topic_name_is_matched_trimmed() {
    // The name reaches the prompt from a source's heading and comes back through a model, so
    // surrounding whitespace is the one difference that must not cost a retry.
    let raw = r#"{"topics":[{"topic":"  Graphs ","estimate_min":120,"reason":"few sources"}],
                 "total_estimate_min":120}"#;
    let estimate = parse_effort(raw).unwrap();
    assert_eq!(
        reconcile(&estimate, &topics(&["Graphs"])).unwrap(),
        vec![("Graphs".to_string(), 120)]
    );
}

#[test]
fn a_call_with_no_topics_is_refused() {
    let raw = r#"{"topics":[],"total_estimate_min":0}"#;
    let estimate = parse_effort(raw).unwrap();
    assert!(matches!(
        reconcile(&estimate, &[]),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_reply_that_invents_a_field_is_refused() {
    let raw = r#"{"topics":[{"topic":"Graphs","estimate_min":120,"reason":"x","hours":2}],
                 "total_estimate_min":120}"#;
    assert!(matches!(parse_effort(raw), Err(GenerationError::Schema(_))));
}

#[test]
fn a_fenced_reply_is_accepted() {
    let raw = "```json\n{\"topics\":[{\"topic\":\"Graphs\",\"estimate_min\":120,\"reason\":\"x\"}],\"total_estimate_min\":120}\n```";
    assert!(parse_effort(raw).is_ok());
}

#[test]
fn a_reply_that_is_not_json_is_a_json_error() {
    assert!(matches!(
        parse_effort("I estimate about three hours."),
        Err(GenerationError::Json(_))
    ));
}
