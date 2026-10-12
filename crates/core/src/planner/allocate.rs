//! Steps 3 to 6 of contract § 6: demand, the review pass, and the spread-capped allocation.
//!
//! Pure: no clock, no IO, and no order that depends on anything but the input. Every
//! iteration is over a `Vec` in a stated order; the weights are `f64` but are only ever
//! compared with a total order (`total_cmp`) and only ever summed in one canonical order, so
//! two construction paths that hold the same topics in different sequences agree byte for
//! byte.

use chrono::{DateTime, Duration, NaiveDate, Utc};

use super::model::{
    BlockKind, GoalInput, Interval, PlanWarning, PlannerInput, ProposedBlock, Topic,
};

/// A free chunk of time, and how much of it is still unclaimed. Not part of the public
/// surface: a caller that could see the leftover capacity could be tempted to schedule
/// against it.
pub(crate) struct Slot {
    pub interval: Interval,
    pub remaining_min: i64,
}

/// The topic label a review block carries (A269). The accept path writes a title that says
/// "Review" from it.
pub(crate) const REVIEW_TOPIC: &str = "Review";

/// Step 3. `D = Σ_topic remaining_min × weight(topic) + due_review_min`.
///
/// The contributions are summed in a canonical order so the `f64` sum does not depend on the
/// order the caller happened to build the goals in.
pub(crate) fn demand(input: &PlannerInput) -> i64 {
    let mut contributions: Vec<f64> = input
        .goals
        .iter()
        .flat_map(|goal| goal.topics.iter())
        .map(|topic| topic.remaining_min as f64 * weight(topic, input.config.spread_k))
        .collect();
    contributions.sort_by(f64::total_cmp);
    contributions.iter().sum::<f64>().round() as i64 + input.due_review_min
}

/// `1 + spread_k × (1 − accuracy)`, and `1` when `accuracy` is `None` (A258).
///
/// A topic with no attempts weighs as if it were accurate: the app must not invent a
/// weakness out of missing data.
pub(crate) fn weight(topic: &Topic, spread_k: f64) -> f64 {
    match topic.accuracy {
        Some(accuracy) => 1.0 + spread_k * (1.0 - accuracy),
        None => 1.0,
    }
}

/// Step 4. Reserves one review block on each of the last `revisit_days` local dates before
/// each goal's `target_date`, consuming the slots it reserves so new material cannot claim
/// them (A269).
pub(crate) fn reserve_reviews(input: &PlannerInput, slots: &mut [Slot]) -> Vec<ProposedBlock> {
    let revisit = input.config.revisit_days.max(0);
    let mut out: Vec<ProposedBlock> = Vec::new();
    if revisit == 0 {
        return out;
    }
    let mut goals: Vec<&GoalInput> = input
        .goals
        .iter()
        .filter(|goal| goal.target_date > input.now)
        .collect();
    goals.sort_by_key(|goal| (goal.target_date, goal.created_at, goal.id));
    for goal in goals {
        let target = goal.target_date.with_timezone(&input.tz).date_naive();
        for back in 1..=revisit {
            let day = target - Duration::days(back);
            let Some(slot) = slots.iter_mut().find(|slot| {
                slot.remaining_min > 0
                    && slot
                        .interval
                        .starts_at
                        .with_timezone(&input.tz)
                        .date_naive()
                        == day
            }) else {
                continue;
            };
            out.push(ProposedBlock {
                starts_at: slot.interval.starts_at,
                ends_at: slot.interval.starts_at + Duration::minutes(slot.remaining_min),
                goal_id: goal.id,
                topic: REVIEW_TOPIC.to_string(),
                kind: BlockKind::Review,
                pinned: false,
            });
            slot.remaining_min = 0;
        }
    }
    out
}

/// One topic's allocation state.
struct Item {
    goal: usize,
    name: String,
    weight: f64,
    /// The minutes this topic is allowed to receive over the horizon.
    target: f64,
    /// What is still unplaced.
    remaining: i64,
    /// The goal's target date in the planner's zone, for the per-day cap.
    target_local: NaiveDate,
    /// The position in the canonical order. The tie-break uses this rather than the caller's
    /// `topic` index: the index is the order the caller built the goal in, and a plan that
    /// depends on it is a plan that changes when the same topics arrive in a different order.
    rank: usize,
}

