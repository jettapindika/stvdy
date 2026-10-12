//! `POST /sessions`, `GET /sessions` and `GET /activity`.
//!
//! A session is one of the six things a client may write offline, so its clock is the
//! device's and the bound has to compare the client's report against the client's own clock
//! (A204). Comparing against the server's clock instead would reject a legitimate offline
//! session from a device whose clock runs fast, and rejecting valid data is its own defect.
//! The rules are `stvdy_core::study::check_session`'s; this module is the shell that reads a
//! payload into them, and it is the **one** shell: `sync::push::decide` calls
//! [`validate_session`] too, so a rule cannot hold on one path and not the other.
//!
//! `GET /activity` exists because the web build has no local cache: the streak is a pure
//! function of the three instants it is made of, and without this route the web client would
//! have no way to compute it at all (A205).

use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use stvdy_core::study::check_session;
use stvdy_sync::proto::Entity;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tasks::Reject;

/// How far the window reaches when the caller gives no `to`.
///
/// One day, because the read this serves is a day's timeline: "what did I study today" is
/// the question the Today view asks, and a caller wanting a longer window passes `to`.
const DEFAULT_WINDOW_DAYS: i64 = 1;

/// The fields a session payload may name: the protocol's own allowlist, plus `client_ts`.
///
/// `client_ts` is on the wire rather than in the table — the mutation contract already
/// carries it (`Mutation.client_ts`) and `POST /sessions`'s body reuses the field, which is
/// what makes one validator serve both callers (A204).
fn client_columns() -> Vec<&'static str> {
    let mut columns = Entity::StudySession.payload_columns().to_vec();
    columns.push("client_ts");
    columns
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(list).post(create))
        .route("/activity", get(activity))
}

/// A session a write carries, after the bounds have run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// The client's own id (A8). `None` when the payload did not name one, which the push
    /// path allows because a mutation carries its id outside the payload.
    pub id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub duration_min: i32,
    pub task_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    /// The device's own report of when it wrote the row.
    pub client_ts: DateTime<Utc>,
}

/// The bounds every writer of a session row applies.
///
/// `client_ts` is passed in rather than read from the payload, because the two callers hold
/// it in different places: `POST /sessions`'s body carries it and a push mutation carries it
/// on the mutation (A204). A payload that also names `client_ts` must agree with the value
/// the caller passed — a client that reports two different clocks has told us it does not
/// know when it wrote the row, and guessing which one it meant is how a fabricated duration
/// gets a plausible-looking timestamp.
///
/// `server_now` is used for the future bound only, which is the one check a client's clock
/// cannot make.
pub fn validate_session(
    payload: &Value,
    client_ts: DateTime<Utc>,
    server_now: DateTime<Utc>,
) -> Result<NewSession, Reject> {
    let object = payload
        .as_object()
        .ok_or_else(|| Reject::validation("the payload must be a JSON object"))?;
    let known = client_columns();
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            return Err(Reject::validation(format!(
                "{key} is not a field a client may write"
            )));
        }
    }

    if let Some(named) = object.get("client_ts").filter(|value| !value.is_null()) {
        let named: DateTime<Utc> = serde_json::from_value(named.clone())
            .map_err(|error| Reject::validation(format!("client_ts: {error}")))?;
        if named != client_ts {
            return Err(Reject::validation(
                "client_ts disagrees with the mutation's own timestamp",
            ));
        }
    }

    let id: Option<Uuid> = optional(object, "id")?;
    let started_at: DateTime<Utc> = required(object, "started_at")?;
    let duration_min: i32 = required(object, "duration_min")?;
    let task_id: Option<Uuid> = optional(object, "task_id")?;
    let workspace_id: Option<Uuid> = optional(object, "workspace_id")?;

    check_session(started_at, duration_min, client_ts, server_now)
        .map_err(|error| Reject::validation(error.to_string()))?;

    Ok(NewSession {
        id,
        started_at,
        duration_min,
        task_id,
        workspace_id,
        client_ts,
    })
}

