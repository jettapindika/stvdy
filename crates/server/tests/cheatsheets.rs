//! Phase 6 cheatsheets: the generation, the stored artifacts, the re-render, and the plan
//! gate on the PDF.
//!
//! The pipeline is what these assert on. The blocks are the document, the HTML is a pure
//! function of them, and the PDF is a print: a test that only checked "the status became
//! ready" would pass while every one of those artifacts was missing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::storage::Storage;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{document, drain_jobs, json, signed_up, workspace_with_text};

const INDONESIAN: &str = "Three-way handshake adalah proses membuka koneksi TCP. \
Klien mengirim SYN, server membalas SYN-ACK, lalu klien mengirim ACK. \
Efisiensi pengiriman dihitung dengan rumus eta = L / (L + H) * 100 persen.";

/// One valid reply, as `prompts/cheatsheet.v1.md` documents it: a chapter, a section, and a
/// formula whose maths is in LaTeX inside the `latex` field.
fn blocks_reply() -> String {
    serde_json::json!({
        "title": "Cheatsheet UTS Jaringan Komputer",
        "subtitle": "Ringkasan materi ujian",
        "course": "Jaringan Komputer",
        "exam": "UTS",
        "chapters": [
            {
                "title": "Bab 1 Transport",
                "sections": [
                    {
                        "title": "Handshake",
                        "blocks": [
                            {
                                "kind": "text",
                                "text": "TCP membuka koneksi dengan three-way handshake.",
                                "labels": [1]
                            },
                            {
                                "kind": "formula",
                                "latex": "\\eta = \\frac{L}{L + H} \\times 100\\%",
                                "caption": "Efisiensi pengiriman",
                                "legend": [
                                    { "symbol": "L", "meaning": "Ukuran payload", "unit": "byte" },
                                    { "symbol": "H", "meaning": "Overhead", "unit": "byte" }
                                ],
                                "labels": [1]
                            }
                        ]
                    }
                ]
            }
        ]
    })
    .to_string()
}

async fn cheatsheet_created(
    app: &axum::Router,
    workspace_id: &str,
    token: &str,
    source_id: &str,
) -> (StatusCode, serde_json::Value) {
    json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/cheatsheets"),
        token,
        serde_json::json!({
            "scope": "UTS Jaringan Komputer",
            "exam_date": "2026-11-01",
            "depth": "normal",
            "source_ids": [source_id],
        }),
    )
    .await
}

async fn get_cheatsheet(app: &axum::Router, token: &str, id: &str) -> serde_json::Value {
    let (_, body) = json(
        app,
        "GET",
        &format!("/cheatsheets/{id}"),
        token,
        serde_json::Value::Null,
    )
    .await;
    body
}

