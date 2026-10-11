//! Job handlers. One module per kind; `jobs::worker::dispatch` is the only
//! caller.

pub mod cheatsheet;
pub mod generate;
pub mod generate_quiz;
pub mod ingest;
pub mod render_pdf;
pub mod report;
