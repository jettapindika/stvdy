//! The quick-capture draft contract.
//!
//! The cases here are the ones a model actually gets wrong, and the ones whose failure is
//! *invisible* in a UTC-only suite: a due date resolved in the student's offset but read as
//! UTC, a naive date interpreted in the wrong zone, and a bad date dropped without a word.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use stvdy_core::generation::capture::{CaptureDraft, parse_draft};
use stvdy_core::generation::{GenerationError, is_retryable};

/// The student's zone, which is the whole point: a UTC-only test suite cannot see the
/// defect this file exists to prevent.
fn jakarta() -> Tz {
    "Asia/Jakarta".parse().unwrap()
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

/// The reply's offset is kept, not re-derived. A model told the timezone resolves "Jumat"
/// to `2026-10-16T23:59:00+07:00`; reading that as UTC moves the deadline seven hours and,
/// at 23:59, a whole day.
#[test]
fn a_due_date_keeps_the_offset_the_model_resolved_it_in() {
    let raw = r#"{"title":"Selesaikan tugas kripto","notes":null,
                  "due_at":"2026-10-16T23:59:00+07:00","priority":1,"estimate_min":90}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.due_at, Some(at("2026-10-16T16:59:00Z")));
    assert_eq!(draft.priority, 1);
    assert_eq!(draft.estimate_min, Some(90));
    assert!(draft.warnings.is_empty(), "{:?}", draft.warnings);
}

/// An unreadable date is a warning, not a lost field and not a failed request (A194): the
/// credits are already spent by the time this runs, so failing here would charge the student
/// and hand them nothing.
#[test]
fn an_unreadable_due_date_becomes_a_warning_and_keeps_the_draft() {
    let raw = r#"{"title":"Baca bab 4","notes":null,"due_at":"friday-ish",
                  "priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("the draft still comes back");
    assert_eq!(draft.due_at, None);
    assert_eq!(draft.warnings.len(), 1);
    assert!(
        draft.warnings[0].contains("friday-ish"),
        "{:?}",
        draft.warnings
    );
}

/// A naive datetime is read in the student's zone, which is the one thing the model was told
/// and the one thing it most often drops. Reading it as UTC would move a 23:59 Jakarta
/// deadline to 23:59 UTC — seven hours later, and a different day for an early-morning one.
#[test]
fn a_naive_due_date_is_read_in_the_students_zone() {
    let raw = r#"{"title":"Kuis","notes":null,"due_at":"2026-10-16T23:59:00",
                  "priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.due_at, Some(at("2026-10-16T16:59:00Z")));
    assert_eq!(
        draft.warnings.len(),
        1,
        "the assumption is stated, not silent"
    );
    assert!(
        draft.warnings[0].contains("2026-10-16T23:59:00"),
        "{:?}",
        draft.warnings
    );
}

/// A date with no time is midnight local, the day the student meant — not midnight UTC,
/// which in Jakarta is 07:00 on that same day but 17:00 on the *previous* UTC day.
#[test]
fn a_date_only_due_date_is_midnight_in_the_students_zone() {
    let raw = r#"{"title":"Kumpulkan laporan","notes":null,"due_at":"2026-10-16",
                  "priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.due_at, Some(at("2026-10-15T17:00:00Z")));
    assert_eq!(
        draft.warnings.len(),
        1,
        "the assumption is stated, not silent"
    );
}

/// An out-of-range priority or estimate would reach the schema and fail the insert. The
/// priority falls back to the documented default rather than to the nearest boundary: a
/// `7` means the model ignored the scale, and clamping it to `3` would record "low" for a
/// value that said nothing about urgency. An oversized estimate is clamped instead, because
/// a number too large is still a number.
#[test]
fn out_of_range_numbers_are_clamped_with_a_warning() {
    let raw = r#"{"title":"Tugas","notes":null,"due_at":null,
                  "priority":7,"estimate_min":9000}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(
        draft.priority, 2,
        "the documented default, not the nearest bound"
    );
    assert_eq!(draft.estimate_min, Some(1440));
    assert_eq!(draft.warnings.len(), 2, "{:?}", draft.warnings);
}

