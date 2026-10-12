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
use stvdy_core::calendar::expand::{
    ExpandError, HORIZON_DAYS, MAX_BODY_BYTES, MAX_OCCURRENCES, Window, expand_feed,
};
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

// --- the bounded expansion (A232) ---------------------------------------------------------

/// A one-`VEVENT` calendar body, CRLF-terminated as RFC 5545 requires, with the `UID` first
/// so a case cannot forget it.
fn feed_of(uid: &str, lines: &[&str]) -> String {
    let mut body = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Fakultas//EN\r\nBEGIN:VEVENT\r\n",
    );
    body.push_str(&format!("UID:{uid}\r\n"));
    for line in lines {
        body.push_str(line);
        body.push_str("\r\n");
    }
    body.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
    body
}

/// A calendar with several `VEVENT`s in it, for the cases where one component must not take
/// the others down with it.
fn feed_of_many(components: &[(&str, Vec<&str>)]) -> String {
    let mut body = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    for (uid, lines) in components {
        body.push_str("BEGIN:VEVENT\r\n");
        body.push_str(&format!("UID:{uid}\r\n"));
        for line in lines {
            body.push_str(line);
            body.push_str("\r\n");
        }
        body.push_str("END:VEVENT\r\n");
    }
    body.push_str("END:VCALENDAR\r\n");
    body
}

/// An unbounded recurrence iteration is a hang, not a wrong answer — and the cap firing is
/// not the same thing as the rule ending.
///
/// `rrule` 0.14 sets `RRuleResult.limited` when the collection stopped at the limit *or* when
/// its own loop cap tripped. With `all(MAX_OCCURRENCES)` that makes a rule with exactly
/// `MAX_OCCURRENCES` occurrences report a truncation it did not have. The first two cases
/// below produce the **same number of occurrences** and must report opposite answers, which
/// is the only way to tell "exactly at the limit" from "stopped by the limit".
#[test]
fn expansion_is_bounded_and_reports_truncation() {
    // No UNTIL and no COUNT: `all()` without a limit never returns. One second apart, so the
    // cap binds long before the horizon does.
    let endless = feed_of(
        "x@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260101T090000",
            "DTEND;TZID=Asia/Jakarta:20260101T100000",
            "RRULE:FREQ=SECONDLY",
            "SUMMARY:Bogus",
        ],
    );
    let out = expand_feed(&endless, Window::days_from(at(2026, 1, 1, 0), HORIZON_DAYS)).unwrap();
    assert_eq!(out.occurrences.len(), MAX_OCCURRENCES, "the cap must bind");
    assert!(out.truncated, "a rule the cap cut must say so");
    // What it read, it keeps: the *earliest* occurrences survive, not the last ones. Dropping
    // the head would put the student's timetable six months out.
    assert_eq!(
        out.occurrences[0].starts_at,
        at(2026, 1, 1, 2),
        "09:00+07:00"
    );
    assert_eq!(
        out.occurrences[MAX_OCCURRENCES - 1].starts_at,
        at(2026, 1, 1, 2) + Duration::seconds(499)
    );

    // Exactly at the limit, and *not* truncated: this rule has 500 occurrences because that
    // is how many it has. Same length as the case above, opposite answer. The window is wider
    // than the horizon so that the horizon is not what ends it either.
    let exactly = feed_of(
        "e@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260101T090000",
            "DTEND;TZID=Asia/Jakarta:20260101T100000",
            "RRULE:FREQ=DAILY;COUNT=500",
            "SUMMARY:Tepat lima ratus",
        ],
    );
    let out = expand_feed(&exactly, Window::days_from(at(2026, 1, 1, 0), 600)).unwrap();
    assert_eq!(
        out.occurrences.len(),
        MAX_OCCURRENCES,
        "the same count as the truncated case"
    );
    assert!(
        !out.truncated,
        "a rule that ended on its own is not a truncation"
    );

    // One more than the cap: 501 occurrences, so the cap does cut one, and the report is a
    // truncation again. This is the boundary the probe's `+1` exists for.
    let over = feed_of(
        "o@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260101T090000",
            "DTEND;TZID=Asia/Jakarta:20260101T100000",
            "RRULE:FREQ=DAILY;COUNT=501",
            "SUMMARY:Lebih satu",
        ],
    );
    let out = expand_feed(&over, Window::days_from(at(2026, 1, 1, 0), 600)).unwrap();
    assert_eq!(out.occurrences.len(), MAX_OCCURRENCES);
    assert!(out.truncated, "the 501st occurrence was dropped");

    // A rule that ends on its own well inside both bounds, and the horizon cutting an endless
    // one: neither is a truncation. The horizon is the design, not a lost occurrence.
    let weekly = feed_of(
        "w@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260105T080000",
            "DTEND;TZID=Asia/Jakarta:20260105T100000",
            "RRULE:FREQ=WEEKLY;COUNT=8",
            "SUMMARY:Kuliah",
        ],
    );
    let out = expand_feed(&weekly, Window::days_from(at(2026, 1, 1, 0), HORIZON_DAYS)).unwrap();
    assert_eq!(out.occurrences.len(), 8);
    assert!(!out.truncated, "a rule that ended is not a truncation");

    let endless_but_short = feed_of(
        "d@f",
        &[
            "DTSTART:20260101T090000Z",
            "RRULE:FREQ=DAILY",
            "SUMMARY:Harian",
        ],
    );
    let out = expand_feed(
        &endless_but_short,
        Window::days_from(at(2026, 1, 1, 0), HORIZON_DAYS),
    )
    .unwrap();
    assert_eq!(out.occurrences.len(), HORIZON_DAYS as usize, "one a day");
    assert!(
        !out.truncated,
        "the horizon ending the walk is not a truncation"
    );
}

