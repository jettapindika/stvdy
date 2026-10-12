//! Parsing someone else's ICS feed, and expanding its recurrence under two bounds.
//!
//! Pure: no database, no network, no clock. The server fetches the bytes and calls
//! [`expand_feed`], which is what lets the phase's only hang risk be tested in a unit test
//! rather than in a worker that never comes back.
//!
//! **The risk this module exists to close.** `rrule`'s iterator will walk a `FREQ=DAILY`
//! rule with no `UNTIL` to the end of time, and a `FREQ=SECONDLY` rule reaches the same
//! place in milliseconds. That is a hang, not a wrong answer, and the roadmap's risk register
//! names it. So expansion is bounded twice — by the caller's [`Window`] and by
//! [`MAX_OCCURRENCES`] — and **truncation is reported, never inferred** (A232). See
//! [`PROBE`] for why the count asked of `rrule` is one more than the count kept: that one
//! extra occurrence is the whole difference between "this rule ended" and "the cap cut it",
//! and getting it wrong records `recurrence_truncated` on a connection that was never
//! truncated.
//!
//! **Timezones.** A `TZID` is resolved through `chrono_tz` and the rule is expanded *in that
//! zone*, converted to UTC only at the boundary. Expanding in UTC and adding a fixed offset
//! is the mistake that breaks at a DST transition: an America/New_York class at 09:00 is
//! 14:00Z in March and 13:00Z the following week. A `TZID` the tz database does not know is a
//! skipped component, never a fallback to UTC — treating a Jakarta class as UTC moves it
//! seven hours and reads as a scheduling bug with no error anywhere.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _, Utc};
use chrono_tz::Tz;
use icalendar::parser::{Component, Parameter as ParsedParameter, Property as ParsedProperty};
use icalendar::{CalendarDateTime, DatePerhapsTime, Property};
use rrule::{RRule, RRuleSet, Tz as RRuleTz, Unvalidated};

/// The hard cap on occurrences of one series (A232).
///
/// The horizon is what Phase 11's planner needs; this is what makes even an `FREQ=SECONDLY`
/// rule finite *inside* the horizon.
pub const MAX_OCCURRENCES: usize = 500;

/// The default horizon, in days. Six months is the far end of what the planner and the
/// calendar screen look at.
pub const HORIZON_DAYS: i64 = 180;

/// The largest feed this will parse. A hostile or broken URL can serve hundreds of megabytes
/// and the parse runs in a worker, so the size is refused before the parser sees it.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// How many occurrences are asked of `rrule`: **one more than are kept**.
///
/// `RRuleResult::limited` means "the list may be incomplete", and `rrule` 0.14 sets it when
/// either its own internal loop cap tripped *or* the collection stopped because it reached
/// the limit it was given (verified in the crate's `core/utils.rs`: `limited = was_limited ||
/// limit == list.len()`). So asking for exactly `MAX_OCCURRENCES` makes a rule that has
/// precisely that many occurrences report `limited == true` — a rule that ended on its own,
/// recorded as truncated. Asking for one more than are kept removes the ambiguity: a natural
/// end at or below the cap gives `limited == false`, and `limited == true` then means one of
/// two real truncations — the cap fired, or the engine gave up mid-iteration — with both
/// costing the caller occurrences it should have had.
const PROBE: u16 = (MAX_OCCURRENCES + 1) as u16;

/// The range of time an expansion is asked for. Occurrences are selected by their start, so
/// an occurrence that starts inside the window and ends after it is included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl Window {
    /// A window starting at `from` and `days` long.
    pub fn days_from(from: DateTime<Utc>, days: i64) -> Self {
        Window {
            from,
            to: from + Duration::days(days),
        }
    }
}

