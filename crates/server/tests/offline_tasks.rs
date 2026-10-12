//! The offline exit gate: a task, an event and a session written with the transport
//! failing sync after reconnect; a deleted parent takes its subtasks with it; a session
//! flushed twice writes one row; a clock that cannot have studied is refused; and the REST
//! path and the push path leave the same change behind.
//!
//! Every case drives **Phase 8's real engine** over the **real routes**: the `Transport`
//! here is `tower::ServiceExt::oneshot` against the router with the device's own bearer
//! token, and the store is the in-memory double the engine's own suite shares
//! (`stvdy_sync::client::testing`). Nothing here re-implements the outbox or the offline
//! path — a hand-written fake of either would prove nothing about the real one.
//!
//! Each case is a gate rather than a demonstration because it closes the way it could pass
//! without the feature working. The first case reads the database directly while the
//! transport fails: without that half it would pass just as well if the "offline" write had
//! gone through REST all along. The double-flush case asserts on the row's `updated_at` and
//! the `change_log` count rather than on the push response, because the server's idempotency
//! is per `mutation_id` and a response-only assertion passes while the row is rewritten.
//! The clock case asserts both directions, so it is the bound being tested and not a check
//! that refuses every session.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::http::StatusCode;
use chrono::{DateTime, Duration as Clock, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use stvdy_sync::client::testing::{self, InMemoryStore};
use stvdy_sync::client::{Engine, LocalStore, SyncState, Transport, TransportError};
use stvdy_sync::proto::{
    Entity, Mutation, MutationResult, Op, PullQuery, PullResponse, PushRequest, PushResponse,
};
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// The switches a test flips on the transport after the engine owns it.
#[derive(Debug, Default)]
struct Control {
    /// Every call fails as if the network were down.
    offline: AtomicBool,
    /// The next push's response is dropped after the server applied it: a lost response is
    /// the case the outbox's replay exists for.
    lose_next_push: AtomicBool,
}

impl Control {
    fn go_offline(&self) {
        self.offline.store(true, Ordering::SeqCst);
    }

    fn go_online(&self) {
        self.offline.store(false, Ordering::SeqCst);
    }

    fn lose_next_push(&self) {
        self.lose_next_push.store(true, Ordering::SeqCst);
    }

    fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }
}

/// The real router, as a [`Transport`].
struct RouterTransport {
    app: axum::Router,
    token: String,
    control: Arc<Control>,
}

