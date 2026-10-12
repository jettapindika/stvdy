//! The study-session rules: the bound on a session a device reports (A204).
//!
//! A session is one of the six things a client may write offline, so its clock is the
//! device's, and the bound has to compare the client's report against the client's own clock.
//! Comparing against the server's instead would reject a legitimate offline session from a
//! device whose clock runs fast — and rejecting valid data is its own defect. `docs/THREAT_MODEL.md`
//! § 6 R1 names this fix; the Today screen and the streak display these minutes as facts, so
//! a number that cannot have been studied must not be storable.
//!
//! This is the same rule `crates/server/src/flashcards/routes.rs` and
//! `crates/server/src/quizzes/routes.rs` already implement for a review and an attempt, and
//! the future bound is the same 24 hours: a second number would be a second answer to "how
//! wrong may a device's clock be".

use chrono::{DateTime, Duration, Utc};

pub const MIN_SESSION_MIN: i32 = 1;
pub const MAX_SESSION_MIN: i32 = 1440;

/// The clock skew tolerated between the device's report and the server's receipt. Two
/// minutes: enough for a slow push, not enough to hide a duration that was never studied.
pub const SESSION_CLOCK_GRACE_MIN: i64 = 2;

/// How far into the future a start may be. The same bound a flashcard review's `reviewed_at`
/// and a quiz attempt's `answered_at` already use.
pub const MAX_FUTURE_HOURS: i64 = 24;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("a session must be between {MIN_SESSION_MIN} and {MAX_SESSION_MIN} minutes")]
    Duration,
    #[error("the session starts more than {MAX_FUTURE_HOURS} hours in the future")]
    StartsInFuture,
    #[error("the session is longer than the time between its start and the device's clock")]
    BeyondElapsed,
}

/// Checks one session against the bounds.
///
/// `client_ts` is the device's own report of when it wrote the row — the field the mutation
/// contract already carries, reused by `POST /sessions`'s body — and `server_now` is only
/// used for the future bound, which is the one check the client's clock cannot make.
///
/// The duration is checked first, because it is the field the client can fix without
/// understanding the bound, and the future bound before the elapsed bound, because a start
/// that has not happened has no elapsed time to compare.
pub fn check_session(
    started_at: DateTime<Utc>,
    duration_min: i32,
    client_ts: DateTime<Utc>,
    server_now: DateTime<Utc>,
) -> Result<(), SessionError> {
    if !(MIN_SESSION_MIN..=MAX_SESSION_MIN).contains(&duration_min) {
        return Err(SessionError::Duration);
    }

    if started_at > server_now + Duration::hours(MAX_FUTURE_HOURS) {
        return Err(SessionError::StartsInFuture);
    }

    let elapsed = client_ts - started_at;
    if Duration::minutes(i64::from(duration_min))
        > elapsed + Duration::minutes(SESSION_CLOCK_GRACE_MIN)
    {
        return Err(SessionError::BeyondElapsed);
    }

    Ok(())
}