/// One occurrence of one `VEVENT`, ready to become an `events` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    /// The series' `UID`. A231 builds `events.external_id` from this.
    pub uid: String,
    /// The occurrence's start in UTC for a recurring series, and `None` for a one-off event
    /// (A231: `"<UID>"` alone for a non-recurring one).
    pub recurrence_id: Option<DateTime<Utc>>,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// What one feed expanded to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// Every occurrence read, in start order.
    pub occurrences: Vec<Occurrence>,
    /// True when the occurrence cap cut a series short, or the engine stopped early. The
    /// server records `recurrence_truncated` on the connection from this and **keeps every
    /// occurrence it read** (A232, A247): one endless rule must not cost the student the
    /// rest of their timetable.
    pub truncated: bool,
    /// One reason per `VEVENT` that could not be expanded, naming the component. Not fatal
    /// (A247): the feed's other events are still stored.
    pub skipped: Vec<String>,
}

/// Why a feed could not be expanded at all. A single bad component is not one of these — it
/// lands in [`Expansion::skipped`].
#[derive(Debug, thiserror::Error)]
pub enum ExpandError {
    #[error("the body is not an iCalendar document: {0}")]
    NotACalendar(String),
    #[error("the feed is {0} bytes, larger than the {MAX_BODY_BYTES} byte limit")]
    TooLarge(usize),
}

/// Parses `body` and expands every `VEVENT` in it over `window`.
///
/// **Never calls `all_unchecked()`**: the unbounded call is the hang this module exists to
/// prevent. `RRuleResult.limited` is read rather than inferred from the result length.
pub fn expand_feed(body: &str, window: Window) -> Result<Expansion, ExpandError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ExpandError::TooLarge(body.len()));
    }

    // `read_components` does not unfold; the crate's own `FromStr` impls call `unfold` first,
    // and a folded `SUMMARY` read without it comes back cut at the fold with its tail parsed
    // as a property nobody wrote.
    let unfolded = icalendar::parser::unfold(body);
    let roots = icalendar::parser::read_components(&unfolded).map_err(ExpandError::NotACalendar)?;
    let calendar = roots
        .iter()
        .find(|root| root.name.as_str().eq_ignore_ascii_case("VCALENDAR"))
        .ok_or_else(|| ExpandError::NotACalendar("no VCALENDAR component".to_string()))?;

    let mut expansion = Expansion {
        occurrences: Vec::new(),
        truncated: false,
        skipped: Vec::new(),
    };
    for component in calendar
        .components
        .iter()
        .filter(|child| child.name.as_str().eq_ignore_ascii_case("VEVENT"))
    {
        match expand_one(component, window) {
            Ok(series) => {
                expansion.truncated |= series.truncated;
                expansion.occurrences.extend(series.occurrences);
            }
            Err(reason) => expansion.skipped.push(reason),
        }
    }

    // Deterministic, and not a database order: `crates/server` upserts these rows and a test
    // reads `occurrences[0]`. The tie-break on `uid` keeps two events at the same instant in
    // a stable order across runs.
    expansion
        .occurrences
        .sort_by(|a, b| (a.starts_at, &a.uid).cmp(&(b.starts_at, &b.uid)));
    Ok(expansion)
}

/// One series' occurrences, and whether the cap cut it.
struct Series {
    occurrences: Vec<Occurrence>,
    truncated: bool,
}