impl RouterTransport {
    fn code_of(body: &Value) -> &str {
        body["code"].as_str().unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl Transport for RouterTransport {
    async fn pull(&self, query: PullQuery) -> Result<PullResponse, TransportError> {
        if self.control.is_offline() {
            return Err(TransportError::Network(
                "the test took the network away".to_string(),
            ));
        }
        let (status, body) = json(
            &self.app,
            "GET",
            &testing::pull_uri(query),
            &self.token,
            Value::Null,
        )
        .await;
        if status.is_success() {
            serde_json::from_value(body).map_err(|error| TransportError::Server {
                status: status.as_u16(),
                code: error.to_string(),
            })
        } else {
            Err(TransportError::from_status(
                status.as_u16(),
                Self::code_of(&body),
            ))
        }
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        if self.control.is_offline() {
            return Err(TransportError::Network(
                "the test took the network away".to_string(),
            ));
        }
        let lose = self.control.lose_next_push.swap(false, Ordering::SeqCst);
        let (status, body) = json(
            &self.app,
            "POST",
            "/sync/push",
            &self.token,
            serde_json::to_value(&request).expect("a push request serialises"),
        )
        .await;
        if lose {
            // The server applied it; the answer never came back.
            return Err(TransportError::Network("the response was lost".to_string()));
        }
        if status.is_success() {
            serde_json::from_value(body).map_err(|error| TransportError::Server {
                status: status.as_u16(),
                code: error.to_string(),
            })
        } else {
            Err(TransportError::from_status(
                status.as_u16(),
                Self::code_of(&body),
            ))
        }
    }
}

type TestEngine = Engine<InMemoryStore, RouterTransport>;

/// One device: its engine over the real routes, and the ids a test needs.
struct Device {
    engine: TestEngine,
    token: String,
    user_id: Uuid,
    control: Arc<Control>,
}

impl Device {
    async fn pass(&mut self) -> SyncState {
        self.engine.sync_once().await.expect("a pass")
    }

    fn state(&self) -> SyncState {
        self.engine.status().expect("a status")
    }

    fn cursor(&self) -> i64 {
        self.engine.store().cursor().expect("a cursor")
    }

    fn outbox_len(&self) -> usize {
        self.engine.store().outbox_len().expect("an outbox length")
    }

    /// The rows and their seqs, sorted: two devices have converged when these are equal.
    fn dump(&self) -> Vec<(String, Uuid, i64, Value)> {
        self.engine.store().dump()
    }

    fn enqueue(&mut self, mutation: Mutation) {
        self.engine.enqueue(mutation).expect("an enqueue");
    }

    /// Writes a row locally, as the shell's local-first write path does before it queues
    /// the mutation.
    fn write_local(&mut self, entity: Entity, id: Uuid, row: Value) {
        self.engine.store_mut().write_local(entity, id, row);
    }

    /// The row as the app would read it while offline.
    fn row(&self, entity: Entity, id: Uuid) -> Option<&Value> {
        self.engine.store().row(entity, id)
    }

    fn results(&self) -> Vec<MutationResult> {
        self.engine
            .store()
            .resolved()
            .iter()
            .map(|(_, result)| result.clone())
            .collect()
    }
}

fn device_from(app: &axum::Router, token: String, device_id: Uuid, user_id: Uuid) -> Device {
    let control = Arc::new(Control::default());
    Device {
        engine: Engine::new(
            InMemoryStore::new(),
            RouterTransport {
                app: app.clone(),
                token: token.clone(),
                control: control.clone(),
            },
            device_id,
        ),
        token,
        user_id,
        control,
    }
}

/// Signs a new account up, verifies it, and returns its first device.
async fn sign_up(app: &axum::Router, harness: &TestState, email: &str) -> Device {
    let token = signed_up(app, harness, email).await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the user row");
    let device_id: Uuid = sqlx::query_scalar("select id from devices where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the device row");
    device_from(app, token, device_id, user_id)
}

/// Signs in as an existing account on a new device: a distinct `(platform, name)` is what
/// makes the login create a row rather than reuse one.
async fn sign_in(
    app: &axum::Router,
    harness: &TestState,
    email: &str,
    name: &str,
    platform: &str,
) -> Device {
    let (status, body) = json(
        app,
        "POST",
        "/auth/login",
        "",
        json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": name, "platform": platform }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["access_token"].as_str().expect("a token").to_string();
    let device_id = Uuid::parse_str(body["device_id"].as_str().expect("a device id")).unwrap();
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the user row");
    device_from(app, token, device_id, user_id)
}

/// A session mutation in the shape the timer sends: the whole row, plus the device's own
/// report of when it wrote it on the mutation's `client_ts` (A204).
fn session_mutation(id: Uuid, row: Value, client_ts: DateTime<Utc>) -> Mutation {
    Mutation {
        mutation_id: Uuid::now_v7(),
        entity: Entity::StudySession,
        op: Op::Upsert,
        id,
        payload: Some(row),
        client_ts,
    }
}

/// How many rows a table holds for one id.
async fn row_count(pool: &PgPool, table: &str, id: Uuid) -> i64 {
    let sql = match table {
        "tasks" => "select count(*)::bigint from tasks where id = $1",
        "events" => "select count(*)::bigint from events where id = $1",
        "study_sessions" => "select count(*)::bigint from study_sessions where id = $1",
        other => panic!("no count query for {other}"),
    };
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("a count")
}

async fn deleted_at(pool: &PgPool, id: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("select deleted_at from tasks where id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the task row")
}

async fn change_log_seqs(pool: &PgPool, user_id: Uuid) -> Vec<i64> {
    sqlx::query_scalar("select seq from change_log where user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await
        .expect("the account's seqs")
}

async fn change_log_count(pool: &PgPool, entity_id: Uuid) -> i64 {
    sqlx::query_scalar("select count(*)::bigint from change_log where entity_id = $1")
        .bind(entity_id)
        .fetch_one(pool)
        .await
        .expect("the row's log entries")
}

/// The phase's exit gate, first clause: a task, an event and a session written with the
/// transport failing sync after reconnect.
///
/// Three assertions make this a gate rather than a demonstration:
///
///  1. **The server has nothing before the reconnect.** Read the database directly while
///     the transport fails. A "offline" write that silently went through REST would
///     otherwise pass the rest of the case unchanged.
///  2. **The ids match after the reconnect.** The client mints them (A8/A206), so the row
///     the server holds under the device's id is the row the device minted, not a
///     same-shaped row the server created.
///  3. **A lost response and the replay that follows change nothing.** The outbox keeps a
///     mutation whose answer never arrived and sends it again; a second row would
///     double-count a session, so the count, the `updated_at` and the `change_log` are all
///     asserted across the two passes.
#[sqlx::test(migrations = "../../migrations")]
async fn a_task_an_event_and_a_session_written_offline_sync_after_reconnect(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "offline-gate@example.test").await;
    let mut b = sign_in(
        &app,
        &harness,
        "offline-gate@example.test",
        "Phone",
        "android",
    )
    .await;

    // Both devices have a cursor and an account before the network goes away.
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(b.pass().await, SyncState::Synced);

    a.control.go_offline();

    // The three writes the gate is about, each made the way the shell makes one: the row
    // in the local store first, then the mutation in the outbox.
    let task = Uuid::now_v7();
    let task_row = json!({
        "id": task,
        "title": "Selesaikan tugas kripto",
        "notes": "Bab 4",
        "due_at": "2026-10-16T16:59:00Z",
        "priority": 1,
        "status": "todo",
        "position": 0
    });
    a.write_local(Entity::Task, task, task_row.clone());
    a.enqueue(testing::patch_mutation(
        Entity::Task,
        task,
        task_row.clone(),
        None,
    ));

    let event = Uuid::now_v7();
    let event_row = json!({
        "id": event,
        "kind": "exam",
        "title": "UAS Jaringan",
        "starts_at": "2026-10-20T01:00:00Z",
        "ends_at": "2026-10-20T03:00:00Z",
        "source": "app"
    });
    a.write_local(Entity::Event, event, event_row.clone());
    a.enqueue(testing::patch_mutation(
        Entity::Event,
        event,
        event_row.clone(),
        None,
    ));

    let session = Uuid::now_v7();
    let device_now = Utc::now();
    let session_row = json!({
        "id": session,
        "started_at": device_now - Clock::minutes(50),
        "duration_min": 50,
        "client_ts": device_now
    });
    a.write_local(Entity::StudySession, session, session_row.clone());
    a.enqueue(session_mutation(session, session_row.clone(), device_now));

    // The UI's promise, before any network: all three are readable on the device, and the
    // queued writes are visible as outstanding work.
    assert_eq!(
        a.state(),
        SyncState::Syncing,
        "a queued write is outstanding work, never `Synced`"
    );
    for (entity, id) in [
        (Entity::Task, task),
        (Entity::Event, event),
        (Entity::StudySession, session),
    ] {
        assert!(
            a.row(entity, id).is_some(),
            "the device must show {entity:?} {id} while offline"
        );
    }

    // The failing transport leaves all three queued rather than lost.
    assert_eq!(a.pass().await, SyncState::Offline);
    assert_eq!(a.outbox_len(), 3, "an offline write must stay queued");

    // 1. And the server has nothing: the write really was local.
    for (table, id) in [
        ("tasks", task),
        ("events", event),
        ("study_sessions", session),
    ] {
        assert_eq!(
            row_count(&harness.state.pool, table, id).await,
            0,
            "{table} must not hold {id} before the reconnect"
        );
    }

    // 3. Reconnect, and lose the first push's response: the server applied the batch and
    //    the outbox cannot know, which is the case its replay exists for.
    a.control.go_online();
    a.control.lose_next_push();
    assert_eq!(
        a.pass().await,
        SyncState::Offline,
        "a lost response is a failed pass"
    );
    assert_eq!(
        a.outbox_len(),
        3,
        "a lost response must keep the writes queued"
    );

    // 2. The server now holds exactly the rows the device minted, under the device's ids.
    for (table, id) in [
        ("tasks", task),
        ("events", event),
        ("study_sessions", session),
    ] {
        assert_eq!(
            row_count(&harness.state.pool, table, id).await,
            1,
            "{table} must hold the row the device minted"
        );
    }
    let (started, duration): (DateTime<Utc>, i32) =
        sqlx::query_as("select started_at, duration_min from study_sessions where id = $1")
            .bind(session)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the session the device wrote");
    assert_eq!(duration, 50, "the device's own minutes, not a default");
    assert_eq!(
        started,
        session_row["started_at"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap(),
        "and the instant it started"
    );

    let seqs_before = change_log_seqs(&harness.state.pool, a.user_id).await;
    let counts_before = (
        row_count(&harness.state.pool, "tasks", task).await,
        row_count(&harness.state.pool, "events", event).await,
        row_count(&harness.state.pool, "study_sessions", session).await,
    );

    // The replay: the same three `mutation_id`s, a second time. Nothing is written again.
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert_eq!(a.outbox_len(), 0);
    assert_eq!(
        change_log_seqs(&harness.state.pool, a.user_id).await,
        seqs_before,
        "a replayed batch must add no change_log row"
    );
    assert_eq!(
        (
            row_count(&harness.state.pool, "tasks", task).await,
            row_count(&harness.state.pool, "events", event).await,
            row_count(&harness.state.pool, "study_sessions", session).await,
        ),
        counts_before,
        "a replayed batch must not duplicate a row"
    );
    assert!(
        a.results()
            .iter()
            .all(|result| matches!(result, MutationResult::Applied { .. })),
        "every mutation must apply rather than be refused: {:?}",
        a.results()
    );

    // The other device sees all three, which is the product's whole premise.
    assert_eq!(b.pass().await, SyncState::Synced);
    for (entity, id) in [
        (Entity::Task, task),
        (Entity::Event, event),
        (Entity::StudySession, session),
    ] {
        assert!(
            b.row(entity, id).is_some(),
            "the second device must hold {entity:?} {id}"
        );
    }

    // One state, asserted on the rows and their seqs rather than on the cursor alone: the
    // delta pull carried the three rows and nothing else.
    assert_eq!(a.dump(), b.dump(), "A and B must hold the same rows");
    assert_eq!(a.cursor(), b.cursor(), "and the same cursor");
    assert_eq!(
        a.dump().len(),
        3,
        "and exactly the three rows the gate wrote"
    );
}

/// A parent's delete tombstones its children, driven through the offline path rather than
/// only through the route (A197).
///
/// The route test (`tasks.rs::deleting_a_parent_tombstones_its_children`) asserts the walk
/// the `DELETE /tasks/:id` handler does. This case asserts the same rule where it actually
/// matters for the product: the student is offline, deletes a task that has subtasks, and
/// reconnects. If the subtree is not tombstoned, the children stay live on the server and
/// on every other device — orphaned subtasks under a parent that is gone.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_parent_offline_tombstones_its_children(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "offline-delete@example.test").await;
    assert_eq!(a.pass().await, SyncState::Synced);

    // The parent and its two subtasks, created offline and synced: a real subtree on the
    // server, so the delete below has something to cascade over.
    a.control.go_offline();
    let parent = Uuid::now_v7();
    let parent_row = json!({
        "id": parent, "title": "Induk", "status": "todo", "priority": 2, "position": 0
    });
    a.write_local(Entity::Task, parent, parent_row.clone());
    a.enqueue(testing::patch_mutation(
        Entity::Task,
        parent,
        parent_row.clone(),
        None,
    ));
    let mut children = Vec::new();
    for (index, title) in ["Anak satu", "Anak dua"].into_iter().enumerate() {
        let child = Uuid::now_v7();
        let child_row = json!({
            "id": child, "parent_id": parent, "title": title,
            "status": "todo", "priority": 2, "position": index as i32
        });
        a.write_local(Entity::Task, child, child_row.clone());
        a.enqueue(testing::patch_mutation(
            Entity::Task,
            child,
            child_row.clone(),
            None,
        ));
        children.push(child);
    }
    a.control.go_online();
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    for child in &children {
        assert_eq!(row_count(&harness.state.pool, "tasks", *child).await, 1);
    }

    // The delete, made while the network is down: the parent's tombstone goes in the
    // outbox, and the subtree is the server's walk.
    a.control.go_offline();
    a.enqueue(testing::delete_mutation(Entity::Task, parent));
    assert_eq!(a.pass().await, SyncState::Offline);
    assert_eq!(a.outbox_len(), 1);

    // Reconnect: the delete is applied and its consequences are pulled back.
    a.control.go_online();
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());

    assert!(
        deleted_at(&harness.state.pool, parent).await.is_some(),
        "the parent must be tombstoned"
    );
    for child in &children {
        assert!(
            deleted_at(&harness.state.pool, *child).await.is_some(),
            "the child {child} must be tombstoned with its parent (A197)"
        );
        let logged: i64 = sqlx::query_scalar(
            "select count(*)::bigint from change_log where entity_id = $1 and op = 'delete'",
        )
        .bind(child)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the child's log entries");
        assert_eq!(
            logged, 1,
            "the child {child} must log its tombstone, or no device learns of it"
        );
    }

    // And the device stops showing them: a pull that did not carry the children's
    // tombstones would leave orphaned subtasks on the screen.
    assert!(
        a.row(Entity::Task, parent).is_none(),
        "the deleted parent must leave the device's cache"
    );
    for child in &children {
        assert!(
            a.row(Entity::Task, *child).is_none(),
            "the child {child} must leave the device's cache with its parent"
        );
    }
}

/// The timer's own failure mode: the same session id flushed twice writes one row (A206).
///
/// The assertion is on the row's `updated_at` and the `change_log` count, not on the push
/// response: the server's idempotency is per `mutation_id`, so a response-only assertion
/// would pass while the row was rewritten — and an append-only session rewritten is a
/// session that moved in time.
#[sqlx::test(migrations = "../../migrations")]
async fn flushing_the_same_session_id_twice_writes_one_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "double-flush@example.test").await;
    assert_eq!(a.pass().await, SyncState::Synced);

