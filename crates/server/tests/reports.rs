//! Phase 6 reports: the outline, per-section generation, the editable outline, the export,
//! and the monthly reservation.
//!
//! Runs against `ScriptedLlm`, so the replies are scripted and the suite asserts on both
//! halves of the exchange: what the prompt contained (via `requests()`) and what was stored.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{drain_jobs, json, signed_up, workspace_with_text};

const INDONESIAN: &str = "TCP memakai three-way handshake untuk membuka koneksi. \
UDP tidak memakai handshake sehingga lebih cepat tetapi tidak menjamin urutan paket.";

/// One valid outline, as `prompts/report_outline.v1.md` documents it: a heading, a purpose,
/// the chunk labels that will support the section, and the honesty flag.
fn outline_reply() -> String {
    serde_json::json!({
        "title": "Laporan Jaringan Komputer",
        "sections": [
            {
                "heading": "Pendahuluan",
                "purpose": "Menjelaskan ruang lingkup laporan.",
                "labels": [1],
                "gaps": false
            },
            {
                "heading": "Perbandingan TCP dan UDP",
                "purpose": "Membandingkan keduanya berdasarkan sumber.",
                "labels": [1],
                "gaps": true
            }
        ]
    })
    .to_string()
}

async fn report_created(
    app: &axum::Router,
    workspace_id: &str,
    token: &str,
    source_id: &str,
) -> (StatusCode, serde_json::Value) {
    json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/reports/outline"),
        token,
        serde_json::json!({
            "topic": "Jaringan Komputer",
            "style": "revisi ujian",
            "length": "sedang",
            "source_ids": [source_id],
        }),
    )
    .await
}

async fn get_report(app: &axum::Router, token: &str, report_id: &str) -> serde_json::Value {
    let (_, body) = json(
        app,
        "GET",
        &format!("/reports/{report_id}"),
        token,
        serde_json::Value::Null,
    )
    .await;
    body
}

