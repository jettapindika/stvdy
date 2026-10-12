//! The reminder engine: what to remind, when, and under which key.
//!
//! Pure. `now` is an argument and never a call, and nothing in here is keyed by iteration
//! order, because the plan's *order* is part of the contract: two phones and the server's own
//! sweep derive the same plan from the same inputs, and a plan whose order or whose keys moved
//! between two runs is a reminder that fires twice on one device and never on another.
//! `tests/reminders_purity.rs` enforces that at the source level, the way
//! `tests/planner_purity.rs` does for the planner.
//!
//! Three rules carry the module, and each is a place the plausible implementation is wrong:
//!
//!  * [`dedupe_key`] is the subject and the user's local date, **never the computed fire
//!    instant** (A284). Two devices that compute 19:00 and 19:01 for the same task produce the
//!    same key, and a quiet-hours deferral moves `fire_at` while leaving the key alone. A key
//!    built from the fire instant fires once per device.
//!  * [`in_quiet_hours`] wraps past midnight. `(22, 7)` is 22:00→07:00, so the naive
//!    `start <= hour && hour < end` is false for every hour it is supposed to cover.
//!  * a fire time that is a **local wall-clock** time (the day's card reminder, and the end of
//!    a quiet window) is resolved through `chrono_tz` and never through a fixed offset. A
//!    local hour that does not exist on a spring-forward day is skipped with a reason, and an
//!    ambiguous one on a fall-back day takes the earlier offset. `Asia/Jakarta` has no DST, so
//!    a test written there proves nothing about either.
//!
//! Task and event reminders need none of that: their `fire_at` is the subject's own instant
//! minus a lead, and an instant arithmetic never lands on a wall-clock hour that does not
//! exist. Only the *local* facts — the local date a reminder belongs to, quiet hours, and the
//! study hour the day's card reminder fires at — go through the zone.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use uuid::Uuid;

use crate::config::ReminderChannel;

/// How far before a task's due time its reminder fires.
pub const LEAD_TASK_MIN: i64 = 60;

/// How far before an event's start time its reminder fires.
pub const LEAD_EVENT_MIN: i64 = 30;

/// After this, a deferred reminder is not worth sending.
///
/// Measured against the *subject* — the task's due instant, or the local day a card reminder
/// is for — and not against the fire instant, so it answers "is this still about the thing the
/// student asked to be reminded of". A quiet window is shorter than a day, so a task or event
/// deferral can never reach it: the bound bites on the day-granular card reminder, whose
/// subject is a whole day and whose fire time can therefore be pushed into the next one.
pub const STALE_AFTER_MIN: i64 = 24 * 60;

/// The furthest ahead a reminder may be scheduled, and the sweep's horizon.
pub const MAX_LOOKAHEAD_DAYS: i64 = 14;

/// How far the walk for a window's end or a missing local hour may run before it gives up.
/// Two days is more than the longest quiet window, and a bound keeps the search total.
const MAX_WALK_HOURS: i64 = 48;

/// The three reminders the product schedules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    /// A task is due. Fires [`LEAD_TASK_MIN`] before its `due_at`.
    TaskDue,
    /// An event starts. Fires [`LEAD_EVENT_MIN`] before its `starts_at`.
    EventSoon,
    /// Cards are due. One per local day, at the student's study start hour.
    CardsDue,
}

impl ReminderKind {
    pub const ALL: [ReminderKind; 3] = [
        ReminderKind::TaskDue,
        ReminderKind::EventSoon,
        ReminderKind::CardsDue,
    ];

    /// The `reminders.kind` value, and the payload's `kind` on `POST /reminders`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReminderKind::TaskDue => "task_due",
            ReminderKind::EventSoon => "event_soon",
            ReminderKind::CardsDue => "cards_due",
        }
    }

    /// The inverse of [`as_str`](Self::as_str). `None` for anything else, so a client's typo
    /// is a validation error rather than a fourth kind.
    ///
    /// The `allow` is the same price the tool registry pays for the same signature: `Option`
    /// rather than the trait's `Result`, because an unknown name is a validation refusal the
    /// caller renders, not an error it propagates (A271's precedent in `agent/tools.rs`).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(name: &str) -> Option<ReminderKind> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The lead before the subject's own instant, or `None` for the day-granular reminder,
    /// whose time is the student's study hour rather than an offset from a row.
    pub fn lead_min(self) -> Option<i64> {
        match self {
            ReminderKind::TaskDue => Some(LEAD_TASK_MIN),
            ReminderKind::EventSoon => Some(LEAD_EVENT_MIN),
            ReminderKind::CardsDue => None,
        }
    }
}

