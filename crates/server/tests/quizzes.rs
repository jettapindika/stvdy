//! Phase 5 quizzes: generation, grading, attempts, the derived score, and the monthly
//! reservation.
//!
//! Runs against `ScriptedLlm`, so the JSON the model returns is scripted and the suite
//! asserts on both halves of the exchange: what the prompt contained (via `requests()`)
//! and what was stored.

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

/// One valid reply, as `prompts/quiz.v1.md` documents it: an MCQ whose answer is one of
/// its options, and a short answer with key points.
fn reply(label: u32) -> String {
    serde_json::json!({
        "questions": [
            {
                "kind": "mcq",
                "prompt": "Apa fungsi three-way handshake?",
                "options": [
                    "Membuka koneksi TCP.",
                    "Menutup koneksi TCP.",
                    "Mengenkripsi data.",
                    "Mengompres data."
                ],
                "answer": "Membuka koneksi TCP.",
                "explanation": "Handshake menegosiasikan parameter sebelum data dikirim.",
                "label": label,
                "key_points": null
            },
            {
                "kind": "short",
                "prompt": "Mengapa UDP lebih cepat daripada TCP?",
                "options": null,
                "answer": "Karena UDP tidak memakai handshake.",
                "explanation": "Tanpa handshake, tidak ada putaran tambahan sebelum data.",
                "label": label,
                "key_points": ["tidak ada handshake", "tidak menjamin urutan"]
            }
        ]
    })
    .to_string()
}

async fn quiz_created(
    app: &axum::Router,
    workspace_id: &str,
    token: &str,
    source_id: &str,
    count: u32,
) -> (StatusCode, serde_json::Value) {
    json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/quizzes"),
        token,
        serde_json::json!({
            "title": "Jaringan",
            "source_ids": [source_id],
            "count": count,
            "difficulty": "medium",
            "types": "mixed",
        }),
    )
    .await
}

