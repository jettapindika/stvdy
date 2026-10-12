//! Steps 1 and 2 of contract § 6: the horizon, the study windows, the busy subtraction and
//! the block chunking.
//!
//! Pure: no clock, no IO. Every local time is resolved through `chrono_tz`, never a fixed
//! offset (A6), and the resolution of an ambiguous or non-existent local time lives in
//! exactly one function ([`local_at`]) because it is the rule that is wrong everywhere it is
//! written twice.

use chrono::{DateTime, Duration, NaiveDate, TimeZone as _, Utc};

use super::model::{Interval, PlannerInput};

/// The hard bound on the horizon, in days (A255).
///
/// The pure planner cannot read `config/plans.toml` (A256) and contract § 6 fixes
/// `PlannerConfig` at six fields (A259), so the bound the server loads as
/// `[planner].max_horizon_days` is mirrored here as a constant rather than passed in. A goal
/// dated five years out would otherwise make the day loop unbounded, which the roadmap's risk
/// register calls "a real server hang".
pub const MAX_HORIZON_DAYS: i64 = 120;

/// Step 1. `None` when there is no goal to plan for, or every goal is in the past.
///
/// The end is `max(target_date).local_date() + 1 day`, so the deadline day is inside the
/// half-open horizon `[today, end)` and a block can be placed on the day of the exam (A267).
/// It is additionally clipped to `[today, today + MAX_HORIZON_DAYS]`.
pub(crate) fn horizon(input: &PlannerInput) -> Option<(NaiveDate, NaiveDate)> {
    let today = input.now.with_timezone(&input.tz).date_naive();
    let max_target = input.goals.iter().map(|goal| goal.target_date).max()?;
    let end = max_target.with_timezone(&input.tz).date_naive() + Duration::days(1);
    let end = end.min(today + Duration::days(MAX_HORIZON_DAYS));
    (end > today).then_some((today, end))
}

/// The one place a local wall-clock time becomes an instant.
///
/// `.earliest()` is the whole DST rule: a non-existent local time (the spring-forward gap)
/// has no instant and is skipped; an ambiguous one (the autumn hour that happens twice)
/// takes the earlier offset, so the hour is placed once, in its first pass, and the second
/// pass is not double-counted. The alternative — a fixed offset — is wrong for every zone
/// east or west of Greenwich, which is every user.
pub(crate) fn local_at(
    day: NaiveDate,
    hour: u32,
    minute: u32,
    tz: chrono_tz::Tz,
) -> Option<DateTime<Utc>> {
    let naive = day.and_hms_opt(hour, minute, 0)?;
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|local| local.with_timezone(&Utc))
}

/// The first instant at or after `hour:00` that exists in `tz` on `day`.
///
/// On a spring-forward day `start_hour:00` can fall in the missing hour; the window then
/// begins at the first real instant after the gap rather than the whole day being dropped.
fn window_start(day: NaiveDate, hour: u32, tz: chrono_tz::Tz) -> Option<DateTime<Utc>> {
    for step in 0..=(3 * 60) {
        let total = hour * 60 + step;
        if total >= 24 * 60 {
            break;
        }
        if let Some(at) = local_at(day, total / 60, total % 60, tz) {
            return Some(at);
        }
    }
    None
}

/// The exclusive end of the window at `hour:00`, or the instant the gap starts when that
/// local time does not exist.
///
/// `study_end_hour` is `between 1 and 24` in the schema, so `24` means midnight of the
/// following local day and is handled before the gap search — the gap search would otherwise
/// stop at the last minute of the current day and silently drop an hour.
fn window_end(day: NaiveDate, hour: u32, tz: chrono_tz::Tz) -> Option<DateTime<Utc>> {
    if hour >= 24 {
        return local_at(day + Duration::days(1), hour - 24, 0, tz);
    }
    let total = hour * 60;
    for step in 0..=(3 * 60) {
        if step > total {
            break;
        }
        if let Some(at) = local_at(day, (total - step) / 60, (total - step) % 60, tz) {
            // The gap starts one minute after the last local minute that exists.
            return Some(if step == 0 {
                at
            } else {
                at + Duration::minutes(1)
            });
        }
    }
    None
}

/// The day's usable window as one interval, or `None` when the whole window falls in a gap.
pub(crate) fn window_for(input: &PlannerInput, day: NaiveDate) -> Option<Interval> {
    let start = window_start(day, input.settings.start_hour, input.tz)?;
    let end = window_end(day, input.settings.end_hour, input.tz)?;
    (end > start).then_some(Interval {
        starts_at: start,
        ends_at: end,
    })
}