/// Expands one `VEVENT`. `Err` is the reason it was skipped, already naming the component.
fn expand_one(component: &Component<'_>, window: Window) -> Result<Series, String> {
    // A231 keys the row on the UID, and RFC 5545 makes it required, so a component without
    // one cannot be stored idempotently and is named rather than guessed at.
    let uid = component
        .find_prop("UID")
        .map(|property| property.val.as_str().trim().to_string())
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "a VEVENT with no UID".to_string())?;
    let name = |reason: &str| format!("{uid}: {reason}");

    let title = component
        .find_prop("SUMMARY")
        .map(|property| property.val.as_str().to_string())
        .unwrap_or_default();

    let start_property = component
        .find_prop("DTSTART")
        .ok_or_else(|| name("no DTSTART"))?;
    let start = parse_instant("DTSTART", start_property).map_err(|reason| name(&reason))?;
    let starts_at = start.to_utc().map_err(|reason| name(&reason))?;

    // The end is read as an instant and subtracted, rather than as a wall clock and added:
    // an event that spans a DST transition is longer than its local clock says, and RFC 5545
    // makes `DTEND` the actual end.
    let duration = match component.find_prop("DTEND") {
        Some(end_property) => {
            let end = parse_instant("DTEND", end_property).map_err(|reason| name(&reason))?;
            let ends_at = end.to_utc().map_err(|reason| name(&reason))?;
            ends_at - starts_at
        }
        None => start.default_duration(),
    };
    if duration < Duration::zero() {
        return Err(name("DTEND is before DTSTART"));
    }

    let rule_start = start.rule_start().map_err(|reason| name(&reason))?;
    let mut set = RRuleSet::new(rule_start);
    let mut has_rule = false;
    let mut has_extra_dates = false;

    if let Some(raw) = component.find_prop("RRULE").map(|p| p.val.as_str()) {
        let rule: RRule<Unvalidated> = raw
            .parse()
            .map_err(|error| name(&format!("RRULE is not a valid rule: {error}")))?;
        let rule = rule
            .validate(rule_start)
            .map_err(|error| name(&format!("RRULE cannot be expanded from DTSTART: {error}")))?;
        set = set.rrule(rule);
        has_rule = true;
    }

    // `RDATE` and `EXDATE` are comma-separated lists, and a cancelled class arrives as an
    // `EXDATE`: dropping either would store an occurrence the faculty calendar says is not
    // happening, or lose one it says is.
    for property in component
        .properties
        .iter()
        .filter(|p| p.name.as_str().eq_ignore_ascii_case("RDATE"))
    {
        for value in property.val.as_str().split(',') {
            let instant =
                parse_value("RDATE", value.trim(), property).map_err(|reason| name(&reason))?;
            set = set.rdate(instant.rule_start().map_err(|reason| name(&reason))?);
            has_extra_dates = true;
        }
    }
    for property in component
        .properties
        .iter()
        .filter(|p| p.name.as_str().eq_ignore_ascii_case("EXDATE"))
    {
        for value in property.val.as_str().split(',') {
            let instant =
                parse_value("EXDATE", value.trim(), property).map_err(|reason| name(&reason))?;
            set = set.exdate(instant.rule_start().map_err(|reason| name(&reason))?);
        }
    }

    if !has_rule {
        // RFC 5545 § 3.6.1: `DTSTART` is always the first occurrence of the set, whether the
        // component is a one-off or carries only `RDATE`s. `rrule` needs to be told — an
        // `RRuleSet` with no rule and no date yields *nothing*, so a one-off event would
        // vanish from the import and an `RDATE`-only series would lose its first session.
        set = set.rdate(rule_start);
    }
    // A series is recurring if it repeats, which is what A231 keys the occurrence on.
    let recurring = has_rule || has_extra_dates;

    let result = set
        .after(window.from.with_timezone(&RRuleTz::UTC))
        .before(window.to.with_timezone(&RRuleTz::UTC))
        .all(PROBE);

    let occurrences = result
        .dates
        .into_iter()
        .take(MAX_OCCURRENCES)
        .map(|date| {
            let starts_at = date.with_timezone(&Utc);
            Occurrence {
                uid: uid.clone(),
                recurrence_id: recurring.then_some(starts_at),
                title: title.clone(),
                starts_at,
                ends_at: starts_at + duration,
            }
        })
        .collect();

    Ok(Series {
        occurrences,
        truncated: result.limited,
    })
}

/// A `DTSTART`, `DTEND`, `RDATE` or `EXDATE` value, before it is resolved to an instant.
enum Instant {
    /// A wall clock in a named zone. The rule is expanded in this zone.
    Zoned(NaiveDateTime, Tz),
    /// An absolute instant: a value ending in `Z`.
    Utc(DateTime<Utc>),
    /// A `VALUE=DATE` value: a whole day, with no wall clock to keep.
    Date(NaiveDate),
}

