//! Phase 12 Task 7: the agent turn — reads execute, writes propose, and both are answered.
//!
//! Every case here runs against `ScriptedLlm`, so no network and no provider key. The
//! properties are:
//!
//!  * **a READ tool runs and its result reaches the model** — otherwise the student paid for a
//!    tool whose answer nobody read;
//!  * **a WRITE tool proposes and never executes** (A271) — the gate's whole point;
//!  * **the offered tool list is the plan's** (A272) — a free account is offered four reads and
//!    no write, which is the anti-vacuity half of A279;
//!  * **`agent_turn` is charged once per turn, not once per call** (A282).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::llm::ToolCall;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up, sse_events};

/// A signed-up account on `plan`, with a live subscription.
///
/// `agent_actions` is false on free and true on pro, so which plan the account is on decides
/// whether the wire carries the six WRITE tools (A272).
async fn account(harness: &TestState, email: &str, plan: &str) -> (axum::Router, String, Uuid) {
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, harness, email).await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    if plan != "free" {
        sqlx::query("update users set plan_id = $2 where id = $1")
            .bind(user_id)
            .bind(plan)
            .execute(&harness.state.pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into subscriptions
                (user_id, plan_id, status, billing_period, current_period_start,
                 current_period_end)
             values ($1, $2, 'active', 'monthly', now(), now() + interval '30 days')",
        )
        .bind(user_id)
        .bind(plan)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }
    (app, token, user_id)
}

/// A workspace, so the chat route has one to address.
async fn workspace(app: &axum::Router, token: &str, name: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["workspace"]["id"].as_str().unwrap().to_string()
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    }
}

/// The ledger total for one action, which is what "charged once" is measured against.
async fn charged(harness: &TestState, user_id: Uuid, action: &str) -> i64 {
    let rows: Vec<(i32,)> =
        sqlx::query_as("select credits from usage_events where user_id = $1 and action = $2")
            .bind(user_id)
            .bind(action)
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    rows.iter().map(|(credits,)| i64::from(*credits)).sum()
}

async fn usage_rows(harness: &TestState, user_id: Uuid, action: &str) -> i64 {
    sqlx::query_scalar("select count(*) from usage_events where user_id = $1 and action = $2")
        .bind(user_id)
        .bind(action)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// A READ tool runs inline, its result reaches the model, and nothing is written.
#[sqlx::test(migrations = "../../migrations")]
async fn a_read_tool_executes_and_its_result_reaches_the_model(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = account(&harness, "read@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Read").await;

    // A task the read tool must find, so its result is non-empty and the assertion below is
    // about the row rather than about an empty list.
    sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, 'Baca bab 3')")
        .bind(Uuid::now_v7())
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "list_tasks",
        serde_json::json!({}),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "What are my tasks?" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    // The second provider call is the one the tool result caused, and it carries the row. This
    // is the assertion that the read's answer actually reached the model: without the follow-up
    // round the tool ran and its result went nowhere.
    let requests = llm.requests();
    assert!(
        requests.len() >= 2,
        "a read tool's result must cause a second call: {} request(s)",
        requests.len()
    );
    let follow_up = requests[1]
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        follow_up.contains("Baca bab 3"),
        "the tool result must reach the model: {follow_up}"
    );
    assert!(
        follow_up.contains("list_tasks"),
        "the result names the tool it answers: {follow_up}"
    );

    // A read writes nothing: no proposal, no new task.
    let proposals: i64 = sqlx::query_scalar("select count(*) from pending_actions")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(proposals, 0, "a read tool must not propose anything");
    let tasks: i64 = sqlx::query_scalar("select count(*) from tasks")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(tasks, 1, "the only task is the fixture's");
}

