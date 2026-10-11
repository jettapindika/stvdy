//! The clock, behind `stvdy_core::traits::clock::Clock`.
//!
//! Injected rather than read directly so the planner, SM-2 replay and the
//! entitlement counters can be tested against a fixed instant.

use chrono::{DateTime, Utc};
use stvdy_core::traits::clock::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