/// Generates one cheatsheet and waits for its jobs.
async fn generated(
    app: &axum::Router,
    harness: &TestState,
    token: &str,
    workspace_id: &str,
    source_id: &str,
) -> String {
    let (status, body) = cheatsheet_created(app, workspace_id, token, source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["cheatsheet"]["id"].as_str().unwrap().to_string();
    drain_jobs(harness).await;
    id
}

/// The generation stores all three artifacts the pipeline names, and the HTML is a real
/// document rather than an empty envelope.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cheatsheet_stores_its_blocks_and_renders_its_document(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![blocks_reply()]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheat@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = cheatsheet_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["cheatsheet"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["cheatsheet"]["status"], "generating", "{body}");
    assert!(body["cheatsheet"]["template_version"].is_null(), "{body}");
    assert!(
        body["cheatsheet"]["page_count"].is_null(),
        "a page count before a print would be a guess: {body}"
    );

    drain_jobs(&harness).await;

    // The prompt carried the Indonesian chunk under label 1, which is what the reply's
    // `labels: [1]` refers to, and the module's own settings.
    let sent = requests.requests();
    let prompt = &sent[0].messages[0].content;
    assert!(prompt.contains("three-way handshake"), "the source's text");
    assert!(prompt.contains("[1]"), "the chunk's label");
    assert!(prompt.contains("UTS Jaringan Komputer"), "the scope");
    assert!(prompt.contains("2026-11-01"), "the exam date");
    assert!(
        !prompt.contains("{{"),
        "no unfilled variable may reach the model"
    );
    assert_eq!(sent.len(), 1, "one call, and no retry");

    let sheet = get_cheatsheet(&app, &token, &id).await;
    assert_eq!(sheet["cheatsheet"]["status"], "ready", "{sheet}");
    assert_eq!(
        sheet["cheatsheet"]["template_version"], "cheatsheet.v1",
        "which template rendered it is recorded, so an old one is visible: {sheet}"
    );
    assert!(sheet["cheatsheet"]["error"].is_null(), "{sheet}");
    // No renderer in a test, so the print never happened — and the document is still `ready`,
    // which is the property that keeps a browser problem from destroying a generation.
    assert!(
        sheet["cheatsheet"]["page_count"].is_null(),
        "the page count is the renderer's, so it stays null without one: {sheet}"
    );

    // The blocks are the model's own text, stored verbatim and re-parsable.
    let (blocks_key, html_key): (Option<String>, Option<String>) =
        sqlx::query_as("select blocks_key, html_key from cheatsheets where id = $1")
            .bind(uuid::Uuid::parse_str(&id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let blocks_key = blocks_key.expect("the blocks were stored");
    let html_key = html_key.expect("the document was rendered");

    let stored = harness.storage.get(&blocks_key).await.unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    let text = stored["blocks"].as_str().expect("the reply text is stored");
    assert!(
        text.contains("\"kind\":\"formula\""),
        "the artifact holds the model's own JSON: {text}"
    );
    assert!(
        !text.contains("```"),
        "the fence is stripped, so the stored form is what the parser reads: {text}"
    );
    // Re-parsing the stored text is what a re-render does, and it must yield a document.
    let parsed = stvdy_core::render::blocks::parse_blocks(text).expect("the stored blocks parse");
    assert_eq!(parsed.chapters.len(), 1);
    // The labels travelled with it, which is what keeps the citations resolvable.
    assert_eq!(stored["labels"][0]["label"], 1, "{stored}");

    let html = harness.storage.get(&html_key).await.unwrap();
    let html = String::from_utf8(html.to_vec()).unwrap();
    assert!(
        html.contains("Cheatsheet UTS Jaringan Komputer"),
        "the document carries its title"
    );
    assert!(html.contains("three-way handshake"), "and its content");
    // The named markers, not the bare `{{` pair: the template's own header comment discusses
    // its placeholders in prose, so a scan for two braces would fail on a correct document.
    for marker in [
        "{{doc_title}}",
        "{{content}}",
        "{{toc}}",
        "{{cover_eyebrow}}",
    ] {
        assert!(!html.contains(marker), "the {marker} marker must be filled");
    }
    // The formula reached the page as LaTeX for KaTeX to render, not as markup the model
    // wrote.
    assert!(html.contains("\\frac"), "the formula is on the page");
}

/// `GET /cheatsheets/:id/html` answers the document itself, not a JSON envelope.
///
/// The free plan's remedy for a withheld PDF is to print the HTML the student already has,
/// which only works if this route serves a document a browser can open; the preview frame
/// consumes the same bytes as `srcdoc`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_html_route_serves_the_document(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![blocks_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheathtml@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let id = generated(&app, &harness, &token, &workspace_id, &source_id).await;

    let (status, content_type, bytes) =
        document(&app, &format!("/cheatsheets/{id}/html"), &token).await;
    assert_eq!(status, StatusCode::OK, "{content_type}");
    assert!(
        content_type.starts_with("text/html"),
        "the preview frame and a browser print both need a document: {content_type}"
    );

    let html = String::from_utf8(bytes).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "{}", &html[..40]);
    assert!(html.contains("Cheatsheet UTS Jaringan Komputer"), "{html}");

    // Another account's document is a 404, like every other scoped read.
    let other = signed_up(&app, &harness, "cheathtmlother@example.test").await;
    let (status, _, _) = document(&app, &format!("/cheatsheets/{id}/html"), &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Re-rendering is byte-identical and free: it makes no model call and charges nothing.
///
/// This is the property the content-versus-layout split exists for. A clock in the cover, a
/// random id in the output, or a re-derived label map would each break it — and the symptom
/// would be a document that quietly differs from the one the student printed.
#[sqlx::test(migrations = "../../migrations")]
async fn re_rendering_is_identical_and_calls_no_model(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![blocks_reply()]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheatrerender@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let id = generated(&app, &harness, &token, &workspace_id, &source_id).await;

    let (_, _, before) = document(&app, &format!("/cheatsheets/{id}/html"), &token).await;
    let calls_before = requests.requests().len();

    let credits_before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'cheatrerender@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/cheatsheets/{id}/re-render"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["cheatsheet"]["template_version"], "cheatsheet.v1",
        "{body}"
    );

    drain_jobs(&harness).await;

    let (_, _, after) = document(&app, &format!("/cheatsheets/{id}/html"), &token).await;
    assert_eq!(
        before, after,
        "a re-render must be byte-identical: the content did not change"
    );
    assert_eq!(
        requests.requests().len(),
        calls_before,
        "a re-render must never call the model"
    );
    let credits_after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'cheatrerender@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        credits_after, credits_before,
        "a re-render costs nothing, which is the whole reason the blocks are persisted"
    );
}

