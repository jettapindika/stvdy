//! `GET/POST/PATCH/DELETE /events`.
//!
//! Two rules here are the ones a plausible implementation gets wrong:
//!
//!  * The window is an **overlap**, not a containment. A three-hour class that started
//!    before `from` and ends after it is on the timeline the student is looking at, and a
//!    `starts_at between from and to` filter drops it with nothing on the page to say why.
//!  * Four columns — `source`, `external_id`, `etag`, `connection_id` — are **Phase 10's**
//!    (A198). Google and ICS fill them; a client that could write them could forge a
//!    "Google" event, hijack the `(connection_id, external_id)` unique index, or overwrite a
//!    synced row the next incremental sync would then not match. The columns exist today
//!    (migration `0013`), so the rule is enforced today rather than when Phase 10 lands.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use stvdy_sync::proto::Entity;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tasks::Reject;

/// The `kind` values `events.kind` accepts.
///
/// A `text` column with no `check` constraint, so this is the only place the set is
/// enforced: a value outside it is a string no screen can render, and storing it would push
/// the failure to the client.
const KINDS: [&str; 5] = ["class", "exam", "study_block", "deadline", "external"];

/// The source a client's own event carries. Everything else is the calendar's (A198).
const APP_SOURCE: &str = "app";

/// How far ahead the window reaches when the caller gives no `to`.
const DEFAULT_WINDOW_DAYS: i64 = 7;

/// The columns the calendar sync owns (A198), named so the refusal can list them.
///
/// `source` is the fourth and is handled separately, because `source = "app"` is the one
/// value a client may state: it is the column's own default, and refusing it would refuse
/// the payload a client that echoes the row it pulled would send.
const CALENDAR_COLUMNS: &[&str] = &["external_id", "etag", "connection_id"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/events", get(list).post(create))
        .route(
            "/events/{id}",
            axum::routing::patch(update).delete(delete_one),
        )
}

/// An event payload after the field rules have run. `None` means the payload did not name
/// the field, `Some(None)` that it named it as null.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewEvent {
    pub workspace_id: Option<Option<Uuid>>,
    pub kind: Option<String>,
    pub title: Option<String>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub task_id: Option<Option<Uuid>>,
}

impl NewEvent {
    /// The named fields as a JSON object, for the writers' `jsonb_populate_record`.
    pub fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        let mut put = |key: &str, value: Value| {
            fields.insert(key.to_string(), value);
        };
        if let Some(value) = &self.workspace_id {
            put("workspace_id", json!(value));
        }
        if let Some(value) = &self.kind {
            put("kind", json!(value));
        }
        if let Some(value) = &self.title {
            put("title", json!(value));
        }
        if let Some(value) = &self.starts_at {
            put("starts_at", json!(value));
        }
        if let Some(value) = &self.ends_at {
            put("ends_at", json!(value));
        }
        if let Some(value) = &self.source {
            put("source", json!(value));
        }
        if let Some(value) = &self.task_id {
            put("task_id", json!(value));
        }
        fields
    }
}