/// A `TZID` is resolved through `chrono_tz`, and the rule is expanded *in that zone*: the
/// wall clock is what repeats, so the UTC instant moves across a DST transition.
///
/// Expanding in UTC and adding a fixed offset passes every Jakarta test (no DST) and puts
/// every New York class an hour out for half the year — a silent wrong answer for the
/// students the feature is for.
#[test]
fn a_tzid_event_keeps_its_wall_clock_across_a_dst_boundary() {
    let jakarta = feed_of(
        "j@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260105T080000",
            "DTEND;TZID=Asia/Jakarta:20260105T100000",
            "RRULE:FREQ=WEEKLY;COUNT=2",
            "SUMMARY:Kuliah Jaringan Komputer",
        ],
    );
    let out = expand_feed(&jakarta, Window::days_from(at(2026, 1, 1, 0), 30)).unwrap();
    assert_eq!(
        out.occurrences[0].starts_at,
        at(2026, 1, 5, 1),
        "08:00+07:00"
    );
    assert_eq!(out.occurrences[0].ends_at, at(2026, 1, 5, 3), "10:00+07:00");
    assert_eq!(out.occurrences[1].starts_at, at(2026, 1, 12, 1));
    assert_eq!(out.occurrences[0].recurrence_id, Some(at(2026, 1, 5, 1)));

    // 2026-03-06 is EST (-05) and 2026-03-13 is EDT (-04): 09:00 local is 14:00Z and then
    // 13:00Z, and the hour-long length is preserved on both sides of the transition.
    let new_york = feed_of(
        "n@f",
        &[
            "DTSTART;TZID=America/New_York:20260306T090000",
            "DTEND;TZID=America/New_York:20260306T100000",
            "RRULE:FREQ=WEEKLY;COUNT=2",
            "SUMMARY:Lecture",
        ],
    );
    let out = expand_feed(&new_york, Window::days_from(at(2026, 3, 1, 0), 30)).unwrap();
    assert_eq!(
        out.occurrences[0].starts_at,
        at(2026, 3, 6, 14),
        "09:00 EST"
    );
    assert_eq!(out.occurrences[0].ends_at, at(2026, 3, 6, 15));
    assert_eq!(
        out.occurrences[1].starts_at,
        at(2026, 3, 13, 13),
        "09:00 EDT"
    );
    assert_eq!(out.occurrences[1].ends_at, at(2026, 3, 13, 14));
}