/// The PDF is plan-gated, and a plan that allows it is told the truth about a print that does
/// not exist yet.
#[sqlx::test(migrations = "../../migrations")]
async fn the_pdf_is_plan_gated_and_reports_a_missing_print(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![blocks_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheatpdf@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let id = generated(&app, &harness, &token, &workspace_id, &source_id).await;

    // Free: `pdf` is not in the plan's `export_formats`, so the answer is 403 `plan_required`
    // with the plan that has it — the shape a mobile build needs to say "not available on
    // your plan" without naming a price.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{id}/pdf"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");
    assert_eq!(body["min_plan"], "pro", "{body}");

    // Pro: allowed, and the print genuinely does not exist — no renderer in a test, which is
    // the state a deployment without Chromium is in too. 409 rather than 404: the cheatsheet
    // is there, and the remedy is a re-render once a renderer is available.
    sqlx::query("update users set plan_id = 'pro' where email = 'cheatpdf@example.test'")
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{id}/pdf"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict", "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("re-render"),
        "the refusal must name the remedy: {body}"
    );
}

/// The retry names the violation, so the second attempt can converge.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_parse_is_retried_with_the_violation_named(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "ini bukan json".to_string(),
        blocks_reply(),
    ]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheatretry@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let id = generated(&app, &harness, &token, &workspace_id, &source_id).await;

    let sheet = get_cheatsheet(&app, &token, &id).await;
    assert_eq!(sheet["cheatsheet"]["status"], "ready", "{sheet}");

    let sent = requests.requests();
    assert_eq!(sent.len(), 2, "the first reply was retried once");
    let correction = &sent[1].messages[2].content;
    assert!(
        correction.contains("The problem was:"),
        "the correction must name the violation: {correction}"
    );
    assert!(
        correction.contains("JSON"),
        "and must quote the parser's own message: {correction}"
    );
}

