//! Goals: the anchor the planner schedules against.
//!
//! A goal is not plan-gated. The free plan has goals and a **manual** planner — the student
//! writes their own blocks — so nothing here calls the entitlement engine; only
//! `POST /goals/:id/plan` does, and that refusal is `Feature::PlanGeneration`'s (A257).
//!
//! The four fields a goal has are the schema's: a title, a target date, the workspace it
//! belongs to, and the sources its topics come from (`source_ids`). There is no `topics`
//! column on purpose (A252): the topics are the goal's sources plus the effort estimate, and
//! the estimate is stored on the proposal.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::planner::{goal_topics, topic_attempts, topics_for};
use crate::state::AppState;

/// The longest a title may be, matching `events`' and `tasks`' own bound: a title a list row
/// cannot render is a title the student cannot use to tell two goals apart.
const MAX_TITLE_CHARS: usize = 200;

/// The window `GET /goals/:id/progress` reads quiz attempts over (spec § Prep progress).
const PROGRESS_WINDOW_DAYS: i64 = 14;

/// How many goals one read returns. Bounded because an unbounded list is a query that grows
/// with the account rather than with the screen.
const MAX_LIMIT: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/goals", get(list).post(create))
        .route("/goals/{id}", get(one).patch(update).delete(delete_one))
        .route("/goals/{id}/progress", get(progress))
}

/// A goal row, as every read and every returning write selects it.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct GoalView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub target_date: DateTime<Utc>,
    /// The sources whose topics this goal is prepared from. `null` means "none selected",
    /// which is a legal state — the student can create a goal before uploading anything.
    pub source_ids: Option<Vec<Uuid>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

fn goal_columns() -> &'static str {
    "id, workspace_id, title, target_date, source_ids, created_at, updated_at"
}

/// The body of `POST /goals` and `PATCH /goals/:id`.
///
/// `deny_unknown_fields` is what makes a drifted client fail loudly rather than have its
/// field silently dropped: a goal edited through a field the server does not read would
/// report success and change nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalBody {
    pub title: Option<String>,
    pub target_date: Option<DateTime<Utc>>,
    pub workspace_id: Option<Uuid>,
    /// Absent means "leave alone" on a patch and "none selected" on a create. An empty
    /// array is a deliberate "the student cleared the selection", which is a different fact
    /// from an absent one.
    #[serde(default)]
    pub source_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub workspace_id: Option<Uuid>,
    pub limit: Option<i64>,
}

/// `GET /goals` — the caller's own, soonest target date first.
///
/// `target_date` ascending rather than `created_at` descending: the planner's whole job is to
/// work backward from a date, and a list ordered by when a goal was typed buries the exam
/// that is nearest.
pub async fn list(
    State(state): State<AppState>,
    user: VerifiedUser,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    let limit = query.limit.unwrap_or(MAX_LIMIT).clamp(1, MAX_LIMIT);
    let sql = format!(
        "select {} from goals
          where user_id = $1 and deleted_at is null
            and ($2::uuid is null or workspace_id = $2)
          order by target_date asc, created_at asc, id asc
          limit $3",
        goal_columns()
    );
    let rows: Vec<GoalView> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user.user_id)
        .bind(query.workspace_id)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("listing goals: {error}")))?;

    Ok(Json(json!({ "goals": rows })))
}

/// `GET /goals/:id`. 404 for another account's goal or a tombstone.
pub async fn one(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let goal = owned_goal(&state, user.user_id, id).await?;
    Ok(Json(json!({ "goal": goal })))
}

/// `POST /goals`. 201 `{goal}`.
///
/// Creating a goal spends nothing, which is why this is not plan-gated: the free plan's
/// `planner = "manual"` is a statement about who places the blocks, not about who may have
/// an exam to prepare for.
pub async fn create(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<GoalBody>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let title = validated_title(body.title.as_deref())?;
    let target_date = body
        .target_date
        .ok_or_else(|| AppError::Validation("a goal needs a target_date".to_string()))?;
    let workspace_id = body
        .workspace_id
        .ok_or_else(|| AppError::Validation("a goal needs a workspace_id".to_string()))?;

    // A workspace the caller does not own is a 404 rather than a 403: whether someone
    // else's workspace id exists is not information this route should give away.
    crate::sources::routes::owned_workspace(&state, user.user_id, workspace_id).await?;
    let source_ids = checked_sources(&state, user.user_id, workspace_id, body.source_ids).await?;

    let sql = format!(
        "insert into goals (id, user_id, workspace_id, title, target_date, source_ids)
         values ($1, $2, $3, $4, $5, $6)
         returning {}",
        goal_columns()
    );
    let goal: GoalView = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(Uuid::now_v7())
        .bind(user.user_id)
        .bind(workspace_id)
        .bind(&title)
        .bind(target_date)
        .bind(source_ids.as_deref())
        .fetch_one(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("creating a goal: {error}")))?;

    Ok((StatusCode::CREATED, Json(json!({ "goal": goal }))))
}