/// Steps 5 and 6.
///
/// Targets are proportional to `remaining × weight` when the horizon cannot hold the weighted
/// demand, and the full remaining demand when it can — that is what makes a weak topic get
/// strictly more minutes than a strong one with the same work left (A258). Placement then
/// walks the slots in time order and gives each one to the topic with the most unplaced
/// target, capped per topic per day at `max(block_len_min, ceil(target / days_left))` so the
/// work spreads instead of front-loading.
pub(crate) fn allocate(
    input: &PlannerInput,
    slots: &mut [Slot],
    total_capacity_min: i64,
) -> (Vec<ProposedBlock>, i64, Vec<PlanWarning>) {
    let mut items: Vec<Item> = Vec::new();
    for (goal, goal_input) in input.goals.iter().enumerate() {
        if goal_input.target_date <= input.now {
            continue;
        }
        let target_local = goal_input.target_date.with_timezone(&input.tz).date_naive();
        for topic_input in &goal_input.topics {
            if topic_input.remaining_min <= 0 {
                continue;
            }
            items.push(Item {
                goal,
                name: topic_input.name.clone(),
                weight: weight(topic_input, input.config.spread_k),
                target: topic_input.remaining_min as f64,
                remaining: topic_input.remaining_min,
                target_local,
                rank: 0,
            });
        }
    }
    // A canonical order, so the weighted sum and every tie-break are independent of the order
    // the caller built the goals in.
    items.sort_by(|a, b| {
        let left = &input.goals[a.goal];
        let right = &input.goals[b.goal];
        (left.target_date, left.created_at, left.id, &a.name).cmp(&(
            right.target_date,
            right.created_at,
            right.id,
            &b.name,
        ))
    });
    for (rank, item) in items.iter_mut().enumerate() {
        item.rank = rank;
    }

    let topic_capacity: i64 = slots.iter().map(|slot| slot.remaining_min).sum();
    let weighted_demand: f64 = items.iter().map(|item| item.target * item.weight).sum();
    if weighted_demand > topic_capacity as f64 && weighted_demand > 0.0 {
        for item in &mut items {
            item.target = item.target * item.weight / weighted_demand * topic_capacity as f64;
        }
    }
    for item in &mut items {
        item.remaining = item.target.round() as i64;
    }

    let mut used_today: Vec<i64> = vec![0; items.len()];
    let mut blocks: Vec<ProposedBlock> = Vec::new();
    let mut current_day: Option<NaiveDate> = None;

    for slot in slots.iter_mut() {
        if slot.remaining_min <= 0 {
            continue;
        }
        let slot_day = slot
            .interval
            .starts_at
            .with_timezone(&input.tz)
            .date_naive();
        if current_day != Some(slot_day) {
            current_day = Some(slot_day);
            used_today.fill(0);
        }
        let mut cursor = slot.interval.starts_at;
        while cursor < slot.interval.ends_at {
            let Some(index) = pick(input, &items, &used_today, cursor) else {
                break;
            };
            let goal = &input.goals[items[index].goal];
            let room = (slot.interval.ends_at - cursor).num_minutes();
            let cap_left =
                cap_for(&items[index], slot_day, input.config.block_len_min) - used_today[index];
            let take = input
                .config
                .block_len_min
                .min(room)
                .min(items[index].remaining)
                .min(cap_left);
            if take <= 0 {
                break;
            }
            blocks.push(ProposedBlock {
                starts_at: cursor,
                ends_at: cursor + Duration::minutes(take),
                goal_id: goal.id,
                topic: items[index].name.clone(),
                kind: BlockKind::NewMaterial,
                pinned: false,
            });
            items[index].remaining -= take;
            used_today[index] += take;
            cursor += Duration::minutes(take + input.config.break_min.max(0));
        }
        slot.remaining_min = 0;
    }

    let deficit = (demand(input) - total_capacity_min).max(0);
    let warnings = if deficit > 0 {
        let days = overloaded_days(input, slots, &blocks);
        if days.is_empty() {
            Vec::new()
        } else {
            vec![PlanWarning::Overloaded { days }]
        }
    } else {
        Vec::new()
    };
    (blocks, deficit, warnings)
}

/// The topic with the most unplaced target that may still take a block at `cursor`.
fn pick(
    input: &PlannerInput,
    items: &[Item],
    used_today: &[i64],
    cursor: DateTime<Utc>,
) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, item) in items.iter().enumerate() {
        if item.remaining <= 0 {
            continue;
        }
        let goal = &input.goals[item.goal];
        if cursor >= goal.target_date - Duration::minutes(input.config.deadline_buffer_min.max(0)) {
            continue;
        }
        if used_today[index]
            >= cap_for(
                item,
                cursor.with_timezone(&input.tz).date_naive(),
                input.config.block_len_min,
            )
        {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => compare(items, index, current),
        };
        if better {
            best = Some(index);
        }
    }
    best
}

/// Strict total order over two candidate topics: more unplaced target first, then higher
/// weight, then topic name. The canonical sort of `items` already breaks a name tie by goal,
/// so this is a total order.
fn compare(items: &[Item], a: usize, b: usize) -> bool {
    let left = &items[a];
    let right = &items[b];
    if left.remaining != right.remaining {
        return left.remaining > right.remaining;
    }
    match left.weight.total_cmp(&right.weight) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => left.rank < right.rank,
    }
}

/// The per-topic per-day cap: the average remaining work per day left, never below one block.
fn cap_for(item: &Item, day: NaiveDate, block_len_min: i64) -> i64 {
    let days_left = (item.target_local - day).num_days().max(1);
    let target = item.target.ceil() as i64;
    let average = target.div_euclid(days_left) + i64::from(target.rem_euclid(days_left) != 0);
    average.max(block_len_min)
}

/// The days whose planned minutes reached the daily maximum, reported only when there is a
/// deficit (contract step 6).
fn overloaded_days(
    input: &PlannerInput,
    slots: &[Slot],
    blocks: &[ProposedBlock],
) -> Vec<NaiveDate> {
    let mut planned: std::collections::BTreeMap<NaiveDate, i64> = std::collections::BTreeMap::new();
    for block in blocks {
        *planned
            .entry(block.starts_at.with_timezone(&input.tz).date_naive())
            .or_insert(0) += (block.ends_at - block.starts_at).num_minutes();
    }
    let capacity = input.settings.max_study_min_per_day;
    let mut days: Vec<NaiveDate> = Vec::new();
    for slot in slots {
        let day = slot
            .interval
            .starts_at
            .with_timezone(&input.tz)
            .date_naive();
        if !days.contains(&day)
            && capacity > 0
            && planned.get(&day).copied().unwrap_or(0) >= capacity
        {
            days.push(day);
        }
    }
    days.sort();
    days
}