/// A generation that fails for good refunds the credits, once.
#[sqlx::test(migrations = "../../migrations")]
async fn a_generation_that_fails_for_good_refunds_the_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        "bukan json".to_string(),
        "masih bukan json".to_string(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheatrefund@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let balance = |pool: PgPool| async move {
        let total: i64 = sqlx::query_scalar(
            "select coalesce(sum(delta), 0)::bigint from credit_ledger
              where user_id = (select id from users where email = 'cheatrefund@example.test')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        total
    };

    let before = balance(harness.state.pool.clone()).await;
    let id = generated(&app, &harness, &token, &workspace_id, &source_id).await;
    let after = balance(harness.state.pool.clone()).await;
    assert_eq!(after, before, "the charge must come back exactly once");

    let sheet = get_cheatsheet(&app, &token, &id).await;
    assert_eq!(sheet["cheatsheet"]["status"], "failed", "{sheet}");
    assert!(
        sheet["cheatsheet"]["error"]
            .as_str()
            .unwrap()
            .contains("JSON"),
        "the error must say what happened: {sheet}"
    );
    // A failed generation stores nothing: the artifacts are only written once the reply is
    // renderable, so there is no half-document to clean up.
    let keys: (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("select blocks_key, html_key, pdf_key from cheatsheets where id = $1")
            .bind(uuid::Uuid::parse_str(&id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(keys, (None, None, None));
}

/// The settings the module declares as required are refused when they are absent or not one of
/// the documented values, before anything is spent.
#[sqlx::test(migrations = "../../migrations")]
async fn the_generation_settings_are_validated(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheatvalidate@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    for (field, body) in [
        (
            "depth",
            serde_json::json!({
                "scope": "UTS", "depth": "deep", "source_ids": [source_id]
            }),
        ),
        (
            "scope",
            serde_json::json!({
                "scope": "   ", "depth": "normal", "source_ids": [source_id]
            }),
        ),
        (
            "source_ids",
            serde_json::json!({
                "scope": "UTS", "depth": "normal", "source_ids": []
            }),
        ),
        (
            "exam_date",
            serde_json::json!({
                "scope": "UTS", "depth": "normal", "source_ids": [source_id],
                "exam_date": "2999-01-01"
            }),
        ),
    ] {
        let (status, response) = json(
            &app,
            "POST",
            &format!("/workspaces/{workspace_id}/cheatsheets"),
            &token,
            body,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {response}");
        assert_eq!(response["code"], "validation", "{field}: {response}");
    }

    // Nothing was charged or stored by any of the refusals.
    let stored: i64 = sqlx::query_scalar(
        "select count(*)::bigint from cheatsheets
          where user_id = (select id from users where email = 'cheatvalidate@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(stored, 0);

    // A source that is not this workspace's is refused too, which is what stops a generation
    // from being built on another account's material.
    let (other_workspace, other_source) =
        workspace_with_text(&app, &harness, &token, "Lain", "Modul 9", INDONESIAN).await;
    let (status, response) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/cheatsheets"),
        &token,
        serde_json::json!({
            "scope": "UTS", "depth": "normal", "source_ids": [other_source]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "validation", "{response}");
    let _ = other_workspace;
}

/// Another account's cheatsheet is a 404, not a 403: the id must not confirm it exists.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_cheatsheet_is_not_readable(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![blocks_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let owner = signed_up(&app, &harness, "cheatowner@example.test").await;
    let other = signed_up(&app, &harness, "cheatother@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &owner, "Jaringan", "Modul 3", INDONESIAN).await;
    let id = generated(&app, &harness, &owner, &workspace_id, &source_id).await;

    for uri in [
        format!("/cheatsheets/{id}"),
        format!("/cheatsheets/{id}/html"),
    ] {
        let (status, body) = json(&app, "GET", &uri, &other, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    // The PDF route checks the *plan* before the resource, and that leaks nothing: a
    // non-owner on the free plan gets the same 403 `plan_required` an owner would, so the
    // answer says nothing about whether the document exists.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{id}/pdf"),
        &other,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/cheatsheets/{id}/re-render"),
        &other,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // And another account's workspace cannot be used to start one, either.
    let (status, body) = cheatsheet_created(&app, &workspace_id, &other, &source_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// The workspace's cheatsheets are listable, which is what makes one reachable after a
/// reload.
#[sqlx::test(migrations = "../../migrations")]
async fn the_workspace_lists_its_cheatsheets(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![blocks_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cheatlist@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let id = generated(&app, &harness, &token, &workspace_id, &source_id).await;

    let (status, body) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/cheatsheets"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cheatsheets"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["cheatsheets"][0]["id"], id);
    assert_eq!(body["cheatsheets"][0]["scope"], "UTS Jaringan Komputer");
    assert_eq!(body["cheatsheets"][0]["exam_date"], "2026-11-01");
    assert_eq!(body["cheatsheets"][0]["status"], "ready");
    // The internal artifact keys are not part of the payload: a client has no use for an
    // object key, and exposing one would invite a direct-storage contract this API does not
    // offer.
    assert!(body["cheatsheets"][0].get("blocks_key").is_none(), "{body}");
    assert!(body["cheatsheets"][0].get("pdf_key").is_none(), "{body}");
}
