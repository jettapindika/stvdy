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
}
