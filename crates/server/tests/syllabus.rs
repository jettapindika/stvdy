//! The syllabus import: the upload, the draft the review screen reads, and the confirm that
//! is the only writer of the rows the student ends up with.
//!
//! Two cases here are the review screen's whole reason for existing, and both are about
//! absence: [`nothing_is_written_until_the_student_confirms`] asserts that a completed
//! extraction leaves `events`, `tasks` and `goals` **empty**, and
//! [`confirming_writes_only_the_kept_items`] asserts that a confirm writes rows for the kept
//! indices and for nothing else. A syllabus written straight through would pass every other
//! assertion in this file while being exactly the failure the review step prevents.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use chrono::{DateTime, TimeZone as _, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{drain_jobs, json, multipart, post_json, signed_up};

/// The extraction the scripted provider answers with: two meetings, two deadlines and one
/// exam, so a confirm can keep some and drop the rest.
fn extraction_reply() -> String {
    r#"{"course":{"name":"Jaringan Komputer","code":"IF2230","instructor":"Dr. Sari"},
        "meetings":[
          {"title":"Lecture","rrule":"FREQ=WEEKLY;BYDAY=MO;COUNT=4",
           "start_time":"08:00","end_time":"09:40","location":"Room 3",
           "evidence":"Kuliah Senin 08:00 ruang 3","confidence":0.95},
          {"title":"Lab","rrule":"FREQ=WEEKLY;BYDAY=WE;COUNT=4",
           "start_time":"13:00","end_time":"15:30","location":null,
           "evidence":"Praktikum Rabu 13:00","confidence":0.55}],
        "deadlines":[
          {"title":"Tugas 1","due":"2026-11-12T23:59:00+07:00","weight_percent":15.0,
           "evidence":"Tugas 1 dikumpulkan 12 Nov","confidence":0.4},
          {"title":"Tugas 2","due":"2026-12-03T23:59:00+07:00","weight_percent":20.0,
           "evidence":"Tugas 2 dikumpulkan 3 Des","confidence":0.9}],
        "exams":[
          {"title":"UAS","starts_at":"2026-12-15T08:00:00+07:00","ends_at":null,
           "topics":["Graphs","Trees"],"evidence":"UAS 15 Desember 08:00","confidence":0.85}],
        "notes":["Materi UAS kumulatif"]}"#
        .to_string()
}

/// The same reply with nothing in it: the state a syllabus whose text states no schedule
/// produces, and a state the contract models rather than one it refuses.
fn empty_reply() -> String {
    r#"{"course":{},"meetings":[],"deadlines":[],"exams":[],"notes":[]}"#.to_string()
}

fn router(harness: &TestState) -> Router {
    stvdy_server::routes::router(harness.state.clone()).unwrap()
}

/// A workspace for the caller, through the API so the row is a real one.
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

/// Uploads a syllabus as pasted text, returning the draft id.
async fn upload(app: &Router, token: &str, workspace_id: Uuid, text: &str) -> Uuid {
    let (content_type, body) = multipart(&[("text", text, None)]);
    let (status, response) = post_json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    Uuid::parse_str(response["draft_id"].as_str().expect("a draft id")).unwrap()
}

