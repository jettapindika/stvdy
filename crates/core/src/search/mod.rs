//! Search ranking, URL normalisation and cache keys.
//!
//! Pure functions only: no IO, no clock, no database. The composite in the server
//! calls these, and the tests assert determinism, which is what makes two runs of
//! the same query return the same order.

pub mod normalize;
pub mod rank;