/// The field rules every writer of an event row applies.
///
/// `is_create` says the payload must state what a create needs: `kind`, `title`, `starts_at`
/// and `ends_at` are all `not null`, so a create without them is a constraint violation
/// dressed as a 500. A `PATCH` names one field and must not be refused for the ones it left
/// alone.
pub fn validate_event(payload: &Value, is_create: bool) -> Result<NewEvent, Reject> {
    let object = payload_object(payload)?;

    // The calendar's columns are refused **first**, before the unknown-key check, and their
    // presence alone is the offence — a null included, because clearing `external_id` breaks
    // the `(connection_id, external_id)` match the next incremental sync makes just as
    // surely as forging one. A198's wire code for this through a push is `read_only_entity`
    // (the client's remedy is to stop writing, never to upgrade), which is the code the REST
    // route maps to its own 400 `validation`.
    for column in CALENDAR_COLUMNS {
        if object.contains_key(*column) {
            return Err(Reject::read_only_field(format!(
                "{column} is written by the calendar sync, not by a client"
            )));
        }
    }
    // Everything the protocol writes for an event, minus the calendar's own columns: the
    // allowlist is `Entity::Event.payload_columns()`, so the two directions cannot disagree
    // about what an event payload is.
    for key in object.keys() {
        let known = Entity::Event.payload_columns().contains(&key.as_str())
            && !CALENDAR_COLUMNS.contains(&key.as_str());
        if !known {
            return Err(Reject::validation(format!(
                "{key} is not a field a client may write"
            )));
        }
    }

    let source: Option<String> = match object.get("source") {
        None => None,
        Some(Value::Null) => {
            return Err(Reject::validation(
                "source must be \"app\"; a client does not state it as null",
            ));
        }
        Some(value) => Some(
            serde_json::from_value(value.clone())
                .map_err(|_| Reject::validation("source must be a string"))?,
        ),
    };
    if let Some(source) = &source
        && source != APP_SOURCE
    {
        return Err(Reject::read_only_field(format!(
            "source is written by the calendar sync; a client may only state \"{APP_SOURCE}\""
        )));
    }

    let kind: Option<String> = field(object, "kind")?;
    if let Some(kind) = &kind
        && !KINDS.contains(&kind.as_str())
    {
        return Err(Reject::validation(format!(
            "{kind} is not an event kind; use one of {}",
            KINDS.join(", ")
        )));
    }

    let title: Option<String> = field(object, "title")?;
    let starts_at: Option<DateTime<Utc>> = field(object, "starts_at")?;
    let ends_at: Option<DateTime<Utc>> = field(object, "ends_at")?;

    if is_create {
        if kind.is_none() {
            return Err(Reject::validation("kind is required"));
        }
        if title.is_none() {
            return Err(Reject::validation("a title is required"));
        }
        if starts_at.is_none() || ends_at.is_none() {
            return Err(Reject::validation("starts_at and ends_at are required"));
        }
    }
    if let (Some(starts_at), Some(ends_at)) = (starts_at, ends_at)
        && ends_at < starts_at
    {
        return Err(Reject::validation("ends_at must not be before starts_at"));
    }

    Ok(NewEvent {
        workspace_id: nullable_field(object, "workspace_id")?,
        kind,
        title,
        starts_at,
        ends_at,
        source,
        task_id: nullable_field(object, "task_id")?,
    })
}

/// The fields a payload names, whatever envelope it arrived in: an `Event` mutation carries
/// `{"patch": …, "base": …}` (A176) and a REST body is the fields themselves.
fn payload_object(payload: &Value) -> Result<&Map<String, Value>, Reject> {
    let object = payload
        .as_object()
        .ok_or_else(|| Reject::validation("the payload must be a JSON object"))?;
    if let Some(patch) = object.get("patch") {
        for key in object.keys() {
            if key != "patch" && key != "base" {
                return Err(Reject::validation(format!(
                    "{key} is not part of an event mutation payload"
                )));
            }
        }
        return patch
            .as_object()
            .ok_or_else(|| Reject::validation("patch must be a JSON object"));
    }
    Ok(object)
}

fn field<T: serde::de::DeserializeOwned>(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<T>, Reject> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Err(Reject::validation(format!("{key} must not be null"))),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

fn nullable_field<T: serde::de::DeserializeOwned>(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Option<T>>, Reject> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) => serde_json::from_value(value.clone())
            .map(|value| Some(Some(value)))
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

#[derive(Debug, Deserialize)]
pub struct WindowQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct EventView {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub source: String,
    pub task_id: Option<Uuid>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

fn event_columns() -> &'static str {
    "id, workspace_id, kind, title, starts_at, ends_at, source, task_id, created_at, updated_at"
}

/// `GET /events?from=&to=` — the events that **overlap** `[from, to)`.
///
/// `from` is required because a window with no start is a full-table scan with no meaning;
/// `to` defaults to `from + 7 days`, which is the horizon the calendar screen shows.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<WindowQuery>,
) -> AppResult<Json<Value>> {
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

    // Overlap, not containment: an event is in the window when it starts before the window
    // ends **and** ends after the window starts. A containment filter (`starts_at between`)
    // silently drops the long event the student most needs to see.
    let sql = format!(
        "select {} from events
          where user_id = $1 and deleted_at is null
            and starts_at < $3 and ends_at > $2
          order by starts_at asc, id asc",
        event_columns()
    );
    let rows: Vec<EventView> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user.user_id)
        .bind(from)
        .bind(to)
        .fetch_all(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("listing events: {error}")))?;

    Ok(Json(json!({ "events": rows })))
}

