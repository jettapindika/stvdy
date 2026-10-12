//! `crates/server/src/study.rs`: the sessions, the bound on their length, and the activity
//! read.
//!
//! The bound case is the one this suite exists for. A session is written offline, so its
//! clock is the device's, and the bound has to compare the client's report against the
//! client's own clock — a server-clock comparison would reject a legitimate offline session
//! from a device whose clock runs fast, which is why the two directions are asserted
//! separately (A204).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// A session body at `started_at`, lasting `duration_min`, reported by a device whose clock
/// says `client_ts`.
fn body(id: Uuid, started_at: &str, duration_min: i64, client_ts: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "started_at": started_at,
        "duration_min": duration_min,
        "client_ts": client_ts,
    })
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The normal path: a session is recorded once and reports `applied`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_is_recorded_once_and_reports_applied(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "session@example.test").await;
    let id = Uuid::now_v7();

    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(id, "2026-10-12T01:00:00Z", 45, "2026-10-12T01:50:00Z"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["applied"], true, "{answer}");

    // The same id again is the same session: a timer that flushes twice after a restart
    // must not log two.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(id, "2026-10-12T01:00:00Z", 45, "2026-10-12T01:50:00Z"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["applied"], false, "{answer}");

    let stored: i64 =
        sqlx::query_scalar("select count(*)::bigint from study_sessions where id = $1")
            .bind(id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(stored, 1, "a replay is a no-op, not a second row");
}

/// The fabrication A204 exists for, and the reason the bound is not a client check.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_longer_than_the_client_reported_elapsed_time_is_refused(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "fabricated@example.test").await;

    // Started at 09:00, the device says it wrote the row at 09:05, and the session claims
    // ten hours.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2026-10-12T09:00:00Z",
            600,
            "2026-10-12T09:05:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
    assert!(
        answer["message"]
            .as_str()
            .is_some_and(|message| message.contains("longer")),
        "the refusal names the bound: {answer}"
    );

    // A duration outside the day is refused on its own, before the elapsed comparison.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2026-10-12T09:00:00Z",
            1441,
            "2026-10-13T09:00:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");

    // The two-minute grace is real: 47 minutes reported against a 45-minute elapsed span is
    // accepted, because a slow flush is not a fabrication.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2026-10-12T09:00:00Z",
            47,
            "2026-10-12T09:45:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["applied"], true, "{answer}");
}

/// A legitimate offline session arrives hours later and is accepted, because the bound reads
/// `client_ts` and not the server's clock.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_pushed_hours_later_is_accepted(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "offline-session@example.test").await;

    // Yesterday's session, reported by a device that was offline all day: the server's
    // clock is hours past the client's, and comparing against it would refuse this.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2020-01-02T09:00:00Z",
            90,
            "2020-01-02T10:30:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer["applied"], true, "{answer}");

    // And a device whose clock runs fast is not punished either: the start is inside the
    // 24 h future bound, and the elapsed comparison uses the same fast clock rather than
    // the server's. `now` is read here rather than written down, because the future bound
    // is the one part of this rule that *is* about the server's clock.
    let started = chrono::Utc::now() + chrono::Duration::hours(12);
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            &started.to_rfc3339(),
            30,
            &(started + chrono::Duration::minutes(30)).to_rfc3339(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");

    // A start beyond the 24 h bound is refused even though the elapsed comparison would be
    // satisfied: a session that has not happened yet is the one thing the client's own
    // clock cannot rule out.
    let too_far = chrono::Utc::now() + chrono::Duration::hours(30);
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            &too_far.to_rfc3339(),
            30,
            &(too_far + chrono::Duration::minutes(30)).to_rfc3339(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
}

/// `activity` reports the three instants and nothing else, in one round trip.
#[sqlx::test(migrations = "../../migrations")]
async fn activity_reports_reviews_attempts_and_sessions(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "activity@example.test").await;
    let user_id = user_id_of(&harness, "activity@example.test").await;

    let workspace = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'Kuliah')")
        .bind(workspace)
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let deck = Uuid::now_v7();
    sqlx::query(
        "insert into decks (id, user_id, workspace_id, title, source_ids)
         values ($1, $2, $3, 'Deck', '{}')",
    )
    .bind(deck)
    .bind(user_id)
    .bind(workspace)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    let card = Uuid::now_v7();
    sqlx::query(
        "insert into flashcards (id, user_id, deck_id, front, back)
         values ($1, $2, $3, 'Depan', 'Belakang')",
    )
    .bind(card)
    .bind(user_id)
    .bind(deck)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into flashcard_reviews (id, user_id, flashcard_id, rating, reviewed_at)
         values ($1, $2, $3, 4, '2026-10-12T02:00:00Z')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(card)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let quiz = Uuid::now_v7();
    sqlx::query(
        "insert into quizzes
            (id, user_id, workspace_id, title, difficulty, status, source_ids,
             requested_count, question_types)
         values ($1, $2, $3, 'Quiz', 'medium', 'ready', '{}', 5, 'mcq')",
    )
    .bind(quiz)
    .bind(user_id)
    .bind(workspace)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    let question = Uuid::now_v7();
    sqlx::query(
        "insert into quiz_questions (id, user_id, quiz_id, kind, prompt, options, answer)
         values ($1, $2, $3, 'mcq', 'Pertanyaan', '[]', 'A')",
    )
    .bind(question)
    .bind(user_id)
    .bind(quiz)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into quiz_attempts
            (id, user_id, quiz_id, question_id, user_answer, correct, answered_at)
         values ($1, $2, $3, $4, 'A', true, '2026-10-12T03:00:00Z')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(quiz)
    .bind(question)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // A session inside the window, and one outside it.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2026-10-12T04:00:00Z",
            30,
            "2026-10-12T04:30:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2026-09-01T04:00:00Z",
            30,
            "2026-09-01T04:30:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");

    let (status, answer) = json(
        &app,
        "GET",
        "/activity?from=2026-10-12T00:00:00Z&to=2026-10-13T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    for (key, expected) in [
        ("reviews", "2026-10-12T02:00:00Z"),
        ("attempts", "2026-10-12T03:00:00Z"),
        ("sessions", "2026-10-12T04:00:00Z"),
    ] {
        let instants = answer[key].as_array().unwrap_or_else(|| {
            panic!("{key} must be an array: {answer}");
        });
        assert_eq!(
            instants.len(),
            1,
            "{key} has exactly the in-window row: {answer}"
        );
        assert_eq!(instants[0].as_str().unwrap(), expected, "{key}: {answer}");
    }

    // And the read is exactly three keys: a fourth would be a number the client cannot
    // trace to a row.
    let keys: Vec<&str> = answer
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["attempts", "reviews", "sessions"], "{answer}");
}

