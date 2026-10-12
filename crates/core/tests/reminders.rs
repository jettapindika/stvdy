//! The reminder engine: the key, the quiet window, and the DST edges.
//!
//! Every case here names the defect it catches. Three of them exist because the plausible
//! implementation passes everything else:
//!
//!  * `the_key_ignores_the_fire_instant` — a key built from the computed instant fires once per
//!    device, because two phones computing the same due time are a minute apart.
//!  * `quiet_hours_wrap_past_midnight` — `start <= hour && hour < end` is false for every hour
//!    inside a window like 22:00→07:00, which is the window the settings screen offers.
//!  * the two London cases — a fixed offset passes every test written in Jakarta, which is
//!    where the rest of this file lives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use stvdy_core::config::ReminderChannel;
use stvdy_core::reminders::{
    MAX_LOOKAHEAD_DAYS, PlannedReminder, ReminderKind, ReminderSettings, SkipReason, Subject,
    dedupe_key, defer_for_quiet_hours, in_quiet_hours, plan, quiet_end,
};
use uuid::Uuid;

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .unwrap()
}

fn jakarta() -> Tz {
    "Asia/Jakarta".parse().unwrap()
}

fn london() -> Tz {
    "Europe/London".parse().unwrap()
}

fn settings(tz: Tz, quiet: Option<(u8, u8)>, cards_due: i64) -> ReminderSettings {
    ReminderSettings {
        tz,
        quiet,
        channels: vec![ReminderChannel::Web, ReminderChannel::Push],
        study_start_hour: 7,
        cards_due,
    }
}

fn task(id: Uuid, due: DateTime<Utc>) -> Subject {
    Subject {
        kind: ReminderKind::TaskDue,
        id,
        at: Some(due),
        done: false,
    }
}

fn only(planned: &[PlannedReminder], kind: ReminderKind) -> &PlannedReminder {
    let matching: Vec<&PlannedReminder> = planned.iter().filter(|item| item.kind == kind).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one {kind:?}: {planned:?}"
    );
    matching[0]
}

/// The key is the subject and the local date, never the fire instant (A284). Two devices
/// that compute 19:00 and 19:01 for the same task produce the same key, and a quiet-hours
/// deferral does not change it.
#[test]
fn the_key_ignores_the_fire_instant() {
    let id = Uuid::new_v4();
    let now = utc(2026, 10, 11, 6, 0);

    // Two producers, an hour apart on the same task: the client computed `fire_at` from its
    // own synced view, and the sweep derived it from the row. Same subject, same local day.
    let earlier = plan(
        now,
        &settings(jakarta(), None, 0),
        &[task(id, utc(2026, 10, 11, 16, 0))],
    );
    let later = plan(
        now,
        &settings(jakarta(), None, 0),
        &[task(id, utc(2026, 10, 11, 17, 0))],
    );

    assert_eq!(earlier[0].dedupe_key, later[0].dedupe_key);
    assert_ne!(earlier[0].fire_at, later[0].fire_at);

    // Both at once is the normal case (two devices, one sweep): one row, and it is the
    // *earliest* fire time — which a fire-time key would fail by producing two rows.
    let both = plan(
        now,
        &settings(jakarta(), None, 0),
        &[
            task(id, utc(2026, 10, 11, 16, 0)),
            task(id, utc(2026, 10, 11, 17, 0)),
        ],
    );
    assert_eq!(both.len(), 1, "{both:?}");
    assert_eq!(both[0].fire_at, utc(2026, 10, 11, 15, 0));

    // And the key is a function of the subject and the date alone.
    assert_eq!(
        earlier[0].dedupe_key,
        dedupe_key(
            ReminderKind::TaskDue,
            &id.to_string(),
            "2026-10-11".parse().unwrap()
        )
    );
}

