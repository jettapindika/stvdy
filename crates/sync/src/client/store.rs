//! What the engine needs from a local database — and nothing more.
//!
//! The engine is generic over this trait so one implementation of the pass runs over
//! `rusqlite` in the Tauri shell and over a `HashMap` in the convergence suite (A182).
//! Nothing here names SQL, a file or a transaction API: "a page lands atomically" is the
//! requirement, and how a store achieves it is the store's business. The trait is
//! deliberately small for the same reason — a method the engine never calls would be a
//! method every implementation has to write for nothing.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::proto::{Change, Entity, Mutation, MutationResult};

use super::engine::SyncState;

/// A local store failure.
///
/// Opaque on purpose: the engine's only reaction to one is to abandon the pass and hand
/// it back, and a taxonomy of storage failures here would be the engine learning about
/// SQLite. An implementation maps its own error in with [`StoreError::backend`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("the local store failed: {0}")]
    Backend(String),
}

impl StoreError {
    /// Wraps an implementation's own error.
    pub fn backend(error: impl std::fmt::Display) -> Self {
        Self::Backend(error.to_string())
    }
}

/// What one page did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Rows written by this call, including any deferred row it retried and landed.
    pub applied: usize,
    /// Rows whose parent is still missing once the whole page is in. The store keeps
    /// them and retries them on the next call, so a child that arrives before its parent
    /// is deferred rather than dropped — the roadmap's rule.
    pub deferred: Vec<(Entity, Uuid)>,
}

/// The local database, as the engine sees it.
pub trait LocalStore: Send {
    /// The last seq this device has applied, or 0 for a device that has never synced.
    fn cursor(&self) -> Result<i64, StoreError>;

    /// Moves the cursor. The engine only ever moves it over a prefix it has applied, and
    /// never past the anchor of an unfinished initial pass (A164).
    fn set_cursor(&mut self, cursor: i64) -> Result<(), StoreError>;

    /// Applies one page of changes **atomically**, in the order given.
    ///
    /// One transaction per page, with foreign keys deferred to its end: a page whose
    /// child arrives before its parent must still land, and a page that fails must leave
    /// nothing behind. A row whose parent is still missing once the page is in is
    /// reported in [`ApplyReport::deferred`] and kept for the next call, where it is
    /// retried — and a retry never overwrites a newer version of the row it names.
    ///
    /// A `delete` change removes the row: the cache holds the rows the account still has,
    /// and a tombstone carries no row to store.
    fn apply_page(&mut self, changes: &[Change]) -> Result<ApplyReport, StoreError>;

    /// Queues a mutation for the next pass, in enqueue order.
    fn enqueue(&mut self, mutation: Mutation) -> Result<(), StoreError>;

    /// The queued mutations, oldest first, at most `limit`.
    fn pending(&self, limit: usize) -> Result<Vec<Mutation>, StoreError>;

    /// Records the server's answer for one mutation and removes it from the outbox.
    ///
    /// A rejection is kept with its code, because the row it describes is not saved and
    /// the app has to be able to say why (gate 7(d)). A `Conflict` carries the server's
    /// row, which is the row this device must now hold (A178).
    fn resolve(&mut self, mutation_id: Uuid, result: &MutationResult) -> Result<(), StoreError>;

    /// How many mutations are still queued. Zero is one of the facts `Synced` needs.
    fn outbox_len(&self) -> Result<usize, StoreError>;

    /// When this device's last pass completed, from the device's own clock, or `None`
    /// before the first one. Never a server time, and never a fresh clock read (A187).
    fn last_synced_at(&self) -> Result<Option<DateTime<Utc>>, StoreError>;

    fn set_last_synced_at(&mut self, at: DateTime<Utc>) -> Result<(), StoreError>;

    /// The last pass's outcome, persisted so `status()` reads a stored fact rather than an
    /// in-memory guess (A187). `None` before the first pass.
    ///
    /// The engine writes only terminal states; `Syncing` is derived, never stored, so a
    /// process killed mid-pass does not come back claiming to be mid-pass.
    fn last_state(&self) -> Result<Option<SyncState>, StoreError>;

    fn set_last_state(&mut self, state: SyncState) -> Result<(), StoreError>;

    /// Empties the cache **and the outbox**, and resets the cursor and the stored state.
    ///
    /// Both callers are load-bearing (A186): a 401 means this device's session is gone,
    /// and a pending offline write from a revoked device must not be replayed by the next
    /// account that signs in on this machine; a 410 means the cursor is older than
    /// retained history and the cache is rebuilt from zero (A166).
    ///
    /// Note what the second caller therefore does: a resync drops the queued writes too,
    /// because A166's reaction is this one call and the outbox is part of what it empties.
    /// A 410 needs the device to have been offline past the retention window (90 days by
    /// default), so the window for losing an unsent write is narrow — but it is real, and
    /// splitting this into "clear the cache" and "clear everything" is the change that
    /// would close it.
    fn wipe(&mut self) -> Result<(), StoreError>;
}