impl Instant {
    /// The instant, as `rrule` wants it: a wall clock in the zone the rule is expanded in.
    ///
    /// A local time that does not exist — the hour a DST transition skips — is an error
    /// rather than a guess, for the same reason an unknown `TZID` is: silently shifting a
    /// class by an hour is a wrong answer that looks like a scheduling bug.
    fn rule_start(&self) -> Result<DateTime<RRuleTz>, String> {
        Ok(match self {
            Instant::Zoned(local, zone) => RRuleTz::from(*zone)
                .from_local_datetime(local)
                .single()
                .ok_or_else(|| format!("{local} does not exist in {zone} (a DST transition)"))?,
            Instant::Utc(instant) => instant.with_timezone(&RRuleTz::UTC),
            Instant::Date(date) => RRuleTz::UTC.from_utc_datetime(&date.and_time(NaiveTime::MIN)),
        })
    }

    /// The same instant in UTC.
    fn to_utc(&self) -> Result<DateTime<Utc>, String> {
        self.rule_start().map(|instant| instant.with_timezone(&Utc))
    }

    /// The length of an event whose `DTEND` is absent. RFC 5545 § 3.6.1: a `DATE` event
    /// with no `DTEND` lasts one day; a `DATE-TIME` event with no `DTEND` is zero-length.
    fn default_duration(&self) -> Duration {
        match self {
            Instant::Date(_) => Duration::days(1),
            Instant::Zoned(_, _) | Instant::Utc(_) => Duration::zero(),
        }
    }
}

/// Reads one property's value.
fn parse_instant(name: &str, property: &ParsedProperty<'_>) -> Result<Instant, String> {
    parse_value(name, property.val.as_str(), property)
}

/// Reads one value of a property, with that property's parameters.
///
/// Split out from [`parse_instant`] because `RDATE` and `EXDATE` carry a comma-separated
/// list in one content line, and each element keeps the line's `TZID`/`VALUE` parameters.
fn parse_value(name: &str, value: &str, property: &ParsedProperty<'_>) -> Result<Instant, String> {
    // The crate's own reader is reused rather than a second date-time parser being written
    // here: it is what handles `VALUE=DATE`, the `TZID` parameter, the `Z` suffix and a
    // floating value, and a second copy of those rules is a second place to get them wrong.
    let bridged = bridge(name, value, &property.params);
    match DatePerhapsTime::from_property(&bridged) {
        Some(DatePerhapsTime::Date(date)) => Ok(Instant::Date(date)),
        Some(DatePerhapsTime::DateTime(CalendarDateTime::Utc(instant))) => {
            Ok(Instant::Utc(instant))
        }
        Some(DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, tzid })) => {
            let zone = resolve_tz(&tzid)
                .ok_or_else(|| format!("{name} names the unknown timezone {tzid}"))?;
            Ok(Instant::Zoned(date_time, zone))
        }
        // A floating value is a wall clock with no zone. There is no user zone in a pure
        // function to resolve it against, and guessing the machine's would make the import
        // depend on where the server runs, so it is taken as UTC — the one choice that is
        // the same everywhere.
        Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(local))) => {
            Ok(Instant::Utc(local.and_utc()))
        }
        None => Err(format!("{name} is not a date or a date-time: {value}")),
    }
}

/// Resolves a `TZID` through `chrono_tz`, never through a fixed offset and never through the
/// machine's zone. `None` is an unknown zone, which the caller reports as a skipped
/// component.
fn resolve_tz(raw: &str) -> Option<Tz> {
    raw.parse::<Tz>().ok()
}

/// A `TZID`-carrying property, in the form the crate's own reader expects.
fn bridge(name: &str, value: &str, params: &[ParsedParameter<'_>]) -> Property {
    let mut property = Property::new(name, value);
    for param in params {
        if let Some(val) = param.val.as_ref() {
            property.add_parameter(param.key.as_str(), val.as_str());
        }
    }
    property
}
