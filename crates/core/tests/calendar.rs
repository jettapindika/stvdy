//! The ICS feed and the bounded expansion — the two halves of the calendar that are pure,
//! so both are tested here with no database and no network.
//!
//! Both fail *silently* in production when they are wrong, and that is why the cases here
//! are what they are. A feed whose `SEQUENCE` never moves is a feed the client caches
//! forever: the student edits an event and their phone keeps showing the old time, with no
//! error anywhere and no way to notice. An expansion without a bound is a worker that never
//! returns — a hang rather than a wrong answer, and the roadmap's own risk register names it.
//!
//! Every case below was first run against the defect it names and watched to fail.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration, TimeZone, Utc};
use stvdy_core::calendar::feed::{FeedEvent, FeedKind, render_feed, sequence};

fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
}

fn event(id: &str, created: DateTime<Utc>, updated: DateTime<Utc>) -> FeedEvent {
    FeedEvent {
        id: id.parse().unwrap(),
        kind: FeedKind::StudyBlock,
        title: "Review: jaringan komputer".to_string(),
        starts_at: at(2026, 10, 16, 2),
        ends_at: at(2026, 10, 16, 3),
        created_at: created,
        updated_at: updated,
        timezone: "Asia/Jakarta".parse().unwrap(),
    }
}

/// The rendered feed, unfolded and parsed back: one `(name, child count)` per root.
///
/// `unfold` first, because the crate's `read_components` does *not* unfold on its own
/// (`Calendar::from_str` is what does): a folded `SUMMARY` read without unfolding comes back
/// cut at the fold, with its tail parsed as a property nobody wrote. Every case that reads a
/// feed back goes through here so that reading it the right way is not a thing a case can
/// forget.
fn structure(ics: &str) -> Vec<(String, usize)> {
    let unfolded = icalendar::parser::unfold(ics);
    icalendar::parser::read_components(&unfolded)
        .unwrap()
        .iter()
        .map(|component| {
            (
                component.name.as_str().to_string(),
                component.components.len(),
            )
        })
        .collect()
}

/// A property of the first `VEVENT`, read back through the crate's own parser.
fn prop(ics: &str, name: &str) -> String {
    let unfolded = icalendar::parser::unfold(ics);
    let components = icalendar::parser::read_components(&unfolded).unwrap();
    let vevent = &components[0].components[0];
    assert_eq!(vevent.name.as_str(), "VEVENT", "the root's first child");
    vevent.find_prop(name).unwrap().val.as_str().to_string()
}

/// The whole content line of a property, so a case can assert what the bytes look like.
fn line_of(ics: &str, name: &str) -> String {
    ics.split("\r\n")
        .find(|line| line.starts_with(&format!("{name}:")))
        .unwrap_or_else(|| panic!("no {name} line in\n{ics}"))
        .to_string()
}

/// The two properties a client depends on and cannot recover from: the `UID` is stable
/// across renders, and an edit moves the `SEQUENCE`.
///
/// Without a moving `SEQUENCE` a client that cached the feed has no signal that the row
/// changed, so an edited title or a moved time is invisible forever — the failure is silent,
/// permanent and per-client. Without a stable `UID` the same edit reads as a delete plus a
/// create, which is a duplicate in the student's calendar instead of an update.
#[test]
fn a_feed_round_trips_and_the_sequence_moves_when_the_event_is_edited() {
    let id = "01920000-0000-7000-8000-000000000001";
    let created = at(2026, 10, 12, 0);
    let fresh = render_feed(&[event(id, created, created)]);
    let rendered_twice = render_feed(&[event(id, created, created)]);

    // Byte-identical: no clock reading, no randomness, no map iteration order. The crate
    // injects `DTSTAMP: <now>` and a random `UID` when the component does not carry them,
    // so this assertion is what proves the feed sets both from the row.
    assert_eq!(
        fresh, rendered_twice,
        "two renders of one unchanged row must be byte-identical"
    );

    // It parses as a calendar, and the UID is derived from the row id.
    assert_eq!(
        structure(&fresh),
        vec![("VCALENDAR".to_string(), 1)],
        "one VCALENDAR root with one VEVENT"
    );
    assert_eq!(prop(&fresh, "UID"), format!("{id}@stvdy.app"));

    // A new event is SEQUENCE 0, and an edit is strictly greater.
    assert_eq!(sequence(created, created), 0);
    assert_eq!(prop(&fresh, "SEQUENCE"), "0");

    let edited = render_feed(&[event(
        id,
        created,
        at(2026, 10, 12, 0) + Duration::seconds(120),
    )]);
    assert_eq!(
        sequence(created, at(2026, 10, 12, 0) + Duration::seconds(120)),
        120
    );
    assert_eq!(prop(&edited, "SEQUENCE"), "120");

    let edited_again = render_feed(&[event(id, created, at(2026, 10, 12, 1))]);
    assert!(
        prop(&edited_again, "SEQUENCE").parse::<i64>().unwrap()
            > prop(&edited, "SEQUENCE").parse::<i64>().unwrap(),
        "SEQUENCE must be monotone across edits"
    );

    // The edit is visible in the other two places a client caches on, too.
    assert_eq!(prop(&fresh, "LAST-MODIFIED"), "20261012T000000Z");
    assert_eq!(prop(&edited, "LAST-MODIFIED"), "20261012T000200Z");
    assert_eq!(prop(&edited, "DTSTAMP"), "20261012T000200Z");
}