/// A WRITE tool creates the proposal, streams the `action` event, and writes no task.
#[sqlx::test(migrations = "../../migrations")]
async fn a_write_tool_proposes_and_never_executes(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, _) = account(&harness, "write@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Write").await;

    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "create_task",
        serde_json::json!({ "title": "Beli buku" }),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "Add a task" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    // The card is streamed, with the roadmap § 4 body and nothing else (A277).
    let action = events
        .iter()
        .find(|event| event.kind() == "action")
        .expect("an action event");
    assert_eq!(action.json()["tool"], "create_task");
    assert_eq!(action.json()["args"]["title"], "Beli buku");
    assert!(
        action.json().get("cost").is_none(),
        "the action event's shape is fixed: the card reads the cost from plans.ts (A277)"
    );
    assert!(action.json()["pending_action_id"].is_string());

    // The property the whole phase turns on: the write did not execute.
    let tasks: i64 = sqlx::query_scalar("select count(*) from tasks")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(tasks, 0, "a WRITE tool never executes (A271)");

    let proposals: i64 = sqlx::query_scalar("select count(*) from pending_actions")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert!(proposals >= 1, "the proposal was stored");
}

/// The anti-vacuity half of A279, first side: the offered tool list is what the plan grants.
/// A free account is offered four reads and no write, so a model cannot propose an action the
/// server would refuse (A272).
#[sqlx::test(migrations = "../../migrations")]
async fn a_free_account_is_offered_only_the_read_tools(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, _) = account(&harness, "free-tools@example.test", "free").await;
    let workspace_id = workspace(&app, &token, "Free").await;

    let llm = Arc::new(ScriptedLlm::new(vec!["nothing to add".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let offered: Vec<String> = llm.requests()[0]
        .tools
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert_eq!(
        offered.len(),
        4,
        "a free account is offered the four reads: {offered:?}"
    );
    for read in [
        "list_tasks",
        "find_free_slots",
        "get_due_flashcards",
        "get_upcoming_deadlines",
    ] {
        assert!(
            offered.contains(&read.to_string()),
            "{read} missing: {offered:?}"
        );
    }
    for write in [
        "create_task",
        "update_task",
        "create_event",
        "schedule_study_block",
        "create_flashcards",
        "create_quiz",
    ] {
        assert!(
            !offered.contains(&write.to_string()),
            "{write} must not be offered on free: {offered:?}"
        );
    }
}

/// The other half of the same property: a plan that grants `AgentAction` is offered all ten.
///
/// Without this, `a_free_account_is_offered_only_the_read_tools` would pass on a route that
/// sent no tools at all — which is what this route did before this phase.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pro_account_is_offered_all_ten_tools(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, _) = account(&harness, "pro-tools@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Pro").await;

    let llm = Arc::new(ScriptedLlm::new(vec!["nothing to add".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, _) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let offered: Vec<String> = llm.requests()[0]
        .tools
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert_eq!(
        offered.len(),
        10,
        "pro is offered all ten tools (A272): {offered:?}"
    );
    for write in ["create_task", "create_flashcards", "create_quiz"] {
        assert!(
            offered.contains(&write.to_string()),
            "{write} missing: {offered:?}"
        );
    }
}

/// A hallucinated tool name is refused and answered, not dropped.
///
/// "Answered" is the load-bearing word: the model is told, so it can say so in the student's
/// language. The pre-phase behaviour — logging and dropping — left the model answering as if
/// the call had succeeded.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_tool_name_is_refused_and_answered(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, _) = account(&harness, "unknown@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Unknown").await;

    // `confirm_action` is the name a poisoned document would try, and it is not a tool.
    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "confirm_action",
        serde_json::json!({ "id": "1" }),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "do it" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    // The refusal reached the model through the follow-up round.
    let requests = llm.requests();
    assert!(
        requests.len() >= 2,
        "a refusal is answered, so the model is called again"
    );
    let follow_up = requests[1]
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        follow_up.contains("confirm_action"),
        "the refusal names the call: {follow_up}"
    );
    assert!(
        follow_up.contains("did not run"),
        "the model is told the call did not run: {follow_up}"
    );

    // Nothing was proposed and nothing was written.
    let proposals: i64 = sqlx::query_scalar("select count(*) from pending_actions")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(proposals, 0, "an unknown tool has nothing to propose");
    assert!(
        !events.iter().any(|event| event.kind() == "action"),
        "and no card to stream: {events:?}"
    );
}

/// `agent_turn` is charged once, on the first call, on top of `chat_message` (A282).
#[sqlx::test(migrations = "../../migrations")]
async fn the_agent_turn_is_charged_once_however_many_tools_run(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = account(&harness, "charge@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Charge").await;

    // A read tool, so several rounds run (the double emits the call every round) and none of
    // them proposes anything: the charge must be one per turn regardless of the round count.
    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "list_tasks",
        serde_json::json!({}),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "What is due?" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");
    assert!(
        llm.requests().len() >= 2,
        "several rounds ran, or this test proves nothing about the count"
    );

    // `chat_message` once, `agent_turn` once — not once per round.
    assert_eq!(usage_rows(&harness, user_id, "chat_message").await, 1);
    assert_eq!(
        usage_rows(&harness, user_id, "agent_turn").await,
        1,
        "the agent turn is charged once per turn, whatever the round count (A282)"
    );
    let agent_cost = charged(&harness, user_id, "agent_turn").await;
    assert!(
        agent_cost > 0,
        "the charge is the matrix's price, not zero: {agent_cost}"
    );

    // The `usage` event reports the whole turn's price, so the client shows what was spent.
    let usage = events
        .iter()
        .find(|event| event.kind() == "usage")
        .expect("a usage event");
    let chat_cost = charged(&harness, user_id, "chat_message").await;
    assert_eq!(
        usage.json()["credits_spent"].as_i64().unwrap(),
        chat_cost + agent_cost,
        "the turn's price is chat_message + agent_turn (A282)"
    );
}

/// A turn with no tool call costs what it costs today.
#[sqlx::test(migrations = "../../migrations")]
async fn a_turn_with_no_tool_call_costs_one_credit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = account(&harness, "notools@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "NoTools").await;

    let llm = Arc::new(ScriptedLlm::new(vec!["An ordinary answer.".to_string()]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "hello" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    assert_eq!(usage_rows(&harness, user_id, "chat_message").await, 1);
    assert_eq!(
        usage_rows(&harness, user_id, "agent_turn").await,
        0,
        "a turn that called no tool must not be charged for one (A282)"
    );
    let usage = events
        .iter()
        .find(|event| event.kind() == "usage")
        .expect("a usage event");
    assert_eq!(
        usage.json()["credits_spent"].as_i64().unwrap(),
        charged(&harness, user_id, "chat_message").await
    );
}

/// A turn whose balance cannot cover the tool call is told so, and the model says it: the call
/// is refused rather than silently not running (A282).
#[sqlx::test(migrations = "../../migrations")]
async fn a_turn_that_cannot_afford_the_agent_turn_is_told_so(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, user_id) = account(&harness, "poor@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Poor").await;

    // The bucket is set to **exactly** the message's own price, so the `chat_message` spend
    // succeeds and the balance is then zero — the state the `agent_turn` charge has to fail in.
    //
    // It is a computed delta rather than a fixed one because signup grants a monthly allowance:
    // a literal `+1` would leave that grant in place, the agent turn would be affordable, and
    // the test would assert nothing. A negative net balance would refuse the *message* instead,
    // so the test would be about the wrong refusal.
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason)
         select $1, (1 - coalesce(sum(delta), 0))::int, 'monthly', 'test'
           from credit_ledger where user_id = $1 and bucket = 'monthly'",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let balance: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        balance, 1,
        "the fixture must leave exactly the message's price across every bucket"
    );

    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "create_task",
        serde_json::json!({ "title": "Too expensive" }),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "add a task" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    // The refusal reaches the model, which is what lets it tell the student to top up.
    let requests = llm.requests();
    assert!(
        requests.len() >= 2,
        "the refusal is answered, so the model is called again"
    );
    let follow_up = requests[1]
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        follow_up.contains("credits"),
        "the model is told about the balance: {follow_up}"
    );
    // The sentence arrives as a `tool_result` that names the call it answers — the tool and
    // the call id — which is what makes it the refusal *path* rather than a remark about the
    // conversation the model could mistake for its own thought.
    assert!(
        follow_up.contains("create_task"),
        "the refusal names the tool it answers: {follow_up}"
    );
    assert!(
        follow_up.contains("call_1"),
        "the refusal names the call id it answers: {follow_up}"
    );

    let ledger: Vec<(String, i32, String)> = sqlx::query_as(
        "select bucket, delta, reason from credit_ledger where user_id = $1 order by created_at",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    let usage: Vec<(String, i32)> = sqlx::query_as(
        "select action, credits from usage_events where user_id = $1 order by created_at",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    eprintln!("LEDGER={ledger:?}\nUSAGE={usage:?}");

    // And no proposal was written: the call did not run.
    let proposals: i64 = sqlx::query_scalar("select count(*) from pending_actions")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        proposals, 0,
        "an unaffordable call does not become a proposal"
    );
    let tasks: i64 = sqlx::query_scalar("select count(*) from tasks")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(tasks, 0);
}

