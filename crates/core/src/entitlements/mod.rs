//! The entitlements engine: a [`UsageSnapshot`] in, a verdict out.
//!
//! Pure: no database, no clock, no IO. The matrix it enforces is
//! `config/plans.toml`, loaded once at startup and passed to
//! [`check_with`] by the caller rather than read here, so the engine can be
//! tested against a scratch config and no second answer to "what does this plan
//! allow" can exist.
//!
//! The server's half — building the snapshot, turning an
//! [`EntitlementError`] into an `AppError` — lives in `crates/server`, which is
//! the only thing that talks to a database.

mod check;
mod feature;
mod usage;

pub use check::{EntitlementError, check_with};
pub use feature::Feature;
pub use usage::{Credits, PlanFacts, SubStatus, UsageSnapshot};
