//! The pass: drain the outbox, pull every page, and say what happened.
//!
//! One `sync_once` is one device's whole conversation with the server, and the order
//! inside it is deliberate. The outbox goes first, because a device with local writes
//! should have them accepted before it pulls a history that may already contain a
//! competing edit. Then the pull, page by page, each page in one transaction. Only then do
//! the cursor and the timestamp move, so a pass that is interrupted leaves the cursor
//! where the next pass can resume from it (A164).
//!
//! The engine never decides whether a mutation may be applied. That is the server's
//! (`crate::client::conflict` is the table both ends read, and `decide` is Phase 7's); this
//! side applies what it is told arrived and reports what was refused.

use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::proto::{DEFAULT_LIMIT, ErrorCode, Mutation, MutationResult, PullQuery, PushRequest};

use super::apply;
use super::outbox::{self, Retry};
use super::store::{LocalStore, StoreError};
use super::transport::{Transport, TransportError};

/// How many pages one pass reads before it stops and lets the next pass continue.
///
/// It bounds one pass's work — a server that answered `has_more: true` for ever must not
/// hang the app — and it sits far above a real account's history, because an initial pass
/// that stops early has to start again from offset 0 (A164). An incremental pass commits
/// its cursor page by page, so for it the budget is a checkpoint rather than a restart.
const MAX_PAGES_PER_PASS: usize = 500;

/// What the app shows about sync (A187), derived from stored facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SyncState {
    /// The outbox is empty and the last pass reached the server.
    Synced,
    /// Work is outstanding: a write is queued, or no pass has completed yet.
    Syncing,
    /// The last pass failed transiently — the network, a rate limit, a server fault.
    /// Waiting is the remedy, and the engine retries on a bounded backoff.
    Offline,
    /// The last pass left a mutation the server refused, with the server's own code.
    NeedsAttention { code: String },
    /// This device's session is gone and its local state was wiped (A186).
    Revoked,
}

/// The engine: a local store, a transport, and this device's identity.
pub struct Engine<S: LocalStore, T: Transport> {
    store: S,
    transport: T,
    device_id: Uuid,
    retry: Retry,
}

impl<S: LocalStore, T: Transport> Engine<S, T> {
    /// `device_id` is this device's own row, which the push route checks against the
    /// authenticated device (A171) and the triggers record as `origin_device_id`.
    pub fn new(store: S, transport: T, device_id: Uuid) -> Self {
        Self {
            store,
            transport,
            device_id,
            retry: Retry::new(),
        }
    }

    /// The local store, for the reads a screen makes while offline.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The local store, mutably, for the local-first write path: the shell writes the row
    /// and then queues the mutation. The engine never writes a row itself.
    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// Queues a local write for the next pass.
    pub fn enqueue(&mut self, mutation: Mutation) -> Result<(), StoreError> {
        self.store.enqueue(mutation)
    }

    /// The status, from stored facts only (A187): the last pass's outcome, the outbox's
    /// length, and the timestamp the last successful pass wrote. Nothing here reads a
    /// clock or guesses, which is what makes "Synced" a claim the app can defend.
    pub fn status(&self) -> Result<SyncState, StoreError> {
        let state = self.store.last_state()?;
        let outstanding = self.store.outbox_len()? > 0 || self.store.last_synced_at()?.is_none();
        Ok(match state {
            Some(SyncState::Revoked) => SyncState::Revoked,
            Some(SyncState::Offline) => SyncState::Offline,
            Some(SyncState::NeedsAttention { code }) => SyncState::NeedsAttention { code },
            // A pass that reached the server but a queued write is still waiting, or a
            // device that has never completed one: work is outstanding, so the honest
            // word is `Syncing` and never `Synced`.
            _ if outstanding => SyncState::Syncing,
            _ => SyncState::Synced,
        })
    }

    /// How long to wait before the next pass. Zero after a pass that reached the server;
    /// growing, capped, after a transient failure. The caller sleeps this, so a flapping
    /// network does not become a hot loop against the server.
    pub fn next_delay(&self) -> Duration {
        self.retry.delay()
    }

    /// One pass: the outbox, then every pull page, then the facts the status reads.
    ///
    /// A transport failure is not an error the caller has to handle — it is a state the
    /// app shows ([`SyncState::Offline`], or `NeedsAttention` with the server's code) —
    /// because each one has a remedy the engine applies itself: a revoked device is wiped,
    /// a stale cursor resyncs, a transient failure backs off. The `Result` is for the local
    /// store, which the caller can do nothing about either, but which is a bug rather than
    /// a state.
    pub async fn sync_once(&mut self) -> Result<SyncState, StoreError> {
        let state = self.run_pass().await?;
        self.store.set_last_state(state.clone())?;
        Ok(state)
    }