/// Generates a quiz and returns `(quiz_id, question_ids)`.
async fn generated_quiz(
    app: &axum::Router,
    harness: &TestState,
    token: &str,
    workspace_id: &str,
    source_id: &str,
) -> (String, Vec<String>) {
    let (status, body) = quiz_created(app, workspace_id, token, source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let quiz_id = body["quiz"]["id"].as_str().unwrap().to_string();
    drain_jobs(harness).await;

    let (_, quiz) = json(
        app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(quiz["quiz"]["status"], "ready", "{quiz}");
    let ids = quiz["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|question| question["id"].as_str().unwrap().to_string())
        .collect();
    (quiz_id, ids)
}

async fn answered(
    app: &axum::Router,
    token: &str,
    quiz_id: &str,
    question_id: &str,
    answer: &str,
    correct: Option<bool>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "question_id": question_id,
        "user_answer": answer,
    });
    if let Some(correct) = correct {
        body["correct"] = serde_json::json!(correct);
    }
    let (status, response) = json(
        app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        token,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response
}

/// The generation path, and roadmap gate 6's server half: an Indonesian source produces
/// Indonesian questions whose labels resolve to chunks of that source.
///
/// Both halves fail *silently* for Indonesian: an `{{output_language}}` default that
/// picked English would generate English questions from Indonesian material with every
/// English test still green, and a label-map mistake would store questions with no
/// source link and no error.
#[sqlx::test(migrations = "../../migrations")]
async fn a_quiz_generates_questions_linked_to_their_chunk(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(1)]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quiz@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;
    assert_eq!(question_ids.len(), 2);

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;

    // The questions are the model's, stored whole.
    assert_eq!(quiz["questions"][0]["kind"], "mcq");
    assert_eq!(
        quiz["questions"][0]["prompt"],
        "Apa fungsi three-way handshake?"
    );
    assert_eq!(quiz["questions"][0]["options"].as_array().unwrap().len(), 4);
    assert_eq!(quiz["questions"][1]["kind"], "short");
    assert!(quiz["questions"][1]["options"].is_null());
    assert_eq!(
        quiz["questions"][1]["key_points"].as_array().unwrap().len(),
        2
    );

    // Gate 6: every question is linked to a chunk **of the Indonesian source that was
    // sent**, and the question text is the Indonesian the model returned, not a
    // translation.
    for index in 0..2 {
        let chunk_id = quiz["questions"][index]["chunk_id"]
            .as_str()
            .unwrap_or_else(|| panic!("question {index} has no chunk link: {quiz}"));
        let belongs: bool = sqlx::query_scalar(
            "select exists(select 1 from chunks where id = $1 and source_id = $2)",
        )
        .bind(uuid::Uuid::parse_str(chunk_id).unwrap())
        .bind(uuid::Uuid::parse_str(&source_id).unwrap())
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        assert!(
            belongs,
            "question {index}'s link must be a chunk of the source that was sent"
        );
    }

    // And the prompt carried the Indonesian chunk under label 1, which is what the
    // reply's `label: 1` refers to.
    let sent = requests.requests();
    let prompt = &sent[0].messages[0].content;
    assert!(
        prompt.contains("three-way handshake"),
        "the prompt must carry the source's text"
    );
    assert!(
        prompt.contains("[1]"),
        "the prompt must label the chunk the model cites"
    );
    assert!(
        !prompt.contains("{{"),
        "no unfilled variable may reach the model"
    );

    // The prompt version is recorded on the usage row, so a quality regression can be
    // attributed to a prompt change (A38).
    let version: Option<String> = sqlx::query_scalar(
        "select prompt_version from usage_events where action = 'generate_quiz' order by created_at desc limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(version.as_deref(), Some("quiz.v1"));
}

/// An MCQ is graded by the server from the stored answer. A client that claims
/// `correct: true` for a wrong option cannot raise its own score, which is the whole
/// reason the claim is ignored.
#[sqlx::test(migrations = "../../migrations")]
async fn an_mcq_attempt_is_graded_by_the_server_not_the_client(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "grade@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    // The wrong option, claimed correct.
    let response = answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[0],
        "Menutup koneksi TCP.",
        Some(true),
    )
    .await;
    assert_eq!(
        response["attempt"]["correct"], false,
        "the server grades the MCQ; the client's claim is ignored: {response}"
    );

    // And the right one, claimed wrong.
    let response = answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[0],
        "Membuka koneksi TCP.",
        Some(false),
    )
    .await;
    assert_eq!(response["attempt"]["correct"], true, "{response}");

    // A letter is not an answer: it would mean guessing which option was meant.
    let response = answered(&app, &token, &quiz_id, &question_ids[0], "A", None).await;
    assert_eq!(response["attempt"]["correct"], false, "{response}");
}

/// A short answer is the student's to mark. An ungraded one is excluded from the
/// denominator rather than counted as wrong, or the score would fall as they worked.
#[sqlx::test(migrations = "../../migrations")]
async fn a_short_answer_is_self_assessed_and_an_ungraded_one_does_not_lower_the_score(
    pool: PgPool,
) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "short@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    // The MCQ, answered correctly.
    answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[0],
        "Membuka koneksi TCP.",
        None,
    )
    .await;
    // The short answer, submitted but not marked yet.
    let response = answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[1],
        "Karena tidak ada handshake.",
        None,
    )
    .await;
    assert!(
        response["attempt"]["correct"].is_null(),
        "an unmarked short answer stores NULL, which is not the same as wrong: {response}"
    );

    let (_, results) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}/results"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(results["summary"]["total"], 2);
    assert_eq!(results["summary"]["answered"], 2);
    assert_eq!(
        results["summary"]["graded"], 1,
        "only the MCQ has been judged: {results}"
    );
    assert_eq!(results["summary"]["correct"], 1);
    assert_eq!(results["summary"]["score"], 1.0);

    // Marking it wrong later lowers the score, because now it is graded.
    answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[1],
        "Karena tidak ada handshake.",
        Some(false),
    )
    .await;
    let (_, results) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}/results"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(results["summary"]["graded"], 2);
    assert_eq!(results["summary"]["score"], 0.5);
}