/// The one key function, called by both producers (A284).
///
/// `subject_key` is the task's or event's id, or the local date for the day-granular card
/// reminder. `local_date` is the reminder's local date in the *user's* zone. The fire instant
/// is deliberately absent: it is the one value a deferral changes and two devices can
/// disagree about by a minute, which is exactly what must not move the key.
pub fn dedupe_key(kind: ReminderKind, subject_key: &str, local_date: NaiveDate) -> String {
    format!("{}:{}:{}", kind.as_str(), subject_key, local_date)
}

/// One thing worth reminding about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub kind: ReminderKind,
    pub id: Uuid,
    /// The task's `due_at` or the event's `starts_at`. `None` for `CardsDue`.
    pub at: Option<DateTime<Utc>>,
    /// A completed subject produces no reminder at all.
    pub done: bool,
}

/// The student's reminder inputs, all of them read from rows the caller already owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderSettings {
    pub tz: Tz,
    /// `(start_hour, end_hour)` in the user's local zone, or `None` when the student has not
    /// set a window. The window may wrap midnight (A202).
    pub quiet: Option<(u8, u8)>,
    pub channels: Vec<ReminderChannel>,
    /// The local hour the day's card reminder fires at.
    pub study_start_hour: u8,
    /// How many cards the student's own deck says are due today.
    pub cards_due: i64,
}

/// One planned reminder. `skip` is a reason, not an omission: a reminder that silently never
/// arrives is what the student reports as "reminders are broken".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedReminder {
    pub kind: ReminderKind,
    pub subject_key: String,
    pub subject_id: Option<Uuid>,
    pub local_date: NaiveDate,
    pub fire_at: DateTime<Utc>,
    pub dedupe_key: String,
    /// True when quiet hours moved it.
    pub deferred: bool,
    pub skip: Option<SkipReason>,
}

/// Why a reminder will not be delivered, kept beside the reminder itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Quiet hours pushed it past [`STALE_AFTER_MIN`] from its subject.
    QuietHours,
    /// The subject's own instant has already passed.
    AlreadyPast,
    /// The local wall-clock time it fires at does not exist on that date (a spring-forward
    /// gap). Named rather than shifted silently, because "the reminder moved by an hour" is a
    /// different claim from "there was no such hour today".
    NonexistentLocalTime,
}

impl SkipReason {
    pub const ALL: [SkipReason; 3] = [
        SkipReason::QuietHours,
        SkipReason::AlreadyPast,
        SkipReason::NonexistentLocalTime,
    ];

    /// The `reminders.skip_reason` / `reminder_deliveries.reason` value.
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::QuietHours => "quiet_hours",
            SkipReason::AlreadyPast => "already_past",
            SkipReason::NonexistentLocalTime => "nonexistent_local_time",
        }
    }
}

