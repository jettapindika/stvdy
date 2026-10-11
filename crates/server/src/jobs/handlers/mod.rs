//! Job handlers. One module per kind; `jobs::worker::dispatch` is the only
//! caller.

pub mod generate;
pub mod ingest;