/// Generates one section and waits for the job.
async fn generate(
    app: &axum::Router,
    harness: &TestState,
    token: &str,
    report_id: &str,
    index: usize,
) -> serde_json::Value {
    let (status, body) = json(
        app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        token,
        serde_json::json!({ "index": index }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain_jobs(harness).await;
    get_report(app, token, report_id).await
}

/// The outline, then two sections, then the `.md` export: the roadmap's exit gate for the
/// report half.
///
/// Every step is asserted on the stored document, not only on the status codes: an outline
/// whose sections are silently dropped, a section that overwrites its neighbour, and an
/// export that emits the wrong text are all states where every status is 200.
#[sqlx::test(migrations = "../../migrations")]
async fn an_outline_then_sections_then_a_markdown_export(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        outline_reply(),
        "TCP membuka koneksi dengan tiga paket [1].\n\n(Not covered in your sources: detail tambahan)".to_string(),
        "UDP lebih cepat karena tidak ada handshake [1].".to_string(),
    ]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "report@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    // The outline is stored as the student will edit it, with the gap flag the roadmap's
    // exit gate names.
    assert_eq!(body["report"]["status"], "draft", "{body}");
    assert_eq!(
        body["report"]["outline"]["title"],
        "Laporan Jaringan Komputer"
    );
    assert_eq!(body["report"]["outline"]["sections"][1]["gaps"], true);
    assert_eq!(
        body["report"]["sections"][1]["heading"],
        "Perbandingan TCP dan UDP"
    );
    assert!(body["report"]["sections"][0]["body_md"].is_null(), "{body}");
    assert_eq!(body["report"]["sections"][0]["generating"], false, "{body}");
    assert!(body["report"]["body_md"].is_null(), "{body}");

    // The prompt carried the Indonesian chunk under label 1, which is what the outline's
    // `labels: [1]` refers to.
    let sent = requests.requests();
    assert!(
        sent[0].messages[0].content.contains("three-way handshake"),
        "the prompt must carry the source's text"
    );
    assert!(
        sent[0].messages[0].content.contains("[1]"),
        "the prompt must label the chunk the model cites"
    );
    assert!(
        !sent[0].messages[0].content.contains("{{"),
        "no unfilled variable may reach the model"
    );

    // Section 0: written, and the other section is untouched.
    let report = generate(&app, &harness, &token, &report_id, 0).await;
    assert_eq!(report["report"]["status"], "draft", "{report}");
    assert!(
        report["report"]["sections"][0]["body_md"]
            .as_str()
            .unwrap()
            .contains("tiga paket"),
        "{report}"
    );
    assert!(
        report["report"]["sections"][1]["body_md"].is_null(),
        "{report}"
    );

    // Section 1: now every section has text, so the report is `ready` and the client stops
    // polling.
    let report = generate(&app, &harness, &token, &report_id, 1).await;
    assert_eq!(report["report"]["status"], "ready", "{report}");
    assert!(
        !report["report"]["sections"][1]["body_md"].is_null(),
        "{report}"
    );

    // The stored document is both sections, under their outline headings, in order.
    let body_md = report["report"]["body_md"].as_str().unwrap();
    let first = body_md.find("## Pendahuluan").expect("the first heading");
    let second = body_md
        .find("## Perbandingan TCP dan UDP")
        .expect("the second heading");
    assert!(
        first < second,
        "the document is in outline order: {body_md}"
    );
    assert!(body_md.contains("tiga paket"), "{body_md}");
    assert!(body_md.contains("tidak ada handshake"), "{body_md}");

    // The section prompt is told what the other sections cover, which is what stops it
    // repeating them.
    let sent = requests.requests();
    let section_prompt = &sent[2].messages[0].content;
    assert!(
        section_prompt.contains("Pendahuluan"),
        "the second section must be told what the first covers: {section_prompt}"
    );
    assert_eq!(
        sent.len(),
        3,
        "one outline call and one call per section, and no retries"
    );

    // The prompt version is recorded, so a quality regression can be attributed to a prompt
    // change (A38). Both steps name their own module.
    let versions: Vec<Option<String>> = sqlx::query_scalar(
        "select prompt_version from usage_events
          where action in ('report_outline', 'report_section') order by created_at, action",
    )
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert!(
        versions.contains(&Some("report_outline.v1".to_string())),
        "{versions:?}"
    );
    assert!(
        versions.contains(&Some("report_section.v1".to_string())),
        "{versions:?}"
    );

    // The export is the stored Markdown, as a file.
    let (status, content_type, bytes) = common::document(
        &app,
        &format!("/reports/{report_id}/export?format=md"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{content_type}");
    assert!(
        content_type.starts_with("text/markdown"),
        "the export is Markdown, not JSON: {content_type}"
    );
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(
        text, *body_md,
        "the export is the document the server stores"
    );
}

/// The retry names the violation, so the second attempt can converge.
///
/// The section's first reply is the outline's schema instead of Markdown. Without the parse
/// error appended, the model is told "that was wrong" and nothing else, and the retry spends
/// a call to make the same mistake.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_parse_is_retried_with_the_violation_named(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        outline_reply(),
        r#"{"body_md":"jawaban yang salah bentuknya"}"#.to_string(),
        "TCP membuka koneksi dengan tiga paket [1].".to_string(),
    ]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportretry@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // The outline is one call and no retry: it is the step whose result the student edits,
    // so a reply that does not parse is worth telling them about rather than silently
    // spending a second call on.
    assert_eq!(
        requests.requests().len(),
        1,
        "the outline makes one call and no retry"
    );

    // A section, by contrast, retries once — and the correction carries the parse error.
    let report_id = body["report"]["id"].as_str().unwrap().to_string();
    let (status, body) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &token,
        serde_json::json!({ "index": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain_jobs(&harness).await;

    let sent = requests.requests();
    assert_eq!(
        sent.len(),
        3,
        "one outline call, the bad section reply, then the retry: {}",
        sent.len()
    );
    let retry = &sent[2].messages;
    assert_eq!(retry.len(), 3, "system, the bad reply, then the correction");
    let correction = &retry[2].content;
    assert!(
        correction.contains("The problem was:"),
        "the correction must name the violation: {correction}"
    );
    assert!(
        correction.contains("JSON"),
        "and must quote the parser's own message: {correction}"
    );

    // The retry's reply is what was stored, and the section that was written is the retry's
    // text rather than the reply the parser refused.
    let report = get_report(&app, &token, &report_id).await;
    assert_eq!(report["report"]["status"], "draft", "{report}");
    assert_eq!(
        report["report"]["sections"][0]["body_md"],
        "TCP membuka koneksi dengan tiga paket [1]."
    );
}

/// A model that never produces an outline costs the student nothing: the charge is reversed
/// and the report is left terminal.
#[sqlx::test(migrations = "../../migrations")]
async fn an_outline_that_never_parses_is_refunded(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::failing("provider is down")));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportrefund@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let balance = |pool: PgPool| async move {
        let total: i64 = sqlx::query_scalar(
            "select coalesce(sum(delta), 0)::bigint from credit_ledger
              where user_id = (select id from users where email = 'reportrefund@example.test')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        total
    };

    let before = balance(harness.state.pool.clone()).await;
    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let after = balance(harness.state.pool.clone()).await;
    assert_eq!(after, before, "the charge must come back exactly once");

    // The row is terminal rather than left `generating`, so the client stops polling it.
    let (status, state): (String, Option<String>) = sqlx::query_as(
        "select status, error from reports where workspace_id = $1 order by created_at desc limit 1",
    )
    .bind(uuid::Uuid::parse_str(&workspace_id).unwrap())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(status, "failed");
    assert!(state.is_some(), "a failed report carries a readable error");

    // And the failed report no longer consumes the month's allowance, because its
    // reservation was refunded.
    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "the second attempt is refused by the model, not by the cap: {body}"
    );
    assert_eq!(body["code"], "provider_error", "{body}");
}

/// A report that is still generating counts against the month, or the cap is exceeded by
/// asking twice in a row. This is the test Phase 4 needed and did not have.
#[sqlx::test(migrations = "../../migrations")]
async fn a_report_still_generating_counts_against_the_monthly_limit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        outline_reply(),
        outline_reply(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportcap@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let cap = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .reports_per_month;
    assert_eq!(
        cap.max(),
        Some(1),
        "the free plan allows one report a month"
    );

    // The whole allowance, spent on an outline that is `draft` and has no sections at all:
    // a count of the *sections* would read zero here and let a second report through.
    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["report"]["status"], "draft", "{body}");

    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a report with no sections must still count: {body}"
    );
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 1, "{body}");
    assert_eq!(body["used"], 1, "{body}");

    // Nothing was charged or stored by the refusal.
    let reports: i64 = sqlx::query_scalar(
        "select count(*)::bigint from reports where user_id = (select id from users where email = 'reportcap@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(reports, 1);
}

