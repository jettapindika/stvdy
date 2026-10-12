//! Phase 12 Tasks 5-6: the pending list, the lazy expiry, the reject, and the confirm.
//!
//! Two properties are the point of this file, and both are asserted rather than described:
//!
//!  * **the confirm path has no second implementation** (A276) — `the_executor_contains_no_dml`
//!    reads `src/actions/execute.rs` and fails on any DML, and
//!    `a_confirmed_proposal_creates_the_row_the_route_creates` asserts the behavioural half: the
//!    row a confirm writes is the row `POST /tasks` writes. Phase 9's offline gate found four
//!    defects of this class (two write paths disagreeing), so the mechanical test is
//!    load-bearing and not a lint.
//!  * **two taps produce one row** (A275) — `a_second_confirm_is_a_conflict_and_writes_nothing`
//!    asserts one task exists and one 409 came back, and
//!    `the_execution_shares_the_claims_transaction` asserts a failure inside the executor rolls
//!    the claim back, so a `confirmed` row can never be a row that did nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// The confirm path's bounds are the **tool's**, not the route's (30 cards / 20 questions in
/// `crates/core/src/agent/tools.rs` against 50/50 in the routes). A proposal cannot reach the
/// route's bound, so a test asserting it would exercise an unreachable branch — see
/// `an_edit_is_validated_by_the_same_contract` for the bound that actually fires.

/// The app plus a signed-up account whose plan grants `Feature::AgentAction`.
///
/// `agent_actions` is false on free, so a free account's confirm is a 403 before the row is
/// looked up; every test below is about what happens *after* that check, so the account is on
/// pro. The plan is written directly rather than bought because there is no purchase path in a
/// test, which is the same reason the sweep's fixture writes rows.
async fn pro_account(harness: &TestState, email: &str) -> (axum::Router, String, Uuid) {
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, harness, email).await;
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
    (app, token, user_id)
}

/// A second device for an existing account, via login. Returns its token and its device id.
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

/// Inserts a proposal directly, so a test can put a row in a state the API would not.
///
/// `expires_in_minutes` is signed: negative means already expired, which is the state
/// `an_expired_proposal_cannot_be_confirmed` needs and which no route can produce.
async fn seed_action(
    harness: &TestState,
    user_id: Uuid,
    tool: &str,
    args: serde_json::Value,
    expires_in_minutes: i64,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into pending_actions (id, user_id, tool, args, status, expires_at)
         values ($1, $2, $3, $4, 'pending', now() + ($5 * interval '1 minute'))",
    )
    .bind(id)
    .bind(user_id)
    .bind(tool)
    .bind(args)
    .bind(expires_in_minutes)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}

async fn action_status(harness: &TestState, id: Uuid) -> String {
    sqlx::query_scalar("select status from pending_actions where id = $1")
        .bind(id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn task_count(harness: &TestState, title: &str) -> i64 {
    sqlx::query_scalar("select count(*) from tasks where title = $1")
        .bind(title)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------------------
// Task 5: the list, the lazy expiry, the reject
// ---------------------------------------------------------------------------------------

/// The list is scoped, and another account's proposal is a 404 rather than an empty list.
#[sqlx::test(migrations = "../../migrations")]
async fn the_pending_list_is_the_callers_own_rows(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "list@example.test").await;

    let mine = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Mine" }),
        30,
    )
    .await;

    // A second account's proposal must not be visible, and must not be reachable by id either.
    let (_, _, other_id) = pro_account(&harness, "other@example.test").await;
    let theirs = seed_action(
        &harness,
        other_id,
        "create_task",
        serde_json::json!({ "title": "Theirs" }),
        30,
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/actions/pending",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let actions = body["actions"].as_array().expect("an array");
    assert_eq!(actions.len(), 1, "only the caller's own rows: {body}");
    assert_eq!(actions[0]["id"], mine.to_string());
    assert_eq!(actions[0]["tool"], "create_task");
    assert_eq!(actions[0]["status"], "pending");
    // The args are what the model produced, verbatim: the card renders them, so a shape
    // change here is a shape change on screen.
    assert_eq!(actions[0]["args"]["title"], "Mine");

    // Rejecting another account's row is a 404, not a 403: a 403 would confirm the id exists.
    let (status, _) = json(
        &app,
        "POST",
        &format!("/actions/{theirs}/reject"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(action_status(&harness, theirs).await, "pending");
}

/// Expiry is lazy and it is the server's: a row past `expires_at` reads `expired` on the next
/// list, with no job having run (A274).
#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_proposal_expires_on_the_next_read(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "stale@example.test").await;

    let stale = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Stale" }),
        -1,
    )
    .await;
    assert_eq!(action_status(&harness, stale).await, "pending");

    let (status, body) = json(
        &app,
        "GET",
        "/actions/pending",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The row is `expired` in the database after the read, which is the assertion that no
    // scheduler is involved: the GET did it.
    assert_eq!(action_status(&harness, stale).await, "expired");
    let actions = body["actions"].as_array().expect("an array");
    assert_eq!(actions.len(), 1, "{body}");
    assert_eq!(actions[0]["status"], "expired");
}

/// A resolved proposal stays visible for a day, so the device that did not resolve it sees what
/// happened instead of a card that disappeared.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resolved_proposal_is_still_readable_for_a_day(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "resolved@example.test").await;

    // Resolved an hour ago: inside the window, so it must be listed.
    let recent = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Recent" }),
        30,
    )
    .await;
    sqlx::query(
        "update pending_actions set status = 'confirmed', resolved_at = now() - interval '1 hour'
          where id = $1",
    )
    .bind(recent)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // Resolved 25 hours ago: outside it, so it must not be. Both halves are asserted, because
    // a window that only ever grows is the same as no window.
    let old = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Old" }),
        30,
    )
    .await;
    sqlx::query(
        "update pending_actions set status = 'rejected', resolved_at = now() - interval '25 hours'
          where id = $1",
    )
    .bind(old)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/actions/pending",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let actions = body["actions"].as_array().expect("an array");
    let ids: Vec<String> = actions
        .iter()
        .map(|action| action["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        ids.contains(&recent.to_string()),
        "the recent resolution is visible: {body}"
    );
    assert!(
        !ids.contains(&old.to_string()),
        "a day-old resolution is not: {body}"
    );
}

/// Rejecting is single-winner, like confirming: the second attempt is 409 and the first
/// device's resolution stands.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_reject_is_a_conflict(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "reject@example.test").await;

    let id = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Nope" }),
        30,
    )
    .await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/reject"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"]["status"], "rejected");
    assert_eq!(action_status(&harness, id).await, "rejected");

    // The second attempt is told, not silently accepted: the client's remedy is to re-read.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/reject"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict");
    assert!(
        body["message"].as_str().unwrap().contains("rejected"),
        "the refusal names the current status: {body}"
    );
    assert_eq!(action_status(&harness, id).await, "rejected");
}

