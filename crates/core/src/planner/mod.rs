//! The deterministic planner. Contract § 6 of the master roadmap, implemented.
//!
//! Pure: no database, no clock, no IO, no network. `now` is a field of [`PlannerInput`], not
//! a call, which is what makes the whole algorithm testable against a fixed instant. Same
//! input ⇒ byte-identical output (A266), which `tests/planner.rs` asserts rather than assumes
//! and `tests/planner_purity.rs` enforces at the source level.
//!
//! Steps 1 and 2 live in [`slots`]; demand, the review pass and allocation (3–6) in
//! [`allocate`]; the Power balancer (7) arrives in the next task.

mod allocate;
mod model;
mod slots;

pub use model::*;

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate};
use uuid::Uuid;

/// The one entry point. The steps run in the contract's fixed order and no other.
pub fn propose(input: &PlannerInput) -> PlanProposal {
    let past = past_deadline_goals(input);
    let Some((start, end)) = slots::horizon(input) else {
        return PlanProposal {
            blocks: Vec::new(),
            day_load: Vec::new(),
            deficit_min: 0,
            warnings: past
                .into_iter()
                .map(|goal_id| PlanWarning::PastDeadline { goal_id })
                .collect(),
        };
    };

    let mut all_slots: Vec<allocate::Slot> = Vec::new();
    let mut capacities: Vec<(NaiveDate, i64)> = Vec::new();
    let mut day = start;
    while day < end {
        let (chunks, capacity) = slots::free_slots(input, day);
        for chunk in chunks {
            let length = (chunk.ends_at - chunk.starts_at).num_minutes();
            all_slots.push(allocate::Slot {
                interval: chunk,
                remaining_min: length,
            });
        }
        capacities.push((day, capacity));
        day += Duration::days(1);
    }
    let total_capacity: i64 = capacities.iter().map(|(_, capacity)| *capacity).sum();

    let mut blocks = allocate::reserve_reviews(input, &mut all_slots);
    let (new_blocks, deficit, overloaded) =
        allocate::allocate(input, &mut all_slots, total_capacity);
    blocks.extend(new_blocks);

    let day_load = compute_day_load(input, &capacities, &blocks);

    // A266: a fixed serialization order, so two identical requests produce byte-identical
    // JSON. Blocks by `(starts_at, goal_id, topic)`.
    blocks.sort_by(|a, b| {
        (a.starts_at, a.goal_id, &a.topic).cmp(&(b.starts_at, b.goal_id, &b.topic))
    });

    let mut warnings: Vec<PlanWarning> = overloaded;
    for goal_id in past {
        warnings.push(PlanWarning::PastDeadline { goal_id });
    }

    PlanProposal {
        blocks,
        day_load,
        deficit_min: deficit,
        warnings,
    }
}

/// The goals whose deadline is at or before `now`. Reported rather than dropped, and they do
/// not stop the other goals (A262).
fn past_deadline_goals(input: &PlannerInput) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = input
        .goals
        .iter()
        .filter(|goal| goal.target_date <= input.now)
        .map(|goal| goal.id)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// `day_load` for every horizon day, by date ascending (A266). A block outside the horizon —
/// a pinned block from before it, say — gets its own entry with zero capacity rather than
/// being silently dropped from the load.
fn compute_day_load(
    input: &PlannerInput,
    capacities: &[(NaiveDate, i64)],
    blocks: &[ProposedBlock],
) -> Vec<DayLoad> {
    let mut planned: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for block in blocks {
        *planned
            .entry(block.starts_at.with_timezone(&input.tz).date_naive())
            .or_insert(0) += (block.ends_at - block.starts_at).num_minutes();
    }
    let mut out: Vec<DayLoad> = capacities
        .iter()
        .map(|(date, capacity)| DayLoad {
            date: *date,
            planned_min: planned.remove(date).unwrap_or(0),
            capacity_min: *capacity,
        })
        .collect();
    for (date, minutes) in planned {
        out.push(DayLoad {
            date,
            planned_min: minutes,
            capacity_min: 0,
        });
    }
    out.sort_by_key(|day| day.date);
    out
}