/// Step 2. Merges overlapping and adjacent intervals first, then subtracts `busy ∪ pinned`,
/// then clips the day to `max_study_min_per_day`, then chunks into `block_len_min` blocks
/// separated by `break_min`. Returns the day's capacity in minutes alongside.
///
/// The capacity is the free window's minutes, not the chunked minutes: a window whose
/// leftover is shorter than one block still contains that time, and contract § 6's deficit
/// counts what the day holds rather than what the chunker could tile.
pub(crate) fn free_slots(input: &PlannerInput, day: NaiveDate) -> (Vec<Interval>, i64) {
    let Some(mut window) = window_for(input, day) else {
        return (Vec::new(), 0);
    };

    // Today is a partial day: nothing may start before the next block boundary at or after
    // `now`, or a request at 14:07 would emit a 14:07 block.
    let today = input.now.with_timezone(&input.tz).date_naive();
    if day == today {
        let floor = ceil_to_block(input);
        if floor > window.starts_at {
            window.starts_at = floor.min(window.ends_at);
        }
    }
    if window.ends_at <= window.starts_at {
        return (Vec::new(), 0);
    }

    let mut blocked: Vec<Interval> = input
        .busy
        .iter()
        .chain(input.pinned.iter())
        .copied()
        .collect();
    blocked = merge(&blocked);
    let free = subtract(window, &blocked);

    // Clip the day's usable time to `max_study_min_per_day`.
    let cap = input.settings.max_study_min_per_day;
    let mut kept: Vec<Interval> = Vec::new();
    let mut total = 0i64;
    for interval in free {
        if total >= cap {
            break;
        }
        let length = minutes(interval);
        if total + length <= cap {
            kept.push(interval);
            total += length;
        } else {
            let take = cap - total;
            kept.push(Interval {
                starts_at: interval.starts_at,
                ends_at: interval.starts_at + Duration::minutes(take),
            });
            total = cap;
        }
    }

    let mut slots: Vec<Interval> = Vec::new();
    for interval in kept {
        slots.extend(chunk(
            interval,
            input.config.block_len_min,
            input.config.break_min,
        ));
    }
    (slots, total)
}

/// Merges overlapping and adjacent intervals. Order-independent, so two runs that feed the
/// same busy time in different orders get the same merged list.
pub(crate) fn merge(intervals: &[Interval]) -> Vec<Interval> {
    let mut sorted: Vec<Interval> = intervals
        .iter()
        .copied()
        .filter(|interval| interval.ends_at > interval.starts_at)
        .collect();
    sorted.sort_by_key(|interval| (interval.starts_at, interval.ends_at));
    let mut out: Vec<Interval> = Vec::new();
    for interval in sorted {
        match out.last_mut() {
            Some(last) if interval.starts_at <= last.ends_at => {
                if interval.ends_at > last.ends_at {
                    last.ends_at = interval.ends_at;
                }
            }
            _ => out.push(interval),
        }
    }
    out
}

/// `window` minus `blocked`. `blocked` must be merged and sorted.
pub(crate) fn subtract(window: Interval, blocked: &[Interval]) -> Vec<Interval> {
    let mut out: Vec<Interval> = Vec::new();
    let mut cursor = window.starts_at;
    for block in blocked {
        if block.ends_at <= cursor {
            continue;
        }
        if block.starts_at >= window.ends_at {
            break;
        }
        if block.starts_at > cursor {
            out.push(Interval {
                starts_at: cursor,
                ends_at: block.starts_at.min(window.ends_at),
            });
        }
        if block.ends_at > cursor {
            cursor = block.ends_at;
        }
        if cursor >= window.ends_at {
            break;
        }
    }
    if cursor < window.ends_at {
        out.push(Interval {
            starts_at: cursor,
            ends_at: window.ends_at,
        });
    }
    out
}

/// Splits one free window into `block_len_min` blocks separated by `break_min`. The last
/// block of a window may be shorter than `block_len_min`; the leftover shorter than one
/// block is dropped.
pub(crate) fn chunk(window: Interval, block_len_min: i64, break_min: i64) -> Vec<Interval> {
    let mut out: Vec<Interval> = Vec::new();
    if block_len_min <= 0 {
        return out;
    }
    let period = block_len_min + break_min.max(0);
    let mut cursor = window.starts_at;
    while cursor < window.ends_at {
        let ends_at = (cursor + Duration::minutes(block_len_min)).min(window.ends_at);
        out.push(Interval {
            starts_at: cursor,
            ends_at,
        });
        cursor += Duration::minutes(period);
    }
    out
}

/// `now` rounded up to the next block boundary, measured from the day's window start.
///
/// Without it the first day's plan would start at the request instant — a block at 14:07,
/// which is not a block length and not a boundary a student can reason about.
pub(crate) fn ceil_to_block(input: &PlannerInput) -> DateTime<Utc> {
    let day = input.now.with_timezone(&input.tz).date_naive();
    let Some(start) = window_start(day, input.settings.start_hour, input.tz) else {
        return input.now;
    };
    if input.now <= start {
        return start;
    }
    let period = input.config.block_len_min.max(1) + input.config.break_min.max(0);
    let elapsed = (input.now - start).num_minutes();
    let steps = elapsed.div_euclid(period) + i64::from(elapsed.rem_euclid(period) != 0);
    start + Duration::minutes(steps * period)
}

fn minutes(interval: Interval) -> i64 {
    (interval.ends_at - interval.starts_at).num_minutes()
}