/// Quiet hours wrap midnight, and a deferred reminder keeps its key and says it moved.
#[test]
fn a_deferral_keeps_the_key_and_is_marked() {
    let id = Uuid::new_v4();
    let now = utc(2026, 10, 11, 6, 0);
    // Due 23:00 Jakarta, so it fires at 22:00 — the first hour of a 22:00→07:00 window.
    let due = utc(2026, 10, 11, 16, 0);

    let undelayed = plan(now, &settings(jakarta(), None, 0), &[task(id, due)]);
    let deferred = plan(
        now,
        &settings(jakarta(), Some((22, 7)), 0),
        &[task(id, due)],
    );

    assert_eq!(undelayed[0].dedupe_key, deferred[0].dedupe_key);
    assert_eq!(undelayed[0].fire_at, utc(2026, 10, 11, 15, 0));
    // 07:00 the next morning in Jakarta is 00:00Z.
    assert_eq!(deferred[0].fire_at, utc(2026, 10, 12, 0, 0));
    assert!(deferred[0].deferred);
    assert_eq!(deferred[0].skip, None);
    // The local date is the one the reminder *belongs* to, not the one it lands on.
    assert_eq!(deferred[0].local_date.to_string(), "2026-10-11");
}

/// The membership test itself, because the naive comparison is false for every hour of a
/// wrapped window — which is the bug the whole of quiet hours turns on.
#[test]
fn quiet_hours_wrap_past_midnight() {
    let quiet = Some((22, 7));
    for hour in [22, 23, 0, 1, 6] {
        assert!(in_quiet_hours(hour, quiet), "{hour} is inside 22:00→07:00");
    }
    for hour in [7, 8, 12, 21] {
        assert!(
            !in_quiet_hours(hour, quiet),
            "{hour} is outside 22:00→07:00"
        );
    }

    // A window that does not wrap is the ordinary case. The membership test is half-open —
    // `start <= hour < end`, the form the plan quotes — so the start hour is inside and the
    // end hour is the first hour of the day.
    assert!(!in_quiet_hours(11, Some((12, 14))));
    assert!(in_quiet_hours(12, Some((12, 14))));
    assert!(in_quiet_hours(13, Some((12, 14))));
    assert!(!in_quiet_hours(14, Some((12, 14))));

    // No window at all, and an empty one: a student who set the same hour twice is not
    // quiet for twenty-four hours.
    assert!(!in_quiet_hours(3, None));
    assert!(!in_quiet_hours(3, Some((3, 3))));

    // From inside the window, the end is the next morning; from outside, there is none.
    assert_eq!(
        quiet_end(utc(2026, 10, 11, 15, 30), jakarta(), (22, 7)).unwrap(),
        utc(2026, 10, 12, 0, 0)
    );
    assert_eq!(
        quiet_end(utc(2026, 10, 11, 15, 30), jakarta(), (20, 21)),
        None
    );
}