async fn user_id(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

async fn count(harness: &TestState, table: &str, workspace_id: Uuid) -> i64 {
    let sql = format!("select count(*)::bigint from {table} where workspace_id = $1");
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(workspace_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The three real tables, as one reading, so a test cannot check two of them and forget the
/// third.
async fn rows_written(harness: &TestState, workspace_id: Uuid) -> (i64, i64, i64) {
    (
        count(harness, "events", workspace_id).await,
        count(harness, "tasks", workspace_id).await,
        count(harness, "goals", workspace_id).await,
    )
}

/// A syllabus upload is charged eight credits, writes a `syllabus` source and a draft, and
/// answers 202 with the draft id to poll.
#[sqlx::test(migrations = "../../migrations")]
async fn uploading_a_syllabus_charges_eight_credits_and_returns_a_draft(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "syllabus@example.test").await;
    let workspace_id = workspace(&app, &token, "Jaringan Komputer").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00\nUAS 15 Des").await;

    let cost = harness.state.config.plans.credits.syllabus_import;
    assert_eq!(
        cost, 8,
        "the matrix prices a syllabus import at eight credits"
    );

    let user_id = user_id(&harness, "syllabus@example.test").await;
    let (delta, action): (i32, String) = sqlx::query_as(
        "select delta, reason from credit_ledger where user_id = $1 order by created_at desc limit 1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(i64::from(delta), -cost, "the import is charged");
    assert_eq!(
        action, "usage:syllabus_import",
        "the ledger names the action that spent, prefixed the way spend_in writes it"
    );

    let (kind, source_status): (String, String) =
        sqlx::query_as("select kind, status from sources where workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(kind, "syllabus");
    // `pending`, not a new word: `sources.status` is documented
    // `pending | processing | ready | failed` and the generic upload writes the same value,
    // so a second spelling here would be a second convention for one column.
    assert_eq!(source_status, "pending");

    let status: String = sqlx::query_scalar("select status from syllabus_drafts where id = $1")
        .bind(draft_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "extracting");
    let extraction: Option<serde_json::Value> =
        sqlx::query_scalar("select extraction from syllabus_drafts where id = $1")
            .bind(draft_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(extraction.is_none(), "the extraction has not run yet");
}

/// Free allows one import a month. The second is a 422 `limit_reached` with the real numbers,
/// and the ledger is untouched — the refusal happens **before** the charge, which is what
/// `reserve_in`'s ordering buys.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_import_on_the_free_plan_is_refused_with_limit_reached(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![
        extraction_reply(),
        extraction_reply(),
    ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "free@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;

    let user_id = user_id(&harness, "free@example.test").await;
    let balance_before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (content_type, body) = multipart(&[("text", "another syllabus", None)]);
    let (status, response) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        &token,
        Some(&content_type),
        body,
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "limit_reached", "{response}");
    assert_eq!(response["limit"], 1, "{response}");
    assert_eq!(response["used"], 1, "{response}");

    let balance_after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger where user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        balance_after, balance_before,
        "a refused import must not have been charged"
    );
}

/// Another account's draft is a 404, never a 403: a 403 would confirm the row exists.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_draft_is_not_found(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![
        extraction_reply(),
        extraction_reply(),
    ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);

    let owner = signed_up(&app, &harness, "owner@example.test").await;
    let workspace_id = workspace(&app, &owner, "Kelas").await;
    let draft_id = upload(&app, &owner, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    let other = signed_up(&app, &harness, "other@example.test").await;
    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &other,
        serde_json::Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found", "{body}");
}

/// The draft a completed extraction leaves is the review screen's whole input: every item
/// with the source line it came from, ordered by the confidence the screen must not show,
/// and the extraction's own `confidence` absent from the response.
#[sqlx::test(migrations = "../../migrations")]
async fn the_draft_carries_every_item_with_the_line_it_came_from(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "review@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;

    // Polling while the extraction runs is a 409, not a failure: the client should come back.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict", "{body}");

    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready", "{body}");

    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 5, "two meetings, two deadlines, one exam");

    // Ordered by confidence ascending: the item most likely to be wrong is at the top, which
    // is where the student's attention is. The order is the *only* thing the confidence
    // number is for.
    let titles: Vec<&str> = items
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec!["Tugas 1", "Lab", "UAS", "Tugas 2", "Lecture"],
        "lowest confidence first"
    );

    for item in items {
        assert!(
            !item["evidence"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "every item must carry the source line the student checks it against: {item}"
        );
        assert!(
            item.get("confidence").is_none(),
            "the honesty rules forbid showing a confidence score: {item}"
        );
    }
    assert_eq!(items[0]["needs_check"], true, "0.4 is below the flag");
    assert_eq!(items[4]["needs_check"], false, "0.95 is not");
    assert_eq!(items[0]["kind"], "deadline");
    assert_eq!(items[4]["kind"], "class");
    assert_eq!(items[4]["start_time"], "08:00");
    assert_eq!(items[4]["rrule"], "FREQ=WEEKLY;BYDAY=MO;COUNT=4");

    // The deadline's instant is the one the module wrote, in the student's own zone.
    assert_eq!(
        items[0]["when"], "2026-11-12T16:59:00Z",
        "23:59+07:00 is 16:59Z"
    );
}

/// An extraction with nothing in it is a state the contract models, not a failure: the draft
/// is `ready` and the screen says there is nothing to review.
#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_extraction_is_a_ready_draft_with_nothing_to_review(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![empty_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "empty@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "no schedule stated").await;
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready", "{body}");
    assert!(body["items"].as_array().unwrap().is_empty(), "{body}");
}