/// The plan for one account: deterministic, sorted by `(fire_at, kind, subject_key)`, and
/// deduped by [`dedupe_key`] so the earliest fire time wins for a key two inputs share.
///
/// A subject beyond [`MAX_LOOKAHEAD_DAYS`] is **not planned at all** rather than planned and
/// skipped: it is not a decision, it is simply not this sweep's business, and a later sweep
/// will see it. A subject that is done, or an account with no channels, produces nothing.
pub fn plan(
    now: DateTime<Utc>,
    settings: &ReminderSettings,
    subjects: &[Subject],
) -> Vec<PlannedReminder> {
    if settings.channels.is_empty() {
        return Vec::new();
    }

    let horizon = now + Duration::days(MAX_LOOKAHEAD_DAYS);
    let mut planned: Vec<PlannedReminder> = Vec::new();

    for subject in subjects {
        // The day's card reminder is planned from the count below, not from a subject per
        // card: one reminder per local day is the whole rule, and a per-card subject would
        // make the plan's size the deck's size.
        if subject.done || subject.kind == ReminderKind::CardsDue {
            continue;
        }
        let (Some(at), Some(lead_min)) = (subject.at, subject.kind.lead_min()) else {
            continue;
        };
        let fire_at = at - Duration::minutes(lead_min);
        if fire_at > horizon {
            continue;
        }

        let subject_key = subject.id.to_string();
        let local_date = local_date_of(fire_at, settings.tz);
        let mut reminder = PlannedReminder {
            kind: subject.kind,
            dedupe_key: dedupe_key(subject.kind, &subject_key, local_date),
            subject_key,
            subject_id: Some(subject.id),
            local_date,
            fire_at,
            deferred: false,
            skip: None,
        };

        if at <= now {
            reminder.skip = Some(SkipReason::AlreadyPast);
        } else {
            let deferral = defer_for_quiet_hours(fire_at, at, settings.tz, settings.quiet);
            reminder.fire_at = deferral.fire_at;
            reminder.deferred = deferral.deferred;
            reminder.skip = deferral.skip;
        }
        planned.push(reminder);
    }

    if let Some(reminder) = cards_due_reminder(now, settings) {
        planned.push(reminder);
    }

    planned.sort_by(|left, right| {
        left.fire_at
            .cmp(&right.fire_at)
            .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
            .then_with(|| left.subject_key.cmp(&right.subject_key))
    });

    // Deduped by key, keeping the first of the sorted order — which is the earliest fire
    // time for that key. A set rather than a scan so this stays linear in the plan's size.
    let mut seen = BTreeSet::new();
    planned.retain(|reminder| seen.insert(reminder.dedupe_key.clone()));
    planned
}

/// The quiet-hours rule for one reminder: where it moves to, whether it moved, and whether
/// the move makes it worthless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deferral {
    pub fire_at: DateTime<Utc>,
    /// True when quiet hours moved it.
    pub deferred: bool,
    pub skip: Option<SkipReason>,
}

/// Moves a fire time out of the quiet window, or says the move makes it worthless.
///
/// Public because there are two callers and they must not disagree: [`plan`] derives a
/// reminder and `POST /reminders` accepts one a client computed, and both have to apply the
/// same rule to the same instant. A second implementation in the server would be a second
/// answer to "where does this land", and the two would diverge the first time one was edited
/// — which is the failure mode the whole module exists to avoid, one layer up.
///
/// `subject_at` is the instant the reminder is *about* — the task's due time, the event's
/// start, or the start of the day a card reminder belongs to — and it is what
/// [`STALE_AFTER_MIN`] is measured against.
pub fn defer_for_quiet_hours(
    fire_at: DateTime<Utc>,
    subject_at: DateTime<Utc>,
    tz: Tz,
    quiet: Option<(u8, u8)>,
) -> Deferral {
    let Some(quiet) = quiet else {
        return Deferral {
            fire_at,
            deferred: false,
            skip: None,
        };
    };
    if !in_quiet_hours(local_hour_of(fire_at, tz), Some(quiet)) {
        return Deferral {
            fire_at,
            deferred: false,
            skip: None,
        };
    }
    let Some(end) = quiet_end(fire_at, tz, quiet) else {
        return Deferral {
            fire_at,
            deferred: false,
            skip: None,
        };
    };

    Deferral {
        fire_at: end,
        deferred: true,
        skip: (end > subject_at + Duration::minutes(STALE_AFTER_MIN))
            .then_some(SkipReason::QuietHours),
    }
}

/// The day's card reminder: one per local day, at the student's study start hour.
///
/// `settings.cards_due` is the count the student's own deck reports; a `CardsDue` subject in
/// the list says the same thing in another shape, and either is enough. Either way this
/// produces at most one reminder, keyed by the local date.
fn cards_due_reminder(now: DateTime<Utc>, settings: &ReminderSettings) -> Option<PlannedReminder> {
    if settings.cards_due <= 0 {
        return None;
    }

    let day = local_date_of(now, settings.tz);
    let hour = u32::from(settings.study_start_hour.min(23));
    let naive = day.and_hms_opt(hour, 0, 0)?;
    let (fire_at, existed) = resolve_local(settings.tz, naive);

    let subject_key = day.to_string();
    let mut reminder = PlannedReminder {
        kind: ReminderKind::CardsDue,
        dedupe_key: dedupe_key(ReminderKind::CardsDue, &subject_key, day),
        subject_key,
        subject_id: None,
        local_date: day,
        fire_at,
        deferred: false,
        skip: if existed {
            None
        } else {
            Some(SkipReason::NonexistentLocalTime)
        },
    };

    if existed && fire_at <= now + Duration::days(MAX_LOOKAHEAD_DAYS) {
        // The subject of a day-granular reminder is the day itself, so staleness is measured
        // from the day's start: a reminder for today that quiet hours push into tomorrow
        // afternoon is not today's reminder any more.
        let day_start = day
            .and_hms_opt(0, 0, 0)
            .and_then(|midnight| local_instant(settings.tz, midnight))
            .unwrap_or(fire_at);
        let deferral = defer_for_quiet_hours(fire_at, day_start, settings.tz, settings.quiet);
        reminder.fire_at = deferral.fire_at;
        reminder.deferred = deferral.deferred;
        reminder.skip = deferral.skip;
    }

    Some(reminder)
}

