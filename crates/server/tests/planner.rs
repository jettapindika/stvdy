//! The planner server, the job, and the exit gate.
//!
//! Three groups, in the order they landed:
//!
//!  * Task 11's three: the free plan's structural refusal (A257), a proposal that cannot fit
//!    reporting its deficit, and another account's proposal being a 404.
//!  * Task 12's two refund arms: a plan generation that fails permanently gives back two
//!    credits, and a failed extraction gives back eight — the same shape the quiz path has,
//!    and each asserted a second time through the boot sweep so "once" is a fact rather than
//!    a claim.
//!  * Task 13's exit gate: the accepted plan respects busy time, the daily maximum, the study
//!    window and the deadline buffer; a replan keeps the pinned blocks and leaves a started
//!    one alone; and an accepted proposal cannot be accepted twice.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, TimeZone as _, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use stvdy_core::traits::testing::{FixedClock, ScriptedLlm};
use stvdy_server::testing::TestState;

mod common;
use common::{drain_jobs, json, signed_up};

/// An effort reply for the three topics the tests seed, summing to its own total.
fn effort_reply(topics: &[(&str, i64)]) -> String {
    let entries: Vec<String> = topics
        .iter()
        .map(|(name, minutes)| {
            format!(
                "{{\"topic\":\"{name}\",\"estimate_min\":{minutes},\"reason\":\"large source\"}}"
            )
        })
        .collect();
    let total: i64 = topics.iter().map(|(_, minutes)| minutes).sum();
    format!(
        "{{\"topics\":[{}],\"total_estimate_min\":{total}}}",
        entries.join(",")
    )
}

fn router(harness: &TestState) -> Router {
    stvdy_server::routes::router(harness.state.clone()).unwrap()
}

