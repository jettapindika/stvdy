//! `GET /dashboard` and `GET /dashboard/topics`.
//!
//! **Every number here traces to a row the student owns** (A289). That is the honesty gate's
//! requirement stated as a design rule, and it is why there is no composite score, no
//! predicted grade and no "retention": each of those is a number the app would be *computing
//! about* the student rather than counting about their own rows, and the second kind is not
//! the same claim as the first.
//!
//! The window is the student's **local days**, not UTC days (A6): a session at 23:30 in Jakarta
//! belongs to that day in Jakarta, and a UTC window would move it to the next one for every
//! user east of Greenwich — which is where most of this product's users are. `from`/`to` are
//! local dates and are turned into instants through `chrono-tz`.
//!
//! Two routes rather than one with a flag, so the sweep can drive a real refusal and the screen
//! can render a locked card for the part that is locked while the rest of the dashboard works.
//! `GET /dashboard/topics` is the `AdvancedAnalytics` half, and a topic with no attempts is
//! **absent** rather than shown as 0%: an uncomputable number must not reach the screen.

use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use stvdy_core::entitlements::Feature;

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How many local days the dashboard covers when the caller names no window.
///
/// Thirty: a month is the shortest window that can show a trend without a screenful of
/// single-bar days, and it is the period the credit ledger already resets on, so "this month"
/// means one thing in the product rather than two.
const DEFAULT_WINDOW_DAYS: i64 = 30;

/// The furthest back a caller may look. Bounded so a crafted `from` cannot make the aggregate
/// scan the whole table.
const MAX_WINDOW_DAYS: i64 = 366;

/// A topic below this accuracy is "weak". Named rather than inline because the client shows the
/// list and a threshold it did not receive would be a second source of truth for the same
/// number.
const WEAK_TOPIC_ACCURACY: f64 = 0.6;

/// At least this many attempts before a topic can be called weak: one wrong answer is not
/// evidence, and a list that says "you are weak at this" from a single attempt is the honesty
/// rule's own failure case.
const WEAK_TOPIC_MIN_ATTEMPTS: i64 = 5;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboard", get(read))
        .route("/dashboard/topics", get(topics))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardQuery {
    /// The first local day of the window, inclusive.
    pub from: Option<NaiveDate>,
    /// The last local day of the window, inclusive. A date rather than an instant, because
    /// the window is a set of the student's own days.
    pub to: Option<NaiveDate>,
}

/// The window as a pair of UTC instants, from local dates.
///
/// `to` is inclusive and becomes the *start of the following local day*, which is what makes
/// a session at 23:59 on the last day fall inside the window rather than out of it. The
/// instants go through `chrono-tz` and never a fixed offset (A6).
fn window(
    query: &DashboardQuery,
    tz: Tz,
    now: DateTime<Utc>,
) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    let today = now.with_timezone(&tz).date_naive();
    let to = query.to.unwrap_or(today);
    let from = query
        .from
        .unwrap_or(to - chrono::Duration::days(DEFAULT_WINDOW_DAYS - 1));

    if to < from {
        return Err(AppError::Validation(
            "to must not be earlier than from".to_string(),
        ));
    }
    if (to - from).num_days() >= MAX_WINDOW_DAYS {
        return Err(AppError::Validation(format!(
            "the window must be no longer than {MAX_WINDOW_DAYS} days"
        )));
    }

    let start = local_midnight(tz, from)
        .ok_or_else(|| AppError::Validation("from is not a resolvable local date".to_string()))?;
    let end = local_midnight(tz, to + chrono::Duration::days(1))
        .ok_or_else(|| AppError::Validation("to is not a resolvable local date".to_string()))?;
    Ok((start, end))
}

/// Midnight at the start of a local date, as UTC.
///
/// A date whose local midnight does not exist (a spring-forward transition at midnight, which
/// several zones have) resolves to the first instant that does, which is what
/// `from_local_datetime(..).earliest()` gives — the same rule Phase 10's expansion and the
/// reminder engine use, so a day boundary means one thing across the product.
fn local_midnight(tz: Tz, date: NaiveDate) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(0, 0, 0)?;
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|instant| instant.with_timezone(&Utc))
}

