//! Reports: an editable outline, then per-section generation with citations.
//!
//! Split the same way `quizzes` is: `routes` owns the HTTP surface, `generate` owns the
//! prompt and the one retry, and the job shell in `jobs::handlers::report` owns the
//! transaction boundary and the refund.
//!
//! The one thing that is *not* like the other generators is the shape of the work. The
//! outline is one short call whose result the student edits before anything else happens,
//! so it is generated synchronously in its route (decision 12) and there is no
//! `generate_report_outline` job — the roadmap's job table is a fixed list that does not
//! contain one. A section, by contrast, is per-index, retryable and long enough to outlive
//! a request, so it *is* a job (`generate_report_section`).

pub mod generate;
pub mod render;
pub mod routes;

pub use routes::routes;