/// A replayed attempt applies once, so a retry after a lost response cannot
/// double-count, and the response says whether it applied.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_attempt_id_applies_once(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "replay@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    let attempt_id = uuid::Uuid::new_v4();
    let body = serde_json::json!({
        "id": attempt_id,
        "question_id": question_ids[0],
        "user_answer": "Membuka koneksi TCP.",
    });

    let (status, first) = json(
        &app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        &token,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["applied"], true);

    // The same id again: the write is ignored, not applied twice.
    let (status, second) = json(
        &app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        &token,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["applied"], false,
        "a replay must be ignored rather than double-counted: {second}"
    );

    let attempts: i64 = sqlx::query_scalar("select count(*)::bigint from quiz_attempts")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1, "the row was written once");
}

/// The summary groups by the question's place in the document, weakest first, and a
/// question whose label never resolved is reported rather than dropped.
#[sqlx::test(migrations = "../../migrations")]
async fn the_results_group_by_source_and_page_weakest_first(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // A label the prompt never sent: the question is kept, its link is dropped.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(9)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "topics@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        quiz["questions"][0]["chunk_id"],
        serde_json::Value::Null,
        "label 9 was never sent, so the link is dropped rather than guessed"
    );
    assert_eq!(
        quiz["questions"][0]["prompt"],
        "Apa fungsi three-way handshake?"
    );

    answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[0],
        "Menutup koneksi TCP.",
        None,
    )
    .await;

    let (_, results) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}/results"),
        &token,
        serde_json::Value::Null,
    )
    .await;

    let topics = results["summary"]["topics"].as_array().unwrap();
    assert_eq!(topics.len(), 1, "one unlinked topic: {results}");
    assert!(topics[0]["source_id"].is_null());
    assert!(topics[0]["source_title"].is_null());
    assert_eq!(topics[0]["answered"], 1);
    assert_eq!(
        topics[0]["correct"], 0,
        "the summary counts the unlinked question rather than omitting it"
    );
    assert_eq!(results["summary"]["score"], 0.0);
    // The source is still this user's, so the question is readable: only the link is gone.
    let _ = source_id;
}

/// The monthly cap counts a quiz that is still generating, or the cap is exceeded by
/// asking twice in a row. This is the test Phase 4 needed and did not have.
#[sqlx::test(migrations = "../../migrations")]
async fn a_quiz_still_generating_counts_against_the_monthly_limit(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "cap@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let cap = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .quizzes_per_month
        .max()
        .expect("free is capped");
    assert_eq!(cap, 2, "the free plan allows two quizzes a month");

    // The whole allowance, deliberately not drained, so no quiz has questions yet.
    for index in 0..cap {
        let (status, body) = quiz_created(&app, &workspace_id, &token, &source_id, 10).await;
        assert_eq!(status, StatusCode::CREATED, "quiz {index}: {body}");
    }

    // One more does not fit, whether or not the earlier ones have finished.
    let (status, body) = quiz_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a quiz still generating must count against the month: {body}"
    );
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], cap as i64, "{body}");
}

/// A generation that fails for good refunds the credits, once.
#[sqlx::test(migrations = "../../migrations")]
async fn a_generation_that_fails_for_good_refunds_the_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![
        "not json".to_string(),
        "still not json".to_string(),
    ])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quizrefund@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'quizrefund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = quiz_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let quiz_id = body["quiz"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(quiz["quiz"]["status"], "failed", "{quiz}");
    assert!(
        quiz["quiz"]["error"].as_str().unwrap().contains("JSON"),
        "the error must say what happened: {quiz}"
    );

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'quizrefund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(after, before, "the charge must come back exactly once");
}