/// An estimate of zero minutes is not an estimate, and a negative one is a model error. Both
/// become "no estimate" with a warning rather than a stored zero, which would show as a task
/// that takes no time.
#[test]
fn a_non_positive_estimate_becomes_no_estimate_with_a_warning() {
    for value in ["0", "-30"] {
        let raw = format!(
            r#"{{"title":"Tugas","notes":null,"due_at":null,"priority":2,"estimate_min":{value}}}"#
        );
        let draft = parse_draft(&raw, jakarta()).expect("parses");
        assert_eq!(draft.estimate_min, None, "estimate_min {value}");
        assert_eq!(draft.warnings.len(), 1, "{:?}", draft.warnings);
        assert!(draft.warnings[0].contains(value), "{:?}", draft.warnings);
    }
}

/// An empty title is a schema failure and therefore retryable, because a task with no title
/// is not a task and the second attempt costs one call rather than a broken row.
#[test]
fn an_empty_title_is_a_schema_failure() {
    let raw = r#"{"title":"   ","notes":null,"due_at":null,"priority":2,"estimate_min":null}"#;
    assert!(matches!(
        parse_draft(raw, jakarta()),
        Err(GenerationError::Schema(_))
    ));
    assert!(is_retryable(&parse_draft(raw, jakarta()).unwrap_err()));
}

/// A fenced reply still parses: a provider adds the fence despite the module saying not to,
/// and the shared `strip_code_fence` is what makes that a retry nobody pays for.
#[test]
fn a_fenced_reply_still_parses() {
    let raw = "```json\n{\"title\":\"Selesaikan tugas kripto\",\"notes\":null,\"due_at\":\"2026-10-16T23:59:00+07:00\",\"priority\":1,\"estimate_min\":90}\n```";
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.title, "Selesaikan tugas kripto");
    assert_eq!(draft.due_at, Some(at("2026-10-16T16:59:00Z")));
}

/// The fields the reply may leave out are genuinely optional, and an absent `priority` is
/// the documented "normal" rather than a zero that would fail the schema.
#[test]
fn an_absent_priority_is_normal_and_absent_fields_are_null() {
    let raw = r#"{"title":"Tugas"}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.priority, 2);
    assert_eq!(draft.notes, None);
    assert_eq!(draft.due_at, None);
    assert_eq!(draft.estimate_min, None);
    assert!(draft.warnings.is_empty(), "{:?}", draft.warnings);
}

/// A blank note is not a note: it would store an empty string where the schema has a null,
/// and the UI would render an empty notes section for every capture.
#[test]
fn a_blank_note_becomes_no_note_and_the_title_is_trimmed() {
    let raw = r#"{"title":"  Tugas kripto  ","notes":"   ","due_at":null,"priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.title, "Tugas kripto");
    assert_eq!(draft.notes, None);

    let raw = r#"{"title":"Tugas kripto","notes":"  Bab 4 dan 5  ","due_at":null,"priority":2,"estimate_min":null}"#;
    let draft = parse_draft(raw, jakarta()).expect("parses");
    assert_eq!(draft.notes.as_deref(), Some("Bab 4 dan 5"));
}

/// A reply that is not JSON at all is a parse failure, and a field the contract does not
/// have is a schema failure: the two retry the same way, but they are different defects and
/// the message is what tells them apart.
#[test]
fn a_non_json_reply_is_a_parse_failure_and_an_unknown_field_is_a_schema_failure() {
    assert!(matches!(
        parse_draft("I could not do that.", jakarta()),
        Err(GenerationError::Json(_))
    ));

    let raw = r#"{"title":"Tugas","notes":null,"due_at":null,"priority":2,"estimate_min":null,"due":"besok"}"#;
    assert!(matches!(
        parse_draft(raw, jakarta()),
        Err(GenerationError::Schema(_))
    ));
}

/// The type is the contract's public face: the route serialises it, and the field names are
/// what the SPA reads. A rename here is a client break, so it is pinned.
#[test]
fn the_draft_serialises_with_the_field_names_the_client_reads() {
    let raw = r#"{"title":"Tugas","notes":null,"due_at":"2026-10-16T23:59:00+07:00","priority":2,"estimate_min":null}"#;
    let draft: CaptureDraft = parse_draft(raw, jakarta()).expect("parses");
    let json = serde_json::to_value(&draft).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "title": "Tugas",
            "notes": null,
            "due_at": "2026-10-16T16:59:00Z",
            "priority": 2,
            "estimate_min": null,
            "warnings": [],
        })
    );
}
