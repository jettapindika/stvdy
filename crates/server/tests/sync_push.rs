//! `POST /sync/push`: idempotency, per-mutation refusal, ownership, and the lock order
//! that keeps two devices from deadlocking.
//!
//! The idempotency cases assert *facts* rather than the response shape: a replay must
//! apply nothing, so the row's `updated_at` and the `change_log` count are both unchanged
//! — a test that only compared the two responses would pass while a mutation applied
//! twice.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::json;

/// Signs up, verifies, and returns `(token, user_id, device_id)`.
async fn account(app: &axum::Router, harness: &TestState, email: &str) -> (String, Uuid, Uuid) {
    let token = common::signed_up(app, harness, email).await;
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
    (token, user_id, device_id)
}

/// A second device for an existing account, via login.
async fn second_device(app: &axum::Router, email: &str, name: &str) -> (String, Uuid) {
    let (status, body) = json(
        app,
        "POST",
        "/auth/login",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": name, "platform": "android" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["access_token"].as_str().expect("a token").to_string(),
        Uuid::parse_str(body["device_id"].as_str().expect("a device id")).unwrap(),
    )
}

fn mutation(
    mutation_id: Uuid,
    entity: &str,
    op: &str,
    id: Uuid,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "mutation_id": mutation_id,
        "entity": entity,
        "op": op,
        "id": id,
        "payload": payload,
        "client_ts": chrono::Utc::now().to_rfc3339(),
    })
}

/// A `Task`/`Event` payload is a patch plus the base it was edited from (A176).
fn patch(patch: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "patch": patch, "base": null })
}

async fn push(
    app: &axum::Router,
    token: &str,
    device_id: Uuid,
    mutations: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    json(
        app,
        "POST",
        "/sync/push",
        token,
        serde_json::json!({ "device_id": device_id, "mutations": mutations }),
    )
    .await
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

/// The row as JSON, which is the `base` a client sends with a patch.
async fn task_row(pool: &PgPool, id: Uuid) -> serde_json::Value {
    sqlx::query_scalar("select to_jsonb(t) from tasks t where t.id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the task row")
}

/// A card to review: a deck, then a card in it.
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

/// The phase's second named risk, asserted as facts and not as a response shape: a
/// replay returns the stored result AND applies nothing — the row's `updated_at` is
/// unchanged and `change_log` gained no row.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_mutation_applies_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "replay@example.test").await;

    let task = Uuid::now_v7();
    let batch = serde_json::json!([mutation(
        Uuid::now_v7(),
        "task",
        "upsert",
        task,
        patch(serde_json::json!({ "title": "Tugas", "status": "todo", "priority": 2 })),
    )]);

    let (status, first) = push(&app, &token, device_id, batch.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["results"][0]["status"], "applied", "{first}");

    let updated_before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select updated_at from tasks where id = $1")
            .bind(task)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let count_before: i64 =
        sqlx::query_scalar("select count(*) from change_log where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    let (status, second) = push(&app, &token, device_id, batch).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["results"], first["results"],
        "a replay returns the stored result"
    );

    let updated_after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select updated_at from tasks where id = $1")
            .bind(task)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let count_after: i64 = sqlx::query_scalar("select count(*) from change_log where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        updated_after, updated_before,
        "a replay must not touch the row"
    );
    assert_eq!(
        count_after, count_before,
        "a replay must add no change_log row"
    );
}

/// A rejected mutation does not abort the batch: one batch carries a `Message`
/// (read_only_entity), a `Source` (read_only_entity) and a `Task` (applied), and the
/// task lands.
#[sqlx::test(migrations = "../../migrations")]
async fn one_rejected_mutation_does_not_abort_the_batch(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "batch@example.test").await;

    let task = Uuid::now_v7();
    let batch = serde_json::json!([
        mutation(
            Uuid::now_v7(),
            "message",
            "upsert",
            Uuid::now_v7(),
            serde_json::json!({ "content": "halo" }),
        ),
        mutation(
            Uuid::now_v7(),
            "source",
            "upsert",
            Uuid::now_v7(),
            serde_json::json!({ "title": "Sumber", "status": "ready" }),
        ),
        mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            patch(serde_json::json!({ "title": "Tugas", "status": "todo", "priority": 2 })),
        ),
    ]);

    let (status, body) = push(&app, &token, device_id, batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "{body}");
    assert_eq!(results[0]["status"], "rejected", "{body}");
    assert_eq!(results[0]["code"], "read_only_entity", "{body}");
    assert_eq!(results[1]["status"], "rejected", "{body}");
    assert_eq!(results[1]["code"], "read_only_entity", "{body}");
    assert_eq!(results[2]["status"], "applied", "{body}");

    let landed: Option<Uuid> =
        sqlx::query_scalar("select id from tasks where id = $1 and user_id = $2")
            .bind(task)
            .bind(user_id)
            .fetch_optional(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(landed, Some(task), "the batch's task must land");
}

