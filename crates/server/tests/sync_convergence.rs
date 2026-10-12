//! The exit gate: three devices converge, a replayed mutation applies nothing, a tombstone
//! beats an edit, a stale cursor resyncs, a revoked device is wiped, and two accounts never
//! cross.
//!
//! Every case drives the **real routes**: the `Transport` here is `tower::ServiceExt::oneshot`
//! against the router, with the device's own bearer token, and the store is the in-memory
//! double the engine's own suite uses (`stvdy_sync::client::testing`). That is what makes
//! these assertions about the engine *and* the server rather than about a mock of either —
//! and the phase's named risk is asserted here as `seq` values rather than as the absence of
//! a 200.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use stvdy_server::jobs::handlers::compact::{Retention, compact_user};
use stvdy_server::testing::TestState;
use stvdy_sync::client::testing::{self, InMemoryStore};
use stvdy_sync::client::{Engine, LocalStore, SyncState, Transport, TransportError};
use stvdy_sync::proto::{
    Entity, Mutation, MutationResult, PullQuery, PullResponse, PushRequest, PushResponse,
};
use uuid::Uuid;

mod common;
use common::{json, signed_up};

const RETENTION: Retention = Retention {
    upsert_days: 90,
    tombstone_days: 180,
};

/// The switches a test flips on the transport after the engine owns it, and what the
/// transport saw while the test drove it.
#[derive(Debug, Default)]
struct Control {
    /// Every call fails as if the network were down.
    offline: AtomicBool,
    /// The next push's response is dropped after the server applied it: a lost response is
    /// the case the outbox's replay exists for.
    lose_next_push: AtomicBool,
    /// Every pull query the engine asked, in order.
    queries: Mutex<Vec<PullQuery>>,
    /// How many pulls the server refused with 410: the resync's cause, counted rather than
    /// inferred from where the cursor ended up.
    stale: AtomicUsize,
}

impl Control {
    fn offline(&self) {
        self.offline.store(true, Ordering::SeqCst);
    }

    fn lose_next_push(&self) {
        self.lose_next_push.store(true, Ordering::SeqCst);
    }

    fn record(&self, query: PullQuery) {
        self.queries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(query);
    }

