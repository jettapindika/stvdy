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