/// The threat model's two rows, through HTTP rather than through `decide`: `Message`
/// and `Source` are `read_only_entity`, with the exact code.
#[sqlx::test(migrations = "../../migrations")]
async fn a_message_and_a_source_are_refused_over_the_wire(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, _, device_id) = account(&app, &harness, "threat@example.test").await;

    let batch = serde_json::json!([
        mutation(
            Uuid::now_v7(),
            "message",
            "upsert",
            Uuid::now_v7(),
            serde_json::json!({ "content": "forged" }),
        ),
        mutation(
            Uuid::now_v7(),
            "source",
            "upsert",
            Uuid::now_v7(),
            serde_json::json!({ "title": "forged", "status": "ready" }),
        ),
    ]);

    let (status, body) = push(&app, &token, device_id, batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for result in body["results"].as_array().unwrap() {
        assert_eq!(result["status"], "rejected", "{body}");
        assert_eq!(result["code"], "read_only_entity", "{body}");
        assert!(result["message"].is_string(), "{body}");
    }
}

/// `flashcards.ease/interval/due` is `read_only_entity` (§5), and a `flashcard_review`
/// — which is pushable — is not confused with it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_derived_flashcard_field_is_refused_and_a_review_is_not(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "derived@example.test").await;
    let card = insert_card(&harness.state.pool, user_id).await;

    let batch = serde_json::json!([
        mutation(
            Uuid::now_v7(),
            "flashcard",
            "upsert",
            Uuid::now_v7(),
            serde_json::json!({ "front": "a", "back": "b", "ease": 2.5 }),
        ),
        mutation(
            Uuid::now_v7(),
            "flashcard_review",
            "upsert",
            Uuid::now_v7(),
            serde_json::json!({
                "flashcard_id": card,
                "rating": "good",
                "reviewed_at": chrono::Utc::now().to_rfc3339(),
            }),
        ),
    ]);

    let (status, body) = push(&app, &token, device_id, batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["status"], "rejected", "{body}");
    assert_eq!(results[0]["code"], "read_only_entity", "{body}");
    assert_eq!(results[1]["status"], "applied", "{body}");
}

/// The lock-order rule, as a test: two devices push a two-mutation batch touching the
/// same rows concurrently (a task plus its subtask) and both succeed — no `40P01`.
#[sqlx::test(migrations = "../../migrations")]
async fn two_devices_editing_overlapping_rows_both_succeed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token1, _, device1) = account(&app, &harness, "race@example.test").await;
    let (token2, device2) = second_device(&app, "race@example.test", "Phone").await;

    let task = Uuid::now_v7();
    let subtask = Uuid::now_v7();
    let batch = |title: &str| {
        serde_json::json!([
            mutation(
                Uuid::now_v7(),
                "task",
                "upsert",
                task,
                patch(serde_json::json!({ "title": title, "status": "todo", "priority": 2 })),
            ),
            mutation(
                Uuid::now_v7(),
                "task",
                "upsert",
                subtask,
                patch(serde_json::json!({
                    "title": format!("{title} sub"),
                    "status": "todo",
                    "priority": 2,
                    "parent_id": task,
                })),
            ),
        ])
    };

    let (first, second) = tokio::join!(
        push(&app, &token1, device1, batch("A")),
        push(&app, &token2, device2, batch("B")),
    );

    for (status, body) in [first, second] {
        assert_eq!(status, StatusCode::OK, "{body}");
        for result in body["results"].as_array().unwrap() {
            assert_eq!(
                result["status"], "applied",
                "both batches must apply, with no deadlock: {body}"
            );
        }
    }
}

