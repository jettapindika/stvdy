//! The deterministic planner — contract § 6's ten unit tests, plus the ones the DST
//! arithmetic and the Power balancer need.
//!
//! Every case here was first run against the defect it names and watched to fail. The two
//! DST cases are the ones that catch local-date arithmetic: `Europe/London` 2026-03-29 gives
//! a 300-minute window because 01:00–02:00 local does not exist, and 2026-10-25 gives 420
//! because 01:00–02:00 happens twice. If the implementation disagrees with those numbers the
//! implementation is wrong, not the test: capacity is what the local day actually contains.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use stvdy_core::planner::{
    GoalInput, Interval, PlanProposal, PlannerConfig, PlannerInput, StudySettings, Topic, propose,
};
use uuid::Uuid;

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .unwrap()
}

fn london() -> Tz {
    "Europe/London".parse().unwrap()
}

fn settings(start_hour: u32, end_hour: u32, max_study_min_per_day: i64) -> StudySettings {
    StudySettings {
        start_hour,
        end_hour,
        max_study_min_per_day,
    }
}

fn config(
    block_len_min: i64,
    break_min: i64,
    deadline_buffer_min: i64,
    revisit_days: i64,
    spread_k: f64,
) -> PlannerConfig {
    PlannerConfig {
        block_len_min,
        break_min,
        deadline_buffer_min,
        revisit_days,
        spread_k,
        balancer_enabled: false,
    }
}

fn topic(name: &str, remaining_min: i64, accuracy: Option<f64>) -> Topic {
    Topic {
        source_id: None,
        name: name.to_string(),
        remaining_min,
        accuracy,
    }
}

fn goal(id: u128, target: DateTime<Utc>, created: DateTime<Utc>, topics: Vec<Topic>) -> GoalInput {
    GoalInput {
        id: Uuid::from_u128(id),
        title: format!("Goal {id}"),
        target_date: target,
        created_at: created,
        topics,
    }
}

fn interval(starts_at: DateTime<Utc>, ends_at: DateTime<Utc>) -> Interval {
    Interval { starts_at, ends_at }
}

#[allow(clippy::too_many_arguments)]
fn input(
    now: DateTime<Utc>,
    tz: Tz,
    settings: StudySettings,
    goals: Vec<GoalInput>,
    due_review_min: i64,
    busy: Vec<Interval>,
    pinned: Vec<Interval>,
    config: PlannerConfig,
) -> PlannerInput {
    PlannerInput {
        now,
        tz,
        settings,
        goals,
        due_review_min,
        busy,
        pinned,
        config,
    }
}

fn local(instant: DateTime<Utc>, tz: Tz) -> DateTime<Tz> {
    instant.with_timezone(&tz)
}

fn day_load(proposal: &PlanProposal, date: NaiveDate) -> i64 {
    proposal
        .day_load
        .iter()
        .find(|load| load.date == date)
        .map(|load| load.capacity_min)
        .unwrap_or(0)
}

/// A block never overlaps busy time. The free slots already exclude it, so this is the
/// assertion that the subtraction ran at all: a planner that scheduled over a class would
/// put a study block on top of a lecture and look perfectly reasonable in a preview.
#[test]
fn blocks_never_overlap_busy_time() {
    let busy = interval(utc(2026, 4, 2, 3, 0), utc(2026, 4, 2, 4, 0));
    let input = input(
        utc(2026, 4, 1, 0, 0),
        london(),
        settings(8, 22, 480),
        vec![goal(
            1,
            utc(2026, 4, 10, 0, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 600, None)],
        )],
        0,
        vec![busy],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    assert!(
        !proposal.blocks.is_empty(),
        "the goal has demand and the week has room"
    );
    for block in &proposal.blocks {
        let overlaps = block.starts_at < busy.ends_at && block.ends_at > busy.starts_at;
        assert!(!overlaps, "a block overlaps the busy interval: {block:?}");
    }
}

