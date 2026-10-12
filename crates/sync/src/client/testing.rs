//! Deterministic doubles for the engine's two traits (A182).
//!
//! Public, not `#[cfg(test)]`, because two suites share them: the engine's own
//! (`crates/sync/tests/engine.rs`) and the convergence suite
//! (`crates/server/tests/sync_convergence.rs`), which drives the real routes through the
//! [`Transport`] trait. A second copy of either double would let the two suites disagree
//! about what a store or a transport does, and the convergence suite's whole claim is that
//! it exercises the same engine the shell runs.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::proto::{
    Change, Entity, Mutation, MutationResult, Op, PullQuery, PullResponse, PushRequest,
    PushResponse,
};

use super::engine::SyncState;
use super::store::{ApplyReport, LocalStore, StoreError};
use super::transport::{Transport, TransportError};

/// The `GET /sync/pull` URI for a query.
///
/// The shell's transport and the convergence suite's both build their request from this,
/// so a transport cannot ask a subtly different question than the one the tests ask — and
/// a transport that forgot `offset` would re-request the first page of a pass for ever.
pub fn pull_uri(query: PullQuery) -> String {
    format!(
        "/sync/pull?cursor={}&offset={}&limit={}",
        query.cursor, query.offset, query.limit
    )
}

/// One `change_log` row as the pull ships it: an upsert carrying the row.
pub fn upsert(entity: Entity, id: Uuid, seq: i64, row: Value) -> Change {
    Change {
        seq,
        entity,
        id,
        op: Op::Upsert,
        data: Some(row),
    }
}

/// One `change_log` row as the pull ships a tombstone: no payload.
pub fn delete(entity: Entity, id: Uuid, seq: i64) -> Change {
    Change {
        seq,
        entity,
        id,
        op: Op::Delete,
        data: None,
    }
}

/// An `upsert` change whose payload is `null`.
///
/// This is the wire shape of a row this device cannot see (A188): the change log names the
/// row, and the payload loader finds nothing to ship. It deserialises to no payload at
/// all, which is why it is its own constructor rather than `upsert(.., Value::Null)`.
pub fn upsert_without_row(entity: Entity, id: Uuid, seq: i64) -> Change {
    Change {
        seq,
        entity,
        id,
        op: Op::Upsert,
        data: None,
    }
}

/// One pull page.
pub fn page(changes: Vec<Change>, next_cursor: i64, has_more: bool) -> PullResponse {
    PullResponse {
        changes,
        next_cursor,
        has_more,
    }
}

/// A push mutation for a `Task` or an `Event`: the patch plus the row it was edited from
/// (A176), with `base: null` meaning create.
pub fn patch_mutation(entity: Entity, id: Uuid, patch: Value, base: Option<Value>) -> Mutation {
    Mutation {
        mutation_id: Uuid::now_v7(),
        entity,
        op: Op::Upsert,
        id,
        payload: Some(serde_json::json!({ "patch": patch, "base": base })),
        client_ts: Utc::now(),
    }
}

/// A push mutation that carries the whole row (every pushable entity but a task or an
/// event).
pub fn row_mutation(entity: Entity, id: Uuid, row: Value) -> Mutation {
    Mutation {
        mutation_id: Uuid::now_v7(),
        entity,
        op: Op::Upsert,
        id,
        payload: Some(row),
        client_ts: Utc::now(),
    }
}

/// A tombstone.
pub fn delete_mutation(entity: Entity, id: Uuid) -> Mutation {
    Mutation {
        mutation_id: Uuid::now_v7(),
        entity,
        op: Op::Delete,
        id,
        payload: None,
        client_ts: Utc::now(),
    }
}

