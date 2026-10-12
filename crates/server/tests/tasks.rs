//! `crates/server/src/tasks.rs`: the CRUD, quick capture and break-down.
//!
//! Two assertions here are the ones that would otherwise be assumed rather than verified:
//!
//!  * a capture and a breakdown each **write no row**. A route that inserted one would pass
//!    every other case in this file — the credits are spent, the draft comes back, the
//!    response shape is right — while being exactly the failure the confirm step exists to
//!    prevent (A193, A195).
//!  * a parent delete reaches its children. Postgres's `on delete cascade` fires the
//!    children's triggers on a real delete, but a tombstone is an UPDATE, so nothing does it
//!    unless the statement does (A197).
//!
//! Nothing here asserts "did not panic" or a bare status: a refusal is asserted by its exact
//! `code`, and a write by the row it left behind.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// The draft `prompts/quick_capture.v1.md`'s own schema example describes.
const CAPTURE_REPLY: &str = r#"{"title":"Selesaikan tugas kripto","notes":null,"due_at":null,
    "priority":2,"estimate_min":null}"#;

/// A three-subtask proposal: the contract's minimum, and a total that equals the sum.
const BREAKDOWN_REPLY: &str = r#"{"subtasks":[
    {"title":"Baca bab 4","estimate_min":30},
    {"title":"Kerjakan latihan","estimate_min":45},
    {"title":"Tinjau jawaban","estimate_min":20}],"total_estimate_min":95}"#;

/// A harness whose model answers with `replies`, in order.
async fn harness_with_llm(pool: PgPool, replies: &[&str]) -> (TestState, Arc<ScriptedLlm>) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(
        replies.iter().map(|reply| reply.to_string()).collect(),
    ));
    let harness = harness.with_llm(llm.clone());
    (harness, llm)
}

/// Puts an account on `pro` **with a live subscription**, which is what makes the plan grant
/// anything (A146): `users.plan_id` alone is a column that reads as paid and behaves free.
async fn upgrade_to_pro(harness: &TestState, email: &str) -> Uuid {
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    user_id
}

async fn count(pool: &PgPool, sql: &str, user_id: Uuid) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_string()))
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The account's balance, as the ledger sums to it (A9).
///
/// Read before and after a charge rather than compared against a literal: signup grants the
/// plan's monthly credits, so a literal would be asserting the grant's size in a test about
/// the charge's.
async fn balance(pool: &PgPool, user_id: Uuid) -> i64 {
    count(
        pool,
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
        user_id,
    )
    .await
}

/// A free account has `quick_capture = false` in the matrix, so the refusal is the engine's
/// and not a literal in this file.
#[sqlx::test(migrations = "../../migrations")]
async fn quick_capture_is_refused_on_a_plan_that_does_not_grant_it(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[CAPTURE_REPLY]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "capture-free@example.test").await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks/quick-capture",
        &token,
        serde_json::json!({ "text": "selesaikan tugas kripto hari Jumat" }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");
    assert_eq!(body["min_plan"], "pro", "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "a refusal must explain itself: {body}"
    );
}