/// The lock-order rule, proved: the push route takes the per-user advisory lock **before
/// it touches a row**.
///
/// The hazard is real — a transaction that took its row locks first could deadlock
/// against the AFTER trigger's advisory lock (40P01) — so the rule is not cosmetic. This
/// test holds the advisory lock in one transaction and then drives a push: while the push
/// is blocked on that lock, no `RowExclusiveLock` on `tasks` is granted to any backend in
/// this database, which is only true if the push has not yet issued a write. A route that
/// wrote first would hold that lock and be blocked on the advisory lock instead.
#[sqlx::test(migrations = "../../migrations")]
async fn the_push_takes_the_advisory_lock_before_it_touches_a_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "order@example.test").await;

    // A transaction that holds the same lock the trigger takes, and touches no row.
    let mut blocker = harness.state.pool.acquire().await.unwrap();
    sqlx::query("begin").execute(&mut *blocker).await.unwrap();
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user_id.to_string())
        .execute(&mut *blocker)
        .await
        .unwrap();

    let batch = serde_json::json!([mutation(
        Uuid::now_v7(),
        "task",
        "upsert",
        Uuid::now_v7(),
        patch(serde_json::json!({ "title": "Tugas", "status": "todo", "priority": 2 })),
    )]);
    let app_for_route = app.clone();
    let token_for_route = token.clone();
    let route =
        tokio::spawn(async move { push(&app_for_route, &token_for_route, device_id, batch).await });

    // Wait until the route is actually waiting on the advisory lock, rather than racing it.
    let mut blocked = false;
    for _ in 0..200 {
        let waiting: i64 = sqlx::query_scalar(
            "select count(*) from pg_locks
              where locktype = 'advisory' and not granted
                and database = (select oid from pg_database where datname = current_database())",
        )
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        if waiting > 0 {
            blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(blocked, "the push must block on the advisory lock");

    let row_locks: i64 = sqlx::query_scalar(
        "select count(*) from pg_locks l join pg_class c on c.oid = l.relation
          where l.database = (select oid from pg_database where datname = current_database())
            and c.relname = 'tasks' and l.mode = 'RowExclusiveLock' and l.granted",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        row_locks, 0,
        "the push must take the advisory lock before it writes: a row lock would mean it \
         had already touched `tasks`"
    );

    // Release the lock; the push must then complete and apply.
    sqlx::query("rollback")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let (status, body) = route.await.expect("the route task");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");
}

/// A mutation whose id belongs to another account is `not_found`, and the other
/// account's row is untouched (A173).
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_row_is_not_found_and_never_written(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (a_token, _, a_device) = account(&app, &harness, "owner-a@example.test").await;
    let (_, b_user, _) = account(&app, &harness, "owner-b@example.test").await;

    let b_task = insert_task(&harness.state.pool, b_user, "B's task").await;

    let batch = serde_json::json!([mutation(
        Uuid::now_v7(),
        "task",
        "upsert",
        b_task,
        patch(serde_json::json!({ "title": "hijacked" })),
    )]);
    let (status, body) = push(&app, &a_token, a_device, batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "rejected", "{body}");
    assert_eq!(body["results"][0]["code"], "not_found", "{body}");

    let title: String = sqlx::query_scalar("select title from tasks where id = $1")
        .bind(b_task)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(title, "B's task", "another account's row must be untouched");
}

/// A `mutation_id` already recorded for a different account is a conflict, and the
/// first account's stored result is not returned (A172).
#[sqlx::test(migrations = "../../migrations")]
async fn a_mutation_id_belonging_to_another_account_is_a_conflict(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (a_token, _, a_device) = account(&app, &harness, "mutation-a@example.test").await;
    let (b_token, _, b_device) = account(&app, &harness, "mutation-b@example.test").await;

    let shared = Uuid::now_v7();
    let a_batch = serde_json::json!([mutation(
        shared,
        "task",
        "upsert",
        Uuid::now_v7(),
        patch(serde_json::json!({ "title": "A's", "status": "todo", "priority": 2 })),
    )]);
    let (status, body) = push(&app, &a_token, a_device, a_batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");
    let a_seq = body["results"][0]["seq"].as_i64().unwrap();

    let b_task = Uuid::now_v7();
    let b_batch = serde_json::json!([mutation(
        shared,
        "task",
        "upsert",
        b_task,
        patch(serde_json::json!({ "title": "B's", "status": "todo", "priority": 2 })),
    )]);
    let (status, body) = push(&app, &b_token, b_device, b_batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "rejected", "{body}");
    assert_eq!(body["results"][0]["code"], "conflict", "{body}");
    assert_ne!(
        body["results"][0]["seq"].as_i64(),
        Some(a_seq),
        "another account's stored result must never be returned: {body}"
    );

    let created: Option<Uuid> = sqlx::query_scalar("select id from tasks where id = $1")
        .bind(b_task)
        .fetch_optional(&harness.state.pool)
        .await
        .unwrap();
    assert!(created.is_none(), "a conflicting mutation must not apply");
}

/// A body claiming another device's id is 403 (A171): `origin_device_id` is only worth
/// recording if the claim is checkable.
#[sqlx::test(migrations = "../../migrations")]
async fn a_body_cannot_claim_another_device(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (a_token, _, _) = account(&app, &harness, "device-a@example.test").await;
    let (_, b_device) = second_device(&app, "device-a@example.test", "Other").await;

    let batch = serde_json::json!([mutation(
        Uuid::now_v7(),
        "task",
        "upsert",
        Uuid::now_v7(),
        patch(serde_json::json!({ "title": "X", "status": "todo", "priority": 2 })),
    )]);
    let (status, body) = push(&app, &a_token, b_device, batch).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "forbidden", "{body}");
}

/// A duplicate append-only id is ignored and reports the row's existing seq, so a
/// retried review does not read as a new change (A174).
#[sqlx::test(migrations = "../../migrations")]
async fn a_duplicate_review_is_ignored_and_reports_the_existing_seq(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "review@example.test").await;
    let card = insert_card(&harness.state.pool, user_id).await;

    let review = Uuid::now_v7();
    let payload = serde_json::json!({
        "flashcard_id": card,
        "rating": "good",
        "reviewed_at": chrono::Utc::now().to_rfc3339(),
    });

    let (status, first) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "flashcard_review",
            "upsert",
            review,
            payload.clone()
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["results"][0]["status"], "applied", "{first}");

    let count_before: i64 =
        sqlx::query_scalar("select count(*) from change_log where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    let (status, second) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "flashcard_review",
            "upsert",
            review,
            payload
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["results"][0]["status"], "applied", "{second}");
    assert_eq!(
        second["results"][0]["seq"], first["results"][0]["seq"],
        "a duplicate reports the row's existing seq: {second}"
    );

    let count_after: i64 = sqlx::query_scalar("select count(*) from change_log where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        count_after, count_before,
        "a duplicate must log no new change"
    );
}