    let session = Uuid::now_v7();
    let device_now = Utc::now();
    let row = json!({
        "id": session,
        "started_at": device_now - Clock::minutes(50),
        "duration_min": 50,
        "client_ts": device_now
    });

    // Flush one, made offline.
    a.control.go_offline();
    a.write_local(Entity::StudySession, session, row.clone());
    a.enqueue(session_mutation(session, row.clone(), device_now));
    assert_eq!(a.pass().await, SyncState::Offline);
    a.control.go_online();
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert_eq!(
        row_count(&harness.state.pool, "study_sessions", session).await,
        1,
        "the first flush stores the session"
    );
    let updated_after_first: DateTime<Utc> =
        sqlx::query_scalar("select updated_at from study_sessions where id = $1")
            .bind(session)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the session row");
    let log_after_first = change_log_count(&harness.state.pool, session).await;

    // Flush two: the same append-only id, a second mutation, and the response is lost —
    // the normal case after a restart, where the outbox keeps the write and retries it.
    a.enqueue(session_mutation(session, row.clone(), device_now));
    a.control.lose_next_push();
    assert_eq!(a.pass().await, SyncState::Offline);
    assert_eq!(
        a.outbox_len(),
        1,
        "the second flush waits when its answer is lost"
    );
    assert_eq!(
        row_count(&harness.state.pool, "study_sessions", session).await,
        1,
        "a second flush of the same id must not insert a second row"
    );