/// Another account's quiz is a 404, not a 403: the id must not confirm it exists.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_quiz_is_not_readable(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let owner = signed_up(&app, &harness, "owner@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &owner, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &owner, &workspace_id, &source_id).await;

    let other = signed_up(&app, &harness, "other@example.test").await;
    for uri in [
        format!("/quizzes/{quiz_id}"),
        format!("/quizzes/{quiz_id}/results"),
    ] {
        let (status, _) = json(&app, "GET", &uri, &other, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }

    // And an attempt on someone else's question is refused the same way.
    let (status, _) = json(
        &app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        &other,
        serde_json::json!({
            "question_id": question_ids[0],
            "user_answer": "Membuka koneksi TCP.",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The settings the module declares as required are refused when they are absent or not
/// one of the values the prompt documents, before anything is spent.
#[sqlx::test(migrations = "../../migrations")]
async fn the_generation_settings_are_validated(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "settings@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let base = serde_json::json!({
        "title": "Jaringan",
        "source_ids": [source_id],
        "count": 10,
        "difficulty": "medium",
        "types": "mixed",
    });

    for (field, value) in [
        ("difficulty", serde_json::json!("impossible")),
        ("types", serde_json::json!("essay")),
        ("count", serde_json::json!(0)),
        ("count", serde_json::json!(51)),
    ] {
        let mut body = base.clone();
        body[field] = value;
        let (status, response) = json(
            &app,
            "POST",
            &format!("/workspaces/{workspace_id}/quizzes"),
            &token,
            body,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field} is a caller mistake, not a plan limit, so it is 400: {response}"
        );
        assert_eq!(response["code"], "validation", "{field}: {response}");
    }

    // Nothing was charged or stored by any of the refusals.
    let quizzes: i64 = sqlx::query_scalar("select count(*)::bigint from quizzes")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(quizzes, 0);
}

/// A quiz row whose `difficulty` is NULL must read, not panic the route.
///
/// `quizzes.difficulty` is nullable (`0010_quizzes_attempts.sql`), and the row
/// decoder read it into a non-`Option` `String`. A NULL therefore reached
/// `row.get("difficulty")` and unwrapped into a `ColumnDecode` panic, which
/// `CatchPanicLayer` turned into a 500 for the whole handler — observed in the
/// server log on a real row, not inferred. The route must answer 200 and report
/// the difficulty as absent.
#[sqlx::test(migrations = "../../migrations")]
async fn a_quiz_with_no_difficulty_still_reads(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "nulldiff@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, _) = generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    // The state a legacy or hand-created row can be in, written directly because
    // the API always supplies a difficulty.
    sqlx::query("update quizzes set difficulty = null where id = $1")
        .bind(uuid::Uuid::parse_str(&quiz_id).unwrap())
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["quiz"]["difficulty"].is_null(), "{body}");
    // The rest of the row still arrives, so this is a decoding fix and not a
    // silently truncated response.
    assert_eq!(body["quiz"]["status"], "ready", "{body}");
    assert_eq!(body["quiz"]["id"], quiz_id, "{body}");
}

