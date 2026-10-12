//! The ICS feed: `events` rows in, RFC 5545 text out.
//!
//! Pure: no clock, no database, no IO. The server reads the rows and calls [`render_feed`],
//! which is what makes the two properties a client depends on — a stable `UID` and a
//! `SEQUENCE` that moves when the row is edited — testable without one.
//!
//! A feed is a cache the client owns, and nothing in it is negotiable. A client that caches
//! on `UID` alone and finds no `SEQUENCE` has no signal that the event changed, so an edited
//! title or a moved time is invisible forever, per client, with no error raised on either
//! side (A230). A `UID` that changes is the same defect from the other end: the edit reads as
//! a delete plus a create, which is a duplicate in the student's calendar instead of an
//! update. Both are why this module sets every property a client caches on from the row.

use chrono::{DateTime, Days, NaiveDate, Utc};
use chrono_tz::Tz;
use icalendar::{Calendar, Component, Event, EventLike, Property};
use uuid::Uuid;

/// The producer id every feed carries (RFC 5545 § 3.7.3). Ours rather than the crate's
/// default, which would name the library in the client's "source" line.
const PRODID: &str = "-//stvdy//study calendar//EN";

/// What a feed row is. `Deadline` is the only kind that is all-day (A248).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Class,
    Exam,
    StudyBlock,
    Deadline,
}

/// One `events` row, as the feed needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEvent {
    /// `events.id`. The `UID` is derived from it, so it must be the row's own id.
    pub id: Uuid,
    pub kind: FeedKind,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The *user's* zone, from `users.timezone`. Only a deadline reads it: every other kind
    /// is a UTC instant, and an instant is the same instant in every zone.
    pub timezone: Tz,
}

/// `SEQUENCE` (A230): 0 for a row that has never been edited, and the whole seconds between
/// creation and the last edit after that, floored at 1 so the first edit is always an
/// increase even when it lands in the same second as the create.
///
/// Two edits inside one second produce the same value, and the same `LAST-MODIFIED`, because
/// both are second-granular. That is the stated limitation: within one second there is no
/// observable difference to lose, and a counter column would be a second write path for a
/// value nobody reads.
pub fn sequence(created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> i64 {
    if updated_at > created_at {
        (updated_at - created_at).num_seconds().max(1)
    } else {
        0
    }
}

/// `VERSION`, `PRODID`, `CALSCALE:GREGORIAN`, `METHOD:PUBLISH`, and one `VEVENT` per row.
///
/// Deterministic: no clock reading, no randomness, no map iteration order. Two calls with
/// the same rows produce byte-identical text, which is what lets a client's cache work and
/// what makes a diff of two feeds readable.
pub fn render_feed(events: &[FeedEvent]) -> String {
    let mut calendar = Calendar::empty();
    calendar
        .append_property(Property::new("VERSION", "2.0"))
        .append_property(Property::new("PRODID", PRODID))
        .append_property(Property::new("CALSCALE", "GREGORIAN"))
        .append_property(Property::new("METHOD", "PUBLISH"));

    for event in events {
        calendar.push(render_event(event));
    }

    calendar.to_string()
}

/// One `VEVENT`, with every property a client caches on set from the row.
///
/// Not optional: the crate writes `DTSTAMP: <now>` and a random `UID` into a component that
/// does not carry them, so a component that omitted either would make two renders of one
/// unchanged row differ — and a feed that differs on every poll is a feed every client
/// re-downloads and re-imports.
fn render_event(event: &FeedEvent) -> Event {
    // RFC 5545 § 3.8.7.4 makes SEQUENCE a 32-bit integer. The clamp is unreachable from a
    // real `created_at`/`updated_at` pair (it needs a 136-year edit history); it exists so
    // the conversion is total rather than a panic in a request handler.
    let seq = u32::try_from(sequence(event.created_at, event.updated_at)).unwrap_or(u32::MAX);

    let mut vevent = Event::with_uid(&format!("{}@stvdy.app", event.id));
    vevent
        .summary(&normalize_text(&event.title))
        .timestamp(event.updated_at)
        .last_modified(event.updated_at)
        .sequence(seq);

    match event.kind {
        FeedKind::Deadline => {
            // A248: date-only in the user's zone, so "due Friday" is the student's Friday.
            // `ends_at` is not read: A199 makes a deadline's `ends_at` equal to its
            // `starts_at`, and a zero-length timed event is drawn as a point or dropped.
            let date = event.starts_at.with_timezone(&event.timezone).date_naive();
            vevent.starts(date).ends(next_day(date));
        }
        FeedKind::Class | FeedKind::Exam | FeedKind::StudyBlock => {
            // A248: a timed event carries a UTC instant, which is `DTSTART:...Z`. The same
            // event is the same instant for a student in Jakarta and one in New York.
            vevent.starts(event.starts_at).ends(event.ends_at);
        }
    }

    vevent.done()
}

/// The day after `date`. `DTEND` of an all-day event is exclusive (RFC 5545 § 3.6.1), and a
/// `DTEND` equal to `DTSTART` is the zero-length event A248 exists to avoid.
fn next_day(date: NaiveDate) -> NaiveDate {
    date.checked_add_days(Days::new(1)).unwrap_or(date)
}

/// Drops what an RFC 5545 TEXT value cannot carry, before the crate escapes the rest.
///
/// `icalendar` escapes `\`, `,`, `;` and LF for every property whose value type is `Text`,
/// which `SUMMARY` is (verified in its `properties.rs`, not assumed). A CR is *not* escapable
/// and is a line break to a lenient parser: a title containing CRLF would split the `SUMMARY`
/// line and hand its tail back as a property nobody wrote — a broken document, not a
/// cosmetic problem. Other C0 control characters are not allowed in TEXT at all. Both are
/// dropped here; HTAB and LF are the two control characters TEXT does allow, and LF is the
/// line break inside a title that `\n` represents.
fn normalize_text(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\t' | '\n') || !c.is_control())
        .collect()
}
