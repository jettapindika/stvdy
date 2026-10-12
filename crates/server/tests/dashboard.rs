//! `crates/server/src/dashboard.rs`: the window, the totals, and the weak-topic list.
//!
//! The window case is the sharp one. The dashboard is the student's **local** days (A6), so a
//! session at 23:30 in Jakarta belongs to that day in Jakarta and one at 00:30 belongs to the
//! next one — and a UTC-day window gets both of them wrong in opposite directions, which is
//! why every session below sits near a local midnight rather than at noon.
//!
//! The weak-topic cases are the honesty rule (A289) stated as behaviour: a topic with no
//! attempts is **absent** rather than 0%, a topic with four attempts is not called weak, and
//! the weak list is a filter of the mastery numbers rather than a second query that could
//! disagree with them.
//!
//! `cards_due` is deliberately *not* windowed: it answers "what should I do next", so a card
//! that came due before the queried window still counts. The case that asserts it seeds a due
//! card forty days before `from` for exactly that reason.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, TimeZone as _, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use stvdy_core::traits::testing::FixedClock;
use stvdy_server::testing::TestState;

mod common;
use common::{json, signed_up};

/// The pinned instant every case reasons from. Twelve noon UTC is 19:00 in Jakarta, so the
/// local day is unambiguous on both sides of the window's arithmetic.
fn pinned() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
}