    fn queries(&self) -> Vec<PullQuery> {
        self.queries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn stale(&self) -> usize {
        self.stale.load(Ordering::SeqCst)
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
        self.control.record(query);
        if self.control.offline.load(Ordering::SeqCst) {
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
            let error = TransportError::from_status(status.as_u16(), Self::code_of(&body));
            if error == TransportError::Stale {
                self.control.stale.fetch_add(1, Ordering::SeqCst);
            }
            Err(error)
        }
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        if self.control.offline.load(Ordering::SeqCst) {
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
    device_id: Uuid,
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

    /// The rows and their seqs, sorted: two devices have converged when these are equal.
    fn dump(&self) -> Vec<(String, Uuid, i64, Value)> {
        self.engine.store().dump()
    }

    fn enqueue(&mut self, mutation: Mutation) {
        self.engine.enqueue(mutation).expect("an enqueue")
    }

    fn write_local(&mut self, entity: Entity, id: Uuid, row: Value) {
        self.engine.store_mut().write_local(entity, id, row);
    }

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
        device_id,
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

/// Puts an account on a plan that allows more than two devices, the way A146 requires: the
/// `plan_id` alone buys nothing, a live subscription is what makes it effective.
async fn upgrade_to_pro(harness: &TestState, user_id: Uuid) {
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .expect("the plan id");
    sqlx::query(
        "insert into subscriptions
           (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .expect("the subscription");
}

async fn insert_task(pool: &PgPool, user_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, $3)")
        .bind(id)
        .bind(user_id)
        .bind(title)
        .execute(pool)
        .await
        .expect("insert task");
    id
}

/// A card to review: a workspace, a deck, and a card in it. A review is one of the six
/// offline-writable entities and needs a card to point at, and a card is server-owned.
async fn insert_card(pool: &PgPool, user_id: Uuid) -> Uuid {
    let workspace = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'Kuliah')")
        .bind(workspace)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("insert workspace");
    let deck = Uuid::now_v7();
    sqlx::query(
        "insert into decks (id, user_id, workspace_id, title, source_ids)
         values ($1, $2, $3, 'Deck', '{}')",
    )
    .bind(deck)
    .bind(user_id)
    .bind(workspace)
    .execute(pool)
    .await
    .expect("insert deck");
    let card = Uuid::now_v7();
    sqlx::query(
        "insert into flashcards (id, user_id, deck_id, front, back)
         values ($1, $2, $3, 'Depan', 'Belakang')",
    )
    .bind(card)
    .bind(user_id)
    .bind(deck)
    .execute(pool)
    .await
    .expect("insert card");
    card
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

/// A task mutation in the shape A176 fixes: a patch plus the row it was edited from.
fn task_upsert(id: Uuid, patch: Value, base: Option<Value>) -> Mutation {
    testing::patch_mutation(Entity::Task, id, patch, base)
}

/// The exit gate, first clause. Three devices, each with its own cache and outbox: device A
/// writes a task offline, device B edits it, device C adds a review; after its passes each
/// device holds the same rows at the same seq. Convergence is asserted on the *content*, not
/// on the cursor alone.
#[sqlx::test(migrations = "../../migrations")]
async fn three_devices_converge(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let mut a = sign_up(&app, &harness, "three@example.test").await;
    // Three devices is more than the free plan allows, so the account gets a plan that
    // does — the gate is about three devices, not about one plan's device count.
    upgrade_to_pro(&harness, a.user_id).await;
    let mut b = sign_in(&app, &harness, "three@example.test", "Phone", "android").await;
    let mut c = sign_in(&app, &harness, "three@example.test", "Tablet", "ios").await;
    let card = insert_card(&harness.state.pool, a.user_id).await;

    // Device A writes a task offline: the row first, then the mutation.
    let task = Uuid::now_v7();
    let row = json!({ "id": task, "title": "Tugas", "status": "todo", "priority": 2 });
    a.write_local(Entity::Task, task, row.clone());
    a.enqueue(task_upsert(task, row, None));
    assert_eq!(a.pass().await, SyncState::Synced);

    // Device B pulls it, edits it offline, and pushes the edit with the row it edited from.
    assert_eq!(b.pass().await, SyncState::Synced);
    let base = b
        .row(Entity::Task, task)
        .cloned()
        .expect("B holds the task");
    let mut edited = base.clone();
    edited["title"] = json!("Tugas (diperbaiki)");
    b.write_local(Entity::Task, task, edited);
    b.enqueue(task_upsert(
        task,
        json!({ "title": "Tugas (diperbaiki)" }),
        Some(base),
    ));
    assert_eq!(b.pass().await, SyncState::Synced);

    // Device C adds a review offline and pushes it.
    let review = Uuid::now_v7();
    c.enqueue(testing::row_mutation(
        Entity::FlashcardReview,
        review,
        json!({
            "id": review,
            "flashcard_id": card,
            "rating": "good",
            "reviewed_at": Utc::now().to_rfc3339(),
        }),
    ));
    assert_eq!(c.pass().await, SyncState::Synced);

    // A and B have not seen C's review yet.
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(b.pass().await, SyncState::Synced);

    // The gate: three devices, one state — asserted on the rows and their seqs.
    assert_eq!(a.dump(), b.dump(), "A and B must hold the same rows");
    assert_eq!(b.dump(), c.dump(), "B and C must hold the same rows");
    assert_eq!(a.cursor(), b.cursor(), "and the same cursor");
    assert_eq!(b.cursor(), c.cursor());

    for device in [&a, &b, &c] {
        let task_row = device
            .row(Entity::Task, task)
            .expect("every device holds the task");
        assert_eq!(
            task_row["title"],
            json!("Tugas (diperbaiki)"),
            "B's edit must have reached every device"
        );
        assert!(
            device.row(Entity::FlashcardReview, review).is_some(),
            "C's review must have reached every device"
        );
    }
}

/// The exit gate, second clause: a replayed mutation applies nothing, through the client as
/// well as through the route — the engine's outbox is retried after a simulated lost
/// response and the second push changes no row and adds no `change_log` entry.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_mutation_through_the_outbox_applies_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "replay-engine@example.test").await;

    let task = Uuid::now_v7();
    let mutation = task_upsert(
        task,
        json!({ "title": "Tugas", "status": "todo", "priority": 2 }),
        None,
    );
    let mutation_id = mutation.mutation_id;
    a.enqueue(mutation);

    // The server applies the push and the response is lost on the way back. The outbox
    // cannot know, so it keeps the mutation and retries it.
    a.control.lose_next_push();
    assert_eq!(a.pass().await, SyncState::Offline);
    assert_eq!(
        a.engine.store().outbox_len().unwrap(),
        1,
        "a lost response must keep the write queued"
    );

    let updated_before: DateTime<Utc> =
        sqlx::query_scalar("select updated_at from tasks where id = $1")
            .bind(task)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the row the lost push created");
    let count_before = change_log_seqs(&harness.state.pool, a.user_id).await.len() as i64;

    // The second pass replays the same `mutation_id`.
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(a.engine.store().outbox_len().unwrap(), 0);
    let results = a.results();
    assert_eq!(
        results.len(),
        1,
        "the replay resolves exactly one mutation: {results:?}"
    );
    assert_eq!(
        a.engine.store().resolved()[0].0,
        mutation_id,
        "and it is the one that was retried"
    );
    let MutationResult::Applied { seq } = results[0] else {
        panic!("a replay returns the stored applied result: {results:?}")
    };
    assert!(
        seq > 0,
        "the stored result carries the seq the first push reported"
    );

    let updated_after: DateTime<Utc> =
        sqlx::query_scalar("select updated_at from tasks where id = $1")
            .bind(task)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the row");
    let count_after = change_log_seqs(&harness.state.pool, a.user_id).await.len() as i64;
    assert_eq!(
        updated_after, updated_before,
        "a replayed mutation must not touch the row"
    );
    assert_eq!(
        count_after, count_before,
        "a replayed mutation must add no change_log row"
    );
}

/// The exit gate, third clause: a tombstone and an edit from two devices, in both arrival
/// orders, end with the tombstone winning and the editing device told — and an explicit
/// restore brings the row back.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tombstone_beats_a_concurrent_edit_in_both_arrival_orders(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "tombstone-a@example.test").await;
    let mut b = sign_in(
        &app,
        &harness,
        "tombstone-a@example.test",
        "Phone",
        "android",
    )
    .await;

    // Two tasks, so both arrival orders are exercised in one run.
    let tombstone_first = Uuid::now_v7();
    let edit_first = Uuid::now_v7();
    for (id, title) in [(tombstone_first, "Pertama"), (edit_first, "Kedua")] {
        let row = json!({ "id": id, "title": title, "status": "todo", "priority": 2 });
        a.write_local(Entity::Task, id, row.clone());
        a.enqueue(task_upsert(id, row, None));
    }
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(b.pass().await, SyncState::Synced, "B must hold both rows");

    // Order one: the tombstone lands first, then B's edit arrives.
    a.enqueue(testing::delete_mutation(Entity::Task, tombstone_first));
    assert_eq!(a.pass().await, SyncState::Synced);

    let base = b
        .row(Entity::Task, tombstone_first)
        .cloned()
        .expect("B holds it");
    b.enqueue(task_upsert(
        tombstone_first,
        json!({ "title": "Diedit" }),
        Some(base),
    ));
    assert_eq!(
        b.pass().await,
        SyncState::NeedsAttention {
            code: "conflict".to_string()
        },
        "the editing device must be told the row it edited is gone: {:?}",
        b.results()
    );
    assert!(
        matches!(b.results().last(), Some(MutationResult::Conflict { .. })),
        "the server's row wins, and the client is handed it: {:?}",
        b.results()
    );
    assert_eq!(
        b.state(),
        SyncState::NeedsAttention {
            code: "conflict".to_string()
        },
        "and the status must name it"
    );
    assert!(
        deleted_at(&harness.state.pool, tombstone_first)
            .await
            .is_some(),
        "the tombstone must win over the later edit"
    );

    // Order two: the edit lands first, then the tombstone.
    let base = b
        .row(Entity::Task, edit_first)
        .cloned()
        .expect("B holds it");
    b.enqueue(task_upsert(
        edit_first,
        json!({ "title": "Diedit" }),
        Some(base),
    ));
    assert_eq!(b.pass().await, SyncState::Synced);
    a.enqueue(testing::delete_mutation(Entity::Task, edit_first));
    assert_eq!(a.pass().await, SyncState::Synced);
    assert!(
        deleted_at(&harness.state.pool, edit_first).await.is_some(),
        "the tombstone must win in either arrival order"
    );

    // The explicit restore the contract names: `deleted_at = null` brings the row back, and
    // it travels to every device as an upsert.
    a.enqueue(task_upsert(
        tombstone_first,
        json!({ "deleted_at": null }),
        None,
    ));
    assert_eq!(a.pass().await, SyncState::Synced);
    assert!(
        deleted_at(&harness.state.pool, tombstone_first)
            .await
            .is_none(),
        "an explicit restore must bring the row back"
    );
    assert_eq!(b.pass().await, SyncState::Synced);
    assert!(
        b.row(Entity::Task, tombstone_first).is_some(),
        "and the other device must see the restore"
    );
}

/// The exit gate, fourth clause: compaction moves the watermark past a device's cursor, the
/// device gets 410, and its cache is rebuilt from zero — ending exactly where a device that
/// synced from scratch ends, which is the honest form of "nothing lost": the history older
/// than the retention window is gone from the change log by design (A184), so a pass from
/// zero can only rebuild what the account still retains.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_cursor_resyncs_from_zero_and_loses_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "stale-engine@example.test").await;

    let compacted = Uuid::now_v7();
    let also_compacted = Uuid::now_v7();
    for (id, title) in [(compacted, "Lama"), (also_compacted, "Juga lama")] {
        let row = json!({ "id": id, "title": title, "status": "todo", "priority": 2 });
        a.write_local(Entity::Task, id, row.clone());
        a.enqueue(task_upsert(id, row, None));
    }
    assert_eq!(a.pass().await, SyncState::Synced);
    let cursor_before = a.cursor();
    assert!(cursor_before > 0, "the device has a cursor to lose");

    // A newer change lands after the device's cursor, so the account has history to keep.
    let retained = insert_task(&harness.state.pool, a.user_id, "Setelah").await;

    // Compaction removes the history the device's cursor points at, which moves the
    // watermark above it.
    sqlx::query(
        "update change_log set changed_at = now() - make_interval(days => 400)
          where user_id = $1 and seq <= $2",
    )
    .bind(a.user_id)
    .bind(cursor_before)
    .execute(&harness.state.pool)
    .await
    .expect("age the compacted history");
    let mut conn = harness.state.pool.acquire().await.unwrap();
    let watermark = compact_user(&mut conn, a.user_id, RETENTION)
        .await
        .expect("compaction runs")
        .expect("compaction wrote a watermark");
    assert!(
        watermark > cursor_before,
        "the watermark must be past the device's cursor: {watermark} vs {cursor_before}"
    );

    // The next pass is told its cursor is older than retained history. That is a resync, not
    // an error the user sees.
    let before = a.control.queries().len();
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(
        a.control.stale(),
        1,
        "the device must have been told 410 resync_required, not quietly served"
    );
    // And it restarted from zero rather than walking a cursor that is gone.
    let queries = a.control.queries();
    assert_eq!(
        queries[before].cursor, cursor_before,
        "the pass begins at the cursor the device holds"
    );
    assert_eq!(
        queries[before + 1].cursor,
        0,
        "and restarts the pull from zero: {:?}",
        &queries[before..]
    );
    assert_eq!(
        a.cursor(),
        watermark,
        "the resync must leave the device servable again"
    );
    assert!(
        a.row(Entity::Task, retained).is_some(),
        "the retained history must arrive"
    );
    assert!(
        a.row(Entity::Task, compacted).is_none(),
        "the wipe is real: history older than the retention window cannot be re-delivered"
    );

    // "Nothing lost", exactly: the rebuilt cache is the cache a device that synced from
    // scratch at this moment holds.
    let mut fresh = sign_in(
        &app,
        &harness,
        "stale-engine@example.test",
        "Fresh",
        "windows",
    )
    .await;
    assert_eq!(fresh.pass().await, SyncState::Synced);
    assert_eq!(
        a.dump(),
        fresh.dump(),
        "the resynced device must equal a device that synced from scratch"
    );
}

/// The exit gate, fifth clause: a revoked device is wiped — its cache and its outbox — and
/// the wipe is asserted on the store, not on the HTTP status alone.
#[sqlx::test(migrations = "../../migrations")]
async fn a_revoked_device_is_wiped(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let a = sign_up(&app, &harness, "revoke-a@example.test").await;
    let mut b = sign_in(&app, &harness, "revoke-a@example.test", "Phone", "android").await;

    let task = Uuid::now_v7();
    let row = json!({ "id": task, "title": "Tugas", "status": "todo", "priority": 2 });
    b.write_local(Entity::Task, task, row.clone());
    b.enqueue(task_upsert(task, row, None));
    assert_eq!(b.pass().await, SyncState::Synced);
    assert!(b.row(Entity::Task, task).is_some());

    // A write made offline, before the device is revoked.
    b.enqueue(task_upsert(
        task,
        json!({ "title": "Belum terkirim" }),
        None,
    ));
    assert_eq!(b.engine.store().outbox_len().unwrap(), 1);

    // The account revokes the device from its other device.
    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/devices/{}", b.device_id),
        &a.token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Its next pass is refused, and it wipes rather than merely stopping.
    assert_eq!(b.pass().await, SyncState::Revoked);
    assert_eq!(b.state(), SyncState::Revoked);
    assert!(
        b.row(Entity::Task, task).is_none(),
        "the cache is wiped, asserted on the store"
    );
    assert_eq!(
        b.engine.store().outbox_len().unwrap(),
        0,
        "a pending write from a revoked device must not be replayed by the next account"
    );
    assert_eq!(b.cursor(), 0);
}

/// The phase's named risk, at the engine level: two accounts syncing concurrently in one
/// test, and neither store ever holds a row or a seq belonging to the other.
#[sqlx::test(migrations = "../../migrations")]
async fn two_accounts_never_cross(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "cross-a@example.test").await;
    let mut b = sign_up(&app, &harness, "cross-b@example.test").await;

    let a_task = Uuid::now_v7();
    let b_task = Uuid::now_v7();
    let a_row = json!({ "id": a_task, "title": "Tugas A", "status": "todo", "priority": 2 });
    let b_row = json!({ "id": b_task, "title": "Tugas B", "status": "todo", "priority": 2 });
    a.write_local(Entity::Task, a_task, a_row.clone());
    a.enqueue(task_upsert(a_task, a_row, None));
    b.write_local(Entity::Task, b_task, b_row.clone());
    b.enqueue(task_upsert(b_task, b_row, None));

    // The two accounts sync at the same time, each through its own device and cache.
    let (a_state, b_state) = tokio::join!(a.pass(), b.pass());
    assert_eq!(a_state, SyncState::Synced);
    assert_eq!(b_state, SyncState::Synced);

    let a_seqs = change_log_seqs(&harness.state.pool, a.user_id).await;
    let b_seqs = change_log_seqs(&harness.state.pool, b.user_id).await;
    assert!(!a_seqs.is_empty() && !b_seqs.is_empty());
    // Not vacuous: each device applied changes, and every one of them came from its own
    // account's log.
    for device in [&a, &b] {
        assert!(
            !device.engine.store().applied_seqs().is_empty(),
            "the isolation loop below would be vacuous if nothing was applied"
        );
    }

    // The named risk, asserted on `seq` values rather than on the absence of a 200: not one
    // seq a store ever applied belongs to the other account's log.
    for seq in a.engine.store().applied_seqs() {
        assert!(
            a_seqs.contains(seq),
            "A's cache applied seq {seq}, which is not in A's change log"
        );
        assert!(
            !b_seqs.contains(seq),
            "A's cache applied seq {seq}, which belongs to B"
        );
    }
    for seq in b.engine.store().applied_seqs() {
        assert!(
            b_seqs.contains(seq),
            "B's cache applied seq {seq}, which is not in B's change log"
        );
        assert!(
            !a_seqs.contains(seq),
            "B's cache applied seq {seq}, which belongs to A"
        );
    }

    // And not one row of the other account either.
    for (_, id, _, _) in a.dump() {
        assert_ne!(id, b_task, "A's cache holds B's row");
    }
    for (_, id, _, _) in b.dump() {
        assert_ne!(id, a_task, "B's cache holds A's row");
    }
    assert!(a.row(Entity::Task, a_task).is_some(), "A holds its own row");
    assert!(b.row(Entity::Task, b_task).is_some(), "B holds its own row");
}

/// The lock-order rule end to end: two devices push a two-mutation batch against the same
/// rows, in opposite orders, concurrently, and both succeed.
///
/// The order is reversed on purpose. Two transactions that take their row locks in opposite
/// orders deadlock (40P01) unless the push route takes the per-user advisory lock as its
/// first statement — which `crates/server/tests/sync_push.rs` proves deterministically. This
/// is the roadmap's core multi-device scenario driven through the engine: no deadlock, no
/// rejection, and both caches converge.
#[sqlx::test(migrations = "../../migrations")]
async fn two_devices_pushing_overlapping_batches_both_succeed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "race-engine@example.test").await;
    let mut b = sign_in(
        &app,
        &harness,
        "race-engine@example.test",
        "Phone",
        "android",
    )
    .await;

    // A task and its subtask, created server-side so both devices edit existing rows.
    let task = insert_task(&harness.state.pool, a.user_id, "Tugas").await;
    let subtask = Uuid::now_v7();
    sqlx::query("insert into tasks (id, user_id, parent_id, title) values ($1, $2, $3, 'Sub')")
        .bind(subtask)
        .bind(a.user_id)
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .expect("insert subtask");

    for (id, title) in [(task, "A"), (subtask, "A sub")] {
        a.enqueue(task_upsert(
            id,
            json!({ "title": title, "status": "todo", "priority": 2 }),
            None,
        ));
    }
    for (id, title) in [(subtask, "B sub"), (task, "B")] {
        b.enqueue(task_upsert(
            id,
            json!({ "title": title, "status": "todo", "priority": 2 }),
            None,
        ));
    }

    let (a_state, b_state) = tokio::join!(a.pass(), b.pass());
    assert_eq!(a_state, SyncState::Synced, "{:?}", a.results());
    assert_eq!(b_state, SyncState::Synced, "{:?}", b.results());
    for device in [&a, &b] {
        assert!(
            device
                .results()
                .iter()
                .all(|result| matches!(result, MutationResult::Applied { .. })),
            "every mutation must apply, with no deadlock and no rejection: {:?}",
            device.results()
        );
        assert_eq!(device.engine.store().outbox_len().unwrap(), 0);
    }

    // One more pass each, and the two devices hold the same rows at the same seq.
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(b.pass().await, SyncState::Synced);
    assert_eq!(a.dump(), b.dump(), "the two devices must converge");

    // And the convergence is on the *content*: each device holds both rows with the title
    // the last writer pushed. This is the assertion that catches a pull which ships the row
    // on only one of a row's several changes — the device then applies the rest as A188's
    // "not visible to you" delete and its cache ends up empty while its cursor advances.
    for device in [&a, &b] {
        for (id, title) in [(task, "B"), (subtask, "B sub")] {
            let row = device.row(Entity::Task, id).unwrap_or_else(|| {
                panic!("a device is missing a converged row: {:?}", device.dump())
            });
            assert_eq!(row["title"], json!(title), "{:?}", device.dump());
        }
    }
}