/// A turn that only called a tool persists its message, so the card has a home (A281).
#[sqlx::test(migrations = "../../migrations")]
async fn a_tool_only_turn_persists_its_message(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, _) = account(&harness, "toolonly@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "ToolOnly").await;

    // A tool-calling double with no scripted text: the turn produces a proposal and no prose,
    // which is the shape the old "a blank answer is not persisted" rule dropped.
    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "create_task",
        serde_json::json!({ "title": "From a card" }),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "add it" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    // `done` arrived rather than `error`, which is the difference between "the card has a
    // message" and "the client is told the model said nothing".
    let kinds: Vec<&str> = events.iter().map(|e| e.kind()).collect();
    assert!(
        kinds.contains(&"done"),
        "a tool-only turn is not an empty turn: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"error"),
        "and it is not reported as a model failure: {kinds:?}"
    );

    // The assistant message exists with empty content, which is what the card hangs on.
    let content: String = sqlx::query_scalar(
        "select content from messages where workspace_id = $1 and role = 'assistant'",
    )
    .bind(Uuid::parse_str(&workspace_id).unwrap())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(content, "", "the tool-only turn's message is empty (A281)");
}

/// The proposal remembers the turn it came from, which is what anchors the card.
#[sqlx::test(migrations = "../../migrations")]
async fn the_proposal_records_the_turn_it_came_from(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let (app, token, _) = account(&harness, "anchor@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Anchor").await;

    let llm = Arc::new(ScriptedLlm::tool_calling(vec![call(
        "call_1",
        "create_task",
        serde_json::json!({ "title": "Anchored" }),
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "add it" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let action_id = events
        .iter()
        .find(|event| event.kind() == "action")
        .expect("an action event")
        .json()["pending_action_id"]
        .as_str()
        .unwrap()
        .to_string();

    // The anchor is the turn's user message, which exists before the model is called — the
    // assistant message is persisted after the stream, so it has no id at proposal time.
    let message_id: Option<Uuid> =
        sqlx::query_scalar("select message_id from pending_actions where id = $1")
            .bind(Uuid::parse_str(&action_id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let message_id = message_id.expect("the proposal records its turn");

    let role: String = sqlx::query_scalar("select role from messages where id = $1")
        .bind(message_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(role, "user", "the anchor is the turn's own user message");

    // The list endpoint surfaces it, so the card has a home on every device.
    let (status, body) = json(
        &app,
        "GET",
        "/actions/pending",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["actions"][0]["message_id"], message_id.to_string());
}
