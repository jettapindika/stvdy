//! The client engine's shared pieces.
//!
//! Task 5 landed the conflict table, because the server's apply is its first reader and the
//! point of the table is that there is one copy (A175). Task 8 adds the engine, the outbox
//! and the local apply to this module: the pass the Tauri shell runs over `rusqlite`, and
//! the same pass the convergence suite runs over the doubles in [`testing`] (A182).

pub mod apply;
pub mod conflict;
pub mod engine;
pub mod outbox;
pub mod store;
pub mod testing;
pub mod transport;

pub use apply::{ChangeSet, plan};
pub use conflict::{Rule, rule_for};
pub use engine::{Engine, SyncState};
pub use outbox::Retry;
pub use store::{ApplyReport, LocalStore, StoreError};
pub use transport::{Transport, TransportError};
