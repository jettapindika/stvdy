//! The syllabus-extraction JSON contract (`prompts/syllabus.v1.md`), parsed and mapped to
//! the review screen's rows.
//!
//! Pure: a reply string in, review items out. The retry rule lives in the caller, but the
//! decision it retries on — is this reply usable — is made here, so the rule is testable
//! without a model.
//!
//! **The evidence quote is the substance of this module, not a field to fill in.** The
//! review screen shows the student the sentence an item was read from, and that quote is
//! what lets them check the item before anything is written. An item with no quote is an
//! item the student cannot check, so an empty one is a parse failure rather than a weaker
//! citation, and the reply is retried instead of reaching the screen.

use chrono::{DateTime, NaiveDateTime, NaiveTime, TimeZone as _, Utc};
use chrono_tz::Tz;
use rrule::{RRule, Unvalidated};
use uuid::Uuid;

use super::{strip_code_fence, GenerationError};

/// The most items one reply may carry. Above what any real syllabus states, so a model that
/// ignores the module cannot fill the review screen with rows the student has to delete.
pub const MAX_MEETINGS: usize = 40;
pub const MAX_DEADLINES: usize = 60;
pub const MAX_EXAMS: usize = 20;
pub const MAX_NOTES: usize = 20;

/// The module says "under 15 words"; this is what makes it enforceable rather than
/// aspirational. The bound is what stops a model quoting a paragraph and calling it evidence.
pub const MAX_EVIDENCE_WORDS: usize = 15;

/// Below this, the review screen marks the item as needing a check (A261). The number itself
/// never reaches the screen: the honesty rules forbid showing a confidence score, so it is
/// used to order the list and to flag an item, and the evidence quote is what is shown.
pub const REVIEW_FLAG_CONFIDENCE: f64 = 0.6;

/// How many occurrences one kept meeting may become (A269).
///
/// Tighter than [`crate::calendar::expand::MAX_OCCURRENCES`], which is the ICS import's cap
/// and has to admit a feed's whole horizon. A semester is about sixteen weeks, so 60 rows
/// covers a class that meets twice a week with room to spare; a rule that asks for more than
/// that is a model malfunction, and the alternative is a typo in an `RRULE` filling a
/// student's calendar with thousands of rows they then have to delete.
pub const MAX_OCCURRENCES: usize = 60;

/// The reply, exactly as `prompts/syllabus.v1.md` documents it.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that invents a
/// field produces a parse error and a retry rather than a review screen missing a field.
/// `Serialize` is the other half of the pair: the draft stores this object verbatim, and the
/// review list is recomputed from it on read rather than stored twice.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SyllabusFile {
    pub course: CourseInfo,
    pub meetings: Vec<Meeting>,
    pub deadlines: Vec<Deadline>,
    pub exams: Vec<Exam>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CourseInfo {
    pub name: Option<String>,
    pub code: Option<String>,
    pub instructor: Option<String>,
}

/// One recurring class. The module gives a rule and a local clock time and no date: the
/// first occurrence's date is derived at confirm time from the rule itself.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Meeting {
    pub title: String,
    pub rrule: String,
    pub start_time: String,
    pub end_time: String,
    pub location: Option<String>,
    pub evidence: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Deadline {
    pub title: String,
    pub due: String,
    pub weight_percent: Option<f64>,
    pub evidence: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exam {
    pub title: String,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub topics: Vec<String>,
    pub evidence: String,
    pub confidence: f64,
}

/// One row of the review screen, with the source line it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewItem {
    pub kind: ReviewKind,
    /// The confirm body names an item by this index. It is the item's position in the reply,
    /// counted across the module's own field order (`meetings`, then `deadlines`, then
    /// `exams`), so it is unique within a reply and stable across a re-parse of it — which is
    /// what lets the student's edits and drops be matched back to the items they saw.
    pub index: usize,
    pub title: String,
    pub evidence: String,
    pub confidence: f64,
    pub needs_check: bool,
    /// The absolute instant, when the item has one. A meeting has none: its dates come from
    /// the rule, and the rule has no anchor date in the extraction.
    pub when: Option<DateTime<Utc>>,
    pub rrule: Option<String>,
    pub start_time: Option<(u32, u32)>,
    pub end_time: Option<(u32, u32)>,
    pub topics: Vec<String>,
    pub weight_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewKind {
    Meeting,
    Deadline,
    Exam,
}

impl ReviewKind {
    /// The word the review screen labels a row with, and the `events.kind` a confirmed
    /// meeting writes (A269).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meeting => "class",
            Self::Deadline => "deadline",
            Self::Exam => "exam",
        }
    }
}