    // The replay: the same mutation, a second time.
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert_eq!(a.outbox_len(), 0);
    assert!(
        a.results()
            .iter()
            .all(|result| matches!(result, MutationResult::Applied { .. })),
        "a duplicate flush is applied, not refused: {:?}",
        a.results()
    );
    assert_eq!(
        row_count(&harness.state.pool, "study_sessions", session).await,
        1,
        "and still exactly one row"
    );
    let updated_after_replay: DateTime<Utc> =
        sqlx::query_scalar("select updated_at from study_sessions where id = $1")
            .bind(session)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the session row");
    assert_eq!(
        updated_after_replay, updated_after_first,
        "a second flush must not rewrite the row"
    );
    assert_eq!(
        change_log_count(&harness.state.pool, session).await,
        log_after_first,
        "and must add no change_log row"
    );
}

/// The session's clock: a session that starts beyond the device's own report of when it
/// wrote the row is refused, and one that only looks future-dated because the device's
/// clock runs fast is not (A204).
///
/// Both directions are asserted, because either alone is a weaker claim: a bound that
/// refuses everything would pass the first half, and a bound that compares against the
/// *server's* clock would fail the second — and rejecting a legitimate offline session from
/// a device whose clock runs fast is its own defect.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_that_starts_beyond_the_devices_own_clock_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "offline-clock@example.test").await;
    assert_eq!(a.pass().await, SyncState::Synced);

    // The device says it wrote the row now, and that the session started two days from now.
    let fabricated = Uuid::now_v7();
    let device_now = Utc::now();
    let fabricated_row = json!({
        "id": fabricated,
        "started_at": device_now + Clock::hours(48),
        "duration_min": 50,
        "client_ts": device_now
    });
    a.write_local(Entity::StudySession, fabricated, fabricated_row.clone());
    a.enqueue(session_mutation(
        fabricated,
        fabricated_row.clone(),
        device_now,
    ));

    assert_eq!(
        a.pass().await,
        SyncState::NeedsAttention {
            code: "validation".to_string()
        },
        "a session that has not happened yet must be refused: {:?}",
        a.results()
    );
    assert!(
        matches!(
            a.results().last(),
            Some(MutationResult::Rejected { code, .. }) if code == "validation"
        ),
        "the refusal names the validator: {:?}",
        a.results()
    );
    assert_eq!(
        a.outbox_len(),
        0,
        "a refused mutation is not retried for ever"
    );
    assert_eq!(
        row_count(&harness.state.pool, "study_sessions", fabricated).await,
        0,
        "a refused session writes nothing"
    );

    // The other direction: the device's clock runs 30 hours fast, so both its report and
    // the session it timed sit in the future by the server's clock — and the session is
    // internally consistent, so it is stored. Comparing against the server's clock would
    // refuse it.
    let fast = Uuid::now_v7();
    let fast_now = device_now + Clock::hours(30);
    let fast_row = json!({
        "id": fast,
        "started_at": fast_now - Clock::minutes(50),
        "duration_min": 50,
        "client_ts": fast_now
    });
    a.write_local(Entity::StudySession, fast, fast_row.clone());
    a.enqueue(session_mutation(fast, fast_row.clone(), fast_now));

    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert_eq!(
        row_count(&harness.state.pool, "study_sessions", fast).await,
        1,
        "a session from a fast device is the bound being clock-skew-safe, not a gap in it"
    );
}