/// The harness with the clock pinned, the plans seeded, and the router built.
async fn harness_at(pool: PgPool) -> (TestState, Router) {
    let harness = TestState::new(pool).with_clock(std::sync::Arc::new(FixedClock::at(pinned())));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    (harness, app)
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The zone the window arithmetic must use. The auth extractor reads `users.timezone` on every
/// request, so a direct update is visible to the next call.
async fn set_timezone(harness: &TestState, user_id: Uuid, tz: &str) {
    sqlx::query("update users set timezone = $2 where id = $1")
        .bind(user_id)
        .bind(tz)
        .execute(&harness.state.pool)
        .await
        .unwrap();
}

async fn workspace(harness: &TestState, user_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar("insert into workspaces (user_id, name) values ($1, $2) returning id")
        .bind(user_id)
        .bind(name)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// One study session, tagged with a workspace or with none.
async fn session(
    harness: &TestState,
    user_id: Uuid,
    workspace_id: Option<Uuid>,
    started_at: DateTime<Utc>,
    minutes: i32,
) {
    sqlx::query(
        "insert into study_sessions (id, user_id, workspace_id, started_at, duration_min)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(workspace_id)
    .bind(started_at)
    .bind(minutes)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// A deck and one card in it, so a review has a card to point at. `due_at` is a parameter
/// because the due count and the review history are two different questions about one card.
async fn card(
    harness: &TestState,
    user_id: Uuid,
    workspace_id: Uuid,
    due_at: DateTime<Utc>,
) -> Uuid {
    let deck_id: Uuid = sqlx::query_scalar(
        "insert into decks (user_id, workspace_id, title, source_ids) values ($1, $2, 'Deck', '{}')
         returning id",
    )
    .bind(user_id)
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "insert into flashcards (user_id, deck_id, front, back, due_at)
         values ($1, $2, 'front', 'back', $3) returning id",
    )
    .bind(user_id)
    .bind(deck_id)
    .bind(due_at)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

async fn review(
    harness: &TestState,
    user_id: Uuid,
    flashcard_id: Uuid,
    reviewed_at: DateTime<Utc>,
) {
    sqlx::query(
        "insert into flashcard_reviews (id, user_id, flashcard_id, rating, reviewed_at)
         values ($1, $2, $3, 'good', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(flashcard_id)
    .bind(reviewed_at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// A topic: a source with a chunk, a quiz over the workspace, and one question attached to the
/// chunk. Returns `(source_id, quiz_id, question_id)`.
async fn topic(
    harness: &TestState,
    user_id: Uuid,
    workspace_id: Uuid,
    title: &str,
) -> (Uuid, Uuid, Uuid) {
    let source_id = Uuid::now_v7();
    sqlx::query(
        "insert into sources (id, user_id, workspace_id, kind, title, status)
         values ($1, $2, $3, 'text', $4, 'ready')",
    )
    .bind(source_id)
    .bind(user_id)
    .bind(workspace_id)
    .bind(title)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let chunk_id = Uuid::now_v7();
    sqlx::query(
        "insert into chunks (id, source_id, idx, content, content_hash, embedder)
         values ($1, $2, 0, 'the material', 'hash', 'test')",
    )
    .bind(chunk_id)
    .bind(source_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let quiz_id = Uuid::now_v7();
    sqlx::query("insert into quizzes (id, user_id, workspace_id, title) values ($1, $2, $3, $4)")
        .bind(quiz_id)
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("{title} quiz"))
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let question_id = Uuid::now_v7();
    sqlx::query(
        "insert into quiz_questions (id, user_id, quiz_id, kind, prompt, answer, chunk_id)
         values ($1, $2, $3, 'mcq', 'q', 'a', $4)",
    )
    .bind(question_id)
    .bind(user_id)
    .bind(quiz_id)
    .bind(chunk_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    (source_id, quiz_id, question_id)
}

async fn attempt(
    harness: &TestState,
    user_id: Uuid,
    quiz_id: Uuid,
    question_id: Uuid,
    correct: bool,
    answered_at: DateTime<Utc>,
) {
    sqlx::query(
        "insert into quiz_attempts (id, user_id, quiz_id, question_id, correct, answered_at)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(quiz_id)
    .bind(question_id)
    .bind(correct)
    .bind(answered_at)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// Grants a live subscription on `plan`, which is what the entitlement decision reads (A146).
async fn upgrade(harness: &TestState, email: &str, plan: &str) {
    let user_id = user_id_of(harness, email).await;
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

/// The window is the student's **local** days (A6), so the sessions near the boundary fall on
/// the side the local clock puts them on and not the UTC one.
///
/// The three sessions are chosen so the correct and the UTC window disagree about *all three*:
/// the 18:00Z one is 01:00 on the 12th in Jakarta (inside the local day, before the UTC day),
/// the 16:30Z one is 23:30 on the 12th (inside both), and the 17:30Z one is 00:30 on the 13th
/// (outside the local day, inside the UTC one). A UTC implementation answers 45+999 and this
/// asserts 30+45.
#[sqlx::test(migrations = "../../migrations")]
async fn the_window_is_the_students_local_days(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-window@example.test").await;
    let user_id = user_id_of(&harness, "dash-window@example.test").await;
    set_timezone(&harness, user_id, "Asia/Jakarta").await;
    let workspace_id = workspace(&harness, user_id, "Jaringan").await;

    session(
        &harness,
        user_id,
        Some(workspace_id),
        Utc.with_ymd_and_hms(2026, 10, 11, 18, 0, 0).unwrap(),
        30,
    )
    .await;
    session(
        &harness,
        user_id,
        Some(workspace_id),
        Utc.with_ymd_and_hms(2026, 10, 12, 16, 30, 0).unwrap(),
        45,
    )
    .await;
    session(
        &harness,
        user_id,
        Some(workspace_id),
        Utc.with_ymd_and_hms(2026, 10, 12, 17, 30, 0).unwrap(),
        999,
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-12&to=2026-10-12",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        body["totals"]["minutes_studied"], 75,
        "the local day 12th is 01:00 and 23:30 Jakarta, not the UTC day: {body}"
    );
    assert_eq!(
        body["from"], "2026-10-12",
        "the window echoes the caller's own dates: {body}"
    );
    assert_eq!(body["to"], "2026-10-12", "{body}");
    let courses = body["time_per_course"].as_array().unwrap();
    assert_eq!(courses.len(), 1, "one workspace, one bucket: {body}");
    assert_eq!(courses[0]["minutes"], 75, "{body}");
    assert_eq!(courses[0]["title"], "Jaringan", "{body}");
}

/// An untagged session is still time the student spent: it is grouped under a null workspace
/// rather than dropped, because a total that excluded it would disagree with the ledger of the
/// student's own sessions.
#[sqlx::test(migrations = "../../migrations")]
async fn a_session_with_no_workspace_is_still_time_studied(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-untagged@example.test").await;
    let user_id = user_id_of(&harness, "dash-untagged@example.test").await;
    let workspace_id = workspace(&harness, user_id, "Kripto").await;

    session(
        &harness,
        user_id,
        Some(workspace_id),
        Utc.with_ymd_and_hms(2026, 10, 10, 9, 0, 0).unwrap(),
        20,
    )
    .await;
    session(
        &harness,
        user_id,
        None,
        Utc.with_ymd_and_hms(2026, 10, 10, 10, 0, 0).unwrap(),
        35,
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-10&to=2026-10-10",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totals"]["minutes_studied"], 55, "{body}");

    let courses = body["time_per_course"].as_array().unwrap();
    assert_eq!(courses.len(), 2, "the untagged session is a bucket: {body}");
    let untagged = courses
        .iter()
        .find(|entry| entry["workspace_id"].is_null())
        .expect("the untagged bucket");
    assert_eq!(untagged["minutes"], 35, "{body}");
    assert!(untagged["title"].is_null(), "{body}");
}

/// Review activity is bucketed by the student's **local** day, in SQL, with the zone as a
/// parameter.
///
/// The two reviews are an hour apart in UTC and a day apart in Jakarta: bucketing them by
/// their UTC date would put both on the 12th, which is the same mistake the session window
/// makes and would be invisible in a zone with a small offset.
#[sqlx::test(migrations = "../../migrations")]
async fn the_review_activity_is_bucketed_by_the_local_day(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-reviews@example.test").await;
    let user_id = user_id_of(&harness, "dash-reviews@example.test").await;
    set_timezone(&harness, user_id, "Asia/Jakarta").await;
    let workspace_id = workspace(&harness, user_id, "Basis Data").await;
    let card_id = card(
        &harness,
        user_id,
        workspace_id,
        Utc.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap(),
    )
    .await;

    review(
        &harness,
        user_id,
        card_id,
        Utc.with_ymd_and_hms(2026, 10, 12, 16, 30, 0).unwrap(),
    )
    .await;
    review(
        &harness,
        user_id,
        card_id,
        Utc.with_ymd_and_hms(2026, 10, 12, 17, 30, 0).unwrap(),
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-12&to=2026-10-13",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totals"]["reviews"], 2, "{body}");

    let buckets = body["review_activity"].as_array().unwrap();
    assert_eq!(
        buckets.len(),
        2,
        "23:30 and 00:30 Jakarta are two local days: {body}"
    );
    assert_eq!(buckets[0]["local_date"], "2026-10-12", "{body}");
    assert_eq!(buckets[0]["reviews"], 1, "{body}");
    assert_eq!(buckets[1]["local_date"], "2026-10-13", "{body}");
    assert_eq!(buckets[1]["reviews"], 1, "{body}");
}

/// `cards_due` is "now" and not the window's: a card that came due before `from` still counts,
/// and one that comes due after the clock does not.
#[sqlx::test(migrations = "../../migrations")]
async fn the_due_card_count_is_now_and_not_the_window(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-due@example.test").await;
    let user_id = user_id_of(&harness, "dash-due@example.test").await;
    let workspace_id = workspace(&harness, user_id, "Statistika").await;

    // Due forty days before the queried window opens — still due today.
    card(
        &harness,
        user_id,
        workspace_id,
        Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
    )
    .await;
    // Due tomorrow, which is not yet.
    card(
        &harness,
        user_id,
        workspace_id,
        Utc.with_ymd_and_hms(2026, 10, 13, 9, 0, 0).unwrap(),
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-12&to=2026-10-12",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["totals"]["cards_due"], 1,
        "a card due before the window is still due now, and one due tomorrow is not: {body}"
    );
    assert_eq!(
        body["totals"]["minutes_studied"], 0,
        "the due count is not study time: {body}"
    );
}

/// A quiz *taken* is a distinct quiz with at least one attempt, and a deleted attempt is not
/// one: "3 quizzes" and "40 answers" are different claims.
#[sqlx::test(migrations = "../../migrations")]
async fn the_quiz_total_counts_quizzes_not_answers(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-quizzes@example.test").await;
    let user_id = user_id_of(&harness, "dash-quizzes@example.test").await;
    let workspace_id = workspace(&harness, user_id, "Aljabar").await;
    let (_, quiz_id, question_id) = topic(&harness, user_id, workspace_id, "Matriks").await;

    for _ in 0..3 {
        attempt(
            &harness,
            user_id,
            quiz_id,
            question_id,
            true,
            Utc.with_ymd_and_hms(2026, 10, 11, 10, 0, 0).unwrap(),
        )
        .await;
    }

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-11&to=2026-10-11",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["totals"]["quizzes_taken"], 1,
        "three answers on one quiz is one quiz taken: {body}"
    );
}

/// The window is bounded in both directions, and a default window is the last thirty local
/// days ending today.
#[sqlx::test(migrations = "../../migrations")]
async fn the_window_is_bounded_and_defaults_to_thirty_days(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-bound@example.test").await;

    let (status, body) = json(&app, "GET", "/dashboard", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["to"], "2026-10-12",
        "the default window ends today, in the caller's zone: {body}"
    );
    assert_eq!(
        body["from"], "2026-09-13",
        "thirty days inclusive, so from is twenty-nine days before to: {body}"
    );

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-13&to=2026-10-12",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2020-01-01&to=2026-10-12",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// A free account reads the ungated half and is refused the advanced one, which is why the
/// dashboard is two routes rather than one: the screen renders the free part and a locked card
/// for the rest (A289).
#[sqlx::test(migrations = "../../migrations")]
async fn the_free_half_works_and_the_advanced_half_is_refused(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-free@example.test").await;

    let (status, body) = json(&app, "GET", "/dashboard", &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "the ungated half is free: {body}");

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard/topics",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "AdvancedAnalytics is Power-only: {body}"
    );
    assert_eq!(body["code"], "plan_required", "{body}");
}

/// The honesty rule (A289) as behaviour: a topic with no attempts is absent, four attempts is
/// not evidence of weakness, and the weak list is a filter of the mastery numbers — the same
/// attempts and the same accuracy, not a second query's answer.
#[sqlx::test(migrations = "../../migrations")]
async fn a_weak_topic_needs_evidence_and_an_unattempted_topic_is_absent(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-weak@example.test").await;
    upgrade(&harness, "dash-weak@example.test", "power").await;
    let user_id = user_id_of(&harness, "dash-weak@example.test").await;
    let workspace_id = workspace(&harness, user_id, "Kelas").await;

    // Weak: six attempts, two right — under 60% with enough evidence to say so.
    let (weak, weak_quiz, weak_question) = topic(&harness, user_id, workspace_id, "Jaringan").await;
    for (index, correct) in [true, true, false, false, false, false]
        .into_iter()
        .enumerate()
    {
        attempt(
            &harness,
            user_id,
            weak_quiz,
            weak_question,
            correct,
            Utc.with_ymd_and_hms(2026, 10, 11, 8 + index as u32, 0, 0)
                .unwrap(),
        )
        .await;
    }

    // Not weak yet: four attempts, none right. The accuracy is 0, and one more attempt is what
    // makes it evidence rather than an accident.
    let (thin, thin_quiz, thin_question) = topic(&harness, user_id, workspace_id, "Kripto").await;
    for index in 0..4 {
        attempt(
            &harness,
            user_id,
            thin_quiz,
            thin_question,
            false,
            Utc.with_ymd_and_hms(2026, 10, 11, 12 + index, 0, 0)
                .unwrap(),
        )
        .await;
    }

    // Attempted nothing: absent rather than shown as 0%.
    let (silent, _, _) = topic(&harness, user_id, workspace_id, "Basis Data").await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard/topics?from=2026-10-11&to=2026-10-11",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mastery = body["quiz_mastery"].as_array().unwrap();
    assert_eq!(
        mastery.len(),
        2,
        "the unattempted topic is absent, not 0%: {body}"
    );
    let ids: Vec<&str> = mastery
        .iter()
        .map(|entry| entry["source_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&weak.to_string().as_str()), "{body}");
    assert!(ids.contains(&thin.to_string().as_str()), "{body}");
    assert!(
        !ids.contains(&silent.to_string().as_str()),
        "a topic with no attempts has no accuracy to report: {body}"
    );

    let weak_entries = body["weak_topics"].as_array().unwrap();
    assert_eq!(
        weak_entries.len(),
        1,
        "four attempts is not evidence of weakness: {body}"
    );
    assert_eq!(weak_entries[0]["source_id"], weak.to_string(), "{body}");
    assert_eq!(weak_entries[0]["attempts"], 6, "{body}");
    assert_eq!(weak_entries[0]["correct"], 2, "{body}");

    // The weak entry is the mastery entry, not a second query's answer.
    let mastery_entry = mastery
        .iter()
        .find(|entry| entry["source_id"] == weak.to_string())
        .unwrap();
    assert_eq!(
        weak_entries[0]["accuracy"], mastery_entry["accuracy"],
        "the list and the table must be one answer: {body}"
    );

    // And the thresholds travel with the data, so the screen's copy cannot drift from the list.
    assert_eq!(body["weak_threshold"]["accuracy"], 0.6, "{body}");
    assert_eq!(body["weak_threshold"]["min_attempts"], 5, "{body}");
}

/// A hand-written question carries no chunk, so its attempts name no source: the topic is
/// **absent** rather than counted as 0%, which is the same rule the weak list follows.
#[sqlx::test(migrations = "../../migrations")]
async fn a_question_with_no_source_is_absent_from_mastery(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let token = signed_up(&app, &harness, "dash-nochunk@example.test").await;
    upgrade(&harness, "dash-nochunk@example.test", "power").await;
    let user_id = user_id_of(&harness, "dash-nochunk@example.test").await;
    let workspace_id = workspace(&harness, user_id, "Kelas").await;

    let quiz_id = Uuid::now_v7();
    sqlx::query(
        "insert into quizzes (id, user_id, workspace_id, title) values ($1, $2, $3, 'Hand')",
    )
    .bind(quiz_id)
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    let question_id = Uuid::now_v7();
    sqlx::query(
        "insert into quiz_questions (id, user_id, quiz_id, kind, prompt, answer)
         values ($1, $2, $3, 'short', 'q', 'a')",
    )
    .bind(question_id)
    .bind(user_id)
    .bind(quiz_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    attempt(
        &harness,
        user_id,
        quiz_id,
        question_id,
        false,
        Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap(),
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard/topics?from=2026-10-11&to=2026-10-11",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["quiz_mastery"].as_array().unwrap().is_empty(),
        "an attempt with no source has no topic to report: {body}"
    );
    assert!(body["weak_topics"].as_array().unwrap().is_empty(), "{body}");
}

/// Another account's rows are not counted, on either route: a session, a review and an attempt
/// belonging to Bob are invisible to Alice's dashboard.
#[sqlx::test(migrations = "../../migrations")]
async fn another_accounts_rows_are_not_counted(pool: PgPool) {
    let (harness, app) = harness_at(pool).await;
    let alice = signed_up(&app, &harness, "dash-alice@example.test").await;
    let bob = signed_up(&app, &harness, "dash-bob@example.test").await;
    upgrade(&harness, "dash-alice@example.test", "power").await;

    let bob_id = user_id_of(&harness, "dash-bob@example.test").await;
    let bob_workspace = workspace(&harness, bob_id, "Bob's class").await;
    session(
        &harness,
        bob_id,
        Some(bob_workspace),
        Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap(),
        120,
    )
    .await;
    let bob_card = card(
        &harness,
        bob_id,
        bob_workspace,
        Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
    )
    .await;
    review(
        &harness,
        bob_id,
        bob_card,
        Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap(),
    )
    .await;
    let (_, bob_quiz, bob_question) = topic(&harness, bob_id, bob_workspace, "Bob's topic").await;
    attempt(
        &harness,
        bob_id,
        bob_quiz,
        bob_question,
        false,
        Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap(),
    )
    .await;

    // Alice's own one row, so the zeros below are about scoping and not about an empty table.
    let alice_id = user_id_of(&harness, "dash-alice@example.test").await;
    let alice_workspace = workspace(&harness, alice_id, "Alice's class").await;
    session(
        &harness,
        alice_id,
        Some(alice_workspace),
        Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap(),
        10,
    )
    .await;

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-11&to=2026-10-11",
        &alice,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totals"]["minutes_studied"], 10, "{body}");
    assert_eq!(body["totals"]["reviews"], 0, "{body}");
    assert_eq!(body["totals"]["quizzes_taken"], 0, "{body}");
    assert_eq!(body["totals"]["cards_due"], 0, "{body}");

    let (status, body) = json(
        &app,
        "GET",
        "/dashboard/topics?from=2026-10-11&to=2026-10-11",
        &alice,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["quiz_mastery"].as_array().unwrap().is_empty(),
        "Bob's attempts are not Alice's mastery: {body}"
    );

    // Bob's own dashboard sees his rows, so the assertion above is about scoping rather than
    // about the seed having failed.
    let (status, body) = json(
        &app,
        "GET",
        "/dashboard?from=2026-10-11&to=2026-10-11",
        &bob,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totals"]["minutes_studied"], 120, "{body}");
    assert_eq!(body["totals"]["reviews"], 1, "{body}");
}