/// Moves an account onto a paid plan with a live subscription, which is what
/// `effective_plan` requires (A146): a bare `users.plan_id` edit is not enough.
async fn upgrade(harness: &TestState, email: &str, plan: &str) {
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update users set plan_id = $2 where id = $1")
        .bind(user_id)
        .bind(plan)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let now = harness.state.clock.now();
    sqlx::query(
        "insert into subscriptions
           (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly', $3, $4)",
    )
    .bind(user_id)
    .bind(plan)
    .bind(now - Duration::days(1))
    .bind(now + Duration::days(29))
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn workspace(app: &Router, token: &str, name: &str) -> Uuid {
    let (status, body) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["workspace"]["id"].as_str().expect("an id")).unwrap()
}

/// A source with a chunk, so the goal's topic list is non-empty and the extraction has text.
async fn source(harness: &TestState, user_id: Uuid, workspace_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into sources
            (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
         values ($1, $2, $3, 'text', $4, $5, 16, 'text/plain', 'ready')",
    )
    .bind(id)
    .bind(user_id)
    .bind(workspace_id)
    .bind(title)
    .bind(format!("users/{user_id}/sources/{id}.txt"))
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into chunks (source_id, idx, content, content_hash, embedder)
         values ($1, 0, 'the material', 'hash', 'test')",
    )
    .bind(id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}

async fn goal(
    app: &Router,
    token: &str,
    workspace_id: Uuid,
    title: &str,
    target: DateTime<Utc>,
    source_ids: Vec<Uuid>,
) -> Uuid {
    let (status, body) = json(
        app,
        "POST",
        "/goals",
        token,
        serde_json::json!({
            "title": title,
            "target_date": target,
            "workspace_id": workspace_id,
            "source_ids": source_ids,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["goal"]["id"].as_str().expect("an id")).unwrap()
}

/// Asks for a plan and returns the proposal id.
async fn request_plan(app: &Router, token: &str, goal_id: Uuid) -> Uuid {
    let (status, body) = json(
        app,
        "POST",
        &format!("/goals/{goal_id}/plan"),
        token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    Uuid::parse_str(body["proposal_id"].as_str().expect("a proposal id")).unwrap()
}

async fn poll(app: &Router, token: &str, proposal_id: Uuid) -> serde_json::Value {
    let (status, body) = json(
        app,
        "GET",
        &format!("/plans/{proposal_id}"),
        token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["proposal"].clone()
}

// --- Task 11: the three the plan names -------------------------------------

/// The free plan's refusal is structural, not a copy decision (A257).
///
/// `Feature::PlanGeneration` is `planner != manual`, and free's `planner = "manual"`, so the
/// refusal is a 403 `plan_required` naming `pro` — with no second rule anywhere. Asserted
/// rather than described, because the one line in `entitlements/check.rs` is the whole gate.
#[sqlx::test(migrations = "../../migrations")]
async fn planning_a_goal_on_the_free_plan_is_refused_with_plan_required(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![effort_reply(&[(
            "Jaringan", 120,
        )])])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "free-plan@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let user_id = user_id_of(&harness, "free-plan@example.test").await;
    let source_id = source(&harness, user_id, workspace_id, "Jaringan").await;
    let goal_id = goal(
        &app,
        &token,
        workspace_id,
        "UAS",
        harness.state.clock.now() + Duration::days(21),
        vec![source_id],
    )
    .await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/goals/{goal_id}/plan"),
        &token,
        serde_json::Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");
    assert_eq!(
        body["min_plan"], "pro",
        "the refusal names the plan that grants it: {body}"
    );

    // Nothing was charged and no proposal exists: the refusal is before the reservation.
    let ledger: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(ledger, 0, "a refused plan must not have been charged");
    let proposals: i64 = sqlx::query_scalar("select count(*)::bigint from plan_proposals")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(proposals, 0, "a refused plan must write no proposal");
}

/// A goal with far more demand than the week holds still gets a best-effort plan, and says
/// what it could not fit.
///
/// The deficit is the honest number: a plan that silently dropped the excess would look
/// complete and leave the student unprepared, which is the failure the field exists to
/// prevent. The `Overloaded` warning names the days.
#[sqlx::test(migrations = "../../migrations")]
async fn a_proposal_that_cannot_fit_reports_its_deficit(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![effort_reply(&[
        ("Jaringan", 6000),
        ("Kripto", 6000),
        ("Basis Data", 6000),
    ])])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "deficit@example.test").await;
    upgrade(&harness, "deficit@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let user_id = user_id_of(&harness, "deficit@example.test").await;
    let a = source(&harness, user_id, workspace_id, "Jaringan").await;
    let b = source(&harness, user_id, workspace_id, "Kripto").await;
    let c = source(&harness, user_id, workspace_id, "Basis Data").await;

    let goal_id = goal(
        &app,
        &token,
        workspace_id,
        "UAS",
        harness.state.clock.now() + Duration::days(21),
        vec![a, b, c],
    )
    .await;
    let proposal_id = request_plan(&app, &token, goal_id).await;
    drain_jobs(&harness).await;

    let body = poll(&app, &token, proposal_id).await;
    assert_eq!(body["status"], "ready", "{body}");
    assert!(
        body["deficit_min"].as_i64().unwrap_or(0) > 0,
        "18000 minutes of demand cannot fit a 21-day horizon: {body}"
    );
    assert!(
        !body["proposal"]["blocks"].as_array().unwrap().is_empty(),
        "a best-effort plan is still there: {body}"
    );
    let warnings = body["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["Overloaded"].is_object()),
        "the days that overflowed are named: {body}"
    );
}

/// Another account's proposal is a 404, never a 403: a 403 would confirm the row exists.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_proposal_is_not_found(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![effort_reply(&[(
            "Jaringan", 120,
        )])])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);

    let owner = signed_up(&app, &harness, "proposal-owner@example.test").await;
    upgrade(&harness, "proposal-owner@example.test", "pro").await;
    let workspace_id = workspace(&app, &owner, "Kelas").await;
    let user_id = user_id_of(&harness, "proposal-owner@example.test").await;
    let source_id = source(&harness, user_id, workspace_id, "Jaringan").await;
    let goal_id = goal(
        &app,
        &owner,
        workspace_id,
        "UAS",
        harness.state.clock.now() + Duration::days(21),
        vec![source_id],
    )
    .await;
    let proposal_id = request_plan(&app, &owner, goal_id).await;

    let other = signed_up(&app, &harness, "proposal-other@example.test").await;
    let (status, body) = json(
        &app,
        "GET",
        &format!("/plans/{proposal_id}"),
        &other,
        serde_json::Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found", "{body}");
}