/// Parses a model reply into the extraction, refusing anything the review screen could not
/// show honestly.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite the
/// contract. It does **not** tolerate prose around the object: guessing which part is the
/// JSON is how a half-parsed import ships.
pub fn parse_syllabus(raw: &str) -> Result<SyllabusFile, GenerationError> {
    let text = strip_code_fence(raw);
    let file: SyllabusFile = serde_json::from_str(text).map_err(|error| {
        // `serde_json` reports both "not JSON" and "wrong shape" through the same error type;
        // the message distinguishes them, and the caller only needs to know a retry is worth
        // one attempt.
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    validate(&file)?;
    Ok(file)
}

/// The rules that do not need a timezone: bounds, the evidence quote, and the confidence.
///
/// One implementation, called by both [`parse_syllabus`] and [`review_items`]: a stored
/// draft is re-parsed and re-mapped on every read, and a second copy of these rules is how
/// the two paths would stop agreeing about what the screen may show.
fn validate(file: &SyllabusFile) -> Result<(), GenerationError> {
    for (count, max, what) in [
        (file.meetings.len(), MAX_MEETINGS, "meetings"),
        (file.deadlines.len(), MAX_DEADLINES, "deadlines"),
        (file.exams.len(), MAX_EXAMS, "exams"),
        (file.notes.len(), MAX_NOTES, "notes"),
    ] {
        if count > max {
            return Err(GenerationError::Schema(format!(
                "the model returned {count} {what}, more than the {max} allowed"
            )));
        }
    }

    for (index, meeting) in file.meetings.iter().enumerate() {
        validate_item(
            "meeting",
            index,
            &meeting.title,
            &meeting.evidence,
            meeting.confidence,
        )?;
        if meeting.rrule.trim().is_empty() {
            return Err(GenerationError::Schema(format!(
                "meeting {index}: the rrule is empty, so the class has no dates at all"
            )));
        }
        parse_clock("meeting", index, "start_time", &meeting.start_time)?;
        parse_clock("meeting", index, "end_time", &meeting.end_time)?;
    }
    for (index, deadline) in file.deadlines.iter().enumerate() {
        validate_item(
            "deadline",
            index,
            &deadline.title,
            &deadline.evidence,
            deadline.confidence,
        )?;
        if deadline.due.trim().is_empty() {
            return Err(GenerationError::Schema(format!(
                "deadline {index}: `due` is empty; a deadline with no date is not a deadline"
            )));
        }
    }
    for (index, exam) in file.exams.iter().enumerate() {
        validate_item("exam", index, &exam.title, &exam.evidence, exam.confidence)?;
    }

    Ok(())
}

/// The three rules every item shares.
///
/// The evidence bound is the module's own ("a short exact snippet copied from the source,
/// under 15 words"), and the message names the item so the retry's corrective turn can be
/// about that item rather than about the reply as a whole.
fn validate_item(
    what: &str,
    index: usize,
    title: &str,
    evidence: &str,
    confidence: f64,
) -> Result<(), GenerationError> {
    if title.trim().is_empty() {
        return Err(GenerationError::Schema(format!(
            "{what} {index}: the title is empty"
        )));
    }
    let words = evidence.split_whitespace().count();
    if words == 0 {
        return Err(GenerationError::Schema(format!(
            "{what} {index}: the evidence is empty, so the student has no source line to \
             check the item against"
        )));
    }
    if words > MAX_EVIDENCE_WORDS {
        return Err(GenerationError::Schema(format!(
            "{what} {index}: the evidence is {words} words, more than the \
             {MAX_EVIDENCE_WORDS} the module allows"
        )));
    }
    if !(0.0..=1.0).contains(&confidence) {
        return Err(GenerationError::Schema(format!(
            "{what} {index}: confidence is {confidence}, outside 0.0..=1.0"
        )));
    }
    Ok(())
}

/// One `HH:MM` clock time.
///
/// The shape is checked before the parse because `chrono` accepts an unpadded hour: `8:00`
/// parses as `%H:%M`, and the module says `HH:MM`. The difference matters downstream, where
/// the value becomes a `DTSTART` property, and a lenient parse here would be the one place
/// the contract is looser than the file that documents it.
fn parse_clock(
    what: &str,
    index: usize,
    field: &str,
    raw: &str,
) -> Result<(u32, u32), GenerationError> {
    use chrono::Timelike as _;

    let raw = raw.trim();
    let digits = raw.as_bytes();
    let shaped = raw.len() == 5
        && digits[2] == b':'
        && [0, 1, 3, 4].iter().all(|at| digits[*at].is_ascii_digit());
    if !shaped {
        return Err(GenerationError::Schema(format!(
            "{what} {index}: {field} is not HH:MM: {raw}"
        )));
    }
    let parsed = NaiveTime::parse_from_str(raw, "%H:%M").map_err(|_| {
        GenerationError::Schema(format!("{what} {index}: {field} is not HH:MM: {raw}"))
    })?;
    Ok((parsed.hour(), parsed.minute()))
}

/// Builds the review list from an extraction, ordered by confidence ascending.
///
/// The order is the point of the confidence number (A261): the items the model was least
/// sure of are at the top, where the student's attention is. The tie-break on
/// `(kind, index)` makes the order total, so two parses of one reply give one list.
///
/// It refuses rather than drops. An item it cannot place — a date that does not parse in the
/// student's zone, a rule `rrule` will not expand — is a `Schema` error and a retry, because
/// a dropped item is a deadline the student never sees.
pub fn review_items(file: &SyllabusFile, tz: Tz) -> Result<Vec<ReviewItem>, GenerationError> {
    validate(file)?;

    let mut items = Vec::new();

    for (index, meeting) in file.meetings.iter().enumerate() {
        let rrule = validate_rrule(index, &meeting.rrule)?;
        let start = parse_clock("meeting", index, "start_time", &meeting.start_time)?;
        let end = parse_clock("meeting", index, "end_time", &meeting.end_time)?;
        items.push(ReviewItem {
            kind: ReviewKind::Meeting,
            index,
            title: meeting.title.trim().to_string(),
            evidence: meeting.evidence.trim().to_string(),
            confidence: meeting.confidence,
            needs_check: meeting.confidence < REVIEW_FLAG_CONFIDENCE,
            when: None,
            rrule: Some(rrule),
            start_time: Some(start),
            end_time: Some(end),
            topics: Vec::new(),
            weight_percent: None,
        });
    }

    let meeting_count = file.meetings.len();
    for (offset, deadline) in file.deadlines.iter().enumerate() {
        let index = meeting_count + offset;
        let when = resolve_instant(tz, "deadline", index, "due", &deadline.due)?;
        items.push(ReviewItem {
            kind: ReviewKind::Deadline,
            index,
            title: deadline.title.trim().to_string(),
            evidence: deadline.evidence.trim().to_string(),
            confidence: deadline.confidence,
            needs_check: deadline.confidence < REVIEW_FLAG_CONFIDENCE,
            when: Some(when),
            rrule: None,
            start_time: None,
            end_time: None,
            topics: Vec::new(),
            weight_percent: deadline.weight_percent,
        });
    }

    let deadline_count = meeting_count + file.deadlines.len();
    for (offset, exam) in file.exams.iter().enumerate() {
        let index = deadline_count + offset;
        let when = match exam.starts_at.as_deref().map(str::trim) {
            Some(raw) if !raw.is_empty() => {
                Some(resolve_instant(tz, "exam", index, "starts_at", raw)?)
            }
            // A syllabus that never states when the exam is has no date to show, and the
            // module is explicit that an absent date is `null` rather than a guess.
            _ => None,
        };
        items.push(ReviewItem {
            kind: ReviewKind::Exam,
            index,
            title: exam.title.trim().to_string(),
            evidence: exam.evidence.trim().to_string(),
            confidence: exam.confidence,
            needs_check: exam.confidence < REVIEW_FLAG_CONFIDENCE,
            when,
            rrule: None,
            start_time: None,
            end_time: None,
            topics: exam.topics.clone(),
            weight_percent: None,
        });
    }

    items.sort_by(|a, b| {
        a.confidence
            .total_cmp(&b.confidence)
            .then_with(|| (a.kind, a.index).cmp(&(b.kind, b.index)))
    });
    Ok(items)
}

/// The rule, parsed and validated the way [`crate::calendar::expand`] will expand it.
///
/// The probe `DTSTART` is deliberately not "now": this is a shape check, and validating
/// against the confirm instant would refuse a syllabus from a past semester whose `UNTIL`
/// has already passed. `rrule`'s only `DTSTART`-sensitive validator is `UNTIL >= DTSTART`.
fn validate_rrule(index: usize, raw: &str) -> Result<String, GenerationError> {
    let rule: RRule<Unvalidated> = raw.trim().parse().map_err(|error| {
        GenerationError::Schema(format!(
            "meeting {index}: the rrule does not parse: {error}"
        ))
    })?;
    let validated = rule
        .validate(DateTime::UNIX_EPOCH.with_timezone(&rrule::Tz::UTC))
        .map_err(|error| {
            GenerationError::Schema(format!(
                "meeting {index}: the rrule cannot be expanded: {error}"
            ))
        })?;
    // `INTERVAL=0` passes `rrule`'s own validation and then never advances its counter, so
    // the expansion at confirm time would walk the window forever rather than return no
    // dates. It is the one shape the crate accepts and cannot answer.
    if validated.get_interval() == 0 {
        return Err(GenerationError::Schema(format!(
            "meeting {index}: the rrule has INTERVAL=0, which can never advance, so the \
             series has no dates"
        )));
    }
    Ok(raw.trim().to_string())
}

/// An ISO 8601 instant, resolved in the student's own zone.
///
/// The module asks for an offset (`2026-01-01T23:59:00+07:00`), and that arm is the one
/// taken for a well-formed reply. A value with no offset is resolved in `tz` rather than
/// refused: the module's rule is that dates are resolved *in the stated zone*, and refusing
/// the naive form would make the one mistake a syllabus import cannot survive — resolving a
/// date in the wrong zone — the likelier outcome. A local time that does not exist (the hour
/// a DST transition skips) is refused, never shifted.
fn resolve_instant(
    tz: Tz,
    what: &str,
    index: usize,
    field: &str,
    raw: &str,
) -> Result<DateTime<Utc>, GenerationError> {
    let raw = raw.trim();
    let refused = || {
        GenerationError::Schema(format!(
            "{what} {index}: {field} is not an ISO 8601 date-time: {raw}"
        ))
    };

    if let Ok(absolute) = DateTime::parse_from_rfc3339(raw) {
        return Ok(absolute.with_timezone(&Utc));
    }

    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(local) = NaiveDateTime::parse_from_str(raw, format) {
            return tz
                .from_local_datetime(&local)
                .earliest()
                .map(|instant| instant.with_timezone(&Utc))
                .ok_or_else(|| {
                    GenerationError::Schema(format!(
                        "{what} {index}: {field} is {local}, which does not exist in {tz} \
                         (a DST transition)"
                    ))
                });
        }
    }

    Err(refused())
}

/// One class occurrence a kept meeting expands to, ready to become an `events` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingOccurrence {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// Expands one kept meeting's rule into the occurrences the confirm writes (A269).
///
/// The expansion is [`crate::calendar::expand::expand_feed`]'s — Phase 10's, the one the ICS
/// import already uses — and this function's job is only to state the meeting in the form
/// that module reads. A second recurrence engine is the failure this avoids: two engines
/// eventually disagree about a `BYDAY` rule, and the disagreement shows up as a class meeting
/// on the wrong day.
///
/// The anchor is `from`'s local date at `start_time`: the rule then expands forward from
/// there, and a `BYDAY` filter picks the matching weekdays out of it. That is why an
/// extraction with no date is still enough to write the series — the module gives a rule and
/// a clock time, and "the next matching weekday" is what a rule anchored today means.
///
/// The window opens at the **start of the anchor's local day**, not at `from` itself. The
/// expander selects occurrences that start after the window opens, so a window starting at the
/// confirmation instant would silently drop a class that meets earlier in the day than the
/// student happened to confirm — Monday's 08:00 lecture, confirmed on Monday at 10:00, is
/// still part of the timetable.
///
/// A `start_time` at or after `end_time` means the class runs past midnight, so the end is
/// the next day's; refusing it would drop a 22:00 lab.
#[allow(clippy::too_many_arguments)]
pub fn expand_meeting(
    uid: Uuid,
    title: &str,
    rrule: &str,
    start_time: (u32, u32),
    end_time: (u32, u32),
    tz: Tz,
    from: DateTime<Utc>,
    horizon_days: i64,
) -> Result<Vec<MeetingOccurrence>, GenerationError> {
    let rrule = validate_rrule(0, rrule)?;
    let date = from.with_timezone(&tz).date_naive();
    let starts = local_instant(tz, date, start_time)?;
    let end_date = if end_time <= start_time {
        date.succ_opt().unwrap_or(date)
    } else {
        date
    };
    let ends = local_instant(tz, end_date, end_time)?;

    let body = one_event_ics(uid, title, starts, ends, tz, &rrule);

    let midnight = local_instant(tz, date, (0, 0))?;
    let window = crate::calendar::expand::Window::days_from(midnight, horizon_days);
    let expansion = crate::calendar::expand::expand_feed(&body, window).map_err(|error| {
        GenerationError::Schema(format!("the meeting's rule could not be expanded: {error}"))
    })?;
    if let Some(reason) = expansion.skipped.first() {
        return Err(GenerationError::Schema(format!(
            "the meeting's rule could not be expanded: {reason}"
        )));
    }

    // `truncated` is reported rather than inferred (A232). Both checks refuse rather than
    // silently writing part of a series: a student who confirmed a class that meets twice a
    // week must not get 60 rows of a daily rule and a calendar that looks right.
    if expansion.truncated {
        return Err(GenerationError::Schema(format!(
            "the meeting's rule produces more than the {MAX_OCCURRENCES} occurrences a \
             semester can hold"
        )));
    }
    if expansion.occurrences.len() > MAX_OCCURRENCES {
        return Err(GenerationError::Schema(format!(
            "the meeting's rule produces {} occurrences in the next semester, more than the \
             {MAX_OCCURRENCES} a class can have",
            expansion.occurrences.len()
        )));
    }

    let occurrences: Vec<MeetingOccurrence> = expansion
        .occurrences
        .iter()
        .map(|occurrence| MeetingOccurrence {
            starts_at: occurrence.starts_at,
            ends_at: occurrence.ends_at,
        })
        .collect();
    if occurrences.is_empty() {
        return Err(GenerationError::Schema(
            "the meeting's rule produces no occurrence in the next semester".to_string(),
        ));
    }
    Ok(occurrences)
}

/// A local wall clock on `date`, resolved in `tz`.
///
/// A time that does not exist — the hour a DST transition skips — is refused rather than
/// shifted, for the same reason [`resolve_instant`] refuses one: silently moving a class by
/// an hour is a wrong answer that looks like a scheduling bug.
fn local_instant(
    tz: Tz,
    date: chrono::NaiveDate,
    time: (u32, u32),
) -> Result<DateTime<Utc>, GenerationError> {
    let local = date.and_hms_opt(time.0, time.1, 0).ok_or_else(|| {
        GenerationError::Schema(format!("{date} {:02}:{:02} is not a time", time.0, time.1))
    })?;
    tz.from_local_datetime(&local)
        .earliest()
        .map(|instant| instant.with_timezone(&Utc))
        .ok_or_else(|| {
            GenerationError::Schema(format!("{local} does not exist in {tz} (a DST transition)"))
        })
}

/// One `VEVENT`, as the ICS text [`crate::calendar::expand::expand_feed`] reads.
///
/// Written here rather than through `calendar::feed::render_feed` because that renderer
/// emits `events` rows, and an `events` row has no rule to render (A231's expand-on-write
/// rule is why). The recurrence engine is still Phase 10's: this is the input its parser
/// documents, not a second implementation of it.
fn one_event_ics(
    uid: Uuid,
    title: &str,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    tz: Tz,
    rrule: &str,
) -> String {
    let local = |instant: DateTime<Utc>| {
        instant
            .with_timezone(&tz)
            .format("%Y%m%dT%H%M%S")
            .to_string()
    };
    let stamp = starts_at.format("%Y%m%dT%H%M%SZ").to_string();
    // RFC 5545 § 3.3.11: TEXT escapes backslash, comma, semicolon and newline. A title with
    // a comma in it would otherwise split the `SUMMARY` line and hand its tail back as a
    // property nobody wrote.
    let summary = title
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(';', "\\;")
        .replace(['\n', '\r'], " ");

    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//stvdy//study calendar//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}@stvdy.app\r\n\
         DTSTAMP:{stamp}\r\n\
         DTSTART;TZID={tz}:{}\r\n\
         DTEND;TZID={tz}:{}\r\n\
         SUMMARY:{summary}\r\n\
         RRULE:{rrule}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n",
        local(starts_at),
        local(ends_at),
    )
}