/// `GET /dashboard`: time per course, review activity, and the totals.
pub async fn read(
    State(state): State<AppState>,
    user: VerifiedUser,
    Query(query): Query<DashboardQuery>,
) -> AppResult<Json<Value>> {
    let tz: Tz = user.timezone.parse().unwrap_or(chrono_tz::UTC);
    let (from, to) = window(&query, tz, state.clock.now())?;

    // Time per course, joined through `workspaces` for the title. A session with no workspace
    // is *not* dropped: an untagged session is still time the student spent, and a total that
    // excluded it would disagree with the ledger of their own sessions. It is grouped under a
    // null id so the client can label it.
    let time_per_course: Vec<(Option<Uuid>, Option<String>, i64)> = sqlx::query_as(
        "select s.workspace_id, w.name, coalesce(sum(s.duration_min), 0)::bigint
           from study_sessions s
           left join workspaces w on w.id = s.workspace_id
          where s.user_id = $1 and s.deleted_at is null
            and s.started_at >= $2 and s.started_at < $3
          group by s.workspace_id, w.name
          order by sum(s.duration_min) desc nulls last, s.workspace_id",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("summing study time: {error}")))?;

    // Review activity, bucketed by the student's **local** day. The bucketing happens in SQL
    // with the zone as a parameter, because doing it in Rust would mean fetching every review
    // in the window to count them.
    let review_activity: Vec<(NaiveDate, i64)> = sqlx::query_as(
        "select (r.reviewed_at at time zone $4)::date as local_date, count(*)::bigint
           from flashcard_reviews r
          where r.user_id = $1 and r.deleted_at is null
            and r.reviewed_at >= $2 and r.reviewed_at < $3
          group by local_date
          order by local_date",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .bind(tz.name())
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("counting reviews: {error}")))?;

    let minutes_studied: i64 = time_per_course.iter().map(|(_, _, minutes)| minutes).sum();
    let reviews: i64 = review_activity.iter().map(|(_, count)| count).sum();

    // The due-card count is "now", not the window's: it answers "what should I do next", and a
    // windowed due count would be a number about the past wearing the present's label.
    let cards_due = crate::jobs::handlers::reminders::due_cards_for_user(&state, user.user_id)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("counting due cards: {error}")))?;

    // Quizzes *taken* in the window: distinct quizzes with at least one attempt, not attempts.
    // "3 quizzes" and "40 answers" are different claims and the screen shows both.
    let quizzes_taken: i64 = sqlx::query_scalar(
        "select count(distinct a.quiz_id)::bigint
           from quiz_attempts a
          where a.user_id = $1 and a.deleted_at is null
            and a.answered_at >= $2 and a.answered_at < $3",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("counting quizzes: {error}")))?;

    Ok(Json(json!({
        "from": from.with_timezone(&tz).date_naive(),
        "to": (to - chrono::Duration::nanoseconds(1)).with_timezone(&tz).date_naive(),
        "time_per_course": time_per_course
            .into_iter()
            .map(|(workspace_id, title, minutes)| json!({
                "workspace_id": workspace_id,
                "title": title,
                "minutes": minutes,
            }))
            .collect::<Vec<Value>>(),
        "review_activity": review_activity
            .into_iter()
            .map(|(local_date, reviews)| json!({ "local_date": local_date, "reviews": reviews }))
            .collect::<Vec<Value>>(),
        "totals": {
            "minutes_studied": minutes_studied,
            "reviews": reviews,
            "cards_due": cards_due,
            "quizzes_taken": quizzes_taken,
        },
    })))
}

/// `GET /dashboard/topics`: per-topic mastery and the weak-topic list, `AdvancedAnalytics` only.
///
/// A topic is a `source` — the material the student uploaded — and mastery is the accuracy of
/// the quiz attempts whose questions came from it. `quiz_questions.chunk_id` is the link: a
/// generated question carries the chunk it was written from, and the chunk names its source.
/// A question with no chunk (a hand-written one) is therefore absent rather than counted as 0%,
/// which is the same rule the weak list follows.
pub async fn topics(
    State(state): State<AppState>,
    user: VerifiedUser,
    Query(query): Query<DashboardQuery>,
) -> AppResult<Json<Value>> {
    crate::entitlements::require(&state, &user, Feature::AdvancedAnalytics).await?;

    let tz: Tz = user.timezone.parse().unwrap_or(chrono_tz::UTC);
    let (from, to) = window(&query, tz, state.clock.now())?;

    let mastery: Vec<(Uuid, String, i64, i64)> = sqlx::query_as(
        "select s.id, s.title, count(*)::bigint as attempts,
                count(*) filter (where a.correct)::bigint as correct
           from quiz_attempts a
           join quiz_questions q on q.id = a.question_id
           join chunks c on c.id = q.chunk_id
           join sources s on s.id = c.source_id
          where a.user_id = $1 and a.deleted_at is null
            and q.deleted_at is null
            and s.deleted_at is null
            and a.answered_at >= $2 and a.answered_at < $3
          group by s.id, s.title
          order by s.title, s.id",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading mastery: {error}")))?;

    let quiz_mastery: Vec<Value> = mastery
        .iter()
        .map(|(source_id, title, attempts, correct)| {
            json!({
                "source_id": source_id,
                "title": title,
                "attempts": attempts,
                "correct": correct,
                // A rate, not a score: `correct / attempts` over rows the student produced.
                "accuracy": *correct as f64 / *attempts as f64,
            })
        })
        .collect();

    // The weak list is a **filter of the same numbers**, never a second query: two queries
    // would be two answers to "how well is this topic going", and the list and the table would
    // eventually disagree.
    let weak_topics: Vec<Value> = quiz_mastery
        .iter()
        .filter(|entry| {
            let attempts = entry["attempts"].as_i64().unwrap_or(0);
            let accuracy = entry["accuracy"].as_f64().unwrap_or(1.0);
            attempts >= WEAK_TOPIC_MIN_ATTEMPTS && accuracy < WEAK_TOPIC_ACCURACY
        })
        .cloned()
        .collect();

    Ok(Json(json!({
        "from": from.with_timezone(&tz).date_naive(),
        "to": (to - chrono::Duration::nanoseconds(1)).with_timezone(&tz).date_naive(),
        "quiz_mastery": quiz_mastery,
        "weak_topics": weak_topics,
        // The thresholds travel with the data, so the screen's copy and the list it is
        // describing cannot drift apart.
        "weak_threshold": {
            "accuracy": WEAK_TOPIC_ACCURACY,
            "min_attempts": WEAK_TOPIC_MIN_ATTEMPTS,
        },
        // A289: the weak-topic card hands its topic to Phase 11's planner rather than proposing
        // blocks itself. Named here so the client does not have to know the route.
        "plan_route": "/goals/{goal_id}/plan",
    })))
}
