//! Small helpers that more than one module needs.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use stvdy_core::traits::clock::Clock;

/// Midnight at the start of the user's current local day, as UTC.
///
/// Local-date maths goes through `chrono-tz` and never through a fixed offset
/// (A6): `Asia/Jakarta` has no DST, but the column accepts any IANA zone, and a
/// fixed offset would silently be an hour or a day out for a user who moves.
/// An unparseable zone falls back to UTC rather than failing the request, and the
/// caller keeps the value the user set.
pub fn local_day_start(timezone: &str, clock: &dyn Clock) -> DateTime<Utc> {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let now_local = clock.now().with_timezone(&tz);

    use chrono::TimeZone;
    let midnight = now_local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        // Unreachable for a real zone at midnight, but a fallback keeps this
        // total rather than panicking in a request path.
        .unwrap_or(now_local);

    midnight.with_timezone(&Utc)
}

/// Midnight on the first of the user's current local month, as UTC.
///
/// The monthly counters (`flashcards_per_month`, and Phase 7's others) reset on a
/// local calendar boundary, never a rolling 30-day window: "30 cards a month" means
/// the month the student is in, and a rolling window would let a student who waits
/// two days get 60. `chrono-tz`, never a fixed offset (A6).
pub fn local_month_start(timezone: &str, clock: &dyn Clock) -> DateTime<Utc> {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let now_local = clock.now().with_timezone(&tz);

    use chrono::{Datelike, TimeZone};
    let first = now_local
        .date_naive()
        .with_day(1)
        .unwrap_or(now_local.date_naive())
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        .unwrap_or(now_local);

    first.with_timezone(&Utc)
}

/// Midnight on the first of the user's **next** local month, as UTC.
///
/// The reset instant the ceiling's refusal names, and the `run_at` the scheduler gives
/// the next period's grant. Same path as its two neighbours: `chrono-tz` and the offset
/// in force *then*, never the offset in force now (A6) — a month boundary that crosses a
/// DST change is exactly where a fixed offset is an hour out.
pub fn next_local_month_start(timezone: &str, clock: &dyn Clock) -> DateTime<Utc> {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let now_local = clock.now().with_timezone(&tz);

    use chrono::{Datelike, Months, TimeZone};
    let first = now_local
        .date_naive()
        .with_day(1)
        .and_then(|day| day.checked_add_months(Months::new(1)))
        .unwrap_or(now_local.date_naive())
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        // Unreachable for a real zone at midnight, but a fallback keeps this total
        // rather than panicking in a request path.
        .unwrap_or(now_local);

    first.with_timezone(&Utc)
}