/// Another account's session is not in the window, and a session for another account's task
/// is a 404 rather than a foreign key violation surfacing as a 500.
#[sqlx::test(migrations = "../../migrations")]
async fn sessions_are_scoped_and_a_foreign_task_is_not_found(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let mine = signed_up(&app, &harness, "mine-session@example.test").await;
    let theirs = signed_up(&app, &harness, "theirs-session@example.test").await;
    let other_user = user_id_of(&harness, "theirs-session@example.test").await;

    // Their session, written directly so it exists without their token.
    let their_session = Uuid::now_v7();
    sqlx::query(
        "insert into study_sessions (id, user_id, started_at, duration_min)
         values ($1, $2, '2026-10-12T05:00:00Z', 20)",
    )
    .bind(their_session)
    .bind(other_user)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, answer) = json(
        &app,
        "GET",
        "/sessions?from=2026-10-12T00:00:00Z&to=2026-10-13T00:00:00Z",
        &mine,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["sessions"].as_array().unwrap().len(),
        0,
        "another account's session must not be listed: {answer}"
    );

    let their_task = Uuid::now_v7();
    sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, 'Milik orang lain')")
        .bind(their_task)
        .bind(other_user)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let mut request = body(
        Uuid::now_v7(),
        "2026-10-12T06:00:00Z",
        30,
        "2026-10-12T06:30:00Z",
    );
    request["task_id"] = serde_json::json!(their_task);
    let (status, answer) = json(&app, "POST", "/sessions", &mine, request).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "not_found", "{answer}");

    let _ = theirs;
}

/// The window is required at its start and half-open at its end, so a session is counted on
/// exactly one day.
#[sqlx::test(migrations = "../../migrations")]
async fn the_window_is_half_open(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "halfopen@example.test").await;

    // Exactly at `to`: outside the window.
    let (status, answer) = json(
        &app,
        "POST",
        "/sessions",
        &token,
        body(
            Uuid::now_v7(),
            "2026-10-13T00:00:00Z",
            10,
            "2026-10-13T00:10:00Z",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");

    let (status, answer) = json(
        &app,
        "GET",
        "/sessions?from=2026-10-12T00:00:00Z&to=2026-10-13T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["sessions"].as_array().unwrap().len(), 0, "{answer}");

    // A missing `from` is a 400 rather than a full scan.
    let (status, answer) = json(&app, "GET", "/sessions", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "validation", "{answer}");
}