/// The credits are spent and the draft comes back, and the task count is unchanged. This is
/// the assertion the "a draft that saves without confirmation" risk is closed by: a capture
/// that inserted a row would pass every other test in this file.
#[sqlx::test(migrations = "../../migrations")]
async fn a_capture_returns_a_draft_and_writes_no_task(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[CAPTURE_REPLY]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "capture-pro@example.test").await;
    let user_id = upgrade_to_pro(&harness, "capture-pro@example.test").await;
    let before = balance(&harness.state.pool, user_id).await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks/quick-capture",
        &token,
        serde_json::json!({ "text": "selesaikan tugas kripto hari Jumat" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["draft"]["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()),
        "{body}"
    );
    assert_eq!(body["credits_spent"], 2, "{body}");
    assert_eq!(body["prompt_version"], "quick_capture.v1", "{body}");
    assert!(
        body["credits_remaining"].is_i64(),
        "the caller is told what is left: {body}"
    );

    assert_eq!(
        count(
            &harness.state.pool,
            "select count(*)::bigint from tasks where user_id = $1",
            user_id
        )
        .await,
        0,
        "a capture must not write a task (A193)"
    );
    assert_eq!(
        count(
            &harness.state.pool,
            "select count(*)::bigint from usage_events where user_id = $1 and action = 'quick_capture'",
            user_id
        )
        .await,
        1,
        "the capture is one usage event under its own action"
    );
    assert_eq!(
        before - balance(&harness.state.pool, user_id).await,
        2,
        "the charge is the matrix's `task_breakdown` price (A190)"
    );
}

/// The prompt the model received carries the student's zone and the current instant, and the
/// captured text inside a `<user_context_data>` block.
///
/// The zone is the assertion that matters: an Indonesian capture resolved in UTC puts a
/// Friday-evening deadline on the wrong day, and the failure is invisible in a test that only
/// checks the draft came back.
#[sqlx::test(migrations = "../../migrations")]
async fn the_capture_prompt_carries_the_zone_the_clock_and_a_data_block(pool: PgPool) {
    let (harness, llm) = harness_with_llm(pool, &[CAPTURE_REPLY]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "capture-prompt@example.test").await;
    upgrade_to_pro(&harness, "capture-prompt@example.test").await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks/quick-capture",
        &token,
        serde_json::json!({ "text": "selesaikan tugas kripto hari Jumat" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let requests = llm.requests();
    assert_eq!(requests.len(), 1, "one call, one prompt");
    let prompt = &requests[0].messages[0].content;

    assert!(
        prompt.contains("Asia/Jakarta"),
        "the student's zone must reach the model: {prompt}"
    );
    assert!(
        prompt.contains("<user_context_data>"),
        "the capture text must be delimited as data: {prompt}"
    );
    assert!(
        prompt.contains("selesaikan tugas kripto hari Jumat"),
        "the capture text must reach the model: {prompt}"
    );
    assert!(
        !prompt.contains("{{"),
        "an unfilled variable reaches the model as a literal: {prompt}"
    );
}

/// A breakdown writes no subtasks: accepting is a separate act (A195).
#[sqlx::test(migrations = "../../migrations")]
async fn a_breakdown_returns_a_proposal_and_writes_no_subtasks(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[BREAKDOWN_REPLY]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "breakdown@example.test").await;
    let user_id = upgrade_to_pro(&harness, "breakdown@example.test").await;
    let parent = insert_task(&harness, user_id, "Tugas kripto", None, 0).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/tasks/{parent}/breakdown"),
        &token,
        serde_json::json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let subtasks = body["proposal"]["subtasks"].as_array().expect("subtasks");
    assert!(
        subtasks.len() >= 3,
        "the contract's minimum is three: {body}"
    );
    assert_eq!(body["credits_spent"], 2, "{body}");
    assert_eq!(body["prompt_version"], "task_breakdown.v1", "{body}");

    assert_eq!(
        count(
            &harness.state.pool,
            "select count(*)::bigint from tasks where parent_id = $1",
            parent
        )
        .await,
        0,
        "a proposal must not write subtasks (A195)"
    );
}

/// Accepting is N ordinary creates, and each one is a real row with the parent and its
/// position, in the order proposed (A196).
///
/// The order is the assertion: a batch insert shares `created_at` to the microsecond, so
/// without `position` the list comes back in a different order than the model wrote it.
#[sqlx::test(migrations = "../../migrations")]
async fn accepting_a_proposal_creates_ordered_subtasks(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "accept@example.test").await;
    let user_id = upgrade_to_pro(&harness, "accept@example.test").await;
    let parent = insert_task(&harness, user_id, "Tugas kripto", None, 0).await;

    for (position, title) in ["Baca bab 4", "Kerjakan latihan", "Tinjau jawaban"]
        .iter()
        .enumerate()
    {
        let (status, body) = json(
            &app,
            "POST",
            "/tasks",
            &token,
            serde_json::json!({
                "title": title,
                "parent_id": parent,
                "position": position,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let (status, body) = json(
        &app,
        "GET",
        &format!("/tasks?parent_id={parent}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let titles: Vec<&str> = body["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| task["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Baca bab 4", "Kerjakan latihan", "Tinjau jawaban"],
        "the proposed order must survive the round trip: {body}"
    );
    let positions: Vec<i64> = body["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .map(|task| task["position"].as_i64().unwrap())
        .collect();
    assert_eq!(positions, [0, 1, 2], "{body}");
}

/// A task whose title is 500 characters is accepted; 501 is 400 validation.
///
/// The bound is asserted on both sides of the edge, because a check that is off by one is
/// exactly the kind of defect a single-sided test cannot see.
#[sqlx::test(migrations = "../../migrations")]
async fn the_title_bound_is_exact(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "title@example.test").await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "title": "a".repeat(500) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "title": "a".repeat(501) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("500"),
        "the message names the bound: {body}"
    );

    // The count is in characters, not bytes: a 400-character Indonesian title is 400
    // characters and more than 400 bytes, and a byte limit would cut it short.
    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "title": "é".repeat(400) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Deleting a parent removes its live children from every read, in one transaction.
///
/// Both halves are asserted: the rows are tombstoned, **and** `change_log` carries a delete
/// for each child. The second is what another device acts on, so a tombstone without a log
/// row is a child that stays visible on every other device.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_parent_tombstones_its_children(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "delete-tree@example.test").await;
    let user_id = upgrade_to_pro(&harness, "delete-tree@example.test").await;
    let parent = insert_task(&harness, user_id, "Induk", None, 0).await;
    let first = insert_task(&harness, user_id, "Anak satu", Some(parent), 0).await;
    let second = insert_task(&harness, user_id, "Anak dua", Some(parent), 1).await;

    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/tasks/{parent}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    for child in [first, second] {
        let deleted: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("select deleted_at from tasks where id = $1")
                .bind(child)
                .fetch_one(&harness.state.pool)
                .await
                .unwrap();
        assert!(deleted.is_some(), "the child {child} must be tombstoned");
    }

    for child in [first, second] {
        let logged: i64 = sqlx::query_scalar(
            "select count(*)::bigint from change_log
              where entity_id = $1 and op = 'delete'",
        )
        .bind(child)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        assert_eq!(logged, 1, "the child {child} must log its tombstone (A197)");
    }

    // And the tombstoned children are gone from the list, which is the consumer-visible half.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/tasks?parent_id={parent}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tasks"].as_array().unwrap().len(), 0, "{body}");
}

/// Another account's task is a 404, never a 403: the existence of a row is not the caller's
/// business.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_task_is_not_found(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mine = signed_up(&app, &harness, "mine@example.test").await;
    let theirs = signed_up(&app, &harness, "theirs@example.test").await;
    let other_user = upgrade_to_pro(&harness, "theirs@example.test").await;
    let task = insert_task(&harness, other_user, "Milik orang lain", None, 0).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/tasks?parent_id={task}"),
        &mine,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["tasks"].as_array().unwrap().len(),
        0,
        "another account's task must not be listed: {body}"
    );

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/tasks/{task}"),
        &mine,
        serde_json::json!({ "title": "Hijacked" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found", "{body}");

    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/tasks/{task}"),
        &mine,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // The row is untouched, which is what makes the 404 a refusal rather than a report.
    let title: String = sqlx::query_scalar("select title from tasks where id = $1")
        .bind(task)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(title, "Milik orang lain");

    let _ = theirs;
}

/// The prompt's settings come from the account, not from the request: a caller cannot make
/// the model answer in a language they did not choose, and `{{output_language}}` is never
/// left unfilled.
#[sqlx::test(migrations = "../../migrations")]
async fn the_breakdown_language_comes_from_the_settings_row(pool: PgPool) {
    let (harness, llm) = harness_with_llm(pool, &[BREAKDOWN_REPLY]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "language@example.test").await;
    let user_id = upgrade_to_pro(&harness, "language@example.test").await;
    let parent = insert_task(&harness, user_id, "Tugas kripto", None, 0).await;

    sqlx::query("insert into user_settings (user_id, language) values ($1, 'id')")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/tasks/{parent}/breakdown"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let prompt = &llm.requests()[0].messages[0].content;
    // The module's own line, and the JSON contract's, both carry the value: a breakdown
    // that came back in English for an Indonesian account is a silent failure, so the
    // assertion is on the rendered line rather than on the field name.
    assert!(
        prompt.contains("Write in id."),
        "the module's `{{{{output_language}}}}` must be filled from the settings row: {prompt}"
    );
    assert!(
        prompt.contains("Write all human-readable text in id."),
        "the shared JSON contract renders with the module's table too: {prompt}"
    );
    assert!(
        !prompt.contains("{{"),
        "an unfilled variable reaches the model as a literal: {prompt}"
    );
}

/// A `PATCH` that names one field leaves the rest alone, and can clear a nullable one.
///
/// The two halves are one test because they are one rule seen from both sides: a partial
/// write must neither reset what it did not mention nor be unable to say "no due date".
#[sqlx::test(migrations = "../../migrations")]
async fn a_partial_write_leaves_the_rest_alone(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "patch@example.test").await;
    let user_id = upgrade_to_pro(&harness, "patch@example.test").await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({
            "title": "Tugas kripto",
            "notes": "bab 4",
            "priority": 1,
            "due_at": "2026-11-01T09:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["task"]["id"].as_str().unwrap().to_string();

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/tasks/{id}"),
        &token,
        serde_json::json!({ "status": "done" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["task"]["status"], "done", "{body}");
    assert_eq!(body["task"]["notes"], "bab 4", "{body}");
    assert_eq!(body["task"]["priority"], 1, "{body}");
    assert!(
        !body["task"]["due_at"].is_null(),
        "a field the patch did not name keeps its value: {body}"
    );

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/tasks/{id}"),
        &token,
        serde_json::json!({ "due_at": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["task"]["due_at"].is_null(),
        "an explicit null clears the field: {body}"
    );
    assert_eq!(body["task"]["title"], "Tugas kripto", "{body}");
    assert_eq!(body["task"]["status"], "done", "{body}");

    let _ = user_id;
}

/// The status allowlist is the column's, and a value outside it is refused rather than
/// stored as a string no screen can render.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_status_is_refused(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "status@example.test").await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "title": "Tugas", "status": "almost" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");
}

/// A field the caller may not write is refused rather than silently dropped: a payload that
/// looks accepted while being ignored is how a forged value reaches a client's cache.
#[sqlx::test(migrations = "../../migrations")]
async fn a_server_owned_field_is_refused(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "owned@example.test").await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "title": "Tugas", "user_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("user_id"),
        "the message names the field: {body}"
    );
}

/// A task that is its own parent is a cycle in the one structure the Tasks screen walks.
#[sqlx::test(migrations = "../../migrations")]
async fn a_task_cannot_be_its_own_parent(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "self@example.test").await;
    let id = Uuid::now_v7();

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "id": id, "title": "Tugas", "parent_id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");
}

/// A client-minted id that is already taken is a 409, not a 500 from the primary key.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reused_id_is_a_conflict(pool: PgPool) {
    let (harness, _) = harness_with_llm(pool, &[]).await;
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "dup@example.test").await;
    let id = Uuid::now_v7();

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "id": id, "title": "Tugas" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({ "id": id, "title": "Tugas lagi" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict", "{body}");
}

/// A provider failure is not a charge for nothing: the reservation is refunded and the
/// caller sees the failure.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_capture_refunds_the_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::failing("the provider is down")));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "refund@example.test").await;
    let user_id = upgrade_to_pro(&harness, "refund@example.test").await;
    let before = balance(&harness.state.pool, user_id).await;

    let (status, body) = json(
        &app,
        "POST",
        "/tasks/quick-capture",
        &token,
        serde_json::json!({ "text": "selesaikan tugas kripto" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");

    assert_eq!(
        balance(&harness.state.pool, user_id).await,
        before,
        "a provider failure must leave the balance where it was"
    );
}

/// Inserts a task directly, so a test can start from a row without going through the route
/// it is about to assert on.
async fn insert_task(
    harness: &TestState,
    user_id: Uuid,
    title: &str,
    parent_id: Option<Uuid>,
    position: i32,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into tasks (id, user_id, title, parent_id, position) values ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(user_id)
    .bind(title)
    .bind(parent_id)
    .bind(position)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}