/// An in-memory [`LocalStore`]: one map of rows, one outbox, one cursor.
///
/// It models the two things a SQLite store does that the engine depends on — a page lands
/// atomically, and a child whose parent is missing is deferred rather than dropped — and
/// it counts its transactions, so "one transaction per page" is asserted rather than
/// assumed.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    /// `(entity, id) -> (seq, row)`. The seq is what lets a convergence assertion compare
    /// two devices' rows *at the same seq*, not merely their content.
    rows: HashMap<(Entity, Uuid), (i64, Value)>,
    outbox: Vec<Mutation>,
    /// The rows the last `apply_page` could not link to a parent, kept for the next call.
    deferred: Vec<Change>,
    /// Every seq this store has ever applied, including before a wipe: an isolation
    /// assertion wants what the device ever held, not only what it holds now.
    applied_seqs: Vec<i64>,
    /// The outcomes the engine resolved, in the order it resolved them.
    resolved: Vec<(Uuid, MutationResult)>,
    /// How many times `apply_page` ran: one page, one transaction.
    transactions: usize,
    cursor: i64,
    last_synced_at: Option<DateTime<Utc>>,
    last_state: Option<SyncState>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The row, as the app would read it while offline.
    pub fn row(&self, entity: Entity, id: Uuid) -> Option<&Value> {
        self.rows.get(&(entity, id)).map(|(_, row)| row)
    }

    /// Every row, sorted, with its seq: two devices have converged when these are equal.
    pub fn dump(&self) -> Vec<(String, Uuid, i64, Value)> {
        let mut rows: Vec<(String, Uuid, i64, Value)> = self
            .rows
            .iter()
            .map(|((entity, id), (seq, row))| {
                (entity.table_name().to_string(), *id, *seq, row.clone())
            })
            .collect();
        rows.sort_by(|left, right| (left.0.as_str(), left.1).cmp(&(right.0.as_str(), right.1)));
        rows
    }

    /// Every seq this store has ever applied, in application order.
    pub fn applied_seqs(&self) -> &[i64] {
        &self.applied_seqs
    }

    /// The outcomes the engine resolved, in order.
    pub fn resolved(&self) -> &[(Uuid, MutationResult)] {
        &self.resolved
    }

    /// How many `apply_page` calls have run.
    pub fn transactions(&self) -> usize {
        self.transactions
    }

    /// The rows still waiting for a parent.
    pub fn deferred(&self) -> &[Change] {
        &self.deferred
    }

    /// Writes a row locally, as the shell's local-first write path does before it queues
    /// the mutation. Not part of [`LocalStore`]: the engine never writes a row itself, and
    /// a store that offered this to the engine would be inviting it to.
    pub fn write_local(&mut self, entity: Entity, id: Uuid, row: Value) {
        let key = (entity, id);
        let seq = self.rows.get(&key).map_or(0, |(seq, _)| *seq);
        self.rows.insert(key, (seq, row));
    }

    /// Removes a row and every descendant of it this store holds.
    ///
    /// The `delete` change carries the parent and not its children, and the children it has
    /// to take with it are the ones already in the cache — so the tree is read from the
    /// store rather than from the page. The relation is [`parents_of`]'s, the same one
    /// `linked` defers on, so the store cannot disagree with the apply about what nests
    /// inside what, and an entity that does not nest (an event, whose `parents_of` names no
    /// row of its own kind) is a single removal.
    fn remove_nested(&mut self, key: (Entity, Uuid)) {
        self.rows.remove(&key);
        let (entity, id) = key;
        let children: Vec<(Entity, Uuid)> = self
            .rows
            .iter()
            .filter(|((child_entity, _), (_, row))| {
                *child_entity == entity
                    && parents_of(*child_entity, Some(row)).contains(&(entity, id))
            })
            .map(|(key, _)| *key)
            .collect();
        for child in children {
            self.remove_nested(child);
        }
    }

    /// Splits a queue into the rows this page can write and the rows still missing a
    /// parent, to a fixed point: a child whose parent is itself deferred is deferred too.
    fn settle(&self, queue: Vec<Change>) -> (Vec<Change>, Vec<Change>) {
        let mut writable = queue;
        let mut blocked: Vec<Change> = Vec::new();
        loop {
            let present: HashSet<(Entity, Uuid)> = writable
                .iter()
                .map(|change| (change.entity, change.id))
                .collect();
            let mut next: Vec<Change> = Vec::with_capacity(writable.len());
            let mut moved = false;
            for change in writable {
                if self.linked(&change, &present) {
                    next.push(change);
                } else {
                    blocked.push(change);
                    moved = true;
                }
            }
            writable = next;
            if !moved || writable.is_empty() {
                return (writable, blocked);
            }
        }
    }

    /// Whether every non-null parent of a row is either stored or written by this page.
    fn linked(&self, change: &Change, present: &HashSet<(Entity, Uuid)>) -> bool {
        parents_of(change.entity, change.data.as_ref())
            .into_iter()
            .all(|(entity, id)| {
                self.rows.contains_key(&(entity, id)) || present.contains(&(entity, id))
            })
    }
}