/// An Indonesian task title and notes survive the round trip byte-exact, in both
/// directions (roadmap gate 6, the data half).
#[sqlx::test(migrations = "../../migrations")]
async fn indonesian_text_round_trips_byte_exact(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, _, device_id) = account(&app, &harness, "indonesia@example.test").await;

    let title = "Menghitung ∫₀¹ x² dx dan \"koordinat polar\"";
    let notes = "Bab 3 — gunakan substitusi; é, ñ, dan ü harus utuh.";
    let task = Uuid::now_v7();
    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            patch(serde_json::json!({
                "title": title,
                "notes": notes,
                "status": "todo",
                "priority": 2,
            })),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    let (status, pull) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pull}");
    let change = pull["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["id"] == task.to_string())
        .expect("the pushed task is pulled back");
    assert_eq!(change["data"]["title"], title, "{change}");
    assert_eq!(change["data"]["notes"], notes, "{change}");
}

/// A push writes `devices.last_sync_at` (A180) and sets `origin_device_id` to the
/// pushing device.
#[sqlx::test(migrations = "../../migrations")]
async fn a_push_records_the_device_and_the_origin(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "origin@example.test").await;

    let task = Uuid::now_v7();
    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            patch(serde_json::json!({ "title": "Tugas", "status": "todo", "priority": 2 })),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    let last_sync: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select last_sync_at from devices where id = $1")
            .bind(device_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        last_sync.is_some(),
        "a push must record the device's last sync"
    );

    let origin: Option<Uuid> = sqlx::query_scalar(
        "select origin_device_id from change_log
          where user_id = $1 and entity = 'tasks' and entity_id = $2
          order by seq desc limit 1",
    )
    .bind(user_id)
    .bind(task)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(origin, Some(device_id), "the change must name its origin");
}

/// Two devices edit disjoint fields of one task and both edits survive — the
/// field-level merge the conflict table promises, computed from the patch and the base
/// the client sent (A176/A177).
#[sqlx::test(migrations = "../../migrations")]
async fn disjoint_task_edits_both_survive(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "disjoint@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Judul asli").await;
    let base = task_row(&harness.state.pool, task).await;

    // Device A edits the notes.
    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            serde_json::json!({ "patch": { "notes": "catatan A" }, "base": base }),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    // Device B edits the title, from the same stale base.
    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            serde_json::json!({ "patch": { "title": "Judul B" }, "base": base }),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    let row = task_row(&harness.state.pool, task).await;
    assert_eq!(row["title"], "Judul B", "B's edit must survive: {row}");
    assert_eq!(row["notes"], "catatan A", "A's edit must survive: {row}");
}

