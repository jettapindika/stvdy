//! The syllabus contract's own rules: what the review screen may show, and what it must
//! never be handed.
//!
//! The evidence quote is the case that matters. The review screen exists so a student can
//! check an extraction against the document before anything is written, and the quote is the
//! only thing that lets them: an item with no quote is an item they cannot check, so it is a
//! parse failure rather than a weaker citation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use stvdy_core::generation::GenerationError;
use stvdy_core::generation::syllabus::{
    MAX_EVIDENCE_WORDS, MAX_MEETINGS, REVIEW_FLAG_CONFIDENCE, ReviewKind, parse_syllabus,
    review_items,
};

fn jakarta() -> Tz {
    "Asia/Jakarta".parse().unwrap()
}

/// A reply with one meeting whose evidence is `evidence`.
fn one_meeting(evidence: &str, confidence: f64) -> String {
    let evidence = serde_json::to_string(evidence).unwrap();
    format!(
        r#"{{"course":{{"name":null,"code":null,"instructor":null}},
            "meetings":[{{"title":"Lecture","rrule":"FREQ=WEEKLY;BYDAY=MO;COUNT=3",
                          "start_time":"08:00","end_time":"09:40","location":null,
                          "evidence":{evidence},"confidence":{confidence}}}],
            "deadlines":[],"exams":[],"notes":[]}}"#
    )
}

