//! SM-2 spaced repetition, as a deterministic replay.
//!
//! `flashcards.ease`, `interval_days` and `due_at` are **derived**: they are never
//! authored by a client (the sync conflict table and `docs/THREAT_MODEL.md` §5 both
//! say so). They are recomputed here by replaying the card's `flashcard_reviews`
//! rows in `(reviewed_at, id)` order from the schema's initial state. That is what
//! makes reviews from several devices conflict-free: the reviews are append-only
//! facts and the card state is a pure function of them, so any arrival order
//! produces one state. An incremental update from the stored values would instead
//! make the result depend on which reviews that device had seen, which is the bug
//! the append-only design exists to prevent.
//!
//! No clock, no IO, no database. `reviewed_at` comes from the review row, never
//! from `Utc::now()`, so a replay is reproducible from stored data alone.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::error::CoreError;

/// The ease factor a card starts with. Matches `flashcards.ease`'s default.
pub const DEFAULT_EASE: f32 = 2.5;
/// The floor SM-2 puts on ease, so a card failed repeatedly still becomes
/// reviewable instead of sitting at a one-day interval forever.
pub const MIN_EASE: f32 = 1.3;
/// The interval after the first successful review, in days.
pub const FIRST_INTERVAL_DAYS: i32 = 1;
/// The interval after the second successful review, in days.
pub const SECOND_INTERVAL_DAYS: i32 = 6;

/// The four grades the study screen offers, matching
/// `flashcard_reviews.rating`'s documented values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    Again,
    Hard,
    Good,
    Easy,
}

impl Rating {
    pub fn as_str(self) -> &'static str {
        match self {
            Rating::Again => "again",
            Rating::Hard => "hard",
            Rating::Good => "good",
            Rating::Easy => "easy",
        }
    }

    /// Parses the wire form. Rejecting an unknown grade is deliberate: a typo
    /// silently treated as `good` would move a card's schedule on bad input.
    pub fn parse(raw: &str) -> Result<Rating, CoreError> {
        match raw {
            "again" => Ok(Rating::Again),
            "hard" => Ok(Rating::Hard),
            "good" => Ok(Rating::Good),
            "easy" => Ok(Rating::Easy),
            other => Err(CoreError::validation(
                "rating",
                format!("unknown review rating {other:?} (expected again, hard, good or easy)"),
            )),
        }
    }

    /// The SM-2 quality grade. `again` is below the pass threshold of 3.
    pub fn quality(self) -> i32 {
        match self {
            Rating::Again => 0,
            Rating::Hard => 3,
            Rating::Good => 4,
            Rating::Easy => 5,
        }
    }
}

/// One `flashcard_reviews` row, as the replay sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub id: Uuid,
    pub rating: Rating,
    pub reviewed_at: DateTime<Utc>,
}

/// A card's derived scheduling state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardState {
    pub ease: f32,
    pub interval_days: i32,
    pub due_at: DateTime<Utc>,
}

impl CardState {
    /// The state of a card that has never been reviewed: due immediately.
    pub fn new(created_at: DateTime<Utc>) -> Self {
        Self {
            ease: DEFAULT_EASE,
            interval_days: 0,
            due_at: created_at,
        }
    }

    /// Whether the card is in the study queue at `now`.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.due_at <= now
    }
}

/// Replays `reviews` onto a card created at `created_at`.
///
/// Sorts a copy of the slice, so the caller's order is irrelevant and two devices
/// that received the reviews in different orders agree.
pub fn replay(created_at: DateTime<Utc>, reviews: &[Review]) -> CardState {
    let mut ordered: Vec<&Review> = reviews.iter().collect();
    ordered.sort_by(|left, right| {
        left.reviewed_at
            .cmp(&right.reviewed_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut state = CardState::new(created_at);
    let mut repetitions: i32 = 0;

    for review in ordered {
        let quality = review.rating.quality();

        if quality < 3 {
            // A failure sends the card back to the start of the ladder rather than
            // to zero: it is due again tomorrow, which is what "relearn" means.
            repetitions = 0;
            state.interval_days = FIRST_INTERVAL_DAYS;
        } else {
            repetitions += 1;
            state.interval_days = match repetitions {
                1 => FIRST_INTERVAL_DAYS,
                2 => SECOND_INTERVAL_DAYS,
                _ => ((state.interval_days as f32) * state.ease).round().max(1.0) as i32,
            };
        }

        // Applied on every review, including a failure: SM-2 lowers ease on a
        // lapse, and a card that never loses ease would grow without bound.
        let q = quality as f32;
        state.ease = (state.ease + 0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02)).max(MIN_EASE);

        state.due_at = review.reviewed_at + Duration::days(state.interval_days as i64);
    }

    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_rating_is_rejected_rather_than_defaulted() {
        assert!(Rating::parse("Good").is_err());
        assert!(Rating::parse("").is_err());
        assert_eq!(Rating::parse("again").unwrap(), Rating::Again);
    }

    #[test]
    fn every_rating_round_trips_through_its_wire_form() {
        for rating in [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy] {
            assert_eq!(Rating::parse(rating.as_str()).unwrap(), rating);
        }
    }
}