/// The user's local calendar month, as `YYYY-MM`.
///
/// The one place the month *string* is produced, because two things have to agree on it:
/// the ledger's `period_key` (which is what makes a grant once-per-period) and the
/// scheduler's `dedupe_key` (which is what makes it once-per-schedule). Formatting the
/// instant in UTC instead would name the previous month for every zone east of Greenwich,
/// which is where most of this product's users are.
pub fn local_month_key(timezone: &str, at: DateTime<Utc>) -> String {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    at.with_timezone(&tz).format("%Y-%m").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stvdy_core::traits::testing::FixedClock;

    #[test]
    fn the_local_day_starts_at_local_midnight() {
        // 2026-10-11T02:00Z is 09:00 in Jakarta (UTC+7), so the local day began
        // at 2026-10-10T17:00Z.
        let clock = FixedClock::at("2026-10-11T02:00:00Z".parse().expect("a valid timestamp"));
        let start = local_day_start("Asia/Jakarta", &clock);
        assert_eq!(start.to_rfc3339(), "2026-10-10T17:00:00+00:00");
    }

    #[test]
    fn a_dst_transition_day_still_resolves() {
        // Europe/Berlin springs forward on 2026-03-29; local midnight exists and
        // is 23:00Z the day before.
        let clock = FixedClock::at("2026-03-29T12:00:00Z".parse().expect("a valid timestamp"));
        let start = local_day_start("Europe/Berlin", &clock);
        assert_eq!(start.to_rfc3339(), "2026-03-28T23:00:00+00:00");
    }

    #[test]
    fn an_unknown_zone_falls_back_to_utc_instead_of_failing() {
        let clock = FixedClock::at("2026-10-11T02:00:00Z".parse().expect("a valid timestamp"));
        assert_eq!(
            local_day_start("Not/AZone", &clock).to_rfc3339(),
            "2026-10-11T00:00:00+00:00"
        );
    }

    #[test]
    fn the_local_month_starts_at_local_midnight_on_the_first() {
        // 2026-10-11T02:00Z is 09:00 in Jakarta (UTC+7) on the 11th, so the month
        // began at 2026-10-01T00:00+07:00, which is 2026-09-30T17:00Z.
        let clock = FixedClock::at("2026-10-11T02:00:00Z".parse().expect("a valid timestamp"));
        let start = local_month_start("Asia/Jakarta", &clock);
        assert_eq!(start.to_rfc3339(), "2026-09-30T17:00:00+00:00");
    }

    #[test]
    fn a_month_start_across_a_dst_change_uses_the_offset_in_force_then() {
        // Europe/Berlin springs forward on 2026-03-29, but the month began on
        // 2026-03-01, still CET (UTC+1): 2026-02-28T23:00Z. Using the offset in
        // force *now* would give 22:00Z and shift the boundary by an hour.
        let clock = FixedClock::at("2026-03-29T12:00:00Z".parse().expect("a valid timestamp"));
        let start = local_month_start("Europe/Berlin", &clock);
        assert_eq!(start.to_rfc3339(), "2026-02-28T23:00:00+00:00");
    }

    #[test]
    fn the_next_local_month_starts_at_local_midnight_on_the_first() {
        // 2026-10-11T02:00Z is 09:00 in Jakarta (UTC+7) on the 11th, so the next
        // month begins at 2026-11-01T00:00+07:00, which is 2026-10-31T17:00Z.
        let clock = FixedClock::at("2026-10-11T02:00:00Z".parse().expect("a valid timestamp"));
        let start = next_local_month_start("Asia/Jakarta", &clock);
        assert_eq!(start.to_rfc3339(), "2026-10-31T17:00:00+00:00");
    }

    #[test]
    fn the_next_month_start_uses_the_offset_in_force_at_that_boundary() {
        // Berlin enters CEST (UTC+2) on 2026-03-29, so the next month after a March
        // clock begins at 2026-04-01T00:00+02:00 = 2026-03-31T22:00Z. Using the offset
        // in force now (CET, +1) would give 23:00Z.
        let clock = FixedClock::at("2026-03-29T12:00:00Z".parse().expect("a valid timestamp"));
        let start = next_local_month_start("Europe/Berlin", &clock);
        assert_eq!(start.to_rfc3339(), "2026-03-31T22:00:00+00:00");
    }

    #[test]
    fn the_next_month_rolls_the_year_over() {
        // 2026-12-20T02:00Z is 2026-12-20T09:00 in Jakarta, so the next boundary is
        // 2027-01-01T00:00+07:00 = 2026-12-31T17:00Z.
        let clock = FixedClock::at("2026-12-20T02:00:00Z".parse().expect("a valid timestamp"));
        let start = next_local_month_start("Asia/Jakarta", &clock);
        assert_eq!(start.to_rfc3339(), "2026-12-31T17:00:00+00:00");
    }

    #[test]
    fn the_month_key_is_the_users_own_month_not_utcs() {
        // 2026-10-31T18:00Z is already November in Jakarta (UTC+7) and still October in
        // UTC. Formatting the instant in UTC would stamp the ledger with the wrong month,
        // and the grant's `period_key` is what makes it once-per-period.
        let november = "2026-10-31T18:00:00Z".parse().expect("a valid timestamp");
        assert_eq!(local_month_key("Asia/Jakarta", november), "2026-11");
        assert_eq!(local_month_key("UTC", november), "2026-10");

        let october = "2026-10-31T16:00:00Z".parse().expect("a valid timestamp");
        assert_eq!(local_month_key("Asia/Jakarta", october), "2026-10");
    }
}