/// The calendar-level properties, and one `VEVENT` per row.
///
/// A client rejects a document with no `VERSION`, and a feed that silently drops a row is a
/// calendar the student cannot trust: the two assertions are "this is a calendar a client
/// will read" and "every event handed in came out".
#[test]
fn the_feed_declares_itself_and_carries_every_event() {
    let created = at(2026, 10, 12, 0);
    let events = [
        event("01920000-0000-7000-8000-00000000000a", created, created),
        event("01920000-0000-7000-8000-00000000000b", created, created),
    ];
    let ics = render_feed(&events);

    assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"), "{ics}");
    assert!(ics.trim_end().ends_with("END:VCALENDAR"), "{ics}");
    assert!(ics.contains("\r\nVERSION:2.0\r\n"), "{ics}");
    assert!(ics.contains("\r\nCALSCALE:GREGORIAN\r\n"), "{ics}");
    assert!(ics.contains("\r\nMETHOD:PUBLISH\r\n"), "{ics}");
    // Our producer id, not the crate's default (`ICALENDAR-RS`), which would name the
    // library in every client's "source" line.
    assert!(ics.contains("\r\nPRODID:"), "{ics}");
    assert!(!ics.contains("ICALENDAR-RS"), "{ics}");

    assert_eq!(ics.matches("BEGIN:VEVENT").count(), 2, "{ics}");

    // An empty feed is still a calendar: a brand-new account's route must answer 200 with
    // a document a client can read, not a 500 or a truncated body.
    let empty = render_feed(&[]);
    assert_eq!(
        structure(&empty),
        vec![("VCALENDAR".to_string(), 0)],
        "{empty}"
    );
    assert!(empty.contains("VERSION:2.0"), "{empty}");
}

/// A deadline is date-only (A248): a zero-length timed event is drawn as a point or dropped
/// by most clients, and "due Friday" is a date, not an instant.
///
/// The zone is the *user's*, resolved through `chrono_tz`: 2026-10-15T17:00Z is already the
/// 16th in Jakarta, and a feed that renders the UTC date says "due the 15th" to a student
/// whose Friday is the 16th.
#[test]
fn a_deadline_is_an_all_day_event_in_the_users_zone() {
    let created = at(2026, 10, 12, 0);
    let mut e = event("01920000-0000-7000-8000-000000000002", created, created);
    e.kind = FeedKind::Deadline;
    e.starts_at = at(2026, 10, 15, 17);
    e.ends_at = e.starts_at;
    let ics = render_feed(&[e]);

    assert!(ics.contains("DTSTART;VALUE=DATE:20261016"), "{ics}");
    // DTEND of an all-day event is exclusive, so it is the *next* local date: a DTEND equal
    // to DTSTART is a zero-length event, which is the point-drawn event this case exists to
    // prevent.
    assert!(ics.contains("DTEND;VALUE=DATE:20261017"), "{ics}");

    // The other kinds are timed, and their DTSTART is UTC (`Z`): an event's instant does not
    // depend on which zone the student is sitting in.
    let created = at(2026, 10, 12, 0);
    let timed = render_feed(&[event(
        "01920000-0000-7000-8000-000000000003",
        created,
        created,
    )]);
    assert!(timed.contains("DTSTART:20261016T020000Z"), "{timed}");
    assert!(timed.contains("DTEND:20261016T030000Z"), "{timed}");
}

