//! The client engine's shared pieces.
//!
//! Task 5 lands the conflict table, because the server's apply is its first reader and the
//! point of the table is that there is one copy (A175). Task 8 adds the engine, the
//! outbox and the local apply to this module.

pub mod conflict;

pub use conflict::{Rule, rule_for};