/// A `DTSTART` ending in `Z` is an absolute instant, not a floating wall clock, and must not
/// be re-zoned. Re-zoning `08:00Z` into Jakarta's zone moves it seven hours.
///
/// The same case covers a one-off event: a `VEVENT` with no `RRULE` has exactly one
/// occurrence, and an implementation that hands `rrule` an empty set stores nothing at all —
/// every non-recurring event in the feed silently disappears.
#[test]
fn a_utc_event_is_taken_as_utc() {
    let utc = feed_of(
        "u@f",
        &[
            "DTSTART:20260105T080000Z",
            "DTEND:20260105T093000Z",
            "SUMMARY:Deadline",
        ],
    );
    let out = expand_feed(&utc, Window::days_from(at(2026, 1, 1, 0), 30)).unwrap();
    assert_eq!(
        out.occurrences.len(),
        1,
        "a one-off event is one occurrence"
    );
    assert_eq!(out.occurrences[0].starts_at, at(2026, 1, 5, 8));
    assert_eq!(
        out.occurrences[0].ends_at,
        at(2026, 1, 5, 9) + Duration::minutes(30)
    );
    assert_eq!(
        out.occurrences[0].recurrence_id, None,
        "a non-recurring occurrence has no recurrence id (A231)"
    );

    // The same value as a recurring series: the UTC instant repeats, the wall clock does not
    // move because there is no zone for it to move in.
    let repeating = feed_of(
        "r@f",
        &[
            "DTSTART:20260105T080000Z",
            "DTEND:20260105T090000Z",
            "RRULE:FREQ=WEEKLY;COUNT=2",
            "SUMMARY:UTC weekly",
        ],
    );
    let out = expand_feed(&repeating, Window::days_from(at(2026, 1, 1, 0), 30)).unwrap();
    assert_eq!(out.occurrences[0].starts_at, at(2026, 1, 5, 8));
    assert_eq!(out.occurrences[1].starts_at, at(2026, 1, 12, 8));
    assert_eq!(out.occurrences[1].recurrence_id, Some(at(2026, 1, 12, 8)));
}

/// A malformed component is skipped and named, not fatal (A247): one broken rule in a faculty
/// calendar must not mean the student gets nothing.
///
/// The defect the first half prevents is subtler than a crash: an unknown `TZID` treated as a
/// fallback to UTC stores the class seven hours early, with no error and no way to tell from
/// the app that anything went wrong.
#[test]
fn a_malformed_vevent_is_reported_and_the_rest_survive() {
    let body = feed_of_many(&[
        (
            "bad@f",
            vec![
                "DTSTART;TZID=Mars/Olympus:20260105T080000",
                "DTEND;TZID=Mars/Olympus:20260105T090000",
                "SUMMARY:Kelas di Mars",
            ],
        ),
        (
            "good@f",
            vec![
                "DTSTART;TZID=Asia/Jakarta:20260105T080000",
                "DTEND;TZID=Asia/Jakarta:20260105T090000",
                "SUMMARY:Kelas nyata",
            ],
        ),
    ]);
    let out = expand_feed(&body, Window::days_from(at(2026, 1, 1, 0), 30)).unwrap();

    assert_eq!(out.occurrences.len(), 1, "the good component survives");
    assert_eq!(out.occurrences[0].uid, "good@f");
    assert_eq!(out.occurrences[0].title, "Kelas nyata");
    assert_eq!(out.occurrences[0].starts_at, at(2026, 1, 5, 1));
    assert_eq!(out.skipped.len(), 1, "{:?}", out.skipped);
    assert!(out.skipped[0].contains("bad@f"), "{:?}", out.skipped);
    assert!(out.skipped[0].contains("Mars/Olympus"), "{:?}", out.skipped);

    // A body that is not a calendar at all is an error rather than an empty expansion: an
    // empty expansion is indistinguishable from a feed with no events, and the connection
    // would look healthy while every poll imported nothing.
    let error = expand_feed(
        "<html><body>not a calendar</body></html>",
        Window::days_from(at(2026, 1, 1, 0), 30),
    )
    .unwrap_err();
    assert!(matches!(error, ExpandError::NotACalendar(_)), "{error:?}");

    // And the size guard, which is why the parse cannot be made to run over hundreds of
    // megabytes by a URL the user pasted.
    let huge = "x".repeat(MAX_BODY_BYTES + 1);
    let error = expand_feed(&huge, Window::days_from(at(2026, 1, 1, 0), 30)).unwrap_err();
    assert!(
        matches!(error, ExpandError::TooLarge(bytes) if bytes == MAX_BODY_BYTES + 1),
        "{error:?}"
    );
}

