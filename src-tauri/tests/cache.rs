//! The SQLite cache's own behaviours: the ones the engine's in-memory store cannot prove.
//!
//! The convergence suite (Task 11) drives the same engine over an in-memory store and the
//! real routes. What it cannot show is anything about *this* file — that opening it twice
//! is safe, that a page lands with foreign keys deferred, that a row whose parent has not
//! arrived survives a restart, that the outbox is on disk, that a wipe takes the outbox with
//! it, and that a token never reaches it.
//!
//! The allowance below is the workspace's: `unwrap`/`expect`/`panic` are denied in non-test
//! code (a keychain and a cache are exactly where a silent `unwrap` hurts), and this file is
//! test code, where they are the idiomatic way to state a precondition.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde_json::{Value, json};
use stvdy_shell_lib::cache::{self, SCHEMA_VERSION, SqliteStore};
use stvdy_shell_lib::sync::{DynTransport, Notifier, SyncNotice, SyncShell, TransportFactory};
use stvdy_sync::client::testing::{self, ScriptedTransport};
use stvdy_sync::client::{LocalStore, SyncState, TransportError};
use stvdy_sync::proto::{Entity, Mutation, MutationResult, Op};
use uuid::Uuid;

/// A cache file of its own, removed when the test ends.
struct TempCache {
    path: PathBuf,
}

impl TempCache {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "stvdy-cache-{name}-{}.sqlite3",
            Uuid::now_v7().simple()
        ));
        let _ = std::fs::remove_file(&path);
        Self { path }
    }

    /// What the app does on every launch.
    fn open(&self) -> SqliteStore {
        let connection = cache::open(&self.path).expect("the cache opens");
        SqliteStore::new(connection).expect("the store is built")
    }
}

