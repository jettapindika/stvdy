//! Quizzes: generation, attempts, and the derived score.
//!
//! Split the same three ways as `flashcards`: `routes` owns the HTTP surface,
//! `generate` owns the prompt and the one retry, and the attempt path lives in
//! `routes` because it is short. The score is never stored: it is computed from the
//! attempts at read time, which is what makes attempts from several devices
//! conflict-free.

pub mod generate;
pub mod routes;

pub use routes::routes;