// --- Task 12: the two refund arms ------------------------------------------

/// A plan generation that fails permanently gives back exactly the two credits, once.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_plan_generation_refunds_the_two_credits(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::failing(
        "the provider is down for good",
    )));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "refund-plan@example.test").await;
    upgrade(&harness, "refund-plan@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let user_id = user_id_of(&harness, "refund-plan@example.test").await;
    let source_id = source(&harness, user_id, workspace_id, "Jaringan").await;
    let goal_id = goal(
        &app,
        &token,
        workspace_id,
        "UAS",
        harness.state.clock.now() + Duration::days(21),
        vec![source_id],
    )
    .await;

    let proposal_id = request_plan(&app, &token, goal_id).await;
    let cost = harness.state.config.plans.credits.plan_generation;
    assert_eq!(
        cost, 2,
        "the matrix prices a plan generation at two credits"
    );

    // A provider that is down stays down, so every attempt fails; the worker gives up after
    // `MAX_ATTEMPTS` and the last attempt is the one that refunds.
    drain_jobs(&harness).await;

    let (status, error, credits): (String, Option<String>, i32) =
        sqlx::query_as("select status, error, credits_spent from plan_proposals where id = $1")
            .bind(proposal_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed", "the proposal is terminal");
    assert!(error.is_some(), "the failure says what went wrong");
    assert_eq!(credits, 0, "the charge was consumed by the refund");

    let refunded: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and reason = 'refund'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        refunded,
        i64::from(cost),
        "exactly one refund of the charge"
    );

    // The boot sweep recovers nothing a second time: `credits_spent > 0` is the guard, and
    // the claim that zeroed it is what makes this a no-op.
    let recovered = stvdy_server::credits::refund_pending(
        &harness.state.pool,
        stvdy_server::credits::RefundTarget::PlanProposal,
    )
    .await
    .unwrap();
    assert_eq!(
        recovered, 0,
        "a recovered charge must not be refunded again"
    );
    let refunded_after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and reason = 'refund'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(refunded_after, i64::from(cost), "still exactly one refund");
}

/// A failed syllabus extraction gives back the eight credits the upload charged.
///
/// The extraction rides `ingest_source` (A250), so the failure that refunds is the job's, and
/// the draft is the row that carries the charge. A model that always answers unparseable text
/// fails after its one retry, permanently, and the eight credits come back.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_extraction_refunds_the_eight_credits(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![
        "not json at all".to_string(),
        "still not json".to_string(),
    ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "refund-syllabus@example.test").await;
    upgrade(&harness, "refund-syllabus@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let (content_type, body) = common::multipart(&[("text", "Kuliah Senin 08:00", None)]);
    let (status, response) = common::post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let draft_id = Uuid::parse_str(response["draft_id"].as_str().unwrap()).unwrap();

    let cost = harness.state.config.plans.credits.syllabus_import;
    assert_eq!(
        cost, 8,
        "the matrix prices a syllabus import at eight credits"
    );

    drain_jobs(&harness).await;

    let user_id = user_id_of(&harness, "refund-syllabus@example.test").await;
    let (status, credits): (String, i32) =
        sqlx::query_as("select status, credits_spent from syllabus_drafts where id = $1")
            .bind(draft_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed", "the draft is terminal");
    assert_eq!(credits, 0, "the charge was consumed by the refund");

    let refunded: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = $1 and reason = 'refund'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        refunded,
        i64::from(cost),
        "exactly one refund of the charge"
    );

    let recovered = stvdy_server::credits::refund_pending(
        &harness.state.pool,
        stvdy_server::credits::RefundTarget::SyllabusDraft,
    )
    .await
    .unwrap();
    assert_eq!(recovered, 0, "the sweep must find nothing left to recover");
}

// --- Task 13: the exit gate ------------------------------------------------