/// Offline reads: with the transport failing, the store still answers a read from the rows
/// already applied, and the status is `offline` rather than `error`.
#[sqlx::test(migrations = "../../migrations")]
async fn offline_reads_work_and_the_status_says_offline(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mut a = sign_up(&app, &harness, "offline@example.test").await;

    let task = Uuid::now_v7();
    let row = json!({
        "id": task,
        "title": "Tugas",
        "notes": "Catatan",
        "status": "todo",
        "priority": 2
    });
    a.write_local(Entity::Task, task, row.clone());
    a.enqueue(task_upsert(task, row, None));
    assert_eq!(a.pass().await, SyncState::Synced);
    assert_eq!(a.state(), SyncState::Synced);

    // The network goes away.
    a.control.offline();
    assert_eq!(a.pass().await, SyncState::Offline);
    assert_eq!(a.state(), SyncState::Offline);
    assert!(
        a.engine.next_delay() > Duration::ZERO,
        "an offline device must back off rather than spin"
    );

    // The cache still answers: that is the point of writing locally first.
    let cached = a.row(Entity::Task, task).expect("the cached row");
    assert_eq!(cached["title"], json!("Tugas"));
    assert_eq!(cached["notes"], json!("Catatan"));

    // And a write made offline waits rather than being lost.
    a.enqueue(task_upsert(task, json!({ "title": "Offline" }), None));
    assert_eq!(a.pass().await, SyncState::Offline);
    assert_eq!(
        a.engine.store().outbox_len().unwrap(),
        1,
        "an offline write must stay queued"
    );
}