    async fn run_pass(&mut self) -> Result<SyncState, StoreError> {
        // 1. The outbox, one push per batch, in enqueue order.
        let mut attention: Option<String> = None;
        loop {
            let pending = self.store.pending(outbox::BATCH)?;
            if pending.is_empty() {
                break;
            }
            let batch = outbox::next_batch(&pending, outbox::BATCH).to_vec();
            let results = match self
                .transport
                .push(PushRequest {
                    device_id: self.device_id,
                    mutations: batch.clone(),
                })
                .await
            {
                Ok(response) => response.results,
                Err(error) => return self.settle_failure(error),
            };

            // One result per mutation is the contract. A short answer leaves the outbox's
            // state unknown, so nothing is resolved and the batch is retried — the replay
            // is idempotent — rather than a guess being made about which mutation landed.
            if results.len() != batch.len() {
                return Ok(SyncState::NeedsAttention {
                    code: ErrorCode::Internal.as_str().to_string(),
                });
            }

            for (mutation, result) in batch.iter().zip(&results) {
                self.store.resolve(mutation.mutation_id, result)?;
                if let Some(code) = attention_code(result) {
                    attention = Some(code);
                }
            }
        }

        // 2. The pull. The initial pass is anchored and paged by offset; the incremental
        //    walk climbs the seq clock (A164).
        let mut cursor = self.store.cursor()?;
        let mut initial = cursor == 0;
        let mut offset = 0i64;
        let mut pages = 0usize;
        let mut restarted = false;
        loop {
            if pages >= MAX_PAGES_PER_PASS {
                break;
            }
            pages += 1;

            let page = match self
                .transport
                .pull(PullQuery {
                    cursor,
                    offset,
                    limit: DEFAULT_LIMIT,
                })
                .await
            {
                Ok(page) => page,
                Err(TransportError::Stale) if !restarted => {
                    // The cursor is older than retained history: wipe, restart the pull
                    // from zero, and finish the resync in this same pass (A166). The
                    // spec's "the client does a full resync" is not an error the user
                    // sees, so nothing is surfaced.
                    restarted = true;
                    self.store.wipe()?;
                    cursor = 0;
                    offset = 0;
                    initial = true;
                    pages = 0;
                    continue;
                }
                Err(error) => return self.settle_failure(error),
            };

            self.store.apply_page(apply::plan(&page.changes).writes())?;

            if !page.has_more {
                // The pass is complete. This is the only point at which the cursor may
                // move past an initial pass's pages (A164).
                self.store.set_cursor(page.next_cursor)?;
                break;
            }
            if page.changes.is_empty() {
                // A page that claims more while carrying nothing cannot make progress; the
                // cursor stays put rather than advancing over rows nothing was applied for.
                break;
            }

            cursor = page.next_cursor;
            if initial {
                offset += page.changes.len() as i64;
            } else {
                // An incremental page is complete in itself, so its cursor can commit now:
                // a pass that hits the page budget resumes where it stopped.
                self.store.set_cursor(page.next_cursor)?;
            }
        }

        // 3. The pass reached the server and its pages are applied. `last_synced_at` is the
        //    local clock at that moment — a time the app computed, never one it invented.
        self.store.set_last_synced_at(Utc::now())?;
        self.retry.reset();
        Ok(match attention {
            Some(code) => SyncState::NeedsAttention { code },
            None => SyncState::Synced,
        })
    }

    /// The state a pass ends in when the transport failed, plus the backoff it leaves
    /// behind.
    ///
    /// A transient failure — the network, a rate limit, a server fault — is `Offline`, and
    /// the remedy is to try again, so the backoff grows. A refusal the request cannot fix
    /// is `NeedsAttention` with the server's own code (gate 7(d)), and the backoff resets,
    /// because retrying would only repeat it.
    ///
    /// A mutation rejected earlier in the same pass is not carried into this state: the
    /// store already recorded it, so the row is marked unsaved where the app reads it, and
    /// what the user needs from a pass that ended here is that the device is offline.
    fn settle_failure(&mut self, error: TransportError) -> Result<SyncState, StoreError> {
        match error {
            TransportError::Network(_) => {
                self.retry.advance();
                Ok(SyncState::Offline)
            }
            TransportError::Server { status, .. } if status == 429 || status >= 500 => {
                self.retry.advance();
                Ok(SyncState::Offline)
            }
            TransportError::Server { code, .. } => {
                self.retry.reset();
                Ok(SyncState::NeedsAttention { code })
            }
            // A revoked device is wiped, not merely refused. The outbox goes with it: a
            // pending offline write from a revoked device must not be replayed by the next
            // account that signs in on this machine (A186).
            TransportError::Revoked => {
                self.retry.reset();
                self.store.wipe()?;
                Ok(SyncState::Revoked)
            }
            // The 410 is handled inside the pull loop, where there is a cursor to reset. One
            // arriving here means a cursor of 0 was refused, which only a server fault can
            // do.
            TransportError::Stale => {
                self.retry.reset();
                Ok(SyncState::NeedsAttention {
                    code: ErrorCode::ResyncRequired.as_str().to_string(),
                })
            }
        }
    }
}

/// The code the status should carry for a mutation's result, or `None` if it was applied.
///
/// A conflict is the server saying the row it holds won (A178): the row is saved, but not
/// as this device asked, so the app is told — with the contract's own word for it.
fn attention_code(result: &MutationResult) -> Option<String> {
    match result {
        MutationResult::Applied { .. } => None,
        MutationResult::Rejected { code, .. } => Some(code.clone()),
        MutationResult::Conflict { .. } => Some(ErrorCode::Conflict.as_str().to_string()),
    }
}
