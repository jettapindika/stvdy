//! Time capability.

use chrono::{DateTime, Utc};

/// The only source of "now" in the domain.
///
/// The planner and SM-2 replay read the clock through this trait so tests can
/// pin time instead of racing it.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