/// The synced parents a row points at, mirroring the schema's foreign keys.
///
/// A parent the protocol does not sync — `chunks`, `calendar_connections`, `devices` — is
/// deliberately absent: a store that deferred every row naming one would never write it.
fn parents_of(entity: Entity, data: Option<&Value>) -> Vec<(Entity, Uuid)> {
    let named = |column: &str| -> Option<Uuid> {
        data?
            .get(column)?
            .as_str()
            .and_then(|id| Uuid::parse_str(id).ok())
    };
    let pairs: Vec<(Entity, Option<Uuid>)> = match entity {
        Entity::Workspace | Entity::UserSettings | Entity::PendingAction => Vec::new(),
        Entity::Source
        | Entity::Message
        | Entity::Deck
        | Entity::Quiz
        | Entity::Report
        | Entity::Cheatsheet
        | Entity::Goal => vec![(Entity::Workspace, named("workspace_id"))],
        Entity::Flashcard => vec![(Entity::Deck, named("deck_id"))],
        Entity::FlashcardReview => vec![(Entity::Flashcard, named("flashcard_id"))],
        Entity::QuizQuestion => vec![(Entity::Quiz, named("quiz_id"))],
        Entity::QuizAttempt => vec![
            (Entity::Quiz, named("quiz_id")),
            (Entity::QuizQuestion, named("question_id")),
        ],
        Entity::Task => vec![
            (Entity::Workspace, named("workspace_id")),
            (Entity::Task, named("parent_id")),
        ],
        Entity::Event => vec![
            (Entity::Workspace, named("workspace_id")),
            (Entity::Task, named("task_id")),
        ],
        Entity::StudySession => vec![
            (Entity::Workspace, named("workspace_id")),
            (Entity::Task, named("task_id")),
        ],
    };
    pairs
        .into_iter()
        .filter_map(|(entity, id)| id.map(|id| (entity, id)))
        .collect()
}

impl LocalStore for InMemoryStore {
    fn cursor(&self) -> Result<i64, StoreError> {
        Ok(self.cursor)
    }

    fn set_cursor(&mut self, cursor: i64) -> Result<(), StoreError> {
        self.cursor = cursor;
        Ok(())
    }

    fn apply_page(&mut self, changes: &[Change]) -> Result<ApplyReport, StoreError> {
        // One transaction. The counter is what lets a test assert that rather than assume
        // it.
        self.transactions += 1;

        // Every row of the page is written before any foreign key is checked, which is what
        // `PRAGMA defer_foreign_keys = ON` buys the SQLite store: a child that arrives
        // before its parent in the same page still lands.
        let mut queue: Vec<Change> = std::mem::take(&mut self.deferred);
        queue.extend(changes.iter().cloned());
        let (writable, blocked) = self.settle(queue);

        for change in &writable {
            self.applied_seqs.push(change.seq);
            let key = (change.entity, change.id);
            let newer = self
                .rows
                .get(&key)
                .is_some_and(|(seq, _)| *seq > change.seq);
            match change.op {
                // A188's normalisation lives in `apply::plan`; an upsert without a payload
                // reaching a store can only be a caller that skipped it, and the safe
                // reading of a row this device cannot see is the same: it is not here.
                Op::Upsert => match &change.data {
                    Some(row) if !newer => {
                        self.rows.insert(key, (change.seq, row.clone()));
                    }
                    Some(_) => {}
                    None => {
                        self.rows.remove(&key);
                    }
                },
                Op::Delete => {
                    if !newer {
                        self.remove_nested(key);
                    }
                }
            }
        }

        let report = ApplyReport {
            applied: writable.len(),
            deferred: blocked
                .iter()
                .map(|change| (change.entity, change.id))
                .collect(),
        };
        self.deferred = blocked;
        Ok(report)
    }

    fn enqueue(&mut self, mutation: Mutation) -> Result<(), StoreError> {
        self.outbox.push(mutation);
        Ok(())
    }

    fn pending(&self, limit: usize) -> Result<Vec<Mutation>, StoreError> {
        Ok(self.outbox.iter().take(limit).cloned().collect())
    }