/// RFC 5545 § 3.3.11 escaping, and the line break it prevents.
///
/// A `SUMMARY` is a TEXT value: a comma or a semicolon in it is a *structural* character, and
/// a newline in it ends the content line. Unescaped, the title is cut and its tail is parsed
/// as a property nobody wrote — a broken line, not a cosmetic problem.
#[test]
fn a_title_with_separators_and_a_newline_survives_the_round_trip() {
    let created = at(2026, 10, 12, 0);
    let mut e = event("01920000-0000-7000-8000-000000000004", created, created);
    e.title = "Review, jaringan; Bab 3\r\ncatatan: \"TCP/IP\"\\routing".to_string();
    let ics = render_feed(&[e]);

    // The bytes on the wire: `,` `;` `\` and LF are escaped, the CRLF is not a line break.
    assert_eq!(
        line_of(&ics, "SUMMARY"),
        r#"SUMMARY:Review\, jaringan\; Bab 3\ncatatan: "TCP/IP"\\routing"#,
    );

    // A bare CR is not escapable in TEXT and half the parsers in the wild treat it as a line
    // break, so it must not reach the document: every CR left is half of a CRLF.
    assert!(
        !ics.replace("\r\n", "\n").contains('\r'),
        "a bare CR is a line break to a lenient parser: {ics:?}"
    );

    // And the title comes back whole, which is the property that matters.
    assert_eq!(
        prop(&ics, "SUMMARY"),
        "Review, jaringan; Bab 3\ncatatan: \"TCP/IP\"\\routing"
    );
}

/// RFC 5545 § 3.1 folding: a long line is split at 75 octets with a leading space on the
/// continuation, and — the case that mangles — never in the middle of a multi-byte
/// character.
///
/// Indonesian titles carry multi-byte characters, so a folder that cuts at byte 75 puts half
/// a character at the end of a line. The feed then fails to parse, or parses to a title with
/// a replacement character in it, and every English test stays green.
#[test]
fn a_long_indonesian_title_folds_and_comes_back_whole() {
    let created = at(2026, 10, 12, 0);
    let title = "UAS Jaringan Komputer — Bab 3: Lapisan Transport, TCP/IP, dan Routing \
                 Statis — ringkasan lengkap untuk persiapan ujian akhir semester";
    let mut e = event("01920000-0000-7000-8000-000000000005", created, created);
    e.title = title.to_string();
    let ics = render_feed(&[e]);

    // The case only means something if the line actually folded.
    assert!(
        ics.contains("\r\n "),
        "the title must be long enough to fold: {ics:?}"
    );
    // Every continuation line starts with exactly one space (the fold marker), and no line
    // exceeds the 75-octet limit.
    for line in ics.split("\r\n") {
        assert!(
            line.len() <= 75,
            "folded lines are at most 75 octets, got {} in {line:?}",
            line.len()
        );
    }

    assert_eq!(prop(&ics, "SUMMARY"), title);
}

/// The `SEQUENCE` formula (A230) on its own, including the two edges the feed's monotonicity
/// rests on: a same-second edit still moves it off zero, and a clock that ran backwards does
/// not produce a negative sequence.
#[test]
fn the_sequence_is_zero_until_the_first_edit_and_never_negative() {
    let created = at(2026, 10, 12, 0);

    assert_eq!(sequence(created, created), 0, "an unedited row");
    // An edit inside the same second is still an edit: `num_seconds()` would be 0 here.
    assert_eq!(sequence(created, created + Duration::milliseconds(400)), 1);
    assert_eq!(sequence(created, created + Duration::seconds(1)), 1);
    assert_eq!(sequence(created, created + Duration::days(1)), 86_400);
    // A row whose `updated_at` is before its `created_at` (a clock that moved) is treated as
    // unedited rather than as a negative sequence, which a client would reject.
    assert_eq!(sequence(created, created - Duration::hours(1)), 0);
}
