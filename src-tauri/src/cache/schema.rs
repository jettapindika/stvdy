//! The cache's tables: one per synced entity, plus the outbox, the rows a page could not
//! link to a parent, the rejections, and the single `sync_state` row.
//!
//! The DDL is generated from `Entity` rather than hand-written, so a table cannot describe
//! a different set of columns than the pull ships: [`columns`] is `Entity::payload_columns`
//! (A168) plus the three local ones, and the foreign keys mirror `migrations/` exactly —
//! the same `on delete cascade` / `on delete set null` a parent's deletion has on the
//! server. Mirroring them is not tidiness: with a bare reference, deleting a cached deck
//! whose flashcards are still cached would fail the deferred check at commit, and a page
//! that cannot commit is a pass that can never make progress.

use rusqlite::Connection;
use stvdy_sync::proto::Entity;

/// The cache's schema version, kept in SQLite's own `user_version`.
///
/// The cache is a cache: a file written by a version this build does not recognise is
/// **dropped and rebuilt** rather than migrated, because everything in it can be pulled
/// again and a half-applied migration on a device is a launch crash with no recovery.
/// Bump this whenever a table's shape changes.
pub const SCHEMA_VERSION: i64 = 1;

/// The app's own key-value table, which is **not** synced data.
///
/// `Platform.localState` is how a native build keeps a UI preference or a running focus
/// timer's marker across a restart, and neither is a row the account owns. It is therefore
/// deliberately absent from [`tables`]: a wipe (a manual reset, or a revoked device, A186)
/// empties the synced rows and the outbox, and clearing a running timer's marker with them
/// would silently stop a timer the student is watching. The two have different lifetimes,
/// so they have different lists.
pub const LOCAL_STATE: &str = "local_state";

/// The tables a wipe empties, so a wipe and a version check cannot drift from the DDL.
///
/// [`LOCAL_STATE`] is not here on purpose: see its own comment.
pub fn tables() -> Vec<String> {
    let mut names: Vec<String> = Entity::ALL
        .into_iter()
        .map(|entity| entity.table_name().to_string())
        .collect();
    names.extend(
        ["outbox", "deferred_changes", "rejections", "sync_state"]
            .into_iter()
            .map(str::to_string),
    );
    names
}

/// The columns of `entity`'s cache table, in DDL order.
///
/// The first three are the cache's own and never travel:
///   - `id` is the row's key. For every entity but `user_settings` it is also a payload
///     column; for `user_settings` the row *is* the user (A179), so the key holds the
///     account id the change was addressed to.
///   - `account` is the account this cache belongs to, written from the store's binding —
///     never from a payload. `user_id` is deliberately absent from the wire (A168), and a
///     payload that carries one is refused rather than trusted (see `queries`).
///   - `seq` is the last change applied to the row, which is what makes "a retry never
///     overwrites a newer version" decidable locally. A row written by the local-first
///     path has no server seq yet and holds 0.
pub fn columns(entity: Entity) -> Vec<String> {
    let mut names = vec!["id".to_string(), "account".to_string(), "seq".to_string()];
    names.extend(
        entity
            .payload_columns()
            .iter()
            .filter(|column| **column != "id")
            .map(|column| (*column).to_string()),
    );
    names
}

/// The columns stored as bare text rather than as JSON: the key, the account, and every
/// foreign key.
///
/// The distinction is load-bearing rather than tidy. SQLite compares a foreign key's stored
/// value against the parent's `id` as text, so a JSON-encoded string — `"1f0c…"` with its
/// quotes — never matches and the reference fails at commit, which is exactly the
/// "FOREIGN KEY constraint failed" a page of a deck and its cards would raise. Nothing else
/// is ever compared in SQL, so every other column is JSON and round-trips its numbers,
/// arrays, objects and `null` unchanged.
pub fn text_columns(entity: Entity) -> Vec<String> {
    let mut names = vec!["id".to_string(), "account".to_string()];
    names.extend(
        parents(entity)
            .iter()
            .map(|(column, _, _)| (*column).to_string()),
    );
    names
}

/// The synced parent of a cache table's column, and what the server's schema does to the
/// child when that parent is deleted.
///
/// A parent the protocol does not sync — `chunks`, `calendar_connections`, `devices` — is
/// deliberately absent, exactly as in the engine's in-memory double: a store that deferred
/// every row naming one would never write it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentAction {
    Cascade,
    SetNull,
}

