//! `SqliteStore`: the `LocalStore` the engine runs on, over the cache file.
//!
//! Two things this file does that the engine's in-memory double cannot, and both are the
//! reason the shell has its own store tests:
//!
//!   - **A page lands in one transaction with foreign keys deferred to its end.** The
//!     pragma is set on the connection *inside* that transaction and read back before
//!     anything is written, so a child that arrives before its parent in one page still
//!     lands — the roadmap's named mechanism for out-of-order arrivals, asserted rather
//!     than assumed. A row whose parent is missing altogether is kept in
//!     `deferred_changes` and retried at the start of the next page, so a device killed
//!     between the two resumes with it.
//!   - **The cache belongs to one account.** Rows are stamped with the account the store
//!     is bound to, and a payload that names a different account is refused rather than
//!     stored — a row for another account must never be *in* this file, which filtering on
//!     read would not achieve. Binding a different account wipes first, which is the other
//!     half of A186: a pending offline write from one account must not be replayed by the
//!     next one that signs in on this machine.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params_from_iter};
use serde_json::{Map, Value};
use stvdy_sync::client::{ApplyReport, LocalStore, StoreError, SyncState};
use stvdy_sync::proto::{Change, Entity, Mutation, MutationResult, Op, PayloadShape};
use uuid::Uuid;

use super::schema;

/// `?` on a SQLite failure, as the engine's opaque store error.
///
/// `StoreError` is `stvdy-sync`'s and deliberately has no `From` for a driver error — the
/// engine does not know about SQLite — so the conversion is named at every call site
/// instead of hidden in an impl this crate is not allowed to write (both types are foreign
/// here).
trait SqliteResult<T> {
    fn store(self) -> Result<T, StoreError>;
}

impl<T> SqliteResult<T> for rusqlite::Result<T> {
    fn store(self) -> Result<T, StoreError> {
        self.map_err(StoreError::backend)
    }
}

/// The cache, as the engine and the SPA's local-first path both see it.
///
/// `Clone`, because the shell hands one clone to the engine for a pass and keeps another
/// for the reads a screen makes while that pass runs: both go through the same connection
/// behind one mutex, so a read never sees a half-applied page.
#[derive(Clone)]
pub struct SqliteStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    conn: Connection,
    /// The account this cache belongs to, as read from `sync_state` and kept in step with
    /// it. `None` until a session configures the cache.
    account: Option<Uuid>,
    /// What `PRAGMA defer_foreign_keys` read back as inside the last apply transaction.
    defer_seen: Option<i64>,
}