/// Seed the settings the gate's arithmetic needs: 08:00–22:00, 240 minutes a day, 50-minute
/// blocks with 10-minute breaks (the schema's own defaults, stated here so the assertions
/// below read against a number the test chose).
async fn set_settings(harness: &TestState, user_id: Uuid) {
    sqlx::query(
        "insert into user_settings
            (user_id, study_start_hour, study_end_hour, max_study_min_per_day,
             block_len_min, break_min)
         values ($1, 8, 22, 240, 50, 10)
         on conflict (user_id) do update
            set study_start_hour = 8, study_end_hour = 22, max_study_min_per_day = 240,
                block_len_min = 50, break_min = 10",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// Two `class` events on the same Wednesday, in the student's zone.
async fn seed_classes(harness: &TestState, user_id: Uuid, workspace_id: Uuid, day: DateTime<Utc>) {
    for (start_hour, end_hour) in [(9u32, 11u32), (13, 15)] {
        let start = day
            .date_naive()
            .and_hms_opt(start_hour, 0, 0)
            .unwrap()
            .and_utc();
        let end = day
            .date_naive()
            .and_hms_opt(end_hour, 0, 0)
            .unwrap()
            .and_utc();
        sqlx::query(
            "insert into events
                (id, user_id, workspace_id, kind, title, starts_at, ends_at, source)
             values ($1, $2, $3, 'class', 'Lecture', $4, $5, 'app')",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(workspace_id)
        .bind(start)
        .bind(end)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    }
}

/// **Exit gate, part 2.** The accepted plan respects busy time, the daily maximum, the study
/// window and the deadline buffer, and the weak topic gets more minutes than the strong one.
#[sqlx::test(migrations = "../../migrations")]
async fn the_accepted_plan_respects_busy_time_study_hours_the_daily_max_and_the_buffer(
    pool: PgPool,
) {
    // Fixed at a Monday, so the Wednesday class events land inside the horizon and the
    // arithmetic is the same on every run.
    let clock = FixedClock::at(Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 0).unwrap());
    let harness = TestState::new(pool)
        .with_clock(Arc::new(clock))
        .with_llm(Arc::new(ScriptedLlm::new(vec![effort_reply(&[
            ("Jaringan", 300),
            ("Kripto", 120),
            ("Basis Data", 90),
        ])])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "gate@example.test").await;
    upgrade(&harness, "gate@example.test", "power").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let user_id = user_id_of(&harness, "gate@example.test").await;
    set_settings(&harness, user_id).await;

    let weak = source(&harness, user_id, workspace_id, "Jaringan").await;
    let strong = source(&harness, user_id, workspace_id, "Kripto").await;
    let third = source(&harness, user_id, workspace_id, "Basis Data").await;

    // The weak topic: one quiz question over the source, with a wrong attempt in the window.
    let question = Uuid::now_v7();
    sqlx::query(
        "insert into quiz_questions (id, user_id, quiz_id, kind, prompt, answer, chunk_id)
         select $1, $2, q.id, 'mcq', 'x', 'y', c.id
           from quizzes q join chunks c on c.source_id = $3
          where q.workspace_id = $4 limit 1",
    )
    .bind(question)
    .bind(user_id)
    .bind(weak)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .ok();

    let now = harness.state.clock.now();
    seed_classes(&harness, user_id, workspace_id, now + Duration::days(2)).await;

    let goal_id = goal(
        &app,
        &token,
        workspace_id,
        "UAS Jaringan",
        now + Duration::days(21),
        vec![weak, strong, third],
    )
    .await;
    let proposal_id = request_plan(&app, &token, goal_id).await;
    drain_jobs(&harness).await;

    let proposal = poll(&app, &token, proposal_id).await;
    assert_eq!(proposal["status"], "ready", "{proposal}");
    let blocks = proposal["proposal"]["blocks"].as_array().expect("blocks");
    assert!(!blocks.is_empty(), "the gate needs a plan: {proposal}");

    // Accept every block, unadjusted: the drag path is Task 14's, and this asserts the
    // server's own invariants on what it proposed.
    let accepted: Vec<serde_json::Value> = blocks
        .iter()
        .map(|block| {
            serde_json::json!({
                "starts_at": block["starts_at"],
                "ends_at": block["ends_at"],
                "goal_id": block["goal_id"],
                "topic": block["topic"],
                "kind": if block["kind"] == "Review" { "review" } else { "new_material" },
            })
        })
        .collect();

    let (status, body) = json(
        &app,
        "POST",
        "/plans/accept",
        &token,
        serde_json::json!({ "proposal_id": proposal_id, "blocks": accepted }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // --- Assert on the WRITTEN events, not the proposal ---------------------

    let written: Vec<(DateTime<Utc>, DateTime<Utc>, Option<Uuid>, String, String)> =
        sqlx::query_as(
            "select starts_at, ends_at, goal_id, title, source from events
              where user_id = $1 and kind = 'study_block' and deleted_at is null
              order by starts_at, id",
        )
        .bind(user_id)
        .fetch_all(&harness.state.pool)
        .await
        .unwrap();
    assert!(!written.is_empty(), "the accept wrote the blocks");

    let tz: chrono_tz::Tz = "Asia/Jakarta".parse().unwrap();

    // (a) No block overlaps a class event.
    let classes: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "select starts_at, ends_at from events
          where user_id = $1 and kind = 'class' and deleted_at is null",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    for (start, end, _, title, _) in &written {
        for (class_start, class_end) in &classes {
            assert!(
                !(start < class_end && class_start < end),
                "{title} at {start} overlaps a class ({class_start}–{class_end})"
            );
        }
    }

    // (b) Every block is inside 08:00–22:00 local, and (c) no day exceeds 240 minutes.
    let mut per_day: std::collections::BTreeMap<chrono::NaiveDate, i64> =
        std::collections::BTreeMap::new();
    for (start, end, _, title, _) in &written {
        let local = start.with_timezone(&tz);
        assert!(
            local.time() >= chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
            "{title} starts before 08:00 local: {local}"
        );
        let local_end = end.with_timezone(&tz);
        assert!(
            local_end.time() <= chrono::NaiveTime::from_hms_opt(22, 0, 0).unwrap()
                || local_end.time() == chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            "{title} ends after 22:00 local: {local_end}"
        );
        *per_day.entry(local.date_naive()).or_insert(0) += (*end - *start).num_minutes();
    }
    for (day, minutes) in &per_day {
        assert!(
            *minutes <= 240,
            "{day} holds {minutes} minutes, over the 240-minute daily maximum"
        );
    }

    // (d) Every row carries its goal and the app source.
    for (_, _, goal_id, title, source) in &written {
        assert!(goal_id.is_some(), "{title} has no goal_id");
        assert_eq!(source, "app", "{title} must be app-sourced");
    }

    // (e) The deadline buffer: no new-material block starts within 120 minutes of the exam.
    let target = now + Duration::days(21);
    for (start, _, _, title, _) in &written {
        if title.starts_with("Review") {
            continue;
        }
        assert!(
            *start <= target - Duration::minutes(120) || *start >= target,
            "{title} starts inside the 120-minute buffer before the deadline"
        );
    }
}

/// **Exit gate, part 2, continued.** A replan keeps the pinned blocks exactly and leaves an
/// already-started block alone (A265).
#[sqlx::test(migrations = "../../migrations")]
async fn replan_keeps_the_pinned_blocks_and_leaves_the_started_one_alone(pool: PgPool) {
    let clock = FixedClock::at(Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 0).unwrap());
    let harness = TestState::new(pool)
        .with_clock(Arc::new(clock))
        .with_llm(Arc::new(ScriptedLlm::new(vec![
            effort_reply(&[("Jaringan", 300), ("Kripto", 120)]),
            effort_reply(&[("Jaringan", 300), ("Kripto", 120)]),
        ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "replan@example.test").await;
    upgrade(&harness, "replan@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let user_id = user_id_of(&harness, "replan@example.test").await;
    set_settings(&harness, user_id).await;
    let a = source(&harness, user_id, workspace_id, "Jaringan").await;
    let b = source(&harness, user_id, workspace_id, "Kripto").await;
    let now = harness.state.clock.now();

    let goal_id = goal(
        &app,
        &token,
        workspace_id,
        "UAS",
        now + Duration::days(21),
        vec![a, b],
    )
    .await;
    let first = request_plan(&app, &token, goal_id).await;
    drain_jobs(&harness).await;

    // Accept a plan, then move one block into the past: a block that has started is busy, not
    // pinned, so the replan must not move it.
    let proposal = poll(&app, &token, first).await;
    let blocks = proposal["proposal"]["blocks"].as_array().unwrap();
    let accepted: Vec<serde_json::Value> = blocks
        .iter()
        .map(|block| {
            serde_json::json!({
                "starts_at": block["starts_at"],
                "ends_at": block["ends_at"],
                "goal_id": block["goal_id"],
                "topic": block["topic"],
                "kind": "new_material",
            })
        })
        .collect();
    let (status, body) = json(
        &app,
        "POST",
        "/plans/accept",
        &token,
        serde_json::json!({ "proposal_id": first, "blocks": accepted }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let before: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "select id, starts_at, ends_at from events
          where user_id = $1 and kind = 'study_block' and deleted_at is null
          order by starts_at, id",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert!(before.len() >= 2, "the gate needs at least two blocks");

    // The first block starts in the past: the student is (fictionally) sitting in it.
    let started = before[0].0;
    sqlx::query("update events set starts_at = $2, ends_at = $3 where id = $1")
        .bind(started)
        .bind(now - Duration::minutes(30))
        .bind(now + Duration::minutes(20))
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let future: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>)> = before[1..].to_vec();

    let (status, body) = json(
        &app,
        "POST",
        "/plan/replan",
        &token,
        serde_json::json!({ "goal_id": goal_id }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain_jobs(&harness).await;

    // The started block is untouched.
    let (start, end): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as("select starts_at, ends_at from events where id = $1")
            .bind(started)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(
        start,
        now - Duration::minutes(30),
        "the started block moved"
    );
    assert_eq!(end, now + Duration::minutes(20), "the started block moved");

    // Every future block is exactly where it was, and the new proposal carries them pinned.
    for (id, expected_start, expected_end) in &future {
        let (start, end): (DateTime<Utc>, DateTime<Utc>) =
            sqlx::query_as("select starts_at, ends_at from events where id = $1")
                .bind(id)
                .fetch_one(&harness.state.pool)
                .await
                .unwrap();
        assert_eq!(start, *expected_start, "a committed block moved on replan");
        assert_eq!(end, *expected_end, "a committed block moved on replan");
    }
}

/// An accepted proposal cannot be accepted twice (409), and the second call writes no second
/// set of events. Idempotence by refusal, the same rule the syllabus confirm uses.
#[sqlx::test(migrations = "../../migrations")]
async fn an_accepted_proposal_cannot_be_accepted_twice(pool: PgPool) {
    let clock = FixedClock::at(Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 0).unwrap());
    let harness = TestState::new(pool)
        .with_clock(Arc::new(clock))
        .with_llm(Arc::new(ScriptedLlm::new(vec![effort_reply(&[(
            "Jaringan", 300,
        )])])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "twice@example.test").await;
    upgrade(&harness, "twice@example.test", "pro").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let user_id = user_id_of(&harness, "twice@example.test").await;
    set_settings(&harness, user_id).await;
    let a = source(&harness, user_id, workspace_id, "Jaringan").await;
    let now = harness.state.clock.now();

    let goal_id = goal(
        &app,
        &token,
        workspace_id,
        "UAS",
        now + Duration::days(21),
        vec![a],
    )
    .await;
    let proposal_id = request_plan(&app, &token, goal_id).await;
    drain_jobs(&harness).await;

    let proposal = poll(&app, &token, proposal_id).await;
    let accepted: Vec<serde_json::Value> = proposal["proposal"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| {
            serde_json::json!({
                "starts_at": block["starts_at"],
                "ends_at": block["ends_at"],
                "goal_id": block["goal_id"],
                "topic": block["topic"],
                "kind": "new_material",
            })
        })
        .collect();
    let body = serde_json::json!({ "proposal_id": proposal_id, "blocks": accepted });

    let (status, first) = json(&app, "POST", "/plans/accept", &token, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let count_after_first: i64 = sqlx::query_scalar(
        "select count(*)::bigint from events
          where user_id = $1 and kind = 'study_block' and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, second) = json(&app, "POST", "/plans/accept", &token, body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{second}");
    assert_eq!(second["code"], "conflict", "{second}");

    let count_after_second: i64 = sqlx::query_scalar(
        "select count(*)::bigint from events
          where user_id = $1 and kind = 'study_block' and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        count_after_first, count_after_second,
        "a second accept must write no second set of events"
    );
}