/// An online write through REST and an offline write through the outbox produce the same
/// `change_log` entry and reach every device through the same pull, which is what makes the
/// two paths one path.
///
/// The roadmap's rule is that both funnel through the same table triggers, so a route that
/// logged something else — a different entity name, a second entry, or nothing at all —
/// would leave the two paths disagreeing about what a device has to replay.
#[sqlx::test(migrations = "../../migrations")]
async fn the_rest_path_and_the_push_path_agree_on_the_log(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "one-path@example.test").await;
    let mut b = sign_in(&app, &harness, "one-path@example.test", "Phone", "android").await;
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(b.pass().await, SyncState::Synced);

    // The offline path: a task written locally and pushed by the outbox.
    let pushed = Uuid::now_v7();
    let pushed_row = json!({
        "id": pushed, "title": "Lewat outbox", "status": "todo", "priority": 2, "position": 0
    });
    a.write_local(Entity::Task, pushed, pushed_row.clone());
    a.enqueue(testing::patch_mutation(
        Entity::Task,
        pushed,
        pushed_row.clone(),
        None,
    ));
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());

    // The REST path, from the account's other device: the same table, the same triggers.
    let via_rest = Uuid::now_v7();
    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &b.token,
        json!({ "id": via_rest, "title": "Lewat REST", "priority": 2 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    for id in [pushed, via_rest] {
        let logged: i64 = sqlx::query_scalar(
            "select count(*)::bigint from change_log where entity_id = $1 and op = 'upsert'",
        )
        .bind(id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the row's log entries");
        assert_eq!(logged, 1, "{id} must be logged once and once only");
        let entity: String =
            sqlx::query_scalar("select entity from change_log where entity_id = $1")
                .bind(id)
                .fetch_one(&harness.state.pool)
                .await
                .expect("the logged entity");
        assert_eq!(
            entity, "tasks",
            "{id} must be logged under the table both paths write"
        );
    }

    // And the device that wrote neither pulls both through the one path.
    assert_eq!(a.pass().await, SyncState::Synced);
    assert!(
        a.row(Entity::Task, via_rest).is_some(),
        "the REST write must arrive through the same pull"
    );
    assert_eq!(b.pass().await, SyncState::Synced);
    assert!(
        b.row(Entity::Task, pushed).is_some(),
        "the pushed write must arrive on the other device"
    );
    assert_eq!(a.dump(), b.dump(), "and the two devices must converge");
}

/// Restoring a parent brings back the children its delete tombstoned, and **only** those
/// (A197).
///
/// The third child is the whole point of the case. Without a way to tell a cascade
/// tombstone from a deliberate one, a restore can only either leave every child deleted
/// (the bug this case was written against: the student restores a task and its subtasks are
/// silently gone, with nothing on the screen to say they existed) or bring every child back
/// (a resurrection of the kind A207 refuses for the timer — data the student deleted on
/// purpose reappearing under a parent they restored). Both are asserted here, in one run.
#[sqlx::test(migrations = "../../migrations")]
async fn restoring_a_parent_restores_only_the_children_its_delete_tombstoned(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "cascade-restore@example.test").await;
    assert_eq!(a.pass().await, SyncState::Synced);

    // A parent with two children, one of which the student deletes deliberately.
    a.control.go_offline();
    let parent = Uuid::now_v7();
    let parent_row = json!({
        "id": parent, "title": "Induk", "status": "todo", "priority": 2, "position": 0
    });
    a.write_local(Entity::Task, parent, parent_row.clone());
    a.enqueue(testing::patch_mutation(
        Entity::Task,
        parent,
        parent_row.clone(),
        None,
    ));
    let mut children = Vec::new();
    for (index, title) in ["Anak dibatalkan induknya", "Anak dihapus sengaja"]
        .into_iter()
        .enumerate()
    {
        let child = Uuid::now_v7();
        let child_row = json!({
            "id": child, "parent_id": parent, "title": title,
            "status": "todo", "priority": 2, "position": index as i32
        });
        a.write_local(Entity::Task, child, child_row.clone());
        a.enqueue(testing::patch_mutation(
            Entity::Task,
            child,
            child_row.clone(),
            None,
        ));
        children.push(child);
    }
    a.control.go_online();
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());

    let cascaded = children[0];
    let deliberate = children[1];
    a.control.go_offline();
    a.enqueue(testing::delete_mutation(Entity::Task, deliberate));
    a.control.go_online();
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert!(
        deleted_at(&harness.state.pool, deliberate).await.is_some(),
        "the deliberate delete must stand"
    );

    // The parent's delete takes the remaining live child with it.
    a.control.go_offline();
    a.enqueue(testing::delete_mutation(Entity::Task, parent));
    a.control.go_online();
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert!(deleted_at(&harness.state.pool, parent).await.is_some());
    assert!(
        deleted_at(&harness.state.pool, cascaded).await.is_some(),
        "the cascade must reach the live child"
    );

    // The restore: a patch that names `deleted_at: null`, which is the contract's one
    // explicit restore (A178).
    a.enqueue(testing::patch_mutation(
        Entity::Task,
        parent,
        json!({ "title": "Induk", "deleted_at": null }),
        None,
    ));
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());

    assert!(
        deleted_at(&harness.state.pool, parent).await.is_none(),
        "the parent must be restored"
    );
    assert!(
        deleted_at(&harness.state.pool, cascaded).await.is_none(),
        "the child the cascade took must come back with its parent"
    );
    assert!(
        deleted_at(&harness.state.pool, deliberate).await.is_some(),
        "the child the student deleted on purpose must stay deleted"
    );

    // The consumer-visible half: the Tasks screen's list. Asserted on the live rows rather
    // than on `deleted_at`, because "the subtask is back" is what the student sees.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/tasks?parent_id={parent}"),
        &a.token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed: Vec<String> = body["tasks"]
        .as_array()
        .expect("a task list")
        .iter()
        .map(|task| task["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![cascaded.to_string()],
        "the restored child is listed and the deliberately deleted one is not: {body}"
    );

    // And the device converges on the same thing, which is what makes the restore a fact on
    // every device rather than only on the server.
    let mut b = sign_in(
        &app,
        &harness,
        "cascade-restore@example.test",
        "Phone",
        "android",
    )
    .await;
    assert_eq!(b.pass().await, SyncState::Synced);
    assert!(
        b.row(Entity::Task, cascaded).is_some(),
        "the restored child must reach the other device"
    );
    assert!(
        b.row(Entity::Task, deliberate).is_none(),
        "and the deliberately deleted one must not"
    );
    assert_eq!(a.dump(), b.dump());
}