    fn resolve(&mut self, mutation_id: Uuid, result: &MutationResult) -> Result<(), StoreError> {
        if let Some(at) = self
            .outbox
            .iter()
            .position(|mutation| mutation.mutation_id == mutation_id)
        {
            let mutation = self.outbox.remove(at);
            if let MutationResult::Conflict { winner } = result {
                // The server's row wins (A178), including its `deleted_at`: that is how the
                // device that edited a tombstoned row learns what the account holds.
                let key = (mutation.entity, mutation.id);
                let tombstoned = winner
                    .get("deleted_at")
                    .is_some_and(|deleted| !deleted.is_null());
                if tombstoned {
                    // The row the server won with is gone, and this store holds the rows the
                    // account still has — so the same write a `delete` change makes.
                    self.rows.remove(&key);
                } else if let Some(object) = winner.as_object() {
                    let seq = self.rows.get(&key).map_or(0, |(seq, _)| *seq);
                    self.rows.insert(key, (seq, Value::Object(object.clone())));
                }
            }
        }
        self.resolved.push((mutation_id, result.clone()));
        Ok(())
    }

    fn outbox_len(&self) -> Result<usize, StoreError> {
        Ok(self.outbox.len())
    }

    fn last_synced_at(&self) -> Result<Option<DateTime<Utc>>, StoreError> {
        Ok(self.last_synced_at)
    }

    fn set_last_synced_at(&mut self, at: DateTime<Utc>) -> Result<(), StoreError> {
        self.last_synced_at = Some(at);
        Ok(())
    }

    fn last_state(&self) -> Result<Option<SyncState>, StoreError> {
        Ok(self.last_state.clone())
    }

    fn set_last_state(&mut self, state: SyncState) -> Result<(), StoreError> {
        self.last_state = Some(state);
        Ok(())
    }

    fn wipe(&mut self) -> Result<(), StoreError> {
        self.rows.clear();
        self.outbox.clear();
        self.deferred.clear();
        self.cursor = 0;
        self.last_synced_at = None;
        self.last_state = None;
        Ok(())
    }
}

/// A `Transport` whose answers the test hands it, in order.
///
/// When a script runs out, the call fails with [`TransportError::Network`]: visible, and
/// harmless — a test that forgot to script an answer fails its own assertions instead of
/// silently succeeding against a made-up one.
#[derive(Debug, Clone, Default)]
pub struct ScriptedTransport {
    inner: Arc<Mutex<Script>>,
}

#[derive(Debug, Default)]
struct Script {
    pulls: VecDeque<Result<PullResponse, TransportError>>,
    pushes: VecDeque<Result<PushResponse, TransportError>>,
    queries: Vec<PullQuery>,
    requests: Vec<PushRequest>,
}

impl ScriptedTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers the next `pull` with this page.
    pub fn answer_pull(&self, response: PullResponse) {
        self.lock().pulls.push_back(Ok(response));
    }

    /// Fails the next `pull` with this error.
    pub fn fail_pull(&self, error: TransportError) {
        self.lock().pulls.push_back(Err(error));
    }

    /// Answers the next `push` with these results.
    pub fn answer_push(&self, response: PushResponse) {
        self.lock().pushes.push_back(Ok(response));
    }

    /// Fails the next `push` with this error.
    pub fn fail_push(&self, error: TransportError) {
        self.lock().pushes.push_back(Err(error));
    }

    /// The queries this transport was asked, in order.
    pub fn queries(&self) -> Vec<PullQuery> {
        self.lock().queries.clone()
    }

    /// The push requests it carried, in order.
    pub fn requests(&self) -> Vec<PushRequest> {
        self.lock().requests.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Script> {
        // A double's mutex cannot be poisoned in a way a test should panic on: recovering
        // the data keeps the assertion's failure the one the test reports.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait::async_trait]
impl Transport for ScriptedTransport {
    async fn pull(&self, query: PullQuery) -> Result<PullResponse, TransportError> {
        let mut script = self.lock();
        script.queries.push(query);
        script.pulls.pop_front().unwrap_or_else(|| {
            Err(TransportError::Network(
                "the scripted transport has no pull answer left".to_string(),
            ))
        })
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        let mut script = self.lock();
        script.requests.push(request);
        script.pushes.pop_front().unwrap_or_else(|| {
            Err(TransportError::Network(
                "the scripted transport has no push answer left".to_string(),
            ))
        })
    }
}