fn required<T: serde::de::DeserializeOwned>(
    object: &Map<String, Value>,
    key: &str,
) -> Result<T, Reject> {
    match object.get(key) {
        None | Some(Value::Null) => Err(Reject::validation(format!("{key} is required"))),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

fn optional<T: serde::de::DeserializeOwned>(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<T>, Reject> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct WindowQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct SessionView {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub duration_min: i32,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// `POST /sessions`: append-only, and `applied` says whether this call was the one that
/// stored it.
///
/// The id is the client's, minted when the timer *started* (A206), so a flush after a
/// restart is the same id: `on conflict (id) do nothing` makes a double flush a no-op, which
/// is the mechanical defence against "a timer that logs a session twice on restart".
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Value>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let client_ts: DateTime<Utc> = body
        .get("client_ts")
        .filter(|value| !value.is_null())
        .ok_or_else(|| AppError::Validation("client_ts is required".to_string()))
        .and_then(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| AppError::Validation(format!("client_ts: {error}")))
        })?;

    let session =
        validate_session(&body, client_ts, state.clock.now()).map_err(Reject::into_app_error)?;
    let id = session.id.ok_or_else(|| {
        AppError::Validation(
            "id is required; the client mints it when the timer starts".to_string(),
        )
    })?;

    // A task link must be the caller's own, or the foreign key would be the first thing to
    // notice and it would answer 500. 404 rather than 403, for the reason every other
    // ownership check here is: the existence of another account's row is not this caller's
    // business.
    if let Some(task_id) = session.task_id {
        let owned: bool = sqlx::query_scalar(
            "select exists (select 1 from tasks
                             where id = $1 and user_id = $2 and deleted_at is null)",
        )
        .bind(task_id)
        .bind(user.user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("checking a task link: {error}")))?;
        if !owned {
            return Err(AppError::NotFound);
        }
    }

    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into study_sessions
            (id, user_id, workspace_id, task_id, started_at, duration_min)
         values ($1, $2, $3, $4, $5, $6)
         on conflict (id) do nothing
         returning id",
    )
    .bind(id)
    .bind(user.user_id)
    .bind(session.workspace_id)
    .bind(session.task_id)
    .bind(session.started_at)
    .bind(session.duration_min)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        if crate::sync::apply::client_error(&error).is_some() {
            AppError::Validation("the workspace does not exist".to_string())
        } else {
            AppError::Internal(anyhow::anyhow!("recording a session: {error}"))
        }
    })?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "applied": inserted.is_some() })),
    ))
}

/// `GET /sessions?from=&to=`: the caller's own sessions, newest first.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<WindowQuery>,
) -> AppResult<Json<Value>> {
    let (from, to) = window(query)?;

    let rows: Vec<SessionView> = sqlx::query_as(
        "select id, workspace_id, task_id, started_at, duration_min, created_at, updated_at
           from study_sessions
          where user_id = $1 and deleted_at is null
            and started_at >= $2 and started_at < $3
          order by started_at desc, id desc",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing sessions: {error}")))?;

    Ok(Json(json!({ "sessions": rows })))
}

/// `GET /activity?from=&to=`: the three instants a streak is made of.
///
/// Three arrays and nothing else, in one round trip. The *rule* that turns them into a
/// streak lives on the client (`app/src/features/today/day.ts`), because the Today view has
/// to work offline and a server route cannot (A205) — a second implementation here would be
/// a second answer to "how many days in a row".
pub async fn activity(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<WindowQuery>,
) -> AppResult<Json<Value>> {
    let (from, to) = window(query)?;

    let reviews: Vec<DateTime<Utc>> = sqlx::query_scalar(
        "select reviewed_at from flashcard_reviews
          where user_id = $1 and deleted_at is null
            and reviewed_at >= $2 and reviewed_at < $3
          order by reviewed_at asc",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing reviews: {error}")))?;

    let attempts: Vec<DateTime<Utc>> = sqlx::query_scalar(
        "select answered_at from quiz_attempts
          where user_id = $1 and deleted_at is null
            and answered_at >= $2 and answered_at < $3
          order by answered_at asc",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing attempts: {error}")))?;

    let sessions: Vec<DateTime<Utc>> = sqlx::query_scalar(
        "select started_at from study_sessions
          where user_id = $1 and deleted_at is null
            and started_at >= $2 and started_at < $3
          order by started_at asc",
    )
    .bind(user.user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing sessions: {error}")))?;

    Ok(Json(json!({
        "reviews": reviews,
        "attempts": attempts,
        "sessions": sessions,
    })))
}

/// The window both reads share: `from` required, `to` defaulted, `to` never before `from`.
fn window(query: WindowQuery) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    let from = query
        .from
        .ok_or_else(|| AppError::Validation("from is required".to_string()))?;
    let to = query
        .to
        .unwrap_or_else(|| from + Duration::days(DEFAULT_WINDOW_DAYS));
    if to < from {
        return Err(AppError::Validation(
            "to must not be before from".to_string(),
        ));
    }
    Ok((from, to))
}