/// The quiz is resumable: `GET /quizzes/:id` returns the student's attempt per question.
///
/// Without it a reload, or the second device the whole product is premised on, restarts
/// at question 1 with every answer lost while the rows sit in the database. An attempt
/// is one of the kinds a client may write offline, so "what did I already answer" has to
/// be readable from the same call that fetches the questions.
#[sqlx::test(migrations = "../../migrations")]
async fn a_quiz_reports_what_the_student_already_answered(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "resume@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    // Nothing answered yet: the field is null rather than absent, so the client can tell
    // "not answered" from "no such field".
    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert!(quiz["questions"][0]["attempt"].is_null(), "{quiz}");

    answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[0],
        "Membuka koneksi TCP.",
        None,
    )
    .await;

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        quiz["questions"][0]["attempt"]["user_answer"], "Membuka koneksi TCP.",
        "the answered question must carry its attempt back: {quiz}"
    );
    assert_eq!(quiz["questions"][0]["attempt"]["correct"], true);
    assert!(
        quiz["questions"][1]["attempt"].is_null(),
        "the unanswered question must stay unanswered: {quiz}"
    );

    // A graded attempt supersedes an ungraded one for the same question, so a resumed
    // short answer shows the student's own verdict rather than the blank they left.
    answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[1],
        "Karena tidak ada handshake.",
        None,
    )
    .await;
    answered(
        &app,
        &token,
        &quiz_id,
        &question_ids[1],
        "Karena tidak ada handshake.",
        Some(true),
    )
    .await;

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        quiz["questions"][1]["attempt"]["correct"], true,
        "the marked attempt must win over the unmarked one, whatever order they share: {quiz}"
    );
}

/// An attempt carries the device's clock, so an offline answer lands where the student
/// answered it rather than where the sync happened, and "the latest attempt" is decided
/// by that timestamp rather than by two rows sharing the server's instant.
#[sqlx::test(migrations = "../../migrations")]
async fn an_attempt_is_stamped_with_the_device_clock_when_it_gives_one(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "clock@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    let earlier = "2026-01-01T00:00:00Z";
    let later = "2026-01-02T00:00:00Z";

    // Sent later-first, which is what a device whose clock ran ahead produces.
    let mut body = serde_json::json!({
        "question_id": question_ids[0],
        "user_answer": "Membuka koneksi TCP.",
        "answered_at": later,
    });
    let (status, response) = json(
        &app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        &token,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["attempt"]["answered_at"], later);

    // The earlier one arrives second and must become "the latest" only if it really is
    // later, which it is not: the stored answer stays the one stamped `later`.
    body["answered_at"] = serde_json::json!(earlier);
    let (status, _) = json(
        &app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        &token,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        quiz["questions"][0]["attempt"]["answered_at"], later,
        "the attempt the student made last is the one reported: {quiz}"
    );

    // A timestamp far in the future is refused, or a device with a wrong clock would pin
    // itself to the top of every read.
    let (status, response) = json(
        &app,
        "POST",
        &format!("/quizzes/{quiz_id}/attempts"),
        &token,
        serde_json::json!({
            "question_id": question_ids[0],
            "user_answer": "Membuka koneksi TCP.",
            "answered_at": "2099-01-01T00:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["code"], "validation", "{response}");
}

/// A retry is told *what* was wrong, not just that something was.
///
/// The quiz contract validates far more than the deck's does (the kind, exactly four
/// options, an answer among them, key points on a short answer), and every one of those
/// messages names the question's index. A fixed correction would leave that index in the
/// logs and tell the model nothing, so the single retry would not converge.
#[sqlx::test(migrations = "../../migrations")]
async fn the_retry_tells_the_model_what_was_wrong(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // The first reply is valid JSON with an MCQ that has three options, so it parses as
    // JSON and fails the schema; the second is well formed.
    let broken = r#"{"questions":[{"kind":"mcq","prompt":"p","options":["a","b","c"],"answer":"a","explanation":"e","label":1,"key_points":null}]}"#;
    let llm = Arc::new(ScriptedLlm::new(vec![broken.to_string(), reply(1)]));
    let requests = llm.clone();
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "retry@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = quiz_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    drain_jobs(&harness).await;

    let sent = requests.requests();
    assert_eq!(sent.len(), 2, "the reply must have been retried once");
    let correction = &sent[1]
        .messages
        .last()
        .expect("a correction message")
        .content;
    assert!(
        correction.contains("exactly 4 options"),
        "the correction must name the violation: {correction}"
    );
    assert!(
        correction.contains("question 0"),
        "the correction must name the question, which is why the index is in the message: {correction}"
    );
}
