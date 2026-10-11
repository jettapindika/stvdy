//! Sources: upload, ingestion lifecycle, status, and the original-file URL.
//!
//! The server owns `status`, `file_key` and `read_only` (roadmap § Sync protocol:
//! a client must not be able to mark a source `ready` or bypass the storage
//! quota), so every field a client could abuse is set here and nowhere else.

pub mod parse;
pub mod routes;
pub mod status;

pub use routes::routes;
