//! The study-session bound (A204).
//!
//! A session is one of the six things a client may write offline, so its clock is the
//! device's. The cases here are the two ways that goes wrong: a bound compared against the
//! server's clock rejects a legitimate offline session from a device whose clock runs fast,
//! and a bound that trusts the client's duration lets a five-minute report claim ten hours —
//! which is a fabricated number the Today screen and the streak would then display as fact.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration, Utc};

use stvdy_core::study::{
    MAX_FUTURE_HOURS, MAX_SESSION_MIN, MIN_SESSION_MIN, SESSION_CLOCK_GRACE_MIN, SessionError,
    check_session,
};

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

/// The normal case: five minutes of focus, reported when it ended.
#[test]
fn a_session_within_its_elapsed_time_is_accepted() {
    assert!(
        check_session(
            at("2026-10-11T09:00:00Z"),
            5,
            at("2026-10-11T09:05:00Z"),
            at("2026-10-11T09:06:00Z")
        )
        .is_ok()
    );
}

/// The timer's own failure mode: a device that was offline for a day reports a session that
/// ended before it pushed. The comparison is against `client_ts`, so the push time is
/// irrelevant and the session is not lost.
#[test]
fn a_session_pushed_a_day_later_is_still_accepted() {
    assert!(
        check_session(
            at("2026-10-10T09:00:00Z"),
            50,
            at("2026-10-10T09:50:00Z"),
            at("2026-10-11T09:00:00Z")
        )
        .is_ok()
    );
}

/// A device whose clock runs two hours fast reports a start in the future and an end "before"
/// the server's now. Rejecting it would reject valid data, which is its own defect: the bound
/// reads the client's own clock (A204).
#[test]
fn a_device_whose_clock_runs_fast_does_not_lose_its_session() {
    assert!(
        check_session(
            at("2026-10-11T11:00:00Z"),
            30,
            at("2026-10-11T11:30:00Z"),
            at("2026-10-11T09:30:00Z")
        )
        .is_ok()
    );
}

/// The fabrication this bound exists for: 600 minutes claimed from a five-minute report. The
/// streak and the Today screen display these minutes as facts, so a number that cannot have
/// been studied must not be storable.
#[test]
fn a_duration_longer_than_the_client_reported_elapsed_time_is_refused() {
    assert!(matches!(
        check_session(
            at("2026-10-11T09:00:00Z"),
            600,
            at("2026-10-11T09:05:00Z"),
            at("2026-10-11T09:06:00Z")
        ),
        Err(SessionError::BeyondElapsed)
    ));
}

/// The grace is two minutes of clock skew, and it is a grace rather than a licence: a session
/// that ends one minute after the device's own report is accepted, and one that ends an hour
/// after it is not.
#[test]
fn the_clock_grace_is_exactly_two_minutes() {
    let start = at("2026-10-11T09:00:00Z");
    let client_ts = at("2026-10-11T09:05:00Z");
    let server_now = at("2026-10-11T09:06:00Z");

    assert!(
        check_session(
            start,
            5 + SESSION_CLOCK_GRACE_MIN as i32,
            client_ts,
            server_now
        )
        .is_ok(),
        "five minutes reported plus the two-minute grace"
    );
    assert!(matches!(
        check_session(
            start,
            5 + SESSION_CLOCK_GRACE_MIN as i32 + 1,
            client_ts,
            server_now
        ),
        Err(SessionError::BeyondElapsed)
    ));
    assert!(matches!(
        check_session(start, 60, client_ts, server_now),
        Err(SessionError::BeyondElapsed)
    ));
}

/// The boundaries: 1 and 1440 accepted, 0 and 1441 refused; 24 hours into the future accepted,
/// 25 refused — the same bound a review and an attempt already use.
#[test]
fn the_bounds_are_exact() {
    let start = at("2026-10-11T09:00:00Z");

    assert!(check_session(start, MIN_SESSION_MIN, start, start).is_ok());
    // A 1440-minute session needs 1440 minutes of elapsed client time to be inside the
    // elapsed bound; the duration bound is what this line is about.
    let day_later = start + Duration::minutes(i64::from(MAX_SESSION_MIN));
    assert!(check_session(start, MAX_SESSION_MIN, day_later, start).is_ok());
    assert!(matches!(
        check_session(start, 0, start, start),
        Err(SessionError::Duration)
    ));
    assert!(matches!(
        check_session(start, MAX_SESSION_MIN + 1, start, start),
        Err(SessionError::Duration)
    ));
    assert!(matches!(
        check_session(start, -1, start, start),
        Err(SessionError::Duration)
    ));

    let server_now = at("2026-10-11T09:00:00Z");
    let exactly_a_day_ahead = server_now + Duration::hours(MAX_FUTURE_HOURS);
    assert!(
        check_session(exactly_a_day_ahead, 1, exactly_a_day_ahead, server_now).is_ok(),
        "exactly {MAX_FUTURE_HOURS} hours ahead is inside the bound"
    );
    let too_far_ahead = server_now + Duration::hours(MAX_FUTURE_HOURS + 1);
    assert!(matches!(
        check_session(too_far_ahead, 1, too_far_ahead, server_now),
        Err(SessionError::StartsInFuture)
    ));
}

/// A duration is checked before the clock: a zero-minute session that also starts in the
/// future is a duration error, because that is the field the client can fix without
/// understanding the bound.
#[test]
fn the_duration_is_checked_before_the_clock() {
    let server_now = at("2026-10-11T09:00:00Z");
    let start = server_now + Duration::days(3);
    assert!(matches!(
        check_session(start, 0, start, server_now),
        Err(SessionError::Duration)
    ));
}

/// The message names the bound, because it is what a push rejection reports and what the
/// client shows the student.
#[test]
fn every_refusal_names_its_bound() {
    let start = at("2026-10-11T09:00:00Z");

    let duration = check_session(start, 0, start, start)
        .unwrap_err()
        .to_string();
    assert!(
        duration.contains(&MIN_SESSION_MIN.to_string())
            && duration.contains(&MAX_SESSION_MIN.to_string()),
        "{duration}"
    );

    let future = check_session(start + Duration::days(2), 1, start, start)
        .unwrap_err()
        .to_string();
    assert!(future.contains(&MAX_FUTURE_HOURS.to_string()), "{future}");

    let elapsed = check_session(start, 600, start, start)
        .unwrap_err()
        .to_string();
    assert!(
        elapsed.contains("device") && elapsed.contains("clock"),
        "the elapsed bound must say what it compared: {elapsed}"
    );
}
