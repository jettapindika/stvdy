//! Step 7 of contract § 6: the Power balancer.
//!
//! Pure, and bounded. An unbounded `while` over "is any day over its limit" is a hang rather
//! than a wrong answer, so the loop is capped and the count is returned for a test to assert
//! the bound was respected rather than merely not exceeded by luck (A268).

use chrono::{Duration, NaiveDate, Utc};

use super::model::{BlockKind, DayLoad, Interval, PlannerInput, ProposedBlock};
use super::slots;

/// The share of `max_study_min_per_day` above which a day counts as overloaded.
///
/// A constant rather than a `PlannerConfig` field, because contract § 6 fixes that struct's
/// six fields and a seventh is a contract change (A259).
pub const OVERLOAD_PCT: f64 = 0.9;

/// The termination bound (A268). The loop cannot spin: it either moves a block, which
/// strictly decreases the overloaded day's load, or it stops.
pub const MAX_ITERATIONS: usize = 100;

/// Step 7. Returns the iterations actually used.
///
/// A block is movable iff it is not pinned and is new material: a review block is reserved by
/// the review pass, which outranks the balancer (A268). After every move `day_load` is
/// recomputed from `blocks` rather than patched, so the two can never disagree.
pub fn rebalance(
    input: &PlannerInput,
    blocks: &mut [ProposedBlock],
    day_load: &mut [DayLoad],
) -> usize {
    let mut iterations = 0usize;
    while iterations < MAX_ITERATIONS {
        refresh_day_load(input, blocks, day_load);
        let Some(heavy) = heaviest_overloaded(input, day_load) else {
            break;
        };
        let heavy_day = day_load[heavy].date;
        let Some(block) = latest_movable(input, blocks, heavy_day) else {
            break;
        };
        let length = (blocks[block].ends_at - blocks[block].starts_at).num_minutes();
        let Some((_day, start)) = least_loaded_with_room(input, blocks, day_load, block, length)
        else {
            break;
        };
        blocks[block].starts_at = start;
        blocks[block].ends_at = start + Duration::minutes(length);
        iterations += 1;
    }
    refresh_day_load(input, blocks, day_load);
    iterations
}

/// The index of the day with the most planned minutes that is over `OVERLOAD_PCT` of the
/// daily maximum. Ties go to the earliest date.
fn heaviest_overloaded(input: &PlannerInput, day_load: &[DayLoad]) -> Option<usize> {
    let threshold = OVERLOAD_PCT * input.settings.max_study_min_per_day as f64;
    let mut best: Option<usize> = None;
    for (index, day) in day_load.iter().enumerate() {
        if day.planned_min as f64 <= threshold {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => {
                day.planned_min > day_load[current].planned_min
                    || (day.planned_min == day_load[current].planned_min
                        && day.date < day_load[current].date)
            }
        };
        if better {
            best = Some(index);
        }
    }
    best
}

/// The latest-starting movable block on `day`.
fn latest_movable(input: &PlannerInput, blocks: &[ProposedBlock], day: NaiveDate) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, block) in blocks.iter().enumerate() {
        if block.pinned || block.kind != BlockKind::NewMaterial {
            continue;
        }
        if block.starts_at.with_timezone(&input.tz).date_naive() != day {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => block.starts_at > blocks[current].starts_at,
        };
        if better {
            best = Some(index);
        }
    }
    best
}

/// The least-loaded day that has a free gap of `length` minutes, and where in it the block
/// would start. The block being moved is excluded from the occupancy so a day is not blocked
/// by the very block leaving it.
fn least_loaded_with_room(
    input: &PlannerInput,
    blocks: &[ProposedBlock],
    day_load: &[DayLoad],
    moving: usize,
    length: i64,
) -> Option<(NaiveDate, chrono::DateTime<Utc>)> {
    let mut order: Vec<usize> = (0..day_load.len()).collect();
    order.sort_by_key(|index| (day_load[*index].planned_min, day_load[*index].date));
    for index in order {
        let day = day_load[index].date;
        if let Some(start) = first_gap(input, blocks, moving, day, length) {
            return Some((day, start));
        }
    }
    None
}

/// The start of the first free gap of at least `length` minutes on `day`.
fn first_gap(
    input: &PlannerInput,
    blocks: &[ProposedBlock],
    moving: usize,
    day: NaiveDate,
    length: i64,
) -> Option<chrono::DateTime<Utc>> {
    let window = slots::window_for(input, day)?;
    let mut used: Vec<Interval> = input
        .busy
        .iter()
        .chain(input.pinned.iter())
        .copied()
        .collect();
    for (index, block) in blocks.iter().enumerate() {
        if index == moving {
            continue;
        }
        if block.starts_at.with_timezone(&input.tz).date_naive() == day {
            used.push(Interval {
                starts_at: block.starts_at,
                ends_at: block.ends_at,
            });
        }
    }
    let merged = slots::merge(&used);
    slots::subtract(window, &merged)
        .into_iter()
        .find(|interval| (interval.ends_at - interval.starts_at).num_minutes() >= length)
        .map(|interval| interval.starts_at)
}

fn refresh_day_load(input: &PlannerInput, blocks: &[ProposedBlock], day_load: &mut [DayLoad]) {
    let mut planned: std::collections::BTreeMap<NaiveDate, i64> = std::collections::BTreeMap::new();
    for block in blocks {
        *planned
            .entry(block.starts_at.with_timezone(&input.tz).date_naive())
            .or_insert(0) += (block.ends_at - block.starts_at).num_minutes();
    }
    for day in day_load.iter_mut() {
        day.planned_min = planned.remove(&day.date).unwrap_or(0);
    }
}