/// Two devices edit the same field: the incoming patch wins whole, because a
/// per-field tie-break would need a per-field clock the schema does not have.
#[sqlx::test(migrations = "../../migrations")]
async fn overlapping_task_edits_resolve_by_server_receive_time(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "overlap@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Judul asli").await;
    let base = task_row(&harness.state.pool, task).await;

    for title in ["Judul A", "Judul B"] {
        let (status, body) = push(
            &app,
            &token,
            device_id,
            serde_json::json!([mutation(
                Uuid::now_v7(),
                "task",
                "upsert",
                task,
                serde_json::json!({ "patch": { "title": title }, "base": base }),
            )]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["status"], "applied", "{body}");
    }

    let row = task_row(&harness.state.pool, task).await;
    assert_eq!(
        row["title"], "Judul B",
        "the last patch to arrive wins whole: {row}"
    );
}

/// The exit gate's "tombstone vs edit": a patch against a tombstoned task is a
/// `conflict` whose `winner` is the tombstone, so the editing device is told rather
/// than silently reverted.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tombstone_beats_a_concurrent_edit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) =
        account(&app, &harness, "tombstone-vs-edit@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Judul asli").await;
    let base = task_row(&harness.state.pool, task).await;

    // One device tombstones it.
    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "delete",
            task,
            serde_json::Value::Null,
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    // The other edits it, from the pre-tombstone base.
    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            serde_json::json!({ "patch": { "title": "Judul baru" }, "base": base }),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "conflict", "{body}");
    assert!(
        !body["results"][0]["winner"]["deleted_at"].is_null(),
        "the winner must be the tombstone: {body}"
    );

    let row = task_row(&harness.state.pool, task).await;
    assert!(
        !row["deleted_at"].is_null(),
        "the tombstone must stand: {row}"
    );
    assert_eq!(row["title"], "Judul asli", "the edit must not apply: {row}");
}

/// The one exception the contract names: an explicit restore (`deleted_at: null`) wins
/// and logs an upsert.
#[sqlx::test(migrations = "../../migrations")]
async fn an_explicit_restore_wins_and_logs_an_upsert(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "restore@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Judul asli").await;
    sqlx::query("update tasks set deleted_at = now() where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            serde_json::json!({
                "patch": { "deleted_at": serde_json::Value::Null, "title": "Dipulihkan" },
                "base": serde_json::Value::Null,
            }),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "applied", "{body}");

    let row = task_row(&harness.state.pool, task).await;
    assert!(
        row["deleted_at"].is_null(),
        "the row must be restored: {row}"
    );
    assert_eq!(row["title"], "Dipulihkan", "{row}");

    let op: String = sqlx::query_scalar(
        "select op from change_log
          where user_id = $1 and entity = 'tasks' and entity_id = $2
          order by seq desc limit 1",
    )
    .bind(user_id)
    .bind(task)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(op, "upsert", "a restore logs an upsert");
}

/// A patch that omits `deleted_at` does not resurrect a tombstone (A178).
#[sqlx::test(migrations = "../../migrations")]
async fn a_patch_that_omits_deleted_at_does_not_resurrect(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "zombie@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Judul asli").await;
    sqlx::query("update tasks set deleted_at = now() where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = push(
        &app,
        &token,
        device_id,
        serde_json::json!([mutation(
            Uuid::now_v7(),
            "task",
            "upsert",
            task,
            serde_json::json!({ "patch": { "title": "Zombie" }, "base": serde_json::Value::Null }),
        )]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["status"], "conflict", "{body}");

    let row = task_row(&harness.state.pool, task).await;
    assert!(
        !row["deleted_at"].is_null(),
        "the row must stay deleted: {row}"
    );
    assert_eq!(row["title"], "Judul asli", "{row}");
}

/// A settings push upserts the caller's single row and keys the change by the user id,
/// matching what `log_change_by_user` writes (A179).
#[sqlx::test(migrations = "../../migrations")]
async fn a_settings_push_upserts_the_callers_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, device_id) = account(&app, &harness, "settings-push@example.test").await;

    for hour in [7, 9] {
        let (status, body) = push(
            &app,
            &token,
            device_id,
            serde_json::json!([mutation(
                Uuid::now_v7(),
                "user_settings",
                "upsert",
                Uuid::now_v7(),
                serde_json::json!({ "study_start_hour": hour }),
            )]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["status"], "applied", "{body}");

        let stored: i32 =
            sqlx::query_scalar("select study_start_hour from user_settings where user_id = $1")
                .bind(user_id)
                .fetch_one(&harness.state.pool)
                .await
                .unwrap();
        assert_eq!(stored, hour, "the caller's row must hold the pushed value");
    }

    let entity_id: Uuid = sqlx::query_scalar(
        "select entity_id from change_log
          where user_id = $1 and entity = 'user_settings'
          order by seq desc limit 1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        entity_id, user_id,
        "a settings change is keyed by the user id (A179)"
    );
}
