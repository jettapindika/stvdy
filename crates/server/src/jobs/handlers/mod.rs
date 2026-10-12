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
pub mod plan;
pub mod poll_payment;
pub mod purge_accounts;
pub mod reconcile_payments;
pub mod reminders;
pub mod render_pdf;
pub mod renewal;
pub mod report;