#[test]
fn an_item_without_evidence_is_refused() {
    // An empty quote is not a weaker citation, it is no citation: the review screen's whole
    // job is to show the student the line, so the parse has to fail and the reply be retried
    // rather than reach the screen as an item nobody can check.
    assert!(matches!(
        parse_syllabus(&one_meeting("", 0.9)),
        Err(GenerationError::Schema(_))
    ));
    // Whitespace is not a quote either.
    assert!(matches!(
        parse_syllabus(&one_meeting("   ", 0.9)),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn evidence_longer_than_the_bound_is_refused() {
    // The module says "under 15 words", and the bound is what stops a model quoting a
    // paragraph and calling it evidence: a review row whose "source line" is a paragraph is
    // one the student reads past rather than checks.
    let fifteen = (1..=MAX_EVIDENCE_WORDS)
        .map(|n| format!("w{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(parse_syllabus(&one_meeting(&fifteen, 0.9)).is_ok());

    let sixteen = format!("{fifteen} w16");
    assert!(matches!(
        parse_syllabus(&one_meeting(&sixteen, 0.9)),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_confidence_outside_zero_to_one_is_refused() {
    // NaN is deliberately not here: JSON has no NaN literal, so a reply carrying one fails
    // as malformed JSON before the range check is reached.
    for confidence in [-0.1, 1.1] {
        assert!(
            matches!(
                parse_syllabus(&one_meeting("Lecture is Monday 08:00", confidence)),
                Err(GenerationError::Schema(_))
            ),
            "confidence {confidence} must be refused"
        );
    }
}

#[test]
fn an_item_carries_the_line_it_was_read_from() {
    let raw = format!(
        r#"{{"course":{{"name":"Jaringan Komputer","code":"IF2230","instructor":null}},
            "meetings":[{{"title":"Lecture","rrule":"FREQ=WEEKLY;BYDAY=MO;COUNT=3",
                          "start_time":"08:00","end_time":"09:40","location":"Room 3",
                          "evidence":"Kuliah Senin 08:00 ruang 3","confidence":0.95}}],
            "deadlines":[{{"title":"Tugas 1","due":"2026-11-12T23:59:00+07:00",
                           "weight_percent":15.0,"evidence":"Tugas 1 dikumpulkan 12 Nov",
                           "confidence":0.4}}],
            "exams":[],"notes":["UAS material is cumulative"]}}"#
    );

    let file = parse_syllabus(&raw).unwrap();
    let items = review_items(&file, jakarta()).unwrap();
    assert_eq!(items.len(), 2);

    // Lowest confidence first: the item most likely to be wrong is the one at the top, which
    // is where the student's attention is.
    assert_eq!(items[0].kind, ReviewKind::Deadline);
    assert_eq!(items[0].title, "Tugas 1");
    assert_eq!(items[0].evidence, "Tugas 1 dikumpulkan 12 Nov");
    assert_eq!(items[0].weight_percent, Some(15.0));
    assert!(
        items[0].needs_check,
        "0.4 is below the {REVIEW_FLAG_CONFIDENCE} flag"
    );
    assert_eq!(
        items[0].when,
        Some("2026-11-12T16:59:00Z".parse::<DateTime<Utc>>().unwrap()),
        "23:59+07:00 is 16:59Z"
    );

    assert_eq!(items[1].kind, ReviewKind::Meeting);
    assert_eq!(items[1].evidence, "Kuliah Senin 08:00 ruang 3");
    assert!(!items[1].needs_check);
    assert_eq!(items[1].start_time, Some((8, 0)));
    assert_eq!(items[1].end_time, Some((9, 40)));
    assert_eq!(
        items[1].rrule.as_deref(),
        Some("FREQ=WEEKLY;BYDAY=MO;COUNT=3")
    );
    assert_eq!(
        items[1].when, None,
        "a meeting has no date of its own: the rule supplies it, and the rule has no anchor"
    );
}

#[test]
fn the_review_list_indexes_every_item_exactly_once() {
    // The confirm body names items by index, so the index has to be unique across the whole
    // reply and cover every item: a collision would let a kept index name two rows, and a
    // gap would make a row unreachable.
    let raw = r#"{"course":{},"meetings":[
            {"title":"A","rrule":"FREQ=WEEKLY;COUNT=2","start_time":"08:00","end_time":"09:00",
             "location":null,"evidence":"A is Monday","confidence":0.9},
            {"title":"B","rrule":"FREQ=WEEKLY;COUNT=2","start_time":"10:00","end_time":"11:00",
             "location":null,"evidence":"B is Wednesday","confidence":0.8}],
        "deadlines":[
            {"title":"T1","due":"2026-11-12T23:59:00+07:00","weight_percent":null,
             "evidence":"T1 due 12 Nov","confidence":0.5}],
        "exams":[
            {"title":"UAS","starts_at":"2026-12-15T08:00:00+07:00","ends_at":null,
             "topics":["Graphs"],"evidence":"UAS 15 Dec","confidence":0.7}],
        "notes":[]}"#;

    let items = review_items(&parse_syllabus(raw).unwrap(), jakarta()).unwrap();
    assert_eq!(
        items.len(),
        4,
        "no item is dropped on the way to the screen"
    );

    let mut indices: Vec<usize> = items.iter().map(|item| item.index).collect();
    indices.sort_unstable();
    assert_eq!(indices, vec![0, 1, 2, 3]);
}

#[test]
fn a_deadline_with_no_date_is_refused() {
    let raw = r#"{"course":{},"meetings":[],"deadlines":[
        {"title":"Tugas 1","due":"","weight_percent":null,"evidence":"Tugas 1","confidence":0.9}],
        "exams":[],"notes":[]}"#;
    assert!(matches!(
        parse_syllabus(raw),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_date_without_an_offset_is_resolved_in_the_students_zone() {
    // The module asks for an offset, and the tolerant arm is the one that keeps a
    // well-formed-but-naive reply from being resolved in the wrong zone — the one mistake a
    // syllabus import cannot survive.
    let raw = r#"{"course":{},"meetings":[],"deadlines":[
        {"title":"Tugas 1","due":"2026-11-12T23:59:00","weight_percent":null,
         "evidence":"Tugas 1 due 12 Nov","confidence":0.9}],
        "exams":[],"notes":[]}"#;
    let items = review_items(&parse_syllabus(raw).unwrap(), jakarta()).unwrap();
    assert_eq!(
        items[0].when,
        Some("2026-11-12T16:59:00Z".parse::<DateTime<Utc>>().unwrap()),
        "23:59 in Asia/Jakarta is 16:59Z"
    );
}

#[test]
fn a_local_time_that_a_dst_transition_skips_is_refused() {
    // Europe/London, 2026-03-29: 01:00–02:00 local does not exist. Shifting it is how a
    // deadline lands an hour off with nothing anywhere saying why.
    let raw = r#"{"course":{},"meetings":[],"deadlines":[
        {"title":"Tugas 1","due":"2026-03-29T01:30:00","weight_percent":null,
         "evidence":"Tugas 1 due 29 Mar","confidence":0.9}],
        "exams":[],"notes":[]}"#;
    let london: Tz = "Europe/London".parse().unwrap();
    assert!(matches!(
        review_items(&parse_syllabus(raw).unwrap(), london),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_date_that_is_not_a_date_is_refused() {
    let raw = r#"{"course":{},"meetings":[],"deadlines":[
        {"title":"Tugas 1","due":"next Friday","weight_percent":null,
         "evidence":"Tugas 1 due Friday","confidence":0.9}],
        "exams":[],"notes":[]}"#;
    assert!(matches!(
        review_items(&parse_syllabus(raw).unwrap(), jakarta()),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_clock_time_that_is_not_hh_mm_is_refused() {
    let raw = one_meeting("Kuliah Senin 8 pagi", 0.9).replace("\"08:00\"", "\"8:00\"");
    assert!(matches!(
        parse_syllabus(&raw),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn an_rrule_the_expander_cannot_use_is_refused() {
    // The rule reaches Phase 10's expander at confirm time, and an unusable one there is a
    // class the student confirmed and never got.
    for rrule in [
        "FREQ=BANANA",
        "FREQ=WEEKLY;BYDAY=XX",
        "FREQ=WEEKLY;INTERVAL=0",
    ] {
        let raw =
            one_meeting("Kuliah Senin 08:00", 0.9).replace("FREQ=WEEKLY;BYDAY=MO;COUNT=3", rrule);
        assert!(
            matches!(
                review_items(&parse_syllabus(&raw).unwrap(), jakarta()),
                Err(GenerationError::Schema(_))
            ),
            "rrule {rrule} must be refused"
        );
    }
}

#[test]
fn a_meeting_with_no_rule_is_refused() {
    let raw = one_meeting("Kuliah Senin 08:00", 0.9).replace("FREQ=WEEKLY;BYDAY=MO;COUNT=3", "");
    assert!(matches!(
        parse_syllabus(&raw),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn an_item_with_no_title_is_refused() {
    let raw = one_meeting("Kuliah Senin 08:00", 0.9).replace("\"Lecture\"", "\"  \"");
    assert!(matches!(
        parse_syllabus(&raw),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn more_items_than_the_bound_are_refused() {
    let meetings = (0..=MAX_MEETINGS)
        .map(|n| {
            format!(
                r#"{{"title":"M{n}","rrule":"FREQ=WEEKLY;COUNT=2","start_time":"08:00",
                     "end_time":"09:00","location":null,"evidence":"class {n}","confidence":0.9}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!(
        r#"{{"course":{{}},"meetings":[{meetings}],"deadlines":[],"exams":[],"notes":[]}}"#
    );
    assert!(matches!(
        parse_syllabus(&raw),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_reply_that_invents_a_field_is_refused() {
    // A drifted prompt has to fail loudly: a field nobody reads is a field the model is
    // spending tokens to fill, and the reply is retried with the real schema.
    let raw = one_meeting("Kuliah Senin 08:00", 0.9)
        .replace("\"location\":null", "\"location\":null,\"room\":\"3\"");
    assert!(matches!(
        parse_syllabus(&raw),
        Err(GenerationError::Schema(_))
    ));
}

#[test]
fn a_fenced_reply_is_accepted() {
    let fenced = format!("```json\n{}\n```", one_meeting("Kuliah Senin 08:00", 0.9));
    assert!(parse_syllabus(&fenced).is_ok());
}

#[test]
fn a_reply_that_is_not_json_is_a_json_error() {
    // The distinction is what the caller retries on, and both retry — but the message the
    // student's log carries has to say which one happened.
    assert!(matches!(
        parse_syllabus("I could not read that syllabus."),
        Err(GenerationError::Json(_))
    ));
}

#[test]
fn an_empty_extraction_is_a_valid_reply() {
    // `json_contract.v1.md` tells the model to return the schema with empty arrays rather
    // than prose when it cannot produce an object, so an empty extraction is a state the
    // contract models: the draft is ready with nothing to review, and the screen says so.
    let raw = r#"{"course":{},"meetings":[],"deadlines":[],"exams":[],"notes":[]}"#;
    let items = review_items(&parse_syllabus(raw).unwrap(), jakarta()).unwrap();
    assert!(items.is_empty());
}