/// `POST /events`. 201 `{event}`.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Value>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .map(|raw| {
            Uuid::parse_str(raw)
                .map_err(|_| Reject::validation("id is not a uuid").into_app_error())
        })
        .transpose()?
        .unwrap_or_else(Uuid::now_v7);

    let event = validate_event(&body, true).map_err(Reject::into_app_error)?;

    let taken: bool = sqlx::query_scalar("select exists (select 1 from events where id = $1)")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("checking an event id: {error}")))?;
    if taken {
        return Err(AppError::Conflict(
            "that event id is already in use".to_string(),
        ));
    }

    let mut row = event.fields();
    row.insert("id".to_string(), json!(id));
    row.insert("user_id".to_string(), json!(user.user_id));
    row.entry("source".to_string())
        .or_insert_with(|| json!(APP_SOURCE));

    let sql = format!(
        "insert into events
            (id, user_id, workspace_id, kind, title, starts_at, ends_at, source, task_id)
         select id, user_id, workspace_id, kind, title, starts_at, ends_at, source, task_id
           from jsonb_populate_record(null::events, $1::jsonb)
         returning {}",
        event_columns()
    );
    let created: EventView = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(Value::Object(row))
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            if crate::sync::apply::client_error(&error).is_some() {
                AppError::Validation("the workspace or task does not exist".to_string())
            } else {
                AppError::Internal(anyhow::anyhow!("creating an event: {error}"))
            }
        })?;

    Ok((StatusCode::CREATED, Json(json!({ "event": created }))))
}

/// `PATCH /events/:id`.
///
/// A row the calendar owns is **not editable at all** (A198): Phase 10's Google and ICS rows
/// are read-only in the UI, and a 404 would read as "deleted". The refusal is a 403 that
/// names the calendar, so the client can say why the event is not editable.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let event = validate_event(&body, false).map_err(Reject::into_app_error)?;

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning an event edit: {error}"))
        })?;

    // The `source` is read in the same transaction as the update, under the row lock, so a
    // Phase 10 sync landing between a check and a write cannot be overwritten.
    let stored: Option<Value> = sqlx::query_scalar(
        "select to_jsonb(t) from events t where t.id = $1 and t.user_id = $2 \
                            and t.deleted_at is null for update",
    )
    .bind(id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading an event: {error}")))?;

    let Some(stored) = stored else {
        return Err(AppError::NotFound);
    };
    if stored.get("source").and_then(Value::as_str) != Some(APP_SOURCE) {
        return Err(calendar_owned());
    }

    let mut merged = stored
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("an event row is not a JSON object")))?;
    for (key, value) in event.fields() {
        merged.insert(key, value);
    }
    // The merge must not be able to move a row into another account's hands or back out of
    // the calendar's: `id`, `user_id` and the four calendar columns come from the stored
    // row and are never in `event.fields()`.
    merged.insert("id".to_string(), json!(id));
    merged.insert("user_id".to_string(), json!(user.user_id));

    let sql = format!(
        "update events t set
            workspace_id = v.workspace_id,
            kind         = v.kind,
            title        = v.title,
            starts_at    = v.starts_at,
            ends_at      = v.ends_at,
            source       = v.source,
            task_id      = v.task_id
           from jsonb_populate_record(null::events, $2::jsonb) v
          where t.id = $1 and t.user_id = $3 and t.deleted_at is null
         returning {}",
        event_columns()
            .split(", ")
            .map(|column| format!("t.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let updated: Option<EventView> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(Value::Object(merged))
        .bind(user.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| {
            if crate::sync::apply::client_error(&error).is_some() {
                AppError::Validation("the workspace or task does not exist".to_string())
            } else {
                AppError::Internal(anyhow::anyhow!("updating an event: {error}"))
            }
        })?;
    let updated = updated.ok_or(AppError::NotFound)?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing an event edit: {error}"))
    })?;

    Ok(Json(json!({ "event": updated })))
}

/// `DELETE /events/:id`. 204, with the same read-only rule as `PATCH`.
pub async fn delete_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning an event delete: {error}"))
    })?;

    let source: Option<String> = sqlx::query_scalar(
        "select source from events
          where id = $1 and user_id = $2 and deleted_at is null for update",
    )
    .bind(id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading an event: {error}")))?;

    let Some(source) = source else {
        return Err(AppError::NotFound);
    };
    if source != APP_SOURCE {
        return Err(calendar_owned());
    }

    sqlx::query("update events set deleted_at = now() where id = $1 and user_id = $2")
        .bind(id)
        .bind(user.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting an event: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing an event delete: {error}"))
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// The refusal for a row the calendar owns (A198).
///
/// 403 rather than 404, because the event exists and is readable — the client renders it —
/// and a 404 would read as "deleted". The message names the calendar, because that is the
/// fact that explains why this one event will not edit while every other one does.
fn calendar_owned() -> AppError {
    AppError::ForbiddenWith(
        "this event comes from a connected calendar and is read-only here".to_string(),
    )
}
