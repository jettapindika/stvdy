//! The native cache: SQLite, owned by the shell and never by JS.
//!
//! `docs/ARCHITECTURE.md` § Local cache fixes the shape — SQLite plus an outbox, owned by
//! the Rust shell (`rusqlite`, `bundled`), with the sync engine and the database on the
//! same side of the boundary. This module is that file: [`open`] creates it and brings its
//! schema up to date, [`queries::SqliteStore`] is the `LocalStore` the engine runs on, and
//! [`queries::SqliteStore::wipe`] is the one operation both a manual reset and a revoked
//! device need (A186).

pub mod queries;
pub mod schema;

use std::path::Path;

use rusqlite::OptionalExtension;

pub use queries::SqliteStore;
pub use rusqlite::Connection;
pub use schema::SCHEMA_VERSION;

/// A failure opening or preparing the cache file.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("the local cache failed: {0}")]
    Backend(#[from] rusqlite::Error),
    #[error("the cache directory could not be created: {0}")]
    Directory(std::io::Error),
}

/// Opens the cache at `path`, creating the file and its parent directory if needed, and
/// makes its schema current.
///
/// Called on every launch, so both halves are idempotent: a second call on an existing file
/// creates nothing and drops nothing.
pub fn open(path: &Path) -> Result<Connection, CacheError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CacheError::Directory)?;
    }
    let conn = Connection::open(path)?;
    prepare(&conn)?;
    Ok(conn)
}

/// The pragmas every cache connection carries.
///
/// `foreign_keys` is off by default in SQLite, which would make every reference in the
/// schema decorative — including the deferral `apply_page` turns on inside its transaction.
/// `journal_mode = WAL` is what lets a read (the SPA showing a list) run while a pass
/// writes, and the busy timeout is what turns that overlap into a short wait instead of an
/// `SQLITE_BUSY` the user would see as a sync error.
pub fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    schema::create_schema(conn)
}

/// Empties the cache and the outbox, leaving the file's schema in place.
///
/// The engine calls this through [`SqliteStore::wipe`]; it is exposed separately so the
/// schema module's own tests can assert what "empty" means without a store.
///
/// [`schema::LOCAL_STATE`] is deliberately not touched: it is the app's own state — a UI
/// preference, a running timer's marker — rather than a row the account owns, and a revoked
/// device has no reason to lose the second one. See the table's own comment.
pub fn wipe(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for table in schema::tables() {
        // `sync_state` is reset rather than emptied: the row is the file's identity
        // (`account`, `device_id`) and deleting it would only be re-inserted on open.
        if table == "sync_state" {
            continue;
        }
        tx.execute(&format!("delete from \"{table}\""), [])?;
    }
    tx.execute(
        "update sync_state
            set cursor = 0, last_synced_at = null, last_state = null",
        [],
    )?;
    tx.commit()
}

/// The app's own state for `key`, or `None` if it was never set or has been deleted.
///
/// A UI preference and a running focus timer's marker live here rather than in the synced
/// rows: they belong to the device, not to the account, and they survive a wipe.
pub fn local_state_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "select value from local_state where key = ?1",
        [key],
        |row| row.get(0),
    )
    .optional()
}

/// Sets or clears the app's own state for `key`.
///
/// `None` **deletes the row** rather than storing the text `"null"`: the SPA's `delete` and
/// a key that was never set are the same absence, and a stored `"null"` would read back as
/// a tombstone instead of as absent. An upsert, because the focus timer writes its marker
/// before the session is flushed and clears it after: a crash in between has to leave the
/// marker rather than lose the session.
pub fn local_state_set(conn: &Connection, key: &str, value: Option<&str>) -> rusqlite::Result<()> {
    match value {
        Some(value) => conn.execute(
            "insert into local_state (key, value, updated_at) values (?1, ?2, ?3)
             on conflict(key) do update set value = excluded.value,
                                           updated_at = excluded.updated_at",
            rusqlite::params![key, value, chrono::Utc::now().to_rfc3339()],
        )?,
        None => conn.execute("delete from local_state where key = ?1", [key])?,
    };
    Ok(())
}