/// `PATCH /goals/:id`. 200 `{goal}`, 404 for another account's row.
///
/// A field the body leaves out is left alone; a field it names is written, including
/// `source_ids: []`, which clears the selection. The distinction is why the body's fields are
/// `Option<Option<..>>`-shaped at the boundary rather than collapsed with `unwrap_or_default`
/// — clearing a goal's sources and not mentioning them are different edits.
pub async fn update(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<GoalBody>,
) -> AppResult<Json<Value>> {
    let current = owned_goal(&state, user.user_id, id).await?;

    let title = match body.title.as_deref() {
        Some(raw) => Some(validated_title(Some(raw))?),
        None => None,
    };
    let workspace_id = body.workspace_id.unwrap_or(current.workspace_id);
    if body.workspace_id.is_some() {
        crate::sources::routes::owned_workspace(&state, user.user_id, workspace_id).await?;
    }
    let source_ids = match &body.source_ids {
        Some(ids) => checked_sources(&state, user.user_id, workspace_id, Some(ids.clone())).await?,
        None => current.source_ids.clone(),
    };

    let sql = format!(
        "update goals set title = $3, target_date = $4, workspace_id = $5, source_ids = $6
          where id = $1 and user_id = $2 and deleted_at is null
         returning {}",
        goal_columns()
    );
    let goal: Option<GoalView> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user.user_id)
        .bind(title.unwrap_or(current.title))
        .bind(body.target_date.unwrap_or(current.target_date))
        .bind(workspace_id)
        .bind(source_ids.as_deref())
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("editing a goal: {error}")))?;

    goal.map(|goal| Json(json!({ "goal": goal })))
        .ok_or(AppError::NotFound)
}

/// `DELETE /goals/:id`. 204.
///
/// A tombstone, not a row delete: the goal is a synced entity, so the removal has to reach
/// every device, and a real delete would leave the change log nothing to say. Its planned
/// blocks survive with `goal_id` set to null (`0032`'s `on delete set null`), which is the
/// honest outcome — the student deleted the anchor, not the study time they had already put
/// in their calendar.
pub async fn delete_one(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let affected = sqlx::query(
        "update goals set deleted_at = now() where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(id)
    .bind(user.user_id)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting a goal: {error}")))?;

    if affected.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /goals/:id/progress` — the goal's true components, and nothing else.
///
/// Spec § Honesty rules: no composite "readiness" and no grade prediction. Every number is
/// either something the app computed from the student's own attempts or `null` because the
/// data to compute it does not exist — `quiz_accuracy_pct` is `null`, never `0`, when the
/// goal has no attempts, because `0%` is a claim the app cannot make from nothing.
pub async fn progress(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let goal = owned_goal(&state, user.user_id, id).await?;
    let now = state.clock.now();
    let since = now - Duration::days(PROGRESS_WINDOW_DAYS);
    let source_ids = goal.source_ids.clone().unwrap_or_default();

    let topics: Vec<stvdy_core::planner::Topic> =
        topics_for(&state, user.user_id, &source_ids, now)
            .await
            .map_err(AppError::Internal)?;
    let attempts = topic_attempts(&state, user.user_id, &source_ids, since).await?;
    let cards_due = crate::planner::due_cards_for(&state, user.user_id, &source_ids, now)
        .await
        .map_err(AppError::Internal)?;

    let progress = stvdy_core::planner::progress::prep_progress(&topics, &attempts, cards_due);

    Ok(Json(json!({
        "goal_id": goal.id,
        "window_days": PROGRESS_WINDOW_DAYS,
        "topic_coverage_pct": progress.topic_coverage_pct,
        "quiz_accuracy_pct": progress.quiz_accuracy_pct,
        "cards_due": progress.cards_due,
        "weak_topics": progress.weak_topics,
        // The topics themselves, so the screen can render the list the coverage is computed
        // over rather than a percentage with nothing behind it.
        "topics": goal_topics(&topics),
    })))
}

/// The caller's own goal, or a 404.
///
/// One function rather than a `where user_id = $1` copied into five statements: whether
/// someone else's goal id exists is not information any of these routes should give away, and
/// five copies of that predicate is five places to forget it.
pub(crate) async fn owned_goal(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<GoalView> {
    let sql = format!(
        "select {} from goals where id = $1 and user_id = $2 and deleted_at is null",
        goal_columns()
    );
    let goal: Option<GoalView> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a goal: {error}")))?;

    goal.ok_or(AppError::NotFound)
}

/// A title a list row can render, or the 400 that says so.
fn validated_title(raw: Option<&str>) -> AppResult<String> {
    let title = raw.map(str::trim).unwrap_or_default();
    if title.is_empty() {
        return Err(AppError::Validation("a goal needs a title".to_string()));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(AppError::Validation(format!(
            "the title must be at most {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(title.to_string())
}

/// Checks that every named source is the workspace's and the caller's, and returns the
/// deduplicated list.
///
/// A goal carrying a source the student cannot see is worse than a refused edit: the planner
/// would schedule material from another workspace, and the prep-progress card would count
/// topics the student never chose.
async fn checked_sources(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
    source_ids: Option<Vec<Uuid>>,
) -> AppResult<Option<Vec<Uuid>>> {
    let Some(ids) = source_ids else {
        return Ok(None);
    };

    let mut unique: Vec<Uuid> = Vec::with_capacity(ids.len());
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return Ok(Some(unique));
    }

    let owned: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources
          where id = any($1) and workspace_id = $2 and user_id = $3 and deleted_at is null",
    )
    .bind(&unique)
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking the goal's sources: {error}")))?;

    if owned != unique.len() as i64 {
        return Err(AppError::Validation(
            "every source must be one of this workspace's".to_string(),
        ));
    }
    Ok(Some(unique))
}
