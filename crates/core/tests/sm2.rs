//! SM-2 replay: the contract two devices have to agree on.
//!
//! Every test here is written so that a change to the interval rule or the ease
//! formula fails loudly. These vectors are also the reference Phase 8's TypeScript
//! replay will be checked against, so the numbers are asserted exactly rather than
//! "roughly".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use stvdy_core::sm2::{CardState, DEFAULT_EASE, MIN_EASE, Rating, Review, replay};
use uuid::Uuid;

fn at(iso: &str) -> DateTime<Utc> {
    iso.parse().expect("a valid timestamp")
}

/// A review with a deterministic id, so ordering is a property of the test and
/// not of a random uuid.
fn review(id: u128, rating: Rating, reviewed_at: &str) -> Review {
    Review {
        id: Uuid::from_u128(id),
        rating,
        reviewed_at: at(reviewed_at),
    }
}

fn ease_of(state: CardState) -> f32 {
    state.ease
}

#[test]
fn a_card_with_no_reviews_is_due_when_it_was_created() {
    let created = at("2026-10-11T00:00:00Z");
    let state = CardState::new(created);
    assert_eq!(state.ease, DEFAULT_EASE);
    assert_eq!(state.interval_days, 0);
    assert_eq!(state.due_at, created);
    assert!(state.is_due(created));
}

#[test]
fn three_good_reviews_give_intervals_one_six_and_fifteen() {
    let created = at("2026-10-11T00:00:00Z");
    let state = replay(
        created,
        &[
            review(1, Rating::Good, "2026-10-11T09:00:00Z"),
            review(2, Rating::Good, "2026-10-12T09:00:00Z"),
            review(3, Rating::Good, "2026-10-18T09:00:00Z"),
        ],
    );
    assert_eq!(state.interval_days, 15);
    // 6 * 2.5 = 15, and `good` leaves ease exactly where it was.
    assert!((ease_of(state) - 2.5).abs() < 1e-6, "{}", ease_of(state));
    assert_eq!(state.due_at, at("2026-11-02T09:00:00Z"));
}

#[test]
fn easy_raises_ease_and_hard_lowers_it() {
    let created = at("2026-10-11T00:00:00Z");
    let easy = replay(created, &[review(1, Rating::Easy, "2026-10-11T09:00:00Z")]);
    assert_eq!(easy.interval_days, 1);
    assert!((ease_of(easy) - 2.6).abs() < 1e-6, "{}", ease_of(easy));

    let hard = replay(created, &[review(1, Rating::Hard, "2026-10-11T09:00:00Z")]);
    assert_eq!(hard.interval_days, 1);
    assert!((ease_of(hard) - 2.36).abs() < 1e-6, "{}", ease_of(hard));
}

#[test]
fn again_resets_the_interval_and_drops_ease_but_not_below_the_floor() {
    let created = at("2026-10-11T00:00:00Z");
    let once = replay(
        created,
        &[
            review(1, Rating::Good, "2026-10-11T09:00:00Z"),
            review(2, Rating::Good, "2026-10-12T09:00:00Z"),
            review(3, Rating::Again, "2026-10-18T09:00:00Z"),
        ],
    );
    // The interval resets, and the ease after two `good`s is still 2.5.
    assert_eq!(once.interval_days, 1);
    assert!((ease_of(once) - 1.7).abs() < 1e-6, "{}", ease_of(once));
    assert_eq!(once.due_at, at("2026-10-19T09:00:00Z"));

    let floored = replay(
        created,
        &[
            review(1, Rating::Again, "2026-10-11T09:00:00Z"),
            review(2, Rating::Again, "2026-10-12T09:00:00Z"),
            review(3, Rating::Again, "2026-10-13T09:00:00Z"),
        ],
    );
    assert_eq!(ease_of(floored), MIN_EASE);
}

/// The property the whole design rests on: the review rows are the facts and the
/// card state is a function of them, so arrival order cannot matter.
#[test]
fn the_arrival_order_of_reviews_does_not_change_the_state() {
    let created = at("2026-10-11T00:00:00Z");
    let forwards = vec![
        review(1, Rating::Good, "2026-10-11T09:00:00Z"),
        review(2, Rating::Again, "2026-10-12T09:00:00Z"),
        review(3, Rating::Easy, "2026-10-13T09:00:00Z"),
    ];
    let mut backwards = forwards.clone();
    backwards.reverse();
    assert_eq!(replay(created, &forwards), replay(created, &backwards));

    // And the state is the one the rules produce, written out rather than compared
    // with another call: `replay(x) == replay(x)` can only fail if `replay` grows a
    // clock or a random source, so it pins nothing about the schedule.
    let state = replay(created, &forwards);
    assert_eq!(state.interval_days, 1);
    assert_eq!(state.due_at, at("2026-10-14T09:00:00Z"));
    assert!((ease_of(state) - 1.8).abs() < 1e-6, "{}", ease_of(state));
}

/// Two reviews that share a timestamp are ordered by id, so a device that
/// reviewed twice in the same second still replays deterministically.
#[test]
fn reviews_at_the_same_instant_are_ordered_by_id() {
    let created = at("2026-10-11T00:00:00Z");
    let low = review(1, Rating::Good, "2026-10-11T09:00:00Z");
    let high = review(2, Rating::Easy, "2026-10-11T09:00:00Z");
    assert_eq!(
        replay(created, &[low.clone(), high.clone()]),
        replay(created, &[high, low])
    );
}