/// An `EXDATE` is a cancelled occurrence, and a cancelled class stored as present is a wrong
/// answer the student acts on.
///
/// The same case covers `RDATE` (an extra session the faculty added) and the `VALUE=DATE`
/// form, which the parser hands over as a date rather than a date-time.
#[test]
fn exdates_are_removed_rdate_added_and_a_date_only_event_is_a_whole_day() {
    let cancelled = feed_of(
        "c@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260105T080000",
            "DTEND;TZID=Asia/Jakarta:20260105T100000",
            "RRULE:FREQ=WEEKLY;COUNT=3",
            "EXDATE;TZID=Asia/Jakarta:20260112T080000",
            "SUMMARY:Kuliah dengan libur",
        ],
    );
    let out = expand_feed(&cancelled, Window::days_from(at(2026, 1, 1, 0), 60)).unwrap();
    let starts: Vec<DateTime<Utc>> = out.occurrences.iter().map(|o| o.starts_at).collect();
    assert_eq!(
        starts,
        vec![at(2026, 1, 5, 1), at(2026, 1, 19, 1)],
        "the 12th is cancelled, the 5th and the 19th are not"
    );

    let extra = feed_of(
        "a@f",
        &[
            "DTSTART;TZID=Asia/Jakarta:20260105T080000",
            "DTEND;TZID=Asia/Jakarta:20260105T100000",
            "RDATE;TZID=Asia/Jakarta:20260107T080000",
            "SUMMARY:Sesi tambahan",
        ],
    );
    let out = expand_feed(&extra, Window::days_from(at(2026, 1, 1, 0), 60)).unwrap();
    let starts: Vec<DateTime<Utc>> = out.occurrences.iter().map(|o| o.starts_at).collect();
    assert_eq!(starts, vec![at(2026, 1, 5, 1), at(2026, 1, 7, 1)]);

    // A `VALUE=DATE` event: one whole day, and `DTEND` is exclusive so two days means two
    // days. A UTC anchor is the only deterministic choice in a pure function — there is no
    // user zone here to resolve a date against.
    let all_day = feed_of(
        "h@f",
        &[
            "DTSTART;VALUE=DATE:20260105",
            "DTEND;VALUE=DATE:20260107",
            "SUMMARY:UAS Jaringan Komputer",
        ],
    );
    let out = expand_feed(&all_day, Window::days_from(at(2026, 1, 1, 0), 60)).unwrap();
    assert_eq!(out.occurrences.len(), 1);
    assert_eq!(out.occurrences[0].starts_at, at(2026, 1, 5, 0));
    assert_eq!(out.occurrences[0].ends_at, at(2026, 1, 7, 0));
    assert_eq!(
        out.occurrences[0].title, "UAS Jaringan Komputer",
        "an Indonesian title survives the import (gate 6)"
    );
}

/// The conflict policy as a function, so it has a test rather than a comment (A234).
///
/// The four cases are the roadmap's table, and the one worth reading twice is the last pair:
/// a remote tombstone is honoured for an event Google owns and **ignored** for one we created,
/// because `calendar.app.created` cannot delete what it cannot write. Acting on it would
/// delete the student's own study block — the exact outcome the scope pair exists to prevent.
#[test]
fn google_wins_for_an_event_we_did_not_create_and_we_win_for_a_study_block() {
    use stvdy_core::calendar::policy::{
        LocalState, Origin, RemoteState, Winner, origin_of, resolve,
    };

    // source = 'google': the remote is authoritative for every field.
    assert_eq!(
        resolve(Origin::Google, LocalState::dirty(), RemoteState::changed()),
        Winner::Remote
    );
    // source = 'app' mirrored to Google: the local edit is the user's, and it is pushed.
    assert_eq!(
        resolve(Origin::App, LocalState::dirty(), RemoteState::changed()),
        Winner::Local
    );
    // A remote tombstone for a Google event is honoured; for an app event it is ignored and
    // the local row is re-pushed, because Google cannot delete what it cannot write.
    assert_eq!(
        resolve(Origin::Google, LocalState::clean(), RemoteState::deleted()),
        Winner::Remote
    );
    assert_eq!(
        resolve(Origin::App, LocalState::clean(), RemoteState::deleted()),
        Winner::Local
    );

    // The origin comes from the stored `source`, and an unknown value is the calendar's: a
    // stale row is recoverable, a silently overwritten edit is not.
    assert_eq!(origin_of("app"), Origin::App);
    assert_eq!(origin_of("google"), Origin::Google);
    assert_eq!(origin_of("ics"), Origin::Google);
}