impl SqliteStore {
    /// Wraps an open connection. The schema is the caller's business: [`super::open`] is
    /// what creates it.
    pub fn new(conn: Connection) -> Result<Self, StoreError> {
        let account = read_account(&conn)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                conn,
                account,
                defer_seen: None,
            })),
        })
    }

    /// Binds the cache to `account`, wiping it first if it belonged to another one.
    ///
    /// The shell calls this when a session is configured, with the account its access token
    /// names. It is not an authorization decision — the server authorizes every request —
    /// it is the local one: whose data is this file, and must it be emptied before the next
    /// account's rows arrive.
    pub fn bind(&self, account: Uuid) -> Result<(), StoreError> {
        let mut inner = self.lock();
        match inner.account {
            Some(stored) if stored == account => return Ok(()),
            Some(stored) => {
                tracing::warn!(
                    previous = %stored,
                    next = %account,
                    "the cache was another account's; wiping it before this one syncs"
                );
                super::wipe(&mut inner.conn).store()?;
            }
            None => {}
        }
        inner
            .conn
            .execute(
                "update sync_state set account = ?1 where id = 1",
                [account.to_string()],
            )
            .store()?;
        inner.account = Some(account);
        Ok(())
    }

    /// What `PRAGMA defer_foreign_keys` read back as inside the last
    /// [`LocalStore::apply_page`], or `None` before one has run.
    pub fn defer_foreign_keys_seen(&self) -> Option<i64> {
        self.lock().defer_seen
    }

    /// The connection, for the shell's own diagnostics and for a test that has to see what
    /// the store did to it. Not a second way to read the cache: reads go through
    /// [`Self::read`].
    pub fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.lock().conn)
    }

    /// The cached row, as JSON: one object, or `null` if the account does not hold it.
    ///
    /// With `id` absent, the entity's rows as `[{id, row}, …]` — the list an offline screen
    /// renders. The row carries the payload's own columns only; the cache's `account` and
    /// `seq` never leave the file.
    pub fn read(&self, entity: Entity, id: Option<Uuid>) -> Result<Value, StoreError> {
        let inner = self.lock();
        match id {
            Some(id) => Ok(row_value(&inner.conn, entity, id)?.unwrap_or(Value::Null)),
            None => {
                let sql = format!(
                    "select {} from \"{}\" order by id",
                    read_columns(entity)
                        .iter()
                        .map(|column| format!("\"{column}\""))
                        .collect::<Vec<_>>()
                        .join(", "),
                    entity.table_name()
                );
                let mut statement = inner.conn.prepare(&sql).store()?;
                let rows = statement
                    .query_map([], |row| row_to_value(entity, row))
                    .store()?;
                let mut list = Vec::new();
                for row in rows {
                    let row = row.store()?;
                    let id = row.get("id").cloned().unwrap_or(Value::Null);
                    list.push(serde_json::json!({ "id": id, "row": row }));
                }
                Ok(Value::Array(list))
            }
        }
    }

    /// Writes a row locally and queues its mutation: the SPA's local-first write path.
    ///
    /// Both land in one transaction, so a crash cannot leave a row whose write was never
    /// queued (silently lost) or a queued write whose row is not there (the SPA would show
    /// a row that does not exist).
    pub fn write(&self, entity: Entity, id: Uuid, payload: &Value) -> Result<(), StoreError> {
        ensure_writable(entity)?;
        let mutation = Mutation {
            mutation_id: Uuid::now_v7(),
            entity,
            op: Op::Upsert,
            id,
            payload: Some(payload.clone()),
            client_ts: Utc::now(),
        };
        let row = local_row(entity, payload);
        let mut inner = self.lock();
        check_account(inner.account, entity, Some(payload))?;
        let account = inner.account;
        let tx = inner.conn.transaction().store()?;
        insert_row(&tx, entity, id, 0, account, &row)?;
        insert_mutation(&tx, &mutation)?;
        tx.commit().store()?;
        Ok(())
    }

    /// Removes a row locally and queues its tombstone.
    pub fn tombstone(&self, entity: Entity, id: Uuid) -> Result<(), StoreError> {
        ensure_writable(entity)?;
        let mutation = Mutation {
            mutation_id: Uuid::now_v7(),
            entity,
            op: Op::Delete,
            id,
            payload: None,
            client_ts: Utc::now(),
        };
        let mut inner = self.lock();
        let tx = inner.conn.transaction().store()?;
        tx.execute(
            &format!("delete from \"{}\" where id = ?1", entity.table_name()),
            [id.to_string()],
        )
        .store()?;
        insert_mutation(&tx, &mutation)?;
        tx.commit().store()?;
        Ok(())
    }

    /// The server's refusal of the last write to this row, if it refused one.
    ///
    /// Kept because a rejected row is *not saved* and the app has to be able to say why
    /// (gate 7(d)); a later successful push clears it.
    pub fn rejection(
        &self,
        entity: Entity,
        id: Uuid,
    ) -> Result<Option<(String, String)>, StoreError> {
        let inner = self.lock();
        let row: Option<(String, String)> = inner
            .conn
            .query_row(
                "select code, message from rejections where entity = ?1 and entity_id = ?2",
                [entity.table_name(), &id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .store()?;
        Ok(row)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned mutex here means another thread panicked while holding the cache.
        // Recovering the guard keeps the caller's own error the one that is reported, and
        // the alternative — propagating a panic from a lock — would turn one thread's bug
        // into the app's.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl LocalStore for SqliteStore {
    fn cursor(&self) -> Result<i64, StoreError> {
        let inner = self.lock();
        inner
            .conn
            .query_row("select cursor from sync_state where id = 1", [], |row| {
                row.get(0)
            })
            .store()
    }

    fn set_cursor(&mut self, cursor: i64) -> Result<(), StoreError> {
        let inner = self.lock();
        inner
            .conn
            .execute("update sync_state set cursor = ?1 where id = 1", [cursor])
            .store()?;
        Ok(())
    }

    fn apply_page(&mut self, changes: &[Change]) -> Result<ApplyReport, StoreError> {
        let mut inner = self.lock();
        let (report, defer_seen) = {
            let Inner { conn, account, .. } = &mut *inner;
            let tx = conn.transaction().store()?;

            // The roadmap's mechanism for out-of-order arrivals, set on this connection
            // inside this transaction and read back before a single row is written. The
            // read-back is what makes it asserted rather than assumed: a pragma that
            // silently did not take is a page that fails on a child whose parent is in it.
            tx.pragma_update(None, "defer_foreign_keys", true).store()?;
            let defer_seen: i64 = tx
                .query_row("pragma defer_foreign_keys", [], |row| row.get(0))
                .store()?;

            // A row for another account is refused before anything is written, so the page
            // rolls back whole rather than storing it and filtering on read.
            for change in changes {
                check_account(*account, change.entity, change.data.as_ref())?;
            }

            // The rows a previous page could not link come first: this page may be the one
            // that brings their parent.
            let mut queue = load_deferred(&tx)?;
            queue.extend(changes.iter().cloned());

            let (writable, blocked) = settle(&tx, queue)?;
            let mut applied = 0usize;
            for change in &writable {
                if write_change(&tx, *account, change)? {
                    applied += 1;
                }
            }

            tx.execute("delete from deferred_changes", []).store()?;
            for change in &blocked {
                insert_deferred(&tx, change)?;
            }
            tx.commit().store()?;

            (
                ApplyReport {
                    applied,
                    deferred: blocked
                        .iter()
                        .map(|change| (change.entity, change.id))
                        .collect(),
                },
                defer_seen,
            )
        };
        inner.defer_seen = Some(defer_seen);
        Ok(report)
    }

    fn enqueue(&mut self, mutation: Mutation) -> Result<(), StoreError> {
        let inner = self.lock();
        check_account(inner.account, mutation.entity, mutation.payload.as_ref())?;
        insert_mutation(&inner.conn, &mutation)?;
        Ok(())
    }

    fn pending(&self, limit: usize) -> Result<Vec<Mutation>, StoreError> {
        let inner = self.lock();
        let mut statement = inner
            .conn
            .prepare(
                "select mutation_id, entity, entity_id, op, payload, client_ts
                   from outbox order by position limit ?1",
            )
            .store()?;
        let rows = statement
            .query_map([limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .store()?;
        let mut pending = Vec::new();
        for row in rows {
            let (mutation_id, entity, id, op, payload, client_ts) = row.store()?;
            pending.push(mutation_from(
                &mutation_id,
                &entity,
                &id,
                &op,
                payload.as_deref(),
                &client_ts,
            )?);
        }
        Ok(pending)
    }

    fn resolve(&mut self, mutation_id: Uuid, result: &MutationResult) -> Result<(), StoreError> {
        let mut inner = self.lock();
        let account = inner.account;
        let tx = inner.conn.transaction().store()?;
        let queued: Option<(String, String)> = tx
            .query_row(
                "select entity, entity_id from outbox where mutation_id = ?1",
                [mutation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .store()?;

        // The mutation leaves the queue whatever the answer: a rejection is not retried
        // (retrying a refusal the request cannot fix would only repeat it), and it is kept
        // in `rejections` instead.
        tx.execute(
            "delete from outbox where mutation_id = ?1",
            [mutation_id.to_string()],
        )
        .store()?;

        if let Some((entity_name, id)) = queued {
            let entity = entity_from(&entity_name)?;
            let id = Uuid::parse_str(&id).map_err(StoreError::backend)?;
            match result {
                MutationResult::Applied { .. } => {
                    tx.execute(
                        "delete from rejections where entity = ?1 and entity_id = ?2",
                        [entity.table_name(), &id.to_string()],
                    )
                    .store()?;
                }
                MutationResult::Rejected { code, message } => {
                    tx.execute(
                        "insert into rejections (entity, entity_id, code, message, at)
                         values (?1, ?2, ?3, ?4, ?5)
                         on conflict(entity, entity_id) do update
                           set code = excluded.code,
                               message = excluded.message,
                               at = excluded.at",
                        rusqlite::params![
                            entity.table_name(),
                            id.to_string(),
                            code,
                            message,
                            Utc::now().to_rfc3339()
                        ],
                    )
                    .store()?;
                }
                MutationResult::Conflict { winner } => {
                    // A178: the server's row is the row this device must now hold,
                    // including its `deleted_at` — that is how a device that edited a
                    // tombstoned row learns what the account has.
                    let tombstoned = winner
                        .get("deleted_at")
                        .is_some_and(|deleted| !deleted.is_null());
                    if tombstoned {
                        tx.execute(
                            &format!("delete from \"{}\" where id = ?1", entity.table_name()),
                            [id.to_string()],
                        )
                        .store()?;
                    } else if let Some(object) = winner.as_object() {
                        let seq = stored_seq(&tx, entity, id)?.unwrap_or(0);
                        insert_row(
                            &tx,
                            entity,
                            id,
                            seq,
                            account,
                            &Value::Object(object.clone()),
                        )?;
                    }
                }
            }
        }
        tx.commit().store()?;
        Ok(())
    }

    fn outbox_len(&self) -> Result<usize, StoreError> {
        let inner = self.lock();
        let count: i64 = inner
            .conn
            .query_row("select count(*) from outbox", [], |row| row.get(0))
            .store()?;
        Ok(count as usize)
    }

    fn last_synced_at(&self) -> Result<Option<DateTime<Utc>>, StoreError> {
        let inner = self.lock();
        let stored: Option<String> = inner
            .conn
            .query_row(
                "select last_synced_at from sync_state where id = 1",
                [],
                |row| row.get(0),
            )
            .store()?;
        stored
            .map(|at| {
                DateTime::parse_from_rfc3339(&at)
                    .map(|at| at.with_timezone(&Utc))
                    .map_err(StoreError::backend)
            })
            .transpose()
    }

    fn set_last_synced_at(&mut self, at: DateTime<Utc>) -> Result<(), StoreError> {
        let inner = self.lock();
        inner
            .conn
            .execute(
                "update sync_state set last_synced_at = ?1 where id = 1",
                [at.to_rfc3339()],
            )
            .store()?;
        Ok(())
    }

    fn last_state(&self) -> Result<Option<SyncState>, StoreError> {
        let inner = self.lock();
        let stored: Option<String> = inner
            .conn
            .query_row(
                "select last_state from sync_state where id = 1",
                [],
                |row| row.get(0),
            )
            .store()?;
        stored
            .map(|state| serde_json::from_str(&state).map_err(StoreError::backend))
            .transpose()
    }

    fn set_last_state(&mut self, state: SyncState) -> Result<(), StoreError> {
        let inner = self.lock();
        let stored = serde_json::to_string(&state).map_err(StoreError::backend)?;
        inner
            .conn
            .execute(
                "update sync_state set last_state = ?1 where id = 1",
                [stored],
            )
            .store()?;
        Ok(())
    }

    fn wipe(&mut self) -> Result<(), StoreError> {
        let mut inner = self.lock();
        super::wipe(&mut inner.conn).store()?;
        Ok(())
    }
}

/// The columns a read returns: the key and the payload's own, never `account` or `seq`.
fn read_columns(entity: Entity) -> Vec<String> {
    schema::columns(entity)
        .into_iter()
        .filter(|column| column != "account" && column != "seq")
        .collect()
}

fn row_to_value(entity: Entity, row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let bare = schema::text_columns(entity);
    let mut object = Map::new();
    for (index, column) in read_columns(entity).iter().enumerate() {
        let raw: Option<String> = row.get(index)?;
        let value = match raw {
            None => Value::Null,
            // A key or a foreign key is bare text, because that is what SQLite compares;
            // every other column is JSON, so a number comes back a number and an array an
            // array. A column that is not JSON is returned as the text it is rather than
            // dropped, which keeps a hand-edited file readable.
            Some(raw) if bare.contains(column) => Value::String(raw),
            Some(raw) => serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
        };
        object.insert(column.clone(), value);
    }
    Ok(Value::Object(object))
}

fn row_value(conn: &Connection, entity: Entity, id: Uuid) -> Result<Option<Value>, StoreError> {
    let sql = format!(
        "select {} from \"{}\" where id = ?1",
        read_columns(entity)
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", "),
        entity.table_name()
    );
    let row = conn
        .query_row(&sql, [id.to_string()], |row| row_to_value(entity, row))
        .optional()
        .store()?;
    Ok(row)
}

/// The row a local-first write leaves in the cache.
///
/// A task's and an event's payload is `{"patch": …, "base": …}` (A176), and the row the
/// device now holds is the base with the patch applied — that is what a screen reads back
/// while offline, and it is what the server will hold once the mutation lands.
fn local_row(entity: Entity, payload: &Value) -> Value {
    match entity.payload_shape() {
        PayloadShape::PatchWithBase => {
            let mut row = payload
                .get("base")
                .filter(|base| base.is_object())
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new()));
            if let (Some(target), Some(patch)) = (
                row.as_object_mut(),
                payload.get("patch").and_then(Value::as_object),
            ) {
                for (key, value) in patch {
                    target.insert(key.clone(), value.clone());
                }
            }
            row
        }
        PayloadShape::Row => payload.clone(),
    }
}

fn ensure_writable(entity: Entity) -> Result<(), StoreError> {
    if entity.is_client_writable() {
        return Ok(());
    }
    Err(StoreError::backend(format!(
        "{} is written by the server, not by the local-first path",
        entity.table_name()
    )))
}

/// The account a payload names, if it names one.
///
/// A legitimate payload never does: `user_id` is excluded from the pull's allowlist (A168)
/// and a mutation carries only the row's own fields. So a payload that carries one is
/// either a bug or an attempt to place another account's row in this file.
fn named_account(payload: Option<&Value>) -> Option<Uuid> {
    payload?
        .get("user_id")?
        .as_str()
        .and_then(|raw| Uuid::parse_str(raw).ok())
}

fn check_account(
    account: Option<Uuid>,
    entity: Entity,
    payload: Option<&Value>,
) -> Result<(), StoreError> {
    let Some(named) = named_account(payload) else {
        return Ok(());
    };
    match account {
        Some(account) if account == named => Ok(()),
        _ => Err(StoreError::backend(format!(
            "a {} row names account {named}, which is not this cache's account",
            entity.table_name()
        ))),
    }
}

fn read_account(conn: &Connection) -> Result<Option<Uuid>, StoreError> {
    let stored: Option<String> = conn
        .query_row("select account from sync_state where id = 1", [], |row| {
            row.get(0)
        })
        .store()?;
    stored
        .map(|account| Uuid::parse_str(&account).map_err(StoreError::backend))
        .transpose()
}

/// Splits a page into the rows that can be written now and the rows still missing a
/// parent, to a fixed point: a child whose parent is itself deferred is deferred too.
fn settle(
    tx: &Transaction<'_>,
    queue: Vec<Change>,
) -> Result<(Vec<Change>, Vec<Change>), StoreError> {
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
            if linked(tx, &change, &present)? {
                next.push(change);
            } else {
                blocked.push(change);
                moved = true;
            }
        }
        writable = next;
        if !moved || writable.is_empty() {
            return Ok((writable, blocked));
        }
    }
}

/// Whether every parent of a row is either stored or written by this page.
///
/// The parent's *existence* is what this checks; that the parent is written before the
/// child in the same transaction is the deferred foreign key's business, not this
/// function's.
fn linked(
    tx: &Transaction<'_>,
    change: &Change,
    present: &HashSet<(Entity, Uuid)>,
) -> Result<bool, StoreError> {
    let Some(data) = change.data.as_ref() else {
        return Ok(true);
    };
    for (column, parent, _) in schema::parents(change.entity) {
        let Some(id) = data
            .get(column)
            .and_then(Value::as_str)
            .and_then(|raw| Uuid::parse_str(raw).ok())
        else {
            continue;
        };
        if present.contains(&(*parent, id)) {
            continue;
        }
        let exists: Option<i64> = tx
            .query_row(
                &format!("select 1 from \"{}\" where id = ?1", parent.table_name()),
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .store()?;
        if exists.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Applies one change, unless a newer version of the row is already stored.
///
/// The `seq` comparison is what makes a retried deferred row safe: a row deferred three
/// pages ago must not overwrite the version the account has since moved on to.
fn write_change(
    tx: &Transaction<'_>,
    account: Option<Uuid>,
    change: &Change,
) -> Result<bool, StoreError> {
    if stored_seq(tx, change.entity, change.id)?.is_some_and(|seq| seq > change.seq) {
        return Ok(false);
    }
    // A188's normalisation lives in `apply::plan`; an upsert carrying no row that reaches
    // a store anyway is read the same way — a row this device cannot see is not here.
    let row = match (change.op, change.data.as_ref()) {
        (Op::Upsert, Some(data)) if !data.is_null() => Some(data),
        _ => None,
    };
    match row {
        Some(row) => insert_row(tx, change.entity, change.id, change.seq, account, row)?,
        None => {
            tx.execute(
                &format!(
                    "delete from \"{}\" where id = ?1",
                    change.entity.table_name()
                ),
                [change.id.to_string()],
            )
            .store()?;
        }
    }
    Ok(true)
}

fn stored_seq(tx: &Transaction<'_>, entity: Entity, id: Uuid) -> Result<Option<i64>, StoreError> {
    let seq: Option<i64> = tx
        .query_row(
            &format!("select seq from \"{}\" where id = ?1", entity.table_name()),
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .store()?;
    Ok(seq)
}

/// Writes one row, whole: an insert that becomes an update on the same key.
///
/// The column names come from [`schema::columns`], which is built from `Entity`'s own
/// payload list, so nothing here is caller-supplied text.
fn insert_row(
    tx: &Transaction<'_>,
    entity: Entity,
    id: Uuid,
    seq: i64,
    account: Option<Uuid>,
    row: &Value,
) -> Result<(), StoreError> {
    let columns = schema::columns(entity);
    let names = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let assignments = columns[1..]
        .iter()
        .map(|column| format!("\"{column}\" = excluded.\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "insert into \"{}\" ({names}) values ({placeholders})
         on conflict(id) do update set {assignments}",
        entity.table_name()
    );

    let bare = schema::text_columns(entity);
    let values: Vec<SqlValue> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| match index {
            0 => SqlValue::Text(id.to_string()),
            1 => account
                .map(|account| SqlValue::Text(account.to_string()))
                .unwrap_or(SqlValue::Null),
            2 => SqlValue::Integer(seq),
            // A foreign key is stored as the bare uuid text, because SQLite compares it
            // against the parent's `id` and a JSON-encoded string would never match; every
            // other column is JSON, which is what makes its number, array or `null`
            // survive the round trip.
            _ if bare.contains(column) => match row.get(column).and_then(Value::as_str) {
                Some(value) => SqlValue::Text(value.to_string()),
                None => SqlValue::Null,
            },
            _ => json_value(row.get(column)),
        })
        .collect();
    tx.execute(&sql, params_from_iter(values)).store()?;
    Ok(())
}

fn json_value(value: Option<&Value>) -> SqlValue {
    match value {
        None | Some(Value::Null) => SqlValue::Null,
        Some(value) => SqlValue::Text(value.to_string()),
    }
}

fn insert_mutation(conn: &Connection, mutation: &Mutation) -> Result<(), StoreError> {
    conn.execute(
        "insert into outbox (mutation_id, entity, entity_id, op, payload, client_ts)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            mutation.mutation_id.to_string(),
            mutation.entity.table_name(),
            mutation.id.to_string(),
            op_name(mutation.op),
            mutation.payload.as_ref().map(|payload| payload.to_string()),
            mutation.client_ts.to_rfc3339(),
        ],
    )
    .store()?;
    Ok(())
}

fn load_deferred(tx: &Transaction<'_>) -> Result<Vec<Change>, StoreError> {
    let mut statement = tx
        .prepare(
            "select entity, entity_id, seq, op, data from deferred_changes order by seq, entity_id",
        )
        .store()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .store()?;
    let mut deferred = Vec::new();
    for row in rows {
        let (entity, id, seq, op, data) = row.store()?;
        deferred.push(Change {
            seq,
            entity: entity_from(&entity)?,
            id: Uuid::parse_str(&id).map_err(StoreError::backend)?,
            op: op_from(&op)?,
            data: data
                .map(|data| serde_json::from_str(&data).map_err(StoreError::backend))
                .transpose()?,
        });
    }
    Ok(deferred)
}

fn insert_deferred(tx: &Transaction<'_>, change: &Change) -> Result<(), StoreError> {
    tx.execute(
        "insert into deferred_changes (entity, entity_id, seq, op, data)
         values (?1, ?2, ?3, ?4, ?5)
         on conflict(entity, entity_id) do update
           set seq = excluded.seq, op = excluded.op, data = excluded.data",
        rusqlite::params![
            change.entity.table_name(),
            change.id.to_string(),
            change.seq,
            op_name(change.op),
            change.data.as_ref().map(|data| data.to_string()),
        ],
    )
    .store()?;
    Ok(())
}

fn mutation_from(
    mutation_id: &str,
    entity: &str,
    id: &str,
    op: &str,
    payload: Option<&str>,
    client_ts: &str,
) -> Result<Mutation, StoreError> {
    Ok(Mutation {
        mutation_id: Uuid::parse_str(mutation_id).map_err(StoreError::backend)?,
        entity: entity_from(entity)?,
        op: op_from(op)?,
        id: Uuid::parse_str(id).map_err(StoreError::backend)?,
        payload: payload
            .map(|payload| serde_json::from_str(payload).map_err(StoreError::backend))
            .transpose()?,
        client_ts: DateTime::parse_from_rfc3339(client_ts)
            .map(|at| at.with_timezone(&Utc))
            .map_err(StoreError::backend)?,
    })
}

fn entity_from(table: &str) -> Result<Entity, StoreError> {
    Entity::from_table_name(table).ok_or_else(|| {
        StoreError::backend(format!(
            "the cache holds a row for an unknown entity {table}"
        ))
    })
}

/// The wire word for an op, which is the same one the pull ships.
fn op_name(op: Op) -> &'static str {
    match op {
        Op::Upsert => "upsert",
        Op::Delete => "delete",
    }
}

fn op_from(op: &str) -> Result<Op, StoreError> {
    match op {
        "upsert" => Ok(Op::Upsert),
        "delete" => Ok(Op::Delete),
        other => Err(StoreError::backend(format!("{other} is not a sync op"))),
    }
}