/// The window's edges at **minute** granularity, through the deferral the producers call.
///
/// `quiet_hours_wrap_past_midnight` asserts the hour predicate, and the hour predicate is the
/// half of it a naive implementation gets wrong. This is the other half: the four instants
/// one minute either side of each edge, because a mid-window hour (01:00) is not where the
/// bug lives. `22:00` and `06:59` are inside a 22:00→07:00 window and a naive
/// `start <= hour && hour < end` says they are outside; `21:59` and `07:00` are outside and it
/// says the same, so only the two inside cases can fail — and they are the ones asserted with
/// their deferral target, not merely with a boolean.
#[test]
fn the_quiet_window_edges_are_minute_exact() {
    let quiet = Some((22, 7));
    // Jakarta is UTC+7 with no DST, so each local edge is a fixed instant and the expected
    // `fire_at` is written as UTC rather than as an offset arithmetic the test could get wrong
    // in the same direction as the code.
    let before_the_window = utc(2026, 10, 11, 14, 59); // 21:59 local
    let window_opens = utc(2026, 10, 11, 15, 0); // 22:00 local
    let window_closes_soon = utc(2026, 10, 11, 23, 59); // 06:59 local on the 12th
    let window_closed = utc(2026, 10, 12, 0, 0); // 07:00 local on the 12th

    // 21:59 is one minute early: the reminder keeps its instant and says it did not move.
    let deferral =
        defer_for_quiet_hours(before_the_window, utc(2026, 10, 12, 3, 0), jakarta(), quiet);
    assert!(!deferral.deferred, "21:59 is outside 22:00→07:00");
    assert_eq!(deferral.fire_at, before_the_window);
    assert_eq!(deferral.skip, None);

    // 22:00 is the first quiet minute, and the deferral lands on the window's close the next
    // morning — 07:00 local on the 12th, which is midnight UTC.
    let deferral = defer_for_quiet_hours(window_opens, utc(2026, 10, 12, 3, 0), jakarta(), quiet);
    assert!(deferral.deferred, "22:00 is the first minute of the window");
    assert_eq!(
        deferral.fire_at,
        utc(2026, 10, 12, 0, 0),
        "it moves to 07:00 local, not to the next hour"
    );
    assert_eq!(deferral.skip, None, "the subject is still ahead of it");

    // 06:59 is the last quiet minute: it moves to 07:00 the same morning, not to the following
    // night, which is the direction a naive walk gets wrong.
    let deferral = defer_for_quiet_hours(
        window_closes_soon,
        utc(2026, 10, 12, 3, 0),
        jakarta(),
        quiet,
    );
    assert!(deferral.deferred, "06:59 is the last minute of the window");
    assert_eq!(deferral.fire_at, utc(2026, 10, 12, 0, 0));

    // 07:00 is the first minute after it.
    let deferral = defer_for_quiet_hours(window_closed, utc(2026, 10, 12, 3, 0), jakarta(), quiet);
    assert!(!deferral.deferred, "07:00 is outside the window");
    assert_eq!(deferral.fire_at, window_closed);

    // And the naive comparison the whole case exists for is asserted to disagree, so this test
    // cannot be satisfied by an implementation that shares the naive rule. The bounds are
    // `let`-bound rather than literals inside the closure: with literals, `22 <= h && h < 7`
    // is a constant `false` that rustc's `impossible_comparisons` rejects, and the closure
    // would be asserting something about the compiler rather than about the two rules.
    let naive = |start: u8, end: u8, hour: u8| start <= hour && hour < end;
    assert!(
        !naive(22, 7, 22),
        "the naive rule is false for 22:00, the window's first hour"
    );
    assert!(in_quiet_hours(22, quiet), "the real rule is true for it");
    assert!(
        !naive(22, 7, 6),
        "and false for 06:59, its last one — so a mid-window hour is no better"
    );
    assert!(in_quiet_hours(6, quiet), "which the real rule covers");

    // A fire time that is exactly the window's close is not deferred again, which is what stops
    // a deferred reminder from walking a whole window per sweep.
    let deferral = defer_for_quiet_hours(
        utc(2026, 10, 12, 0, 0),
        utc(2026, 10, 12, 3, 0),
        jakarta(),
        quiet,
    );
    assert!(!deferral.deferred, "the window's own close is outside it");
}

/// Past the stale window it is skipped with a reason, and it is still in the output —
/// a reminder that silently never arrives is what the student reports as "broken".
///
/// The bound can only bite the day-granular reminder: a task or event fires at most an hour
/// before its own subject and a quiet window is shorter than a day, so a deferral can never
/// carry one past `STALE_AFTER_MIN`. A card reminder's subject is the *day*, and a
/// twenty-three-hour window can push it into the next one — which is exactly the reminder
/// that is no longer about today.
#[test]
fn a_deferral_past_the_stale_window_is_skipped_not_dropped() {
    let now = utc(2026, 10, 11, 0, 0);
    // 06:00→05:00 is a legal twenty-three-hour window, and 07:00 is inside it.
    let planned = plan(now, &settings(jakarta(), Some((6, 5)), 12), &[]);

    assert_eq!(planned.len(), 1, "{planned:?}");
    let cards = &planned[0];
    assert_eq!(cards.kind, ReminderKind::CardsDue);
    assert!(cards.deferred);
    assert_eq!(cards.skip, Some(SkipReason::QuietHours));
    // Deferred to 05:00 the next morning Jakarta, which is 29 hours past the day's start.
    assert_eq!(cards.fire_at, utc(2026, 10, 11, 22, 0));
    assert_eq!(cards.local_date.to_string(), "2026-10-11");

    // A window that ends at midnight defers without skipping: the deferral lands exactly
    // twenty-four hours after the day began, which is the bound's edge rather than past it.
    let mut late = settings(jakarta(), Some((22, 0)), 12);
    late.study_start_hour = 23;
    let short = plan(now, &late, &[]);
    assert!(short[0].deferred);
    assert_eq!(short[0].skip, None);
    // 00:00 the next morning in Jakarta is 17:00Z.
    assert_eq!(short[0].fire_at, utc(2026, 10, 11, 17, 0));
}