/// The synced parents of `entity`, mirroring `migrations/`.
pub fn parents(entity: Entity) -> &'static [(&'static str, Entity, ParentAction)] {
    use Entity::*;
    use ParentAction::{Cascade, SetNull};
    match entity {
        Workspace | UserSettings | PendingAction => &[],
        Source | Message | Deck | Quiz | Report | Cheatsheet => {
            &[("workspace_id", Workspace, Cascade)]
        }
        Goal => &[("workspace_id", Workspace, Cascade)],
        Flashcard => &[("deck_id", Deck, Cascade)],
        FlashcardReview => &[("flashcard_id", Flashcard, Cascade)],
        QuizQuestion => &[("quiz_id", Quiz, Cascade)],
        QuizAttempt => &[
            ("quiz_id", Quiz, Cascade),
            ("question_id", QuizQuestion, Cascade),
        ],
        Task => &[
            ("workspace_id", Workspace, SetNull),
            ("parent_id", Task, Cascade),
        ],
        Event => &[
            ("workspace_id", Workspace, SetNull),
            ("task_id", Task, SetNull),
            // A263: a planned block names the goal it was planned for. This entry and the
            // `Entity::Event` arm of `stvdy_sync::client::testing::parents` are a **pair**:
            // edit one and you must edit the other, or the cache and the engine's double
            // disagree about whether an event can defer on a goal.
            ("goal_id", Goal, SetNull),
        ],
        StudySession => &[
            ("workspace_id", Workspace, SetNull),
            ("task_id", Task, SetNull),
        ],
    }
}

/// Brings `conn`'s schema up to [`SCHEMA_VERSION`], creating it if the file is new.
///
/// Idempotent by construction, because the app opens the cache on every launch: the
/// `create table if not exists` clauses make a second open a no-op, and the version check
/// only drops a file written by a *different* version.
pub fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("pragma user_version", [], |row| row.get(0))?;
    if version != 0 && version != SCHEMA_VERSION {
        tracing::warn!(
            found = version,
            expected = SCHEMA_VERSION,
            "the local cache was written by another version; rebuilding it from scratch"
        );
        drop_all(conn)?;
    }

    let mut ddl = String::new();
    for entity in Entity::ALL {
        ddl.push_str(&entity_ddl(entity));
    }
    ddl.push_str(
        // The queue, in enqueue order: `position` is the insertion order the engine
        // pushes in, and `mutation_id` is the idempotency key the outbox may not
        // duplicate.
        "create table if not exists outbox (
           position integer primary key autoincrement,
           mutation_id text not null unique,
           entity text not null,
           entity_id text not null,
           op text not null,
           payload text,
           client_ts text not null,
           attempts integer not null default 0,
           last_error text
         );
         create index if not exists outbox_position on outbox (position);
         -- A row whose parent has not arrived yet. It is retried at the start of the next
         -- page rather than dropped, which is the roadmap's rule: a child that arrives
         -- before its parent is deferred, and a device killed in between resumes with it.
         create table if not exists deferred_changes (
           entity text not null,
           entity_id text not null,
           seq integer not null,
           op text not null,
           data text,
           primary key (entity, entity_id)
         );
         -- A mutation the server refused, kept with its code: the row it describes is not
         -- saved and the app has to be able to say why (gate 7(d)).
         create table if not exists rejections (
           entity text not null,
           entity_id text not null,
           code text not null,
           message text not null,
           at text not null,
           primary key (entity, entity_id)
         );
         create table if not exists sync_state (
           id integer primary key check (id = 1),
           cursor integer not null default 0,
           account text,
           device_id text,
           last_synced_at text,
           last_state text
         );
         insert or ignore into sync_state (id) values (1);
         -- The app's own key-value store, which is not synced data and which a wipe
         -- deliberately leaves alone (see `LOCAL_STATE`). `value` is not null because
         -- deleting a key deletes its row: a stored 'null' would read back as a tombstone
         -- rather than as absent, and the SPA's `delete` is the same operation as an unset.
         create table if not exists local_state (
           key text not null primary key,
           value text not null,
           updated_at text not null
         );",
    );

    conn.execute_batch(&ddl)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Drops every table the cache owns, leaving an empty file.
fn drop_all(conn: &Connection) -> rusqlite::Result<()> {
    let mut sql = String::new();
    for table in tables() {
        sql.push_str(&format!("drop table if exists \"{table}\";\n"));
    }
    conn.execute_batch(&sql)
}

/// One entity's `create table`.
///
/// Every column but `seq` is declared `text` and always bound as text. `seq` is an integer
/// because it is compared and ordered in SQL. The text/JSON split of the other columns is
/// [`text_columns`]' business, not the declared type's: SQLite's affinity would convert a
/// bound integer back to text anyway, and a hand-inspected file is easier to read when the
/// declared type says "this is a value, not a number".
fn entity_ddl(entity: Entity) -> String {
    let columns = columns(entity);
    let mut body = vec!["id text not null primary key".to_string()];
    for column in &columns[1..] {
        // Everything but `seq` is a JSON-encoded value: the pull ships a row's numbers,
        // arrays and objects, and a text column holding their JSON round-trips them
        // unchanged instead of flattening a `study_start_hour` into a string.
        let kind = if column == "seq" {
            "integer not null default 0"
        } else {
            "text"
        };
        body.push(format!("\"{column}\" {kind}"));
    }
    for (column, parent, action) in parents(entity) {
        let action = match action {
            ParentAction::Cascade => "on delete cascade",
            ParentAction::SetNull => "on delete set null",
        };
        body.push(format!(
            "foreign key (\"{column}\") references \"{}\" (id) {action}",
            parent.table_name()
        ));
    }
    format!(
        "create table if not exists \"{table}\" (\n  {body}\n);\n",
        table = entity.table_name(),
        body = body.join(",\n  "),
    )
}
