//! Job handlers. One module per kind; `jobs::worker::dispatch` is the only
//! caller.

pub mod calendar;
pub mod cheatsheet;
pub mod compact;
pub mod counters;
pub mod generate;
pub mod generate_quiz;
pub mod grants;
pub mod ingest;
pub mod render_pdf;
pub mod report;