/// The fire time is the student's local hour in Jakarta, which is the whole point of the
/// timezone rule.
#[test]
fn the_fire_time_is_the_users_local_hour() {
    let now = utc(2026, 10, 10, 20, 0);
    let planned = plan(now, &settings(jakarta(), None, 4), &[]);

    assert_eq!(planned.len(), 1);
    // 07:00 in Jakarta (UTC+7) is midnight UTC.
    assert_eq!(planned[0].fire_at, utc(2026, 10, 11, 0, 0));
    assert_eq!(planned[0].local_date.to_string(), "2026-10-11");
    assert_eq!(planned[0].subject_key, "2026-10-11");
    assert_eq!(planned[0].subject_id, None);

    // The study hour is the input, and it is read as a local hour.
    let mut late = settings(jakarta(), None, 4);
    late.study_start_hour = 21;
    assert_eq!(
        plan(now, &late, &[])[0].fire_at,
        utc(2026, 10, 11, 14, 0),
        "21:00 Jakarta is 14:00Z"
    );
}

/// DST is where local-date arithmetic silently skips or duplicates. London, 2026-03-29: a
/// fire time that does not exist is skipped; 2026-10-25: an ambiguous one takes the
/// earlier offset. `Asia/Jakarta` has no DST, so a test that used it would prove nothing.
#[test]
fn a_spring_forward_fire_time_that_does_not_exist_is_skipped() {
    // London springs forward at 01:00 GMT on 2026-03-29: 01:00–01:59 local never happens.
    let now = utc(2026, 3, 29, 0, 30);
    let mut cfg = settings(london(), None, 3);
    cfg.study_start_hour = 1;

    let planned = plan(now, &cfg, &[]);
    assert_eq!(planned.len(), 1, "{planned:?}");
    assert_eq!(planned[0].skip, Some(SkipReason::NonexistentLocalTime));
    assert_eq!(planned[0].local_date.to_string(), "2026-03-29");
    // The first hour that does exist is 02:00 BST, which is 01:00Z. A fixed offset of +0
    // would have said 01:00Z too and a fixed +1 would have said 00:00Z, so this is the
    // instant the zone actually resolves to rather than either guess.
    assert_eq!(planned[0].fire_at, utc(2026, 3, 29, 1, 0));
    // The key does not move with it, because the key is the day.
    assert_eq!(
        planned[0].dedupe_key,
        dedupe_key(
            ReminderKind::CardsDue,
            "2026-03-29",
            "2026-03-29".parse().unwrap()
        )
    );

    // The same hour the next day exists, and is not skipped.
    let next_day = plan(utc(2026, 3, 30, 0, 30), &cfg, &[]);
    assert_eq!(next_day[0].skip, None);
    // 01:00 BST on the 30th is 00:00Z.
    assert_eq!(next_day[0].fire_at, utc(2026, 3, 30, 0, 0));
}

#[test]
fn an_ambiguous_fall_back_fire_time_takes_the_earlier_offset() {
    // London falls back at 02:00 BST on 2026-10-25: 01:00–01:59 local happens twice, first
    // at 00:00Z (BST, +1) and again at 01:00Z (GMT, +0).
    let now = utc(2026, 10, 24, 23, 30);
    let mut cfg = settings(london(), None, 3);
    cfg.study_start_hour = 1;

    let planned = plan(now, &cfg, &[]);
    assert_eq!(planned.len(), 1, "{planned:?}");
    assert_eq!(planned[0].skip, None);
    assert_eq!(
        planned[0].fire_at,
        utc(2026, 10, 25, 0, 0),
        "the earlier of the two 01:00s is the one the student's clock showed first"
    );

    // The quiet window's end has the same rule: a window ending in the ambiguous hour stops
    // at its first occurrence rather than an hour later.
    assert_eq!(
        quiet_end(utc(2026, 10, 24, 22, 30), london(), (22, 1)).unwrap(),
        utc(2026, 10, 25, 0, 0)
    );
}

