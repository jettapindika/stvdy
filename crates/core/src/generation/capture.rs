//! The quick-capture draft contract (`prompts/quick_capture.v1.md`), parsed and normalised.
//!
//! This is the *proposal* half of quick capture: a model reply becomes a draft a student
//! confirms or edits, and never a row (A193). Everything that can go wrong in the reply is
//! resolved here rather than at the insert, because the credits are spent by the time this
//! runs: an unreadable date is a visible warning and the draft still comes back (A194),
//! rather than a request that charged the student and failed.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::generation::{GenerationError, strip_code_fence};

/// The priority range `tasks.priority` accepts. A value outside it would fail the insert,
/// so the draft never carries one.
pub const PRIORITY_MIN: i32 = 1;
pub const PRIORITY_MAX: i32 = 3;

/// What an out-of-range priority becomes. The documented default, not the nearest boundary:
/// a `7` says the model ignored the scale, and clamping it to `3` would record "low" for a
/// value that stated nothing about urgency.
pub const DEFAULT_PRIORITY: i32 = 2;

/// The longest estimate a task may carry, in minutes — `tasks.estimate_min`'s bound.
pub const MAX_ESTIMATE_MIN: i32 = 1440;

/// The model's reply to `quick_capture.v1`, before any interpretation.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that invents a
/// field produces a parse error and a retry rather than a draft with a silently dropped one.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftFile {
    pub title: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default)]
    pub estimate_min: Option<i32>,
}

fn default_priority() -> i32 {
    DEFAULT_PRIORITY
}

/// The draft the route returns: a proposal, never a row (A193).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CaptureDraft {
    pub title: String,
    pub notes: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub estimate_min: Option<i32>,
    /// One line per field the reply could not supply as asked (A194). Returned beside the
    /// draft rather than logged, because the student is the one who can fix it.
    pub warnings: Vec<String>,
}

/// Parses a capture reply into a draft.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite the
/// contract. It does **not** tolerate prose around the object: guessing which part is the
/// JSON is how a half-parsed draft ships.
///
/// `tz` is the student's zone, used only to *read* the reply's offset and to interpret a
/// date the model left naive — never to invent one.
pub fn parse_draft(raw: &str, tz: Tz) -> Result<CaptureDraft, GenerationError> {
    let text = strip_code_fence(raw);
    let file: DraftFile = serde_json::from_str(text).map_err(|error| {
        // `serde_json` reports both "not JSON" and "wrong shape" through the same error
        // type; the message distinguishes them, and the caller only needs to know a retry
        // is worth one attempt.
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    let title = file.title.trim().to_string();
    if title.is_empty() {
        // A task with no title is not a task. Schema, so the caller's existing retry runs
        // rather than the student being handed an unusable draft.
        return Err(GenerationError::Schema(
            "the draft has no title".to_string(),
        ));
    }

    let mut warnings = Vec::new();

    let due_at = match file.due_at.as_deref() {
        Some(raw_due) => parse_due_at(raw_due, tz, &mut warnings),
        None => None,
    };

    let priority = if (PRIORITY_MIN..=PRIORITY_MAX).contains(&file.priority) {
        file.priority
    } else {
        warnings.push(format!(
            "priority: {} is outside {PRIORITY_MIN}-{PRIORITY_MAX}; using the default \
             {DEFAULT_PRIORITY}",
            file.priority
        ));
        DEFAULT_PRIORITY
    };

    let estimate_min = match file.estimate_min {
        None => None,
        Some(minutes) if minutes < 1 => {
            // An estimate of zero minutes is not an estimate, and a negative one is a model
            // error. Either way "no estimate" is the honest reading; storing a zero would
            // show the student a task that takes no time.
            warnings.push(format!(
                "estimate_min: {minutes} is not a positive number of minutes; ignored"
            ));
            None
        }
        Some(minutes) if minutes > MAX_ESTIMATE_MIN => {
            warnings.push(format!(
                "estimate_min: {minutes} is above the {MAX_ESTIMATE_MIN}-minute maximum; \
                 clamped"
            ));
            Some(MAX_ESTIMATE_MIN)
        }
        Some(minutes) => Some(minutes),
    };

    // A blank note is not a note: it would store an empty string where the schema has a
    // null, and every capture would render an empty notes section.
    let notes = file
        .notes
        .map(|notes| notes.trim().to_string())
        .filter(|notes| !notes.is_empty());

    Ok(CaptureDraft {
        title,
        notes,
        due_at,
        priority,
        estimate_min,
        warnings,
    })
}

/// The three shapes the model actually produces, in the order that avoids a silent
/// reinterpretation: an offset-aware datetime is taken as written, and a naive one is read
/// in the student's zone and said out loud.
///
/// A value that is none of the three is a warning naming the raw string (A194), not a
/// failure: the request has already been paid for.
fn parse_due_at(raw: &str, tz: Tz, warnings: &mut Vec<String>) -> Option<DateTime<Utc>> {
    let text = raw.trim();
    if text.is_empty() {
        // A model that writes `""` where the schema says `null` means "no date". That is a
        // state the draft models, not a value it lost, so it is not worth a warning.
        return None;
    }

    if let Ok(fixed) = DateTime::parse_from_rfc3339(text) {
        return Some(fixed.with_timezone(&Utc));
    }

    if let Ok(naive) = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
    {
        return resolve_naive(naive, text, tz, warnings);
    }

    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d")
        && let Some(naive) = date.and_hms_opt(0, 0, 0)
    {
        return resolve_naive(naive, text, tz, warnings);
    }

    warnings.push(format!("due_at: could not read \"{text}\""));
    None
}

/// Reads a naive local datetime in the student's zone.
///
/// A local time that does not exist in that zone (a spring-forward gap) is not guessed at:
/// the value is dropped with a warning, because the alternative is picking an instant the
/// student did not name.
fn resolve_naive(
    naive: NaiveDateTime,
    raw: &str,
    tz: Tz,
    warnings: &mut Vec<String>,
) -> Option<DateTime<Utc>> {
    match tz.from_local_datetime(&naive).earliest() {
        Some(local) => {
            warnings.push(format!("due_at: \"{raw}\" has no timezone; read as {tz}"));
            Some(local.with_timezone(&Utc))
        }
        None => {
            warnings.push(format!(
                "due_at: could not read \"{raw}\": that local time does not exist in {tz}"
            ));
            None
        }
    }
}