/// The resolution carries the device that made it, which is the exit gate's "device B".
#[sqlx::test(migrations = "../../migrations")]
async fn the_resolution_names_the_device_that_made_it(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, _, user_id) = pro_account(&harness, "deviceb@example.test").await;
    let (token_b, device_b) = second_device(&app, "deviceb@example.test", "Phone").await;

    let id = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "From B" }),
        30,
    )
    .await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/reject"),
        &token_b,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["action"]["resolved_by_device_id"],
        device_b.to_string()
    );

    let stored: Option<Uuid> =
        sqlx::query_scalar("select resolved_by_device_id from pending_actions where id = $1")
            .bind(id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, Some(device_b));
}

// ---------------------------------------------------------------------------------------
// Task 6: the confirm
// ---------------------------------------------------------------------------------------

/// The exit gate's first half: a proposal created by the agent turn is executed by a confirm,
/// and the row it produced is the row `POST /tasks` produces.
#[sqlx::test(migrations = "../../migrations")]
async fn a_confirmed_proposal_creates_the_row_the_route_creates(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "confirm@example.test").await;

    let args = serde_json::json!({
        "title": "Read chapter 4",
        "priority": 1,
        "estimate_min": 45
    });
    let id = seed_action(&harness, user_id, "create_task", args, 30).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"]["status"], "confirmed");
    // The card shows what the confirm produced, so the result is the route's own shape.
    assert_eq!(body["action"]["result"]["task"]["title"], "Read chapter 4");

    // The same body through the route, to compare row against row. The point is not that both
    // exist but that they agree: a second implementation would differ in priority, in status or
    // in the defaulted fields, and this is where it would show.
    let (status, direct) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({
            "title": "Read chapter 4",
            "priority": 1,
            "estimate_min": 45
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{direct}");

    let confirmed = &body["action"]["result"]["task"];
    let routed = &direct["task"];
    for field in ["priority", "status", "position", "estimate_min"] {
        assert_eq!(
            confirmed[field], routed[field],
            "a confirm and the route must agree about {field}: {confirmed} vs {routed}"
        );
    }
    assert_eq!(confirmed["title"], routed["title"]);
    // One transaction, one `change_log` row per write: the confirm path is a writer like any
    // other, so the second device learns about the task through the ordinary pull.
    let logged: i64 = sqlx::query_scalar(
        "select count(*) from change_log where entity = 'tasks' and user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(logged, 2, "one change_log row per task, confirm included");
}

/// The exit gate's cross-device half: the confirm is made by device B and the row records it.
#[sqlx::test(migrations = "../../migrations")]
async fn confirming_on_a_second_device_names_that_device(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, _, user_id) = pro_account(&harness, "crossdevice@example.test").await;
    let (token_b, device_b) = second_device(&app, "crossdevice@example.test", "Tablet").await;

    let id = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Confirmed on B" }),
        30,
    )
    .await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token_b,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["action"]["resolved_by_device_id"],
        device_b.to_string()
    );

    // And device A reads both facts: the new task and who confirmed it.
    let (status, body) = json(
        &app,
        "GET",
        "/actions/pending",
        &token_b,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let action = &body["actions"][0];
    assert_eq!(action["status"], "confirmed");
    assert_eq!(action["resolved_by_device_id"], device_b.to_string());

    let (_, tasks) = json(&app, "GET", "/tasks", &token_b, serde_json::Value::Null).await;
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 1, "{tasks}");
    assert_eq!(tasks["tasks"][0]["title"], "Confirmed on B");
}