/// The day-granular card reminder is one per local day, not one per card.
#[test]
fn the_due_cards_reminder_is_one_per_local_day() {
    let now = utc(2026, 10, 11, 2, 0);
    let planned = plan(now, &settings(jakarta(), None, 137), &[]);
    assert_eq!(
        planned.len(),
        1,
        "137 cards is still one reminder: {planned:?}"
    );
    assert_eq!(planned[0].kind, ReminderKind::CardsDue);

    // A `CardsDue` subject in the list says the same thing in another shape, and does not
    // add a second reminder — nor does it grow the plan with the deck's size.
    let also_listed = plan(
        now,
        &settings(jakarta(), None, 137),
        &[Subject {
            kind: ReminderKind::CardsDue,
            id: Uuid::new_v4(),
            at: None,
            done: false,
        }],
    );
    assert_eq!(also_listed, planned);

    // No cards due is no reminder at all: a zero is not something to be told about.
    assert!(plan(now, &settings(jakarta(), None, 0), &[]).is_empty());
}

/// A completed subject produces no reminder, and a subject with no channels produces none.
#[test]
fn a_done_subject_and_a_channelless_user_produce_nothing() {
    let now = utc(2026, 10, 11, 6, 0);
    let due = utc(2026, 10, 11, 16, 0);
    let id = Uuid::new_v4();

    let done = Subject {
        done: true,
        ..task(id, due)
    };
    assert!(
        plan(now, &settings(jakarta(), None, 0), &[done]).is_empty(),
        "a finished task is not worth reminding about"
    );

    let mut no_channels = settings(jakarta(), None, 5);
    no_channels.channels.clear();
    assert!(
        plan(now, &no_channels, &[task(id, due)]).is_empty(),
        "there is nowhere to deliver a reminder with no channels"
    );

    // A subject beyond the horizon is not planned at all rather than planned and skipped:
    // it is not this sweep's business, and a later sweep will see it.
    let far = due + chrono::Duration::days(MAX_LOOKAHEAD_DAYS + 1);
    assert!(plan(now, &settings(jakarta(), None, 0), &[task(id, far)]).is_empty());
    let near = due + chrono::Duration::days(MAX_LOOKAHEAD_DAYS - 1);
    assert_eq!(
        plan(now, &settings(jakarta(), None, 0), &[task(id, near)]).len(),
        1
    );

    // And a subject whose own instant has passed says so rather than being dropped.
    let past = plan(
        now,
        &settings(jakarta(), None, 0),
        &[task(id, utc(2026, 10, 11, 5, 0))],
    );
    assert_eq!(past[0].skip, Some(SkipReason::AlreadyPast));
}

/// Determinism, asserted on the serialised form byte for byte — and against a reordered
/// input, which is the shape a `HashMap` or an unstable sort would fail.
#[test]
fn the_plan_is_deterministic() {
    let now = utc(2026, 10, 11, 6, 0);
    let cfg = settings(jakarta(), Some((22, 7)), 9);

    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let third = Uuid::new_v4();
    let subjects = vec![
        task(first, utc(2026, 10, 11, 16, 0)),
        Subject {
            kind: ReminderKind::EventSoon,
            id: second,
            at: Some(utc(2026, 10, 11, 9, 0)),
            done: false,
        },
        task(third, utc(2026, 10, 11, 16, 0)),
    ];

    let planned = plan(now, &cfg, &subjects);
    let again = plan(now, &cfg, &subjects);
    let json = serde_json::to_string(&planned).unwrap();
    assert_eq!(json, serde_json::to_string(&again).unwrap());
    assert_eq!(
        json,
        serde_json::to_string(&plan(now, &cfg, &subjects)).unwrap()
    );

    // The same three subjects in the other order: the plan is a function of the set, and
    // its order is `(fire_at, kind, subject_key)` rather than the input's.
    let mut reversed = subjects.clone();
    reversed.reverse();
    assert_eq!(
        json,
        serde_json::to_string(&plan(now, &cfg, &reversed)).unwrap()
    );

    // The order is the stated total order, and it is what makes the plan diffable.
    let times: Vec<DateTime<Utc>> = planned.iter().map(|item| item.fire_at).collect();
    let mut sorted = times.clone();
    sorted.sort();
    assert_eq!(times, sorted, "{planned:?}");
    assert_eq!(
        planned.len(),
        4,
        "two tasks, one event and the day's card reminder: {planned:?}"
    );
    assert_eq!(only(&planned, ReminderKind::CardsDue).subject_id, None);
    assert_eq!(
        only(&planned, ReminderKind::EventSoon).subject_id,
        Some(second)
    );
}