/// A model that answers prose fails the extraction, and the draft says so in a body the
/// client can render rather than as an HTTP error on a poll.
#[sqlx::test(migrations = "../../migrations")]
async fn a_provider_that_answers_prose_fails_the_draft_readably(pool: PgPool) {
    // Both attempts are prose: the retry is made and it fails the same way, so the draft ends
    // `failed` rather than looping.
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![
        "I could not read that.".to_string(),
        "Still prose.".to_string(),
    ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "prose@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "failed", "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("extraction"),
        "the failure must say what went wrong: {body}"
    );
}

/// A failed extraction charges nothing for the model and refunds nothing for the import: the
/// student has a `failed` draft they can see and a source they can delete. This is the state
/// the roadmap's § 8 job list leaves it in — the retry is the worker's, and after the last
/// attempt the draft is terminal.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_extraction_leaves_a_terminal_draft(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::failing("provider is down")));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "down@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    let (status, _) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let draft_status: String =
        sqlx::query_scalar("select status from syllabus_drafts where id = $1")
            .bind(draft_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(draft_status, "failed");

    // A failed extraction still wrote no real rows: the failure mode of a model outage must
    // not be a half-written calendar.
    assert_eq!(rows_written(&harness, workspace_id).await, (0, 0, 0));
}

/// A file that is not a PDF is refused before the charge, and a request with neither a file
/// nor text is refused too: both are 400s from the route, not from the extractor.
#[sqlx::test(migrations = "../../migrations")]
async fn a_non_pdf_file_is_refused_before_the_charge(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "file@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let (content_type, body) = multipart(&[("file", "syllabus.docx", Some(b"not a pdf"))]);
    let (status, response) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "validation", "{response}");

    let (content_type, body) = multipart(&[("title", "Empty", None)]);
    let (status, response) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");

    let user_id = user_id(&harness, "file@example.test").await;
    let charged: i64 = sqlx::query_scalar(
        "select count(*)::bigint from credit_ledger where user_id = $1 and reason = 'syllabus_import'",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(charged, 0, "a refused upload must not be charged");
}

/// A draft in another workspace cannot be confirmed by a caller who does not own it, and an
/// unowned workspace cannot take an upload: both are 404s.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_workspace_takes_no_upload(pool: PgPool) {
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![
        extraction_reply(),
        extraction_reply(),
    ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);

    let owner = signed_up(&app, &harness, "own@example.test").await;
    let workspace_id = workspace(&app, &owner, "Kelas").await;

    let other = signed_up(&app, &harness, "intruder@example.test").await;
    let (content_type, body) = multipart(&[("text", "Kuliah Senin", None)]);
    let (status, response) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        &other,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
}

/// The extraction's prompt is composed from the module, with the syllabus's own chunks as the
/// `<sources>` block — asserted on the request the model actually received, because a
/// composer bug would otherwise ship green.
#[sqlx::test(migrations = "../../migrations")]
async fn the_extraction_prompt_carries_the_document_and_the_data_block(pool: PgPool) {
    let llm = Arc::new(ScriptedLlm::new(vec![extraction_reply()]));
    let harness = TestState::new(pool).with_llm(llm.clone());
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "prompt@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    upload(
        &app,
        &token,
        workspace_id,
        "Kuliah Senin 08:00 di ruang 3. UAS 15 Desember.",
    )
    .await;
    drain_jobs(&harness).await;

    let requests = llm.requests();
    let sent = requests.first().expect("the model was called");
    let text = &sent.messages[0].content;

    assert!(
        text.contains("<user_context_data>"),
        "the module's data block must reach the model: {text}"
    );
    assert!(
        text.contains("semester_context"),
        "the semester context line is what the module resolves a year-less date against: {text}"
    );
    assert!(
        text.contains("Kuliah Senin 08:00"),
        "the document is the sources block: {text}"
    );
    assert!(
        !text.contains("{{"),
        "no variable may reach the model unfilled: {text}"
    );
    assert_eq!(
        sent.temperature, 0.1,
        "an extraction is a reading of a document, not a piece of writing"
    );
}

/// A kept meeting's rule is expanded through Phase 10's engine, so a confirm writes one
/// `events` row per occurrence and the rows are on the days the rule names.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kept_meeting_becomes_one_event_per_occurrence(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "expand@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": [{ "index": 4, "keep": true }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["events"], 4, "COUNT=4 is four occurrences: {body}");
    assert_eq!(body["tasks"], 0);
    assert_eq!(body["goals"], 0);

    let kinds: Vec<String> =
        sqlx::query_scalar("select kind from events where workspace_id = $1 order by starts_at")
            .bind(workspace_id)
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(kinds, vec!["class"; 4]);
    assert_eq!(rows_written(&harness, workspace_id).await, (4, 0, 0));

    // A second confirm is a 409 rather than a second set of rows, which is what makes the
    // endpoint safe to retry after a timeout.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": [{ "index": 4, "keep": true }] }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(rows_written(&harness, workspace_id).await, (4, 0, 0));
}