impl Drop for TempCache {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn read_pragma(store: &SqliteStore, name: &str) -> i64 {
    store.with_connection(|conn| {
        conn.query_row(&format!("pragma {name}"), [], |row| row.get(0))
            .expect("the pragma reads")
    })
}

fn table_exists(store: &SqliteStore, table: &str) -> bool {
    store.with_connection(|conn| {
        conn.query_row(
            "select count(*) from sqlite_master where type = 'table' and name = ?1",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .expect("sqlite_master reads")
            == 1
    })
}

/// A JWT-shaped access token naming `account`, which is what `sync_configure` reads the
/// account out of.
fn token_for(account: Uuid) -> String {
    use base64::Engine;

    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"HS256","typ":"JWT"}"#),
        encode(format!(r#"{{"sub":"{account}","did":"device"}}"#).as_bytes()),
        encode(b"signature")
    )
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A notifier that records which notices were raised.
fn recorder() -> (Notifier, Arc<Mutex<Vec<&'static str>>>) {
    let seen: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let notifier: Notifier = Arc::new(move |notice| {
        let word = match notice {
            SyncNotice::Status(_) => "status",
            SyncNotice::Revoked => "revoked",
            SyncNotice::AuthExpired => "auth_expired",
        };
        sink.lock()
            .expect("the recorder is not poisoned")
            .push(word);
    });
    (notifier, seen)
}

/// A shell whose passes go to `transport` instead of the network.
fn shell_over(
    store: &SqliteStore,
    transport: &ScriptedTransport,
) -> (SyncShell, Arc<Mutex<Vec<&'static str>>>) {
    let (notifier, seen) = recorder();
    let scripted = transport.clone();
    let factory: TransportFactory =
        Arc::new(move |_credentials| Ok(DynTransport::new(scripted.clone())));
    (
        SyncShell::with_factory(store.clone(), factory, notifier),
        seen,
    )
}

fn queued_mutation(entity: Entity, id: Uuid) -> Mutation {
    Mutation {
        mutation_id: Uuid::now_v7(),
        entity,
        op: Op::Upsert,
        id,
        payload: Some(json!({ "id": id, "title": "Sel" })),
        client_ts: Utc::now(),
    }
}

/// The schema is created on open and opening twice is a no-op, because the app opens the
/// cache on every launch and a migration that only works once is a launch crash.
#[test]
fn the_schema_is_created_and_reopening_is_idempotent() {
    let temp = TempCache::new("reopen");
    let deck = Uuid::now_v7();
    {
        let mut store = temp.open();
        let report = store
            .apply_page(&[testing::upsert(
                Entity::Deck,
                deck,
                7,
                json!({ "id": deck, "title": "Biologi" }),
            )])
            .expect("the page lands");
        assert_eq!(report.applied, 1);
    }

    // The second open is the launch path. It must not drop the file's contents, and it
    // must not fail because the tables already exist.
    let store = temp.open();
    assert_eq!(
        store.read(Entity::Deck, Some(deck)).expect("read")["title"],
        json!("Biologi"),
        "reopening the cache lost a row"
    );
    for entity in Entity::ALL {
        assert!(
            table_exists(&store, entity.table_name()),
            "{} has no cache table",
            entity.table_name()
        );
    }
    assert_eq!(
        read_pragma(&store, "user_version"),
        SCHEMA_VERSION,
        "the schema version was not recorded"
    );
}

/// The pragma the roadmap names, asserted on the connection rather than assumed:
/// `PRAGMA defer_foreign_keys` reads back as 1 inside the apply transaction.
#[test]
fn the_apply_transaction_defers_foreign_keys() {
    let temp = TempCache::new("pragma");
    let mut store = temp.open();

    // Turned off on the connection first, so what the assertions below see is what
    // `apply_page` did rather than a value it inherited.
    store.with_connection(|conn| {
        conn.pragma_update(None, "defer_foreign_keys", false)
            .expect("the pragma is turned off")
    });
    assert_eq!(read_pragma(&store, "defer_foreign_keys"), 0);

    store.apply_page(&[]).expect("an empty page lands");

    assert_eq!(
        store.defer_foreign_keys_seen(),
        Some(1),
        "the apply transaction must read the pragma back as on"
    );
    // SQLite switches `defer_foreign_keys` off at every COMMIT, so the deferral is the
    // transaction's and not the connection's — which is why it is set inside each page
    // rather than once at open. Asserting the reset here is what keeps a later edit from
    // moving it to `prepare()` and leaving a page that fails on a child whose parent is in
    // it.
    assert_eq!(read_pragma(&store, "defer_foreign_keys"), 0);
}

/// A child that arrives before its parent in the same page lands, because the FK check is
/// deferred to commit, by which point the parent is there.
#[test]
fn a_child_before_its_parent_in_one_page_still_lands() {
    let temp = TempCache::new("child-first");
    let mut store = temp.open();
    let deck = Uuid::now_v7();
    let card = Uuid::now_v7();

    // The card first, on purpose: `apply::plan` would order these, but the store's own
    // contract is to apply a page in the order it is given, and the deferral is what makes
    // that order safe.
    let report = store
        .apply_page(&[
            testing::upsert(
                Entity::Flashcard,
                card,
                1,
                json!({ "id": card, "deck_id": deck, "front": "Sel", "back": "Cell" }),
            ),
            testing::upsert(
                Entity::Deck,
                deck,
                2,
                json!({ "id": deck, "title": "Biologi" }),
            ),
        ])
        .expect("the page lands");

    assert_eq!(report.deferred, vec![]);
    assert_eq!(report.applied, 2);
    assert_eq!(
        store.read(Entity::Flashcard, Some(card)).expect("read")["front"],
        json!("Sel"),
        "the child did not land"
    );
    // The reference has to be the bare uuid, because that is what SQLite compared against
    // the parent's key; a JSON-encoded string would have quotes in it and no page of a
    // parent and its children could ever land.
    assert_eq!(
        store.read(Entity::Flashcard, Some(card)).expect("read")["deck_id"],
        json!(deck.to_string())
    );
    assert_eq!(
        store.read(Entity::Deck, Some(deck)).expect("read")["title"],
        json!("Biologi")
    );
}

/// A row whose parent is missing after the page is deferred and survives a restart: it is
/// retried on the next pull rather than dropped.
#[test]
fn a_deferred_row_survives_a_restart_and_is_retried() {
    let temp = TempCache::new("deferred");
    let deck = Uuid::now_v7();
    let card = Uuid::now_v7();

    {
        let mut store = temp.open();
        let report = store
            .apply_page(&[testing::upsert(
                Entity::Flashcard,
                card,
                1,
                json!({ "id": card, "deck_id": deck, "front": "Sel", "back": "Cell" }),
            )])
            .expect("the page lands");
        assert_eq!(report.applied, 0);
        assert_eq!(report.deferred, vec![(Entity::Flashcard, card)]);
        assert_eq!(
            store.read(Entity::Flashcard, Some(card)).expect("read"),
            Value::Null,
            "a row whose parent is missing must not be stored"
        );
    }

    // Killed and restarted before the parent arrived.
    let mut store = temp.open();
    let report = store
        .apply_page(&[testing::upsert(
            Entity::Deck,
            deck,
            2,
            json!({ "id": deck, "title": "Biologi" }),
        )])
        .expect("the parent lands");

    assert_eq!(report.deferred, vec![]);
    assert_eq!(
        report.applied, 2,
        "the deferred child is retried by the page that brings its parent"
    );
    assert_eq!(
        store.read(Entity::Flashcard, Some(card)).expect("read")["back"],
        json!("Cell"),
        "the deferred child was dropped instead of retried"
    );
}

/// The outbox survives a restart in order, so an offline write made before the app was
/// killed is pushed when it comes back.
#[test]
fn the_outbox_survives_a_restart_in_order() {
    let temp = TempCache::new("outbox");
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::now_v7()).collect();

    {
        let mut store = temp.open();
        for id in &ids {
            store
                .enqueue(queued_mutation(Entity::Task, *id))
                .expect("the write is queued");
        }
        assert_eq!(store.outbox_len().expect("the length reads"), 3);
    }

    let mut store = temp.open();
    let pending = store.pending(10).expect("the queue reads");
    assert_eq!(
        pending
            .iter()
            .map(|mutation| mutation.id)
            .collect::<Vec<_>>(),
        ids,
        "the queue came back out of order"
    );

    // Resolving the first leaves the rest, in the order they were made.
    store
        .resolve(pending[0].mutation_id, &MutationResult::Applied { seq: 11 })
        .expect("the first is resolved");
    let left = store.pending(10).expect("the queue reads");
    assert_eq!(
        left.iter().map(|mutation| mutation.id).collect::<Vec<_>>(),
        ids[1..].to_vec()
    );
    assert_eq!(store.outbox_len().expect("the length reads"), 2);
}

/// The wipe removes the cache AND the outbox, and leaves no row behind that the next
/// account could read (A186).
#[test]
fn a_wipe_removes_the_cache_and_the_outbox() {
    let temp = TempCache::new("wipe");
    let mut store = temp.open();
    let deck = Uuid::now_v7();
    let orphan = Uuid::now_v7();
    let task = Uuid::now_v7();

    store
        .apply_page(&[
            testing::upsert(
                Entity::Deck,
                deck,
                4,
                json!({ "id": deck, "title": "Biologi" }),
            ),
            testing::upsert(
                Entity::Flashcard,
                orphan,
                5,
                json!({ "id": orphan, "deck_id": Uuid::now_v7(), "front": "Sel", "back": "Cell" }),
            ),
        ])
        .expect("the page lands");
    store
        .enqueue(queued_mutation(Entity::Task, task))
        .expect("the write is queued");
    store.set_cursor(9).expect("the cursor moves");
    store
        .set_last_synced_at(Utc::now())
        .expect("the timestamp is stored");
    store
        .set_last_state(SyncState::Synced)
        .expect("the state is stored");

    store.wipe().expect("the wipe runs");

    assert_eq!(store.outbox_len().expect("the length reads"), 0);
    assert!(store.pending(10).expect("the queue reads").is_empty());
    assert_eq!(store.cursor().expect("the cursor reads"), 0);
    assert_eq!(store.last_synced_at().expect("the timestamp reads"), None);
    assert_eq!(store.last_state().expect("the state reads"), None);
    for entity in Entity::ALL {
        assert_eq!(
            store.read(entity, None).expect("the list reads"),
            json!([]),
            "{} still holds rows after a wipe",
            entity.table_name()
        );
    }

    // The wipe survives a restart, so a wiped cache cannot come back: this is the second
    // caller's guarantee as much as the first's.
    let mut store = temp.open();
    assert_eq!(
        store.read(Entity::Deck, None).expect("the list reads"),
        json!([]),
        "a wiped cache came back after a restart"
    );
    assert_eq!(store.outbox_len().expect("the length reads"), 0);

    // The deferred row went with everything else: the page that brings its parent must not
    // land it afterwards.
    let report = store
        .apply_page(&[testing::upsert(
            Entity::Deck,
            deck,
            6,
            json!({ "id": deck, "title": "Biologi" }),
        )])
        .expect("the page lands");
    assert_eq!(report.applied, 1);
    assert_eq!(report.deferred, vec![]);
    assert_eq!(
        store.read(Entity::Flashcard, Some(orphan)).expect("read"),
        Value::Null,
        "a deferred row survived the wipe and landed after it"
    );
}

/// The wipe's other caller: a revoked device. The engine wipes through the same store
/// method, so the outbox goes with the cache — a pending offline write from a revoked
/// device must not be replayed by the next account that signs in on this machine (A186).
#[tokio::test]
async fn a_revoked_device_wipes_the_cache_and_the_outbox_too() {
    let temp = TempCache::new("revoked");
    let mut store = temp.open();
    let deck = Uuid::now_v7();
    let task = Uuid::now_v7();
    store
        .apply_page(&[testing::upsert(
            Entity::Deck,
            deck,
            3,
            json!({ "id": deck, "title": "Biologi" }),
        )])
        .expect("the page lands");
    store
        .enqueue(queued_mutation(Entity::Task, task))
        .expect("the write is queued");

    let transport = ScriptedTransport::new();
    // The queued write is pushed first, so the refusal has to arrive there: a revoked device
    // is refused on any sync route.
    transport.fail_push(TransportError::Revoked);
    let (shell, notices) = shell_over(&store, &transport);
    shell
        .configure(
            "http://127.0.0.1:1",
            &token_for(Uuid::now_v7()),
            Uuid::now_v7(),
        )
        .expect("the session is configured");

    let report = shell.now().await.expect("the pass runs");

    assert_eq!(report.state, "revoked");
    assert_eq!(
        notices
            .lock()
            .expect("the recorder is not poisoned")
            .as_slice(),
        ["revoked"]
    );
    assert_eq!(store.outbox_len().expect("the length reads"), 0);
    assert_eq!(
        store.read(Entity::Deck, None).expect("the list reads"),
        json!([])
    );
    // The session is gone with it: a second pass has nothing to send.
    assert_eq!(
        shell.now().await.expect("the second pass runs").state,
        "offline"
    );
}

/// The access token never reaches the database: the file is scanned for the token string
/// after a configure + pass, because the keychain is the only place a token may live.
#[tokio::test]
async fn the_access_token_is_not_written_to_the_database() {
    let temp = TempCache::new("token");
    let store = temp.open();
    let account = Uuid::now_v7();
    let token = token_for(account);
    let deck = Uuid::now_v7();

    let transport = ScriptedTransport::new();
    transport.answer_pull(testing::page(
        vec![testing::upsert(
            Entity::Deck,
            deck,
            5,
            json!({ "id": deck, "title": "Biologi" }),
        )],
        5,
        false,
    ));
    let (shell, _notices) = shell_over(&store, &transport);
    shell
        .configure("http://127.0.0.1:1", &token, Uuid::now_v7())
        .expect("the session is configured");

    let report = shell.now().await.expect("the pass runs");

    // The pass did real work, so the scan below is not a scan of an empty file.
    assert_eq!(report.state, "synced");
    assert_eq!(report.outbox, 0);
    assert_eq!(
        store.read(Entity::Deck, Some(deck)).expect("read")["title"],
        json!("Biologi")
    );

    for suffix in ["", "-wal"] {
        let path = PathBuf::from(format!("{}{suffix}", temp.path.display()));
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        assert!(
            !contains(&bytes, token.as_bytes()),
            "the access token was written to {}",
            path.display()
        );
    }
}

/// The cache holds only the user's own synced entities: a row for another account is
/// rejected by the store's own scoping rather than stored and filtered on read.
#[test]
fn the_store_rejects_a_row_for_another_account() {
    let temp = TempCache::new("account");
    let mut store = temp.open();
    let ours = Uuid::now_v7();
    let theirs = Uuid::now_v7();
    store.bind(ours).expect("the cache is bound");

    let deck = Uuid::now_v7();
    let error = store
        .apply_page(&[testing::upsert(
            Entity::Deck,
            deck,
            3,
            json!({ "id": deck, "title": "Biologi", "user_id": theirs }),
        )])
        .expect_err("a row for another account is refused");
    assert!(
        error.to_string().contains(&theirs.to_string()),
        "the refusal does not name the account it refused: {error}"
    );
    assert_eq!(
        store.read(Entity::Deck, Some(deck)).expect("read"),
        Value::Null,
        "a refused row was stored anyway"
    );

    // This account's own row still lands, and the refusal is per page rather than sticky.
    store
        .apply_page(&[testing::upsert(
            Entity::Deck,
            deck,
            4,
            json!({ "id": deck, "title": "Biologi", "user_id": ours }),
        )])
        .expect("this account's row lands");

    // Binding another account wipes what the first one left, outbox included: the pending
    // offline write must not be replayed by the next account that signs in.
    store
        .enqueue(queued_mutation(Entity::Task, Uuid::now_v7()))
        .expect("the write is queued");
    store.bind(theirs).expect("the cache is rebound");

    assert_eq!(store.outbox_len().expect("the length reads"), 0);
    assert_eq!(
        store.read(Entity::Deck, None).expect("the list reads"),
        json!([])
    );
}

/// A refusal is kept with the server's code, because the row it describes is not saved and
/// the app has to be able to say why (gate 7(d)) — and a later acceptance clears it, so a
/// row that was refused and then written is not marked unsaved for ever.
#[test]
fn a_rejection_is_kept_with_its_code_and_cleared_by_an_acceptance() {
    let temp = TempCache::new("rejection");
    let mut store = temp.open();
    let task = Uuid::now_v7();

    store
        .enqueue(queued_mutation(Entity::Task, task))
        .expect("the write is queued");
    let queued = store.pending(1).expect("the queue reads");
    store
        .resolve(
            queued[0].mutation_id,
            &MutationResult::Rejected {
                code: "read_only_entity".to_string(),
                message: "this row is written by the server, not by a client push".to_string(),
            },
        )
        .expect("the refusal is recorded");

    assert_eq!(store.outbox_len().expect("the length reads"), 0);
    assert_eq!(
        store
            .rejection(Entity::Task, task)
            .expect("the rejection reads"),
        Some((
            "read_only_entity".to_string(),
            "this row is written by the server, not by a client push".to_string()
        ))
    );

    // The row is written again and accepted: nothing is marked unsaved any more. A task's
    // payload is the patch plus the row it was edited from (A176), and the cache holds the
    // base with the patch applied — that is what a screen reads back while offline.
    store
        .write(
            Entity::Task,
            task,
            &json!({
                "patch": { "title": "Bab 3", "status": "doing" },
                "base": { "id": task, "title": "Bab", "status": "todo" },
            }),
        )
        .expect("the row lands");
    let held = store.read(Entity::Task, Some(task)).expect("read");
    assert_eq!(held["title"], json!("Bab 3"));
    assert_eq!(
        held["status"],
        json!("doing"),
        "the local-first write did not leave the row in the cache"
    );

    let queued = store.pending(1).expect("the queue reads");
    store
        .resolve(queued[0].mutation_id, &MutationResult::Applied { seq: 12 })
        .expect("the acceptance is recorded");

    assert_eq!(
        store
            .rejection(Entity::Task, task)
            .expect("the rejection reads"),
        None
    );
}

/// A178 at the store: a conflict carries the server's row, which is the row this device must
/// now hold — including its `deleted_at`, which is how a device that edited a tombstoned row
/// learns what the account has.
#[test]
fn a_conflict_leaves_the_servers_row_in_the_cache() {
    let temp = TempCache::new("conflict");
    let mut store = temp.open();
    let task = Uuid::now_v7();

    store
        .write(Entity::Task, task, &json!({ "id": task, "title": "Bab 3" }))
        .expect("the local write lands");

    // The server won with an edit of its own.
    let queued = store.pending(1).expect("the queue reads");
    store
        .resolve(
            queued[0].mutation_id,
            &MutationResult::Conflict {
                winner: json!({ "id": task, "title": "Bab 3 (server)", "status": "doing" }),
            },
        )
        .expect("the conflict is resolved");

    let held = store.read(Entity::Task, Some(task)).expect("read");
    assert_eq!(held["title"], json!("Bab 3 (server)"));
    assert_eq!(held["status"], json!("doing"));

    // And the same row edited against a tombstone: the server's row is gone, so this device
    // holds no row for it rather than the stale one it had.
    store
        .write(
            Entity::Task,
            task,
            &json!({ "id": task, "title": "Bab 3 (mine)" }),
        )
        .expect("the local write lands");
    let queued = store.pending(1).expect("the queue reads");
    store
        .resolve(
            queued[0].mutation_id,
            &MutationResult::Conflict {
                winner: json!({ "id": task, "deleted_at": "2026-10-12T00:00:00Z" }),
            },
        )
        .expect("the conflict is resolved");

    assert_eq!(
        store.read(Entity::Task, Some(task)).expect("read"),
        Value::Null,
        "a tombstoned winner left the stale row in the cache"
    );
}

/// The app's own key-value state: an upsert, an absent read for a key that was never set
/// and for one that was deleted, and — the point of the table — a wipe that leaves it alone.
///
/// A UI preference or a running timer's marker is device state, not a row the account owns.
/// Clearing it with the cache would silently stop a timer the student is watching, and a
/// revoked device has no reason to lose it either.
#[test]
fn local_state_is_an_upsert_and_outlives_a_wipe() {
    let temp = TempCache::new("local-state");
    let store = temp.open();

    assert_eq!(
        store
            .with_connection(|conn| cache::local_state_get(conn, "stvdy.focus_timer"))
            .expect("the read runs"),
        None,
        "a key that was never set must read as absent"
    );

    let marker = json!({ "startedAt": "2026-10-12T03:00:00Z", "taskId": Uuid::now_v7() });
    store
        .with_connection(|conn| {
            cache::local_state_set(conn, "stvdy.focus_timer", Some(&marker.to_string()))
        })
        .expect("the write runs");
    assert_eq!(
        store
            .with_connection(|conn| cache::local_state_get(conn, "stvdy.focus_timer"))
            .expect("the read runs"),
        Some(marker.to_string())
    );

    // The timer's marker is written before the session is flushed and cleared after, so a
    // second write has to replace the first rather than fail on the key.
    let restarted = json!({ "startedAt": "2026-10-12T04:00:00Z", "taskId": Uuid::now_v7() });
    store
        .with_connection(|conn| {
            cache::local_state_set(conn, "stvdy.focus_timer", Some(&restarted.to_string()))
        })
        .expect("the second write runs");
    assert_eq!(
        store
            .with_connection(|conn| cache::local_state_get(conn, "stvdy.focus_timer"))
            .expect("the read runs"),
        Some(restarted.to_string())
    );

    // A wipe of the synced rows and the outbox leaves it: the same store method both a
    // manual reset and a revoked device use.
    let mut store = store;
    let task = Uuid::now_v7();
    store
        .enqueue(queued_mutation(Entity::Task, task))
        .expect("the write is queued");
    store.wipe().expect("the wipe runs");

    assert_eq!(store.outbox_len().expect("the length reads"), 0);
    assert_eq!(
        store
            .with_connection(|conn| cache::local_state_get(conn, "stvdy.focus_timer"))
            .expect("the read runs"),
        Some(restarted.to_string()),
        "a wipe cleared the app's own state along with the account's"
    );

    // And it is still there after a restart, which is the whole point of persisting it.
    let store = temp.open();
    assert_eq!(
        store
            .with_connection(|conn| cache::local_state_get(conn, "stvdy.focus_timer"))
            .expect("the read runs"),
        Some(restarted.to_string())
    );

    // Clearing deletes the row rather than storing a tombstone: a key that was deleted and
    // one that was never set have to read the same way.
    store
        .with_connection(|conn| cache::local_state_set(conn, "stvdy.focus_timer", None))
        .expect("the delete runs");
    assert_eq!(
        store
            .with_connection(|conn| cache::local_state_get(conn, "stvdy.focus_timer"))
            .expect("the read runs"),
        None
    );
}