/// The export is gated on the plan's `export_formats`, and the refusal names the plan.
///
/// `pdf` is not in the free plan's list, so the answer is 403 `plan_required` with `min_plan`
/// — the shape the mobile build needs to say "not available on your plan" without naming a
/// price.
#[sqlx::test(migrations = "../../migrations")]
async fn a_format_the_plan_does_not_include_is_refused_with_min_plan(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![outline_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportgate@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/reports/{report_id}/export?format=pdf"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "plan_required", "{body}");
    assert_eq!(body["min_plan"], "pro", "{body}");

    // An unknown format is a validation failure rather than a plan refusal: the two are
    // different remedies, and a client told "upgrade" for a typo would be misled.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/reports/{report_id}/export?format=docx"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");

    // The plan's own format is allowed, and a report with no sections is a conflict rather
    // than an empty file.
    let (status, body) = json(
        &app,
        "GET",
        &format!("/reports/{report_id}/export?format=md"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict", "{body}");
}

/// The outline is editable, and the whole edit round-trips: `labels` and `gaps` are required
/// by the contract, so an edit that drops them cannot be stored.
#[sqlx::test(migrations = "../../migrations")]
async fn an_outline_edit_round_trips_and_must_carry_every_field(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![outline_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportedit@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    // A rename, a reorder and a new section: an edit the generator could not have produced is
    // still valid input as long as it parses.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::json!({
            "outline": {
                "title": "Laporan saya",
                "sections": [
                    {
                        "heading": "Perbandingan TCP dan UDP",
                        "purpose": "Membandingkan keduanya.",
                        "labels": [1],
                        "gaps": true
                    },
                    {
                        "heading": "Kesimpulan",
                        "purpose": "Merangkum.",
                        "labels": [],
                        "gaps": false
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["report"]["outline"]["title"], "Laporan saya");
    assert_eq!(body["report"]["sections"].as_array().unwrap().len(), 2);
    assert_eq!(
        body["report"]["sections"][0]["heading"],
        "Perbandingan TCP dan UDP"
    );
    assert_eq!(body["report"]["sections"][0]["generating"], false, "{body}");

    // Dropping `labels` — the field the section call cites — is refused, not defaulted: the
    // SPA round-trips it, and a stored outline without it cannot be generated from.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::json!({
            "outline": {
                "title": "Laporan saya",
                "sections": [
                    { "heading": "Kesimpulan", "purpose": "Merangkum.", "gaps": false }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("labels"),
        "the refusal must name the missing field: {body}"
    );

    // And the stored outline is unchanged by the refused edit.
    let report = get_report(&app, &token, &report_id).await;
    assert_eq!(
        report["report"]["outline"]["sections"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// Regenerating one section replaces that section and nothing else, and a reorder keeps the
/// text of the sections that moved.
///
/// The failure this prevents is silent: index-aligned texts under a reordered outline print
/// one section's prose under another section's heading, and nothing on the page says so.
#[sqlx::test(migrations = "../../migrations")]
async fn regenerating_a_section_replaces_only_that_section(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        outline_reply(),
        "isi pertama versi A".to_string(),
        "isi kedua".to_string(),
        "isi pertama versi B".to_string(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportregen@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    generate(&app, &harness, &token, &report_id, 0).await;
    let report = generate(&app, &harness, &token, &report_id, 1).await;
    assert_eq!(report["report"]["sections"][1]["body_md"], "isi kedua");

    // Regenerate section 0: its text is replaced, its neighbour is not.
    let report = generate(&app, &harness, &token, &report_id, 0).await;
    assert_eq!(
        report["report"]["sections"][0]["body_md"],
        "isi pertama versi B"
    );
    assert_eq!(
        report["report"]["sections"][1]["body_md"], "isi kedua",
        "regenerating one section must not touch another: {report}"
    );

    // A reorder keeps each section's own text, because the heading is what identifies it.
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::json!({
            "outline": {
                "title": "Laporan Jaringan Komputer",
                "sections": [
                    {
                        "heading": "Perbandingan TCP dan UDP",
                        "purpose": "Membandingkan.",
                        "labels": [1],
                        "gaps": true
                    },
                    {
                        "heading": "Pendahuluan",
                        "purpose": "Ruang lingkup.",
                        "labels": [1],
                        "gaps": false
                    }
                ]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["report"]["sections"][0]["body_md"], "isi kedua",
        "{body}"
    );
    assert_eq!(
        body["report"]["sections"][1]["body_md"], "isi pertama versi B",
        "{body}"
    );

    // And the stored document follows the new order.
    let body_md = body["report"]["body_md"].as_str().unwrap();
    let first = body_md.find("## Perbandingan TCP dan UDP").unwrap();
    let second = body_md.find("## Pendahuluan").unwrap();
    assert!(first < second, "{body_md}");

    // The edit kept the previous body as a revision, which is the roadmap's conflict rule.
    let revisions: i64 =
        sqlx::query_scalar("select count(*)::bigint from report_revisions where report_id = $1")
            .bind(uuid::Uuid::parse_str(&report_id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(revisions, 1, "the replaced body is recoverable");
}

/// A section generation that fails for good refunds the credits, once, and says which section
/// failed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_section_refunds_and_names_its_section(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // Both replies are JSON objects, which is the schema failure `parse_section` refuses: a
    // reply that is prose is a section, however short, so the failure has to be the wrong
    // *shape* rather than the wrong length.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        outline_reply(),
        r#"{"body_md":"jawaban"}"#.to_string(),
        r#"{"body_md":"masih jawaban"}"#.to_string(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportsectionfail@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    let balance = |pool: PgPool| async move {
        let total: i64 = sqlx::query_scalar(
            "select coalesce(sum(delta), 0)::bigint from credit_ledger
              where user_id = (select id from users where email = 'reportsectionfail@example.test')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        total
    };

    let before = balance(harness.state.pool.clone()).await;
    let (status, body) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &token,
        serde_json::json!({ "index": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    drain_jobs(&harness).await;

    let report = get_report(&app, &token, &report_id).await;
    assert_eq!(report["report"]["status"], "failed", "{report}");
    assert!(
        report["report"]["error"].as_str().unwrap().contains("JSON"),
        "the error must say what happened: {report}"
    );
    // The failure is attributed to the section that failed, not to the report as a whole.
    assert_eq!(
        report["report"]["sections"][1]["generating"], false,
        "{report}"
    );
    assert!(
        !report["report"]["sections"][1]["error"].is_null(),
        "the section that failed carries the error: {report}"
    );
    assert!(
        report["report"]["sections"][0]["error"].is_null(),
        "{report}"
    );

    let after = balance(harness.state.pool.clone()).await;
    assert_eq!(after, before, "the charge must come back exactly once");

    // A second generation on the same report is refused while it is failed? No: `failed` is
    // one of the states a section may be started from, because a retry is the student's to
    // ask for and it is charged again.
    let (status, _) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &token,
        serde_json::json!({ "index": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

/// A report the boot sweep finds charged and terminal gets its money back.
///
/// The state is written directly because the path that produces it is a ledger insert that
/// cannot be written — an infrastructure failure rather than a business one. Without this
/// sweep that charge is permanent, and it is invisible: the row reads `failed`, so nothing
/// else would ever look at it again.
#[sqlx::test(migrations = "../../migrations")]
async fn a_charge_left_pending_by_a_failed_refund_is_recovered(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportpending@example.test").await;
    let (workspace_id, _source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let user_id: uuid::Uuid =
        sqlx::query_scalar("select id from users where email = 'reportpending@example.test'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    // The row a failed generation with an unwritable refund leaves behind.
    let report_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into reports (id, user_id, workspace_id, title, status, credits_spent)
         values ($1, $2, $3, 'Laporan', 'failed', 6)",
    )
    .bind(report_id)
    .bind(user_id)
    .bind(uuid::Uuid::parse_str(&workspace_id).unwrap())
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let recovered = stvdy_server::credits::refund_pending(
        &harness.state.pool,
        stvdy_server::credits::RefundTarget::Report,
    )
    .await
    .unwrap();
    assert_eq!(recovered, 1, "the pending charge is recovered once");

    let left: i32 = sqlx::query_scalar("select credits_spent from reports where id = $1")
        .bind(report_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        left, 0,
        "the charge is consumed by the claim that refunds it"
    );

    // Idempotent: a second sweep finds nothing, so a restart cannot refund twice.
    let again = stvdy_server::credits::refund_pending(
        &harness.state.pool,
        stvdy_server::credits::RefundTarget::Report,
    )
    .await
    .unwrap();
    assert_eq!(again, 0);
}

/// Another account's report is a 404, not a 403: the id must not confirm it exists.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_report_is_not_readable(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![outline_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let owner = signed_up(&app, &harness, "reportowner@example.test").await;
    let other = signed_up(&app, &harness, "reportother@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &owner, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = report_created(&app, &workspace_id, &owner, &source_id).await;
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    for (method, uri) in [
        ("GET", format!("/reports/{report_id}")),
        ("GET", format!("/reports/{report_id}/export?format=md")),
    ] {
        let (status, body) = json(&app, method, &uri, &other, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/reports/{report_id}"),
        &other,
        serde_json::json!({ "outline": { "title": "punya saya", "sections": [] } }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &other,
        serde_json::json!({ "index": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // And another account's workspace cannot be used to propose one, either.
    let (status, body) = report_created(&app, &workspace_id, &other, &source_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// A section index outside the outline is refused before anything is charged, and a second
/// section cannot be started while one is in flight.
#[sqlx::test(migrations = "../../migrations")]
async fn a_section_index_outside_the_outline_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        outline_reply(),
        "isi".to_string(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportindex@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &token,
        serde_json::json!({ "index": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");

    // The first section is accepted, and a second request while it is queued is a conflict
    // rather than a second charge.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &token,
        serde_json::json!({ "index": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let (status, body) = json(
        &app,
        "POST",
        &format!("/reports/{report_id}/generate"),
        &token,
        serde_json::json!({ "index": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "conflict", "{body}");

    drain_jobs(&harness).await;
}

/// The workspace's reports are listable, which is what makes one reachable after a reload.
#[sqlx::test(migrations = "../../migrations")]
async fn the_workspace_lists_its_reports(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![outline_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "reportlist@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = report_created(&app, &workspace_id, &token, &source_id).await;
    let report_id = body["report"]["id"].as_str().unwrap().to_string();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/reports"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reports"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["reports"][0]["id"], report_id);
    assert_eq!(
        body["reports"][0]["outline"]["sections"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