/// Two taps, one task (the third risk in the plan's opening section).
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_confirm_is_a_conflict_and_writes_nothing(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "twotaps@example.test").await;

    let id = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "One task" }),
        30,
    )
    .await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict");

    // The assertion the guarded update exists for. A read-then-write would have written twice
    // here, and a test that only checked the status code would still pass with a second row.
    assert_eq!(task_count(&harness, "One task").await, 1);
}

/// A failure inside the executor rolls the claim back, so a `confirmed` row can never be a row
/// that did nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn the_execution_shares_the_claims_transaction(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "rollback@example.test").await;

    // A workspace id that is nobody's: `validate_task` accepts it (a uuid is a uuid) and the
    // insert fails on the foreign key, which is a failure *inside* the executor.
    let args = serde_json::json!({
        "title": "Doomed",
        "workspace_id": Uuid::now_v7().to_string()
    });
    let id = seed_action(&harness, user_id, "create_task", args, 30).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // The claim rolled back with it: the proposal is still actionable, so the student can fix
    // the workspace and retry rather than being left with a card that claims to be confirmed.
    assert_eq!(action_status(&harness, id).await, "pending");
    assert_eq!(task_count(&harness, "Doomed").await, 0);

    // And the retry with a valid body succeeds, which is what "still actionable" means.
    let good = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Doomed" }),
        30,
    )
    .await;
    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{good}/confirm"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(task_count(&harness, "Doomed").await, 1);
}

/// An edit is re-validated by the same contract the proposal was: a title past the tool's bound
/// is a 400 naming the field, and the row stays `pending`.
///
/// The bound asserted is the **tool's** (`MAX_TITLE` via `parse_args`), not the route's: a
/// proposal's count or title can never exceed the tool's bound, because `gate` refused it
/// before a row existed, so the route's check is unreachable from this path.
#[sqlx::test(migrations = "../../migrations")]
async fn an_edit_is_validated_by_the_same_contract(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "edit@example.test").await;

    let id = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Original" }),
        30,
    )
    .await;

    // A title past the contract's bound. `parse_args` refuses it, which is the same refusal a
    // proposal with that title would have met — an edit cannot widen what a proposal could say.
    let long = "x".repeat(10_000);
    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({ "args": { "title": long } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation");
    assert!(
        body["message"].as_str().unwrap().contains("title"),
        "the refusal names the field: {body}"
    );
    assert_eq!(action_status(&harness, id).await, "pending");
    assert_eq!(task_count(&harness, "Original").await, 0);

    // A `due_at` without an offset is refused too: assuming UTC would silently place a Jakarta
    // student's 19:00 at 02:00 the next morning.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({ "args": { "title": "Fine", "due_at": "2026-11-01T19:00:00" } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("due_at"),
        "the refusal names the field: {body}"
    );
    assert_eq!(action_status(&harness, id).await, "pending");

    // A valid edit goes through, and the row carries the edit rather than what was proposed.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({ "args": { "title": "Edited" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"]["result"]["task"]["title"], "Edited");
    assert_eq!(task_count(&harness, "Edited").await, 1);
    assert_eq!(task_count(&harness, "Original").await, 0);
}

/// An expired proposal cannot be confirmed even though the lazy expiry has not run: the claim's
/// `expires_at > now()` is the enforcement, and the list is only the convenience.
#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_proposal_cannot_be_confirmed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = pro_account(&harness, "expired@example.test").await;

    let id = seed_action(
        &harness,
        user_id,
        "create_task",
        serde_json::json!({ "title": "Too late" }),
        -5,
    )
    .await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/actions/{id}/confirm"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(task_count(&harness, "Too late").await, 0);
    // The list has not run, so the row still reads `pending` in the database — which is exactly
    // why the claim re-checks the deadline itself rather than trusting the status.
    assert_eq!(action_status(&harness, id).await, "pending");

    // The lazy expiry then agrees with the claim, so the card stops being offered at all.
    let (status, _) = json(
        &app,
        "GET",
        "/actions/pending",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(action_status(&harness, id).await, "expired");
}

/// The mechanical half of A276.
///
/// `execute.rs` must contain no DML on a domain table: every arm calls the service function the
/// REST route calls, so a confirmed task is the row `POST /tasks` writes. A second
/// implementation is a second set of validation, and Phase 9's offline gate found four defects
/// of exactly this class — every one of them two write paths disagreeing.
///
/// The test reads the source rather than the behaviour on purpose: the behavioural half
/// (`a_confirmed_proposal_creates_the_row_the_route_creates`) only covers the arms a test
/// exercises, while this covers a new arm the moment it is written.
#[test]
fn the_executor_contains_no_dml() {
    let source = include_str!("../src/actions/execute.rs");
    for forbidden in ["insert into", "update ", "delete from"] {
        assert!(
            !source.contains(forbidden),
            "execute.rs must not contain `{forbidden}`: a confirm calls the route's own \
             service function, never its own statement (A276)"
        );
    }
}