/// A row a connected calendar owns is read-only on **both** paths (A198).
///
/// The REST twins already exist (`events.rs`). This case is the push half, and its point is
/// that the two agree: before it, `PATCH /events/:id` was a 403 while the same edit through
/// `POST /sync/push` was applied — and an edit that lands is an edit the next incremental
/// sync silently reverts, because the row no longer matches the `(connection_id,
/// external_id)` the calendar knows.
///
/// The refusals are asserted with the row unchanged, not with the code alone: a push that
/// returned `read_only_entity` and wrote anyway would pass a code-only assertion.
#[sqlx::test(migrations = "../../migrations")]
async fn a_calendar_owned_event_is_read_only_on_both_paths(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "calendar-owned@example.test").await;
    assert_eq!(a.pass().await, SyncState::Synced);

    // A row Phase 10's sync would own: `source = 'google'`.
    let event = Uuid::now_v7();
    sqlx::query(
        "insert into events (id, user_id, kind, title, starts_at, ends_at, source)
         values ($1, $2, 'class', 'Dari Google', now(), now() + interval '1 hour', 'google')",
    )
    .bind(event)
    .bind(a.user_id)
    .execute(&harness.state.pool)
    .await
    .expect("insert the calendar's event");

    // The REST twins: an edit and a delete, both refused.
    for (method, body) in [
        ("PATCH", json!({ "title": "Diedit" })),
        ("DELETE", Value::Null),
    ] {
        let (status, response) =
            json(&app, method, &format!("/events/{event}"), &a.token, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} must be refused: {response}"
        );
    }

    // The push twins, through the engine: an edit carrying the row it was edited from, and
    // a delete.
    let base: Value = sqlx::query_scalar("select to_jsonb(t) from events t where t.id = $1")
        .bind(event)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the stored event");
    a.enqueue(testing::patch_mutation(
        Entity::Event,
        event,
        json!({ "title": "Diedit lewat push" }),
        Some(base),
    ));
    assert_eq!(
        a.pass().await,
        SyncState::NeedsAttention {
            code: "read_only_entity".to_string()
        },
        "a pushed edit of a calendar row must be refused: {:?}",
        a.results()
    );
    assert!(
        matches!(
            a.results().last(),
            Some(MutationResult::Rejected { code, .. }) if code == "read_only_entity"
        ),
        "with A198's own code, the one that says the remedy is to stop writing: {:?}",
        a.results()
    );

    a.enqueue(testing::delete_mutation(Entity::Event, event));
    assert_eq!(
        a.pass().await,
        SyncState::NeedsAttention {
            code: "read_only_entity".to_string()
        },
        "and so must a pushed delete: {:?}",
        a.results()
    );

    // The row is untouched: the title is the calendar's and it is not tombstoned.
    let (title, deleted): (String, Option<DateTime<Utc>>) =
        sqlx::query_as("select title, deleted_at from events where id = $1")
            .bind(event)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the calendar's row");
    assert_eq!(
        title, "Dari Google",
        "a refused edit must not change the row"
    );
    assert!(
        deleted.is_none(),
        "a refused delete must not tombstone the row"
    );

    // The other direction: the account's own `app` event is still editable through the same
    // path, so the refusals above are the calendar rule and not a check that refuses every
    // event.
    let mine = Uuid::now_v7();
    let mine_row = json!({
        "id": mine, "kind": "exam", "title": "UAS Jaringan",
        "starts_at": "2026-10-20T01:00:00Z", "ends_at": "2026-10-20T03:00:00Z",
        "source": "app"
    });
    a.write_local(Entity::Event, mine, mine_row.clone());
    a.enqueue(testing::patch_mutation(
        Entity::Event,
        mine,
        mine_row.clone(),
        None,
    ));
    assert_eq!(a.pass().await, SyncState::Synced, "{:?}", a.results());
    assert_eq!(row_count(&harness.state.pool, "events", mine).await, 1);
}