/// Every block sits inside the student's study hours, checked in the input's zone.
///
/// Comparing UTC hours would pass in Jakarta and be wrong everywhere else, which is the whole
/// reason the local time is converted back rather than the numbers being compared directly.
#[test]
fn study_hours_are_respected() {
    let tz = london();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(8, 22, 480),
        vec![goal(
            1,
            utc(2026, 4, 12, 0, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 900, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    assert!(!proposal.blocks.is_empty());
    for block in &proposal.blocks {
        let start = local(block.starts_at, tz);
        let end = local(block.ends_at, tz);
        assert!(
            start.hour() >= 8,
            "a block starts before the window: {start}"
        );
        assert!(
            end.hour() <= 22 && (end.hour() < 22 || end.minute() == 0),
            "a block ends after the window: {end}"
        );
    }
}

/// `study_end_hour` is legal up to `24` (the schema's own bound), and `24` means midnight of
/// the following local day — not the last minute of the current one.
///
/// A window of 08:00–24:00 in `Europe/London` on a BST day is 16 hours, so the day's capacity
/// is 960 minutes. An implementation that stopped the window at 23:59 would report 959 and
/// silently drop a minute; one that treated `24:00` as `23:01` would drop an hour.
#[test]
fn an_end_hour_of_twenty_four_reaches_midnight() {
    let tz = london();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(8, 24, 1440),
        vec![goal(
            1,
            utc(2026, 4, 10, 12, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 900, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    let first = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    assert_eq!(
        day_load(&proposal, first),
        960,
        "08:00 to midnight is sixteen hours"
    );
}

/// The spring-forward day. `Europe/London`, 2026-03-29: 01:00–02:00 local does not exist, so
/// the 00:00–06:00 window holds 300 minutes, not 360 — and no block starts in the missing
/// hour, because there is no such instant to start in.
#[test]
fn study_hours_survive_a_dst_spring_forward_day() {
    let tz = london();
    let input = input(
        utc(2026, 3, 28, 0, 0),
        tz,
        settings(0, 6, 600),
        vec![goal(
            1,
            utc(2026, 4, 10, 0, 0),
            utc(2026, 3, 28, 0, 0),
            vec![topic("Jaringan", 900, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    let spring = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
    assert_eq!(
        day_load(&proposal, spring),
        300,
        "the missing hour is not capacity"
    );

    for block in &proposal.blocks {
        let start = local(block.starts_at, tz);
        if start.date_naive() == spring {
            assert_ne!(
                start.hour(),
                1,
                "no instant exists at local 01:xx on the transition day"
            );
        }
    }
}

/// The autumn day. `Europe/London`, 2026-10-25: 01:00–02:00 local happens twice, so the
/// 00:00–06:00 window holds 420 minutes; and the ambiguous hour takes its earlier offset, so
/// the 01:30 block is the BST one (00:30Z), not the GMT one (01:30Z).
#[test]
fn study_hours_take_the_earlier_offset_on_an_ambiguous_day() {
    let tz = london();
    let wide = input(
        utc(2026, 10, 24, 0, 0),
        tz,
        settings(0, 6, 600),
        vec![goal(
            1,
            utc(2026, 11, 10, 0, 0),
            utc(2026, 10, 24, 0, 0),
            vec![topic("Jaringan", 100_000, None)],
        )],
        0,
        vec![],
        vec![],
        config(30, 0, 0, 0, 0.0),
    );
    let proposal = propose(&wide);
    let autumn = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
    assert_eq!(
        day_load(&proposal, autumn),
        420,
        "the repeated hour is capacity"
    );

    let bst = utc(2026, 10, 25, 0, 30);
    let first_half_past_one = proposal
        .blocks
        .iter()
        .filter(|block| {
            let at = local(block.starts_at, tz);
            at.date_naive() == autumn && at.hour() == 1 && at.minute() == 30
        })
        .min_by_key(|block| block.starts_at)
        .expect("the window reaches local 01:30");
    assert_eq!(
        first_half_past_one.starts_at, bst,
        "01:30 local on the ambiguous day must resolve to the earlier (BST) offset"
    );

    // The window itself starts at local 01:00 on the ambiguous day: `earliest()` puts it at
    // 00:00Z, so a 01:00–06:00 window holds 360 minutes, not the 300 a later-offset read
    // would give.
    let ambiguous_start = input(
        utc(2026, 10, 24, 0, 0),
        tz,
        settings(1, 6, 600),
        vec![goal(
            1,
            utc(2026, 11, 10, 0, 0),
            utc(2026, 10, 24, 0, 0),
            vec![topic("Jaringan", 100_000, None)],
        )],
        0,
        vec![],
        vec![],
        config(30, 0, 0, 0, 0.0),
    );
    let proposal = propose(&ambiguous_start);
    assert_eq!(
        day_load(&proposal, autumn),
        360,
        "an ambiguous window start takes the earlier offset"
    );
}

/// No day is planned past `max_study_min_per_day`.
#[test]
fn max_study_min_per_day_is_respected() {
    let input = input(
        utc(2026, 4, 1, 0, 0),
        london(),
        settings(8, 22, 100),
        vec![goal(
            1,
            utc(2026, 4, 12, 0, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 900, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    for load in &proposal.day_load {
        assert!(
            load.planned_min <= 100,
            "a day is over the daily maximum: {load:?}"
        );
    }
}

/// Blocks are exactly `block_len_min`, the last in a window may be shorter, and consecutive
/// blocks in one window are separated by exactly `break_min`.
///
/// A 2-hour window clipped to 105 minutes, 50-minute blocks and 10-minute breaks, gives
/// `[50, 45]` with one 10-minute gap — the shorter tail block and the exact break in one
/// setup.
#[test]
fn block_and_break_lengths_are_exact() {
    let input = input(
        utc(2026, 4, 1, 0, 0),
        london(),
        settings(8, 10, 105),
        vec![goal(
            1,
            utc(2026, 4, 1, 20, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 1000, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    let mut blocks = proposal.blocks.clone();
    blocks.sort_by_key(|block| block.starts_at);
    let lengths: Vec<i64> = blocks
        .iter()
        .map(|block| (block.ends_at - block.starts_at).num_minutes())
        .collect();
    assert_eq!(
        lengths,
        vec![50, 45],
        "one full block then the shorter tail"
    );
    let gap = (blocks[1].starts_at - blocks[0].ends_at).num_minutes();
    assert_eq!(
        gap, 10,
        "consecutive blocks are separated by exactly the break"
    );
}

fn minutes_for_topic(proposal: &PlanProposal, name: &str) -> i64 {
    proposal
        .blocks
        .iter()
        .filter(|block| block.topic == name)
        .map(|block| (block.ends_at - block.starts_at).num_minutes())
        .sum()
}

fn minutes_on(proposal: &PlanProposal, date: NaiveDate, tz: Tz) -> i64 {
    proposal
        .blocks
        .iter()
        .filter(|block| local(block.starts_at, tz).date_naive() == date)
        .map(|block| (block.ends_at - block.starts_at).num_minutes())
        .sum()
}

/// A weak topic gets strictly more minutes than a strong one with the same work left, and the
/// ratio is the weighted one — not merely "more".
///
/// A test that only asserted "more" would pass on a formula that was monotone and wrong by a
/// factor; the ratio `(1 + 1.5×0.6) / (1 + 1.5×0.05)` is what pins the formula itself.
#[test]
fn weak_topics_weigh_more_than_strong_ones() {
    let tz = london();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(0, 10, 600),
        vec![goal(
            1,
            utc(2026, 4, 1, 18, 0),
            utc(2026, 4, 1, 0, 0),
            vec![
                topic("Lemah", 1000, Some(0.4)),
                topic("Kuat", 1000, Some(0.95)),
            ],
        )],
        0,
        vec![],
        vec![],
        config(10, 0, 0, 0, 1.5),
    );
    let proposal = propose(&input);
    let weak = minutes_for_topic(&proposal, "Lemah");
    let strong = minutes_for_topic(&proposal, "Kuat");
    assert!(
        weak > strong,
        "the weak topic must get strictly more: {weak} vs {strong}"
    );
    let expected = (1.0 + 1.5 * 0.6) / (1.0 + 1.5 * 0.05);
    let actual = weak as f64 / strong as f64;
    assert!(
        (actual - expected).abs() / expected < 0.01,
        "ratio {actual} (weak {weak}, strong {strong}) is not within 1% of {expected}"
    );
}

/// A258: a topic with no attempts weighs the same as a perfect one. Missing data is not
/// evidence of weakness, and inventing a weakness out of it is the honesty rule this
/// prevents.
#[test]
fn a_topic_with_no_attempts_weighs_the_same_as_a_perfect_one() {
    let tz = london();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(0, 10, 600),
        vec![goal(
            1,
            utc(2026, 4, 1, 18, 0),
            utc(2026, 4, 1, 0, 0),
            vec![
                topic("Tanpa data", 1000, None),
                topic("Sempurna", 1000, Some(1.0)),
            ],
        )],
        0,
        vec![],
        vec![],
        config(10, 0, 0, 0, 1.5),
    );
    let proposal = propose(&input);
    assert_eq!(
        minutes_for_topic(&proposal, "Tanpa data"),
        minutes_for_topic(&proposal, "Sempurna"),
        "no attempts must weigh exactly as much as a perfect score"
    );
}

/// The spread cap: one topic, 600 minutes of work, ten days available, so no single day gets
/// more than `max(block_len_min, ceil(remaining / days_left))` and the work is spread instead
/// of finished on day one.
#[test]
fn the_spread_cap_prevents_front_loading() {
    let tz = london();
    let start = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let exam = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(8, 22, 240),
        vec![goal(
            1,
            utc(2026, 4, 10, 12, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 600, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    let mut placed = 0i64;
    for offset in 0..10 {
        let day = start + chrono::Duration::days(offset);
        let days_left = (exam - day).num_days().max(1);
        let cap = ((600.0 / days_left as f64).ceil() as i64).max(50);
        let on_day = minutes_on(&proposal, day, tz);
        assert!(
            on_day <= cap,
            "{day} got {on_day} minutes, over the cap {cap}"
        );
        placed += on_day;
    }
    assert_eq!(placed, 600, "all the work is placed across the horizon");
    assert!(
        minutes_on(&proposal, start, tz) < 600,
        "the work is not finished on day one"
    );
}

/// The deadline buffer is left free of new material: no `NewMaterial` block starts inside
/// `[target_date - deadline_buffer_min, target_date)`.
///
/// Asserted on the block kind, not on a gap, so a review block placed there does not make the
/// test vacuous.
///
/// The demand is set to the whole horizon's capacity and the daily maximum to the whole study
/// window, so the deadline day fills. Non-vacuity is then proved rather than assumed: the same
/// input with `deadline_buffer_min = 0` **does** place a block starting inside the buffer, so
/// the assertion above is the rule doing work and not a fixture that never reaches it.
#[test]
fn the_deadline_buffer_is_left_free_of_new_material() {
    let tz = london();
    let target = utc(2026, 4, 5, 18, 0);
    let build = |buffer_min: i64| {
        input(
            utc(2026, 4, 1, 0, 0),
            tz,
            settings(8, 22, 840),
            vec![goal(
                1,
                target,
                utc(2026, 4, 1, 0, 0),
                vec![topic("Jaringan", 4200, None)],
            )],
            0,
            vec![],
            vec![],
            config(50, 10, buffer_min, 0, 0.0),
        )
    };
    let buffer_start = target - chrono::Duration::minutes(120);
    let starts_in_buffer = |proposal: &PlanProposal| {
        proposal
            .blocks
            .iter()
            .filter(|block| {
                block.kind == stvdy_core::planner::BlockKind::NewMaterial
                    && block.starts_at >= buffer_start
                    && block.starts_at < target
            })
            .count()
    };

    let buffered = propose(&build(120));
    assert_eq!(
        starts_in_buffer(&buffered),
        0,
        "no new material starts inside the deadline buffer"
    );

    let unbuffered = propose(&build(0));
    assert!(
        starts_in_buffer(&unbuffered) > 0,
        "without the buffer the fixture places a block there, so the rule was exercised"
    );
}

/// The review pass reserves one block on each of the last `revisit_days` local dates before
/// the exam (contract step 4 / A269).
#[test]
fn the_review_pass_reserves_one_block_per_day_before_the_exam() {
    let tz = london();
    let exam = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(8, 22, 240),
        vec![goal(
            1,
            utc(2026, 4, 10, 12, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 600, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 3, 0.0),
    );
    let proposal = propose(&input);
    for back in 1..=3 {
        let day = exam - chrono::Duration::days(back);
        let reviews = proposal
            .blocks
            .iter()
            .filter(|block| {
                block.kind == stvdy_core::planner::BlockKind::Review
                    && local(block.starts_at, tz).date_naive() == day
            })
            .count();
        assert_eq!(reviews, 1, "exactly one review block on {day}");
    }
}

/// Twenty hours of demand in a three-hour week: a best-effort plan is still emitted, the
/// deficit is the shortfall, and `Overloaded` names the full days. "Never silently drop
/// demand" is the contract's own sentence.
#[test]
fn deficit_is_reported_when_capacity_is_short() {
    let input = input(
        utc(2026, 4, 1, 0, 0),
        london(),
        settings(8, 9, 50),
        vec![goal(
            1,
            utc(2026, 4, 7, 12, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 1200, None)],
        )],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    assert!(
        !proposal.blocks.is_empty(),
        "a best-effort plan is still emitted"
    );
    assert_eq!(
        proposal.deficit_min,
        1200 - 350,
        "the deficit is demand minus capacity"
    );
    let overloaded = proposal
        .warnings
        .iter()
        .find_map(|warning| match warning {
            stvdy_core::planner::PlanWarning::Overloaded { days } => Some(days.clone()),
            stvdy_core::planner::PlanWarning::PastDeadline { .. } => None,
        })
        .expect("a saturated week reports Overloaded");
    assert_eq!(overloaded.len(), 7, "all seven days are full");
}

/// A deadline before `now` is a warning, not a panic, and it does not stop the other goal.
#[test]
fn a_deadline_before_now_is_a_warning_not_a_panic() {
    let past = Uuid::from_u128(7);
    let input = input(
        utc(2026, 4, 10, 0, 0),
        london(),
        settings(8, 22, 240),
        vec![
            goal(
                7,
                utc(2026, 4, 1, 12, 0),
                utc(2026, 4, 1, 0, 0),
                vec![topic("Jaringan", 600, None)],
            ),
            goal(
                8,
                utc(2026, 4, 20, 12, 0),
                utc(2026, 4, 10, 0, 0),
                vec![topic("Algoritma", 600, None)],
            ),
        ],
        0,
        vec![],
        vec![],
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = propose(&input);
    assert!(
        proposal.warnings.iter().any(|warning| matches!(
            warning,
            stvdy_core::planner::PlanWarning::PastDeadline { goal_id } if *goal_id == past
        )),
        "the past deadline is reported"
    );
    assert!(
        proposal.blocks.iter().all(|block| block.goal_id != past),
        "no blocks are planned for the past goal"
    );
    assert!(
        proposal
            .blocks
            .iter()
            .any(|block| block.goal_id == Uuid::from_u128(8)),
        "the other goal is still planned"
    );
}

/// The same input built two ways produces byte-identical JSON.
///
/// One path assembles the accuracy map from an unordered map and reads it out in insertion
/// order; the other from an ordered map. A container whose iteration order reaches the output
/// cannot pass this, because the two paths hand the planner the same topics in different
/// sequences.
///
/// Every topic has the **same remaining work and the same accuracy**, so weight and remaining
/// tie and the allocation's tie-break falls all the way through to the topic's position in
/// the input. A fixture with distinct weights would let the weight tie-break resolve
/// everything and pass with the canonical ordering deleted, which is the leak this test is
/// for.
#[test]
fn the_same_input_produces_byte_identical_output() {
    use std::collections::{BTreeMap, HashMap};

    let mut unordered: HashMap<&str, Option<f64>> = HashMap::new();
    unordered.insert("Algoritma", Some(0.5));
    unordered.insert("Basis data", Some(0.5));
    unordered.insert("Jaringan", Some(0.5));
    unordered.insert("Kriptografi", Some(0.5));
    unordered.insert("Sistem operasi", Some(0.5));

    let ordered: BTreeMap<&str, Option<f64>> = unordered.iter().map(|(k, v)| (*k, *v)).collect();

    let build = |pairs: Vec<(&str, Option<f64>)>| {
        input(
            utc(2026, 4, 1, 0, 0),
            london(),
            settings(8, 22, 240),
            vec![goal(
                1,
                utc(2026, 4, 20, 0, 0),
                utc(2026, 4, 1, 0, 0),
                pairs
                    .into_iter()
                    .map(|(name, accuracy)| topic(name, 400, accuracy))
                    .collect(),
            )],
            30,
            vec![],
            vec![],
            config(50, 10, 120, 3, 1.5),
        )
    };

    let a = build(unordered.into_iter().collect());
    let b = build(ordered.into_iter().collect());
    let json_a = serde_json::to_vec(&propose(&a)).unwrap();
    let json_b = serde_json::to_vec(&propose(&b)).unwrap();
    assert_eq!(
        json_a, json_b,
        "the same logical input must serialize byte for byte"
    );
}

/// The balancer terminates, respects its bound, moves the latest movable block, and strictly
/// lowers the heavy day's load.
///
/// An unbounded `while` here is a hang rather than a wrong answer, so the returned iteration
/// count is what lets this test assert the bound rather than merely observe that the call
/// came back.
#[test]
fn the_balancer_terminates_and_moves_the_latest_movable_block() {
    let tz = london();
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(8, 22, 200),
        vec![
            goal(
                1,
                utc(2026, 4, 1, 18, 0),
                utc(2026, 4, 1, 0, 0),
                vec![topic("A", 240, None)],
            ),
            goal(
                2,
                utc(2026, 4, 2, 18, 0),
                utc(2026, 4, 1, 0, 0),
                vec![topic("B", 30, None)],
            ),
        ],
        0,
        vec![],
        vec![],
        config(50, 0, 0, 0, 0.0),
    );
    let pre = propose(&input);
    let heavy = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let before = pre
        .day_load
        .iter()
        .find(|load| load.date == heavy)
        .unwrap()
        .planned_min;
    assert_eq!(
        before, 200,
        "the fixture fills the first day to the daily maximum"
    );
    let latest_start = pre
        .blocks
        .iter()
        .filter(|block| {
            !block.pinned
                && block.kind == stvdy_core::planner::BlockKind::NewMaterial
                && local(block.starts_at, tz).date_naive() == heavy
        })
        .map(|block| block.starts_at)
        .max()
        .expect("the heavy day has movable blocks");

    let mut blocks = pre.blocks.clone();
    let mut day_load = pre.day_load.clone();
    let iterations = stvdy_core::planner::rebalance(&input, &mut blocks, &mut day_load);
    assert!(iterations >= 1, "the heavy day was rebalanced");
    assert!(
        iterations <= stvdy_core::planner::MAX_ITERATIONS,
        "the balancer respected its bound"
    );
    let after = day_load
        .iter()
        .find(|load| load.date == heavy)
        .unwrap()
        .planned_min;
    assert!(
        after < before,
        "the heavy day's load strictly fell: {before} -> {after}"
    );

    let moved = blocks
        .iter()
        .zip(&pre.blocks)
        .position(|(now, then)| now.starts_at != then.starts_at)
        .expect("a block moved");
    assert_eq!(
        pre.blocks[moved].starts_at, latest_start,
        "the latest movable block is the one that moved"
    );
    assert_ne!(
        local(blocks[moved].starts_at, tz).date_naive(),
        heavy,
        "the moved block left the heavy day"
    );
}

/// A pinned block is never moved, and a plan whose only movable work is pinned is unchanged —
/// byte for byte the same as the same input with the balancer off.
#[test]
fn the_balancer_never_moves_a_pinned_block() {
    let input = input(
        utc(2026, 4, 1, 0, 0),
        london(),
        settings(8, 22, 200),
        vec![goal(
            1,
            utc(2026, 4, 5, 18, 0),
            utc(2026, 4, 1, 0, 0),
            vec![],
        )],
        0,
        vec![],
        vec![interval(utc(2026, 4, 1, 7, 0), utc(2026, 4, 1, 10, 20))],
        config(50, 0, 0, 0, 0.0),
    );
    let pre = propose(&input);
    assert!(
        pre.blocks.iter().any(|block| block.pinned),
        "the pinned block is emitted"
    );

    let mut blocks = pre.blocks.clone();
    let mut day_load = pre.day_load.clone();
    let iterations = stvdy_core::planner::rebalance(&input, &mut blocks, &mut day_load);
    assert_eq!(iterations, 0, "nothing movable exists to move");
    assert_eq!(blocks, pre.blocks, "the blocks are untouched");

    let mut enabled = input.clone();
    enabled.config.balancer_enabled = true;
    assert_eq!(
        serde_json::to_vec(&propose(&enabled)).unwrap(),
        serde_json::to_vec(&pre).unwrap(),
        "a pinned-only plan is identical with the balancer on and off"
    );
}

/// Replan keeps every pinned block exactly, and no new block overlaps one.
///
/// This is the property that makes replanning trustworthy at all: a student who committed to
/// Tuesday 08:00 must find Tuesday 08:00 still there after they ask for a new week. The
/// pinned intervals are subtracted from the free slots (step 2) and emitted as blocks, so the
/// two halves of the guarantee are checked separately — the block survives, and nothing is
/// placed on top of it.
///
/// The pinned slots are the **first block of three days**, which is exactly where the
/// allocator would otherwise put new material: a pin somewhere the plan never reaches would
/// make the overlap half of this test vacuous.
#[test]
fn replan_keeps_pinned_blocks_exactly() {
    let tz = london();
    let pinned = vec![
        interval(utc(2026, 4, 2, 7, 0), utc(2026, 4, 2, 7, 50)),
        interval(utc(2026, 4, 3, 7, 0), utc(2026, 4, 3, 7, 50)),
        interval(utc(2026, 4, 6, 7, 0), utc(2026, 4, 6, 7, 50)),
    ];
    let input = input(
        utc(2026, 4, 1, 0, 0),
        tz,
        settings(8, 22, 240),
        vec![goal(
            1,
            utc(2026, 4, 10, 12, 0),
            utc(2026, 4, 1, 0, 0),
            vec![topic("Jaringan", 3000, None)],
        )],
        0,
        vec![],
        pinned.clone(),
        config(50, 10, 0, 0, 0.0),
    );
    let proposal = stvdy_core::planner::replan(&input);

    for interval in &pinned {
        let found = proposal.blocks.iter().find(|block| {
            block.pinned
                && block.starts_at == interval.starts_at
                && block.ends_at == interval.ends_at
        });
        assert!(
            found.is_some(),
            "the pinned interval {interval:?} is missing or moved"
        );
    }
    for block in &proposal.blocks {
        if block.pinned {
            continue;
        }
        for interval in &pinned {
            let overlaps = block.starts_at < interval.ends_at && block.ends_at > interval.starts_at;
            assert!(!overlaps, "a new block overlaps a pinned one: {block:?}");
        }
    }
}
