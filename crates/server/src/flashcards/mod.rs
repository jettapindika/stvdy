//! Decks, cards and reviews.
//!
//! Split three ways: `routes` owns the HTTP surface, `generate` owns the prompt and
//! the insert for one generation, and the review path lives in `routes` because it
//! is short and is the only place that replays SM-2 in a transaction.

pub mod generate;
pub mod routes;

pub use routes::routes;