/// Wrap-midnight membership, the same semantics as Phase 9's A202.
///
/// `(22, 7)` is 22:00→07:00: every hour from 22 through 6 is inside, and the naive
/// `start <= hour && hour < end` is false for all of them. `start == end` is an empty window
/// rather than an always-on one — the settings screen's "off" — because a window that never
/// ends is not a window a student can be inside of.
pub fn in_quiet_hours(local_hour: u8, quiet: Option<(u8, u8)>) -> bool {
    let Some((start, end)) = quiet else {
        return false;
    };
    if start == end {
        return false;
    }
    if start < end {
        start <= local_hour && local_hour < end
    } else {
        local_hour >= start || local_hour < end
    }
}

/// The instant a quiet window ends, from inside it.
///
/// `None` when `from` is not inside the window: there is nothing to end. The walk steps one
/// local hour at a time, which is what makes it correct on both sides of a transition — a
/// spring-forward hour that does not exist is stepped over, and a fall-back hour that happens
/// twice stops at the first occurrence at or after `from`.
pub fn quiet_end(from: DateTime<Utc>, tz: Tz, quiet: (u8, u8)) -> Option<DateTime<Utc>> {
    let local = from.with_timezone(&tz);
    if !in_quiet_hours(local.hour() as u8, Some(quiet)) {
        return None;
    }

    let mut naive = local
        .naive_local()
        .with_minute(0)?
        .with_second(0)?
        .with_nanosecond(0)?;

    for _ in 0..MAX_WALK_HOURS {
        naive += Duration::hours(1);
        let Some(instant) = local_instant(tz, naive) else {
            // The hour does not exist on this date; the walk steps over it.
            continue;
        };
        if instant > from && !in_quiet_hours(naive.hour() as u8, Some(quiet)) {
            return Some(instant);
        }
    }
    None
}

/// The local date an instant falls on, in the user's zone.
fn local_date_of(at: DateTime<Utc>, tz: Tz) -> NaiveDate {
    at.with_timezone(&tz).date_naive()
}

/// The local hour an instant falls in, in the user's zone.
fn local_hour_of(at: DateTime<Utc>, tz: Tz) -> u8 {
    at.with_timezone(&tz).hour() as u8
}

/// The instant a local wall-clock time resolves to, or `None` when that time does not exist.
///
/// An ambiguous time (a fall-back hour that happens twice) resolves to the **earlier** offset,
/// which is the first moment the student's clock showed that hour.
fn local_instant(tz: Tz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|instant| instant.with_timezone(&Utc))
}

/// The instant a local wall-clock time resolves to, and whether that time existed at all.
///
/// A spring-forward hour that never happens walks forward to the first hour that does, so the
/// field still holds a real instant while `skip` says why it is not the requested one.
fn resolve_local(tz: Tz, naive: NaiveDateTime) -> (DateTime<Utc>, bool) {
    if let Some(instant) = local_instant(tz, naive) {
        return (instant, true);
    }
    let mut probe = naive;
    for _ in 0..MAX_WALK_HOURS {
        probe += Duration::hours(1);
        if let Some(instant) = local_instant(tz, probe) {
            return (instant, false);
        }
    }
    // Unreachable for a real zone: a gap is at most a couple of hours wide. Interpreting the
    // wall clock as UTC keeps this total rather than panicking in a scheduling path.
    (naive.and_utc(), false)
}