/// An index the draft does not have is a 400, and nothing is written: a stale client must not
/// get a success response for a confirm that dropped the item the student meant to keep.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_index_is_refused_before_anything_is_written(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "stale@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": [{ "index": 99, "keep": true }] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(rows_written(&harness, workspace_id).await, (0, 0, 0));

    // The draft is still `ready`, so the student can send a corrected confirm.
    let draft_status: String =
        sqlx::query_scalar("select status from syllabus_drafts where id = $1")
            .bind(draft_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(draft_status, "ready");
}

/// An exam the document never dated cannot become a goal: `goals.target_date` is `not null`,
/// and inventing a date is exactly what the honesty rules forbid. The refusal names the item.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exam_with_no_date_is_refused_rather_than_invented(pool: PgPool) {
    let undated = extraction_reply().replace(
        "\"starts_at\":\"2026-12-15T08:00:00+07:00\"",
        "\"starts_at\":null",
    );
    let harness = TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![undated])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "undated@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "UAS, tanggal menyusul").await;
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let exam = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "exam")
        .expect("the exam is on the list");
    let index = exam["index"].as_u64().unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": [{ "index": index, "keep": true }] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("date"),
        "the refusal must say what is missing: {body}"
    );

    // And with a date the student supplies, it becomes a goal with the workspace's sources.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({
            "items": [{ "index": index, "keep": true, "when": "2026-12-15T01:00:00Z" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["goals"], 1);

    let (title, target_date, source_ids): (String, DateTime<Utc>, Vec<Uuid>) =
        sqlx::query_as("select title, target_date, source_ids from goals where workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(title, "UAS");
    assert_eq!(
        target_date,
        Utc.with_ymd_and_hms(2026, 12, 15, 1, 0, 0)
            .single()
            .unwrap()
    );
    assert!(
        source_ids.is_empty(),
        "a workspace with no sources gives a goal no topics"
    );
}

/// An exam whose workspace has sources carries them, so the planner has topics to schedule.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kept_exam_goal_carries_the_workspaces_sources(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "topics@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    // A source to draw topics from, through the API so it is a real row.
    let (content_type, body) = multipart(&[("kind", "text", None), ("text", "Materi graf", None)]);
    let (status, _) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let source_id: Uuid = sqlx::query_scalar("select id from sources where workspace_id = $1")
        .bind(workspace_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let draft_id = upload(&app, &token, workspace_id, "UAS 15 Desember").await;
    drain_jobs(&harness).await;

    let (_, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let index = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "exam")
        .unwrap()["index"]
        .as_u64()
        .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": [{ "index": index, "keep": true }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let source_ids: Vec<Uuid> =
        sqlx::query_scalar("select source_ids from goals where workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(source_ids, vec![source_id]);
}

/// A source from another workspace cannot be attached to a goal: the topics would be ones the
/// student cannot see, and the planner would schedule material from a workspace they did not
/// pick.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exam_goal_refuses_a_source_outside_the_workspace(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "cross@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;
    let other_workspace = workspace(&app, &token, "Other").await;

    let (content_type, body) = multipart(&[("kind", "text", None), ("text", "Elsewhere", None)]);
    let (status, _) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{other_workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let foreign: Uuid = sqlx::query_scalar("select id from sources where workspace_id = $1")
        .bind(other_workspace)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let draft_id = upload(&app, &token, workspace_id, "UAS 15 Desember").await;
    drain_jobs(&harness).await;

    let (_, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let index = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "exam")
        .unwrap()["index"]
        .as_u64()
        .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({
            "items": [{ "index": index, "keep": true, "source_ids": [foreign] }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// A kept deadline becomes one task: `due_at` set, `priority` 1, no estimate.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kept_deadline_becomes_one_task(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "deadline@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Tugas 1 dikumpulkan 12 Nov").await;
    drain_jobs(&harness).await;

    let (_, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let index = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == "Tugas 1")
        .unwrap()["index"]
        .as_u64()
        .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": [{ "index": index, "keep": true }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tasks"], 1);

    let (title, due_at, priority, estimate_min): (String, Option<DateTime<Utc>>, i32, Option<i32>) =
        sqlx::query_as(
            "select title, due_at, priority, estimate_min from tasks where workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(title, "Tugas 1");
    assert_eq!(
        due_at,
        Some(
            Utc.with_ymd_and_hms(2026, 11, 12, 16, 59, 0)
                .single()
                .unwrap()
        )
    );
    assert_eq!(
        priority, 1,
        "a date the faculty set is not the student's to rank"
    );
    assert!(estimate_min.is_none(), "the module states no estimate");
}

/// A student's edit replaces what the model extracted, and an item they drop produces no row.
#[sqlx::test(migrations = "../../migrations")]
async fn a_students_edit_replaces_the_extraction(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "edit@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({
            "items": [
                { "index": 0, "keep": true, "title": "Tugas 1 (revisi)", "when": "2026-11-13T16:59:00Z" },
                { "index": 1, "keep": false }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (title, due_at): (String, Option<DateTime<Utc>>) =
        sqlx::query_as("select title, due_at from tasks where workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(title, "Tugas 1 (revisi)");
    assert_eq!(
        due_at,
        Some(
            Utc.with_ymd_and_hms(2026, 11, 13, 16, 59, 0)
                .single()
                .unwrap()
        )
    );

    // The dropped item exists in no table.
    let tasks: i64 = count(&harness, "tasks", workspace_id).await;
    assert_eq!(tasks, 1);
}

/// **Nothing is written until the student confirms.**
///
/// This is the assertion that makes the review screen a screen rather than a formality. An
/// extraction that wrote `events`, `tasks` and `goals` as it parsed would pass every other
/// test in this file while being exactly the failure the review step exists to prevent: a
/// model's misreading of a syllabus becomes the student's semester, with nothing on the page
/// to say where it came from.
#[sqlx::test(migrations = "../../migrations")]
async fn nothing_is_written_until_the_student_confirms(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "review-only@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    // The extraction completed and the review list is real.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "ready", "{body}");
    assert_eq!(
        body["items"].as_array().unwrap().len(),
        5,
        "the screen has five items to show: {body}"
    );

    // And not one of them is a row anywhere.
    assert_eq!(
        rows_written(&harness, workspace_id).await,
        (0, 0, 0),
        "an extraction must write nothing until the student confirms"
    );
}

/// Every kept item is written once, and no dropped one is: the review screen's contract in one
/// assertion over the three real tables.
#[sqlx::test(migrations = "../../migrations")]
async fn confirming_writes_only_the_kept_items(pool: PgPool) {
    let harness =
        TestState::new(pool).with_llm(Arc::new(ScriptedLlm::new(vec![extraction_reply()])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "kept@example.test").await;
    let workspace_id = workspace(&app, &token, "Kelas").await;

    let draft_id = upload(&app, &token, workspace_id, "Kuliah Senin 08:00").await;
    drain_jobs(&harness).await;

    // The draft is what the screen renders: two meetings, two deadlines, one exam.
    let (_, body) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let items = body["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 5);

    let index_of = |title: &str| {
        items.iter().find(|item| item["title"] == title).unwrap()["index"]
            .as_u64()
            .unwrap()
    };
    let lecture = index_of("Lecture");
    let tugas_2 = index_of("Tugas 2");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({
            "items": [
                { "index": lecture, "keep": true },
                { "index": tugas_2, "keep": true },
                { "index": index_of("Lab"), "keep": false },
                { "index": index_of("Tugas 1"), "keep": false },
                { "index": index_of("UAS"), "keep": false }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // One meeting with COUNT=4 is four events, one kept deadline is one task, and the exam was
    // dropped so there is no goal.
    let (events, tasks, goals) = rows_written(&harness, workspace_id).await;
    assert_eq!((events, tasks, goals), (4, 1, 0), "{body}");

    let task_titles: Vec<String> =
        sqlx::query_scalar("select title from tasks where workspace_id = $1 order by title")
            .bind(workspace_id)
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(task_titles, vec!["Tugas 2"], "Tugas 1 was dropped");

    // The dropped items' titles exist in no table at all.
    for dropped in ["Lab", "Tugas 1", "UAS"] {
        let in_events: i64 = sqlx::query_scalar(
            "select count(*)::bigint from events where workspace_id = $1 and title = $2",
        )
        .bind(workspace_id)
        .bind(dropped)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        assert_eq!(in_events, 0, "{dropped} was dropped but reached events");
    }

    let draft_status: String =
        sqlx::query_scalar("select status from syllabus_drafts where id = $1")
            .bind(draft_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(draft_status, "confirmed");
}
