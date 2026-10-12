//! `GET/POST/PATCH/DELETE /tasks`, `POST /tasks/quick-capture` and
//! `POST /tasks/:id/breakdown`.
//!
//! Two things here are load-bearing beyond CRUD:
//!
//!  * [`validate_task`] is the **one** implementation of the task field rules. These routes
//!    call it, and so does `crate::sync::push::decide` for a pushed mutation (Task 10). A
//!    second implementation would be a second answer to "may this row exist", and the two
//!    would disagree the first time one of them was edited.
//!  * The two generative routes **write no task** (A193, A195). A capture returns a draft
//!    and a breakdown returns a proposal; the student confirms, and a confirm is an ordinary
//!    `POST /tasks` per kept item. A capture that inserted a row would pass every other
//!    assertion in this file while being exactly the failure the confirm step exists to
//!    prevent.
//!
//! The order `GET /tasks` sorts by is `stvdy_core::tasks::order_tasks`'s order, written out
//! in SQL because the server sorts in the database and that function sorts a vector. The
//! comment on the query names it, so an edit to one is visibly an edit to both.

use std::collections::BTreeMap;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use stvdy_core::entitlements::Feature;
use stvdy_core::generation::{GenerationError, capture, is_retryable};
use stvdy_core::prompts::{ChatVars, ModuleId, compose_task};
use stvdy_core::tasks as rules;
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role, Usage};
use stvdy_sync::proto::{Entity, ErrorCode};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::credits::Spent;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{credits, entitlements};

/// The action name a quick capture is recorded under.
///
/// The *price* is `task_breakdown`'s (A190 — the matrix has no `quick_capture` key and
/// `CreditAction` has no variant for one), but the *action* is its own: the margin report
/// and the usage screen read `usage_events.action`, and recording a capture as a breakdown
/// would make the two indistinguishable in the one place they are counted.
const QUICK_CAPTURE_ACTION: &str = "quick_capture";

/// The action name a break-down is recorded under, which is also its price key.
const BREAKDOWN_ACTION: &str = "task_breakdown";

/// What `{{workspace_name}}` carries for a task prompt.
///
/// A capture and a breakdown are account-level: neither belongs to a workspace, and the
/// base template's `<user_context_data>` block always names one. "not set" is the registry's
/// own wording for an absent optional (`NO_EXAM_DATE`), and an empty string is not an option
/// at all — `render_with` treats an empty `Caller` value as *missing* and refuses to compose,
/// which is how a blank workspace name would become a 500 rather than a prompt.
const NO_WORKSPACE: &str = "not set";

/// What `{{due_at}}` and `{{notes}}` carry for a task that has neither.
///
/// Both are `Caller`, not `Optional`, because the module renders them as lines the model
/// reads — `Due: {{due_at}}` — and the registry names the caller's own word as the right
/// value for an absent one. An empty string is not an absence to `render_with`: it is a
/// missing variable, which is a 500 on a request the student has already paid for.
const NONE: &str = "none";

/// The column's default, and what an absent `priority` means: `tasks.priority` is
/// `not null default 2`, and 2 is the documented "unset" (A210).
const DEFAULT_PRIORITY: i32 = 2;

/// The most rows one `GET /tasks` answers.
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 500;

/// The longest capture text the prompt will carry.
const MAX_CAPTURE_CHARS: usize = 2000;

const TEMPERATURE: f32 = 0.2;
const MAX_TOKENS: u32 = 1200;

/// What the model is told when its first reply was not usable. The same corrective turn
/// every strict-JSON generator sends, because the failure is the same one.
const CORRECTION: &str = "Your previous reply was not a valid JSON object matching the schema. Reply with only the \
     JSON object, with no prose before or after it and no markdown fences.";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tasks", get(list).post(create))
        .route(
            "/tasks/{id}",
            axum::routing::patch(update).delete(delete_one),
        )
        .route("/tasks/quick-capture", post(quick_capture))
        .route("/tasks/{id}/breakdown", post(breakdown))
}

/// Why a payload may not become a row.
///
/// `code` is the **push** wire code, because a push result has no HTTP status to carry it:
/// `MutationResult::Rejected { code, message }` is what `decide` returns. The REST routes
/// answer `400 validation` for every one of these, which is the same rule stated in the
/// vocabulary each path's client reads — A198 is the case that makes the distinction
/// load-bearing, where a calendar-owned column is `read_only_entity` through a push and
/// `400 validation` through REST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    pub code: &'static str,
    pub message: String,
}

impl Reject {
    /// A field rule the payload broke.
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Validation.as_str(),
            message: message.into(),
        }
    }

    /// A field the server owns, in the vocabulary the push path uses for one.
    pub fn read_only_field(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::ReadOnlyEntity.as_str(),
            message: message.into(),
        }
    }

    /// The REST answer. A REST client's remedy for any of these is to send a different
    /// body, never to upgrade, so all of them are the 400 the client already maps.
    pub fn into_app_error(self) -> AppError {
        AppError::Validation(self.message)
    }
}

/// A task payload after the field rules have run.
///
/// `None` means the payload did not name the field; `Some(None)` means it named it as null.
/// The two are different requests — a partial write must be able to clear a due date *and*
/// to leave it alone — and collapsing them is how a patch that meant to clear one field
/// nulls out another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTask {
    pub workspace_id: Option<Option<Uuid>>,
    pub parent_id: Option<Option<Uuid>>,
    pub title: Option<String>,
    pub notes: Option<Option<String>>,
    pub due_at: Option<Option<DateTime<Utc>>>,
    pub estimate_min: Option<Option<i32>>,
    pub priority: Option<i32>,
    pub status: Option<rules::TaskStatus>,
    pub position: Option<i32>,
    /// Named only by a restore. A REST `PATCH` does not accept it (deleting is `DELETE`),
    /// but a push patch may carry `deleted_at: null` to undo a tombstone (A178), so the
    /// field is allowed and passed through rather than dropped.
    pub deleted_at: Option<Option<DateTime<Utc>>>,
}

impl NewTask {
    /// The named fields as a JSON object, which is what the writers consume: both the
    /// insert and the update go through `jsonb_populate_record` over a row, so a field the
    /// payload did not name is simply not in the map and keeps its stored value.
    pub fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        let mut put = |key: &str, value: Value| {
            fields.insert(key.to_string(), value);
        };
        if let Some(value) = &self.workspace_id {
            put("workspace_id", json!(value));
        }
        if let Some(value) = &self.parent_id {
            put("parent_id", json!(value));
        }
        if let Some(value) = &self.title {
            put("title", json!(value));
        }
        if let Some(value) = &self.notes {
            put("notes", json!(value));
        }
        if let Some(value) = &self.due_at {
            put("due_at", json!(value));
        }
        if let Some(value) = &self.estimate_min {
            put("estimate_min", json!(value));
        }
        if let Some(value) = &self.priority {
            put("priority", json!(value));
        }
        if let Some(value) = &self.status {
            put("status", json!(value));
        }
        if let Some(value) = &self.position {
            put("position", json!(value));
        }
        if let Some(value) = &self.deleted_at {
            put("deleted_at", json!(value));
        }
        fields
    }
}

/// The field rules every writer of a task row applies.
///
/// `id` is the row's identity, which a push carries on the mutation and a REST body may
/// carry in the payload (A8): the self-parent rule cannot be checked without it, and a
/// task that is its own parent is a cycle in the one structure the Tasks screen walks.
///
/// `is_create` says the payload must state what a create needs — a title. It is `false` for
/// a `PATCH` and for a push whose payload carries a non-null `base`, because a partial
/// write legitimately names one field and must not be refused for the ones it left alone.
///
/// The bounds themselves are `stvdy_core::tasks::validate`'s, called once with the values
/// the payload states. A field the payload did not name is passed the value its rule
/// accepts: every rule there is a per-field bound, so a placeholder cannot mask a failure
/// of a field that *is* present, and a second set of bounds in this file would be a second
/// answer to how long a title may be.
pub fn validate_task(id: Uuid, payload: &Value, is_create: bool) -> Result<NewTask, Reject> {
    let object = payload_object(payload)?;
    reject_unknown(object, Entity::Task.payload_columns())?;

    let title: Option<String> = field(object, "title")?;
    if is_create && title.is_none() {
        return Err(Reject::validation("a title is required"));
    }
    let priority: Option<i32> = field(object, "priority")?;
    let estimate_min: Option<Option<i32>> = nullable_field(object, "estimate_min")?;
    let status: Option<String> = field(object, "status")?;
    let position: Option<i32> = field(object, "position")?;
    let parent_id: Option<Option<Uuid>> = nullable_field(object, "parent_id")?;

    let status = match status.as_deref() {
        None => None,
        Some(raw) => Some(
            serde_json::from_value::<rules::TaskStatus>(json!(raw))
                .map_err(|_| Reject::validation(format!("{raw} is not a task status")))?,
        ),
    };

    rules::validate(
        id,
        parent_id.flatten(),
        title.as_deref().unwrap_or("x"),
        priority.unwrap_or(DEFAULT_PRIORITY),
        estimate_min.flatten(),
    )
    .map_err(|error| Reject::validation(error.to_string()))?;

    if let Some(position) = position
        && position < 0
    {
        return Err(Reject::validation("position must not be negative"));
    }

    Ok(NewTask {
        workspace_id: nullable_field(object, "workspace_id")?,
        parent_id,
        title,
        notes: nullable_field(object, "notes")?,
        due_at: nullable_field(object, "due_at")?,
        estimate_min,
        priority,
        status,
        position,
        deleted_at: nullable_field(object, "deleted_at")?,
    })
}

/// The fields a payload names, whatever envelope it arrived in.
///
/// A `Task` mutation carries `{"patch": {…}, "base": {…}|null}` (A176) while a REST body is
/// the fields themselves, so the envelope is unwrapped here rather than in either caller:
/// one rule, two callers, and the caller that forgot to unwrap would validate the envelope.
fn payload_object(payload: &Value) -> Result<&Map<String, Value>, Reject> {
    let object = payload
        .as_object()
        .ok_or_else(|| Reject::validation("the payload must be a JSON object"))?;
    if let Some(patch) = object.get("patch") {
        // `base` is the only other key the envelope may carry; a `patch` next to anything
        // else is not this protocol's shape.
        for key in object.keys() {
            if key != "patch" && key != "base" {
                return Err(Reject::validation(format!(
                    "{key} is not part of a task mutation payload"
                )));
            }
        }
        return patch
            .as_object()
            .ok_or_else(|| Reject::validation("patch must be a JSON object"));
    }
    Ok(object)
}

/// Refuses a field the entity's writer does not accept.
///
/// `known` is the entity's own `payload_columns` — the allowlist the pull ships and the
/// push writes — so the two directions cannot disagree: a field this validator allows and
/// the writer drops cannot exist, and neither can a field the writer accepts that this
/// refuses. A hand-written list here would be a second answer to "what is a task's column",
/// and it would drift the first time a column was added to one of them.
///
/// `user_id` is deliberately absent from every allowlist: the row already belongs to the
/// caller, and a payload free to state it is a payload that looks accepted while being
/// ignored.
fn reject_unknown(object: &Map<String, Value>, known: &[&str]) -> Result<(), Reject> {
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            return Err(Reject::validation(format!(
                "{key} is not a field a client may write"
            )));
        }
    }
    Ok(())
}

/// One non-null field, or `None` when the payload did not name it.
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

/// One nullable field: `None` when unnamed, `Some(None)` when named as null.
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
pub struct ListQuery {
    pub status: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub due_before: Option<DateTime<Utc>>,
    pub due_after: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct TaskView {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub title: String,
    pub notes: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub estimate_min: Option<i32>,
    pub priority: i32,
    pub status: String,
    pub position: i32,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// The columns every read and every returning write selects.
///
/// One list rather than three literals, because the update's `from` clause adds a second
/// relation that also has an `id`: a `returning id` there is an ambiguous-column error, so
/// that one statement asks for the same list qualified by its alias.
fn task_columns(alias: Option<&str>) -> String {
    const NAMES: &[&str] = &[
        "id",
        "workspace_id",
        "parent_id",
        "title",
        "notes",
        "due_at",
        "estimate_min",
        "priority",
        "status",
        "position",
        "created_at",
        "updated_at",
    ];
    match alias {
        Some(alias) => NAMES
            .iter()
            .map(|name| format!("{alias}.{name}"))
            .collect::<Vec<_>>()
            .join(", "),
        None => NAMES.join(", "),
    }
}

/// `GET /tasks`
///
/// Tombstones are excluded and the caller's `user_id` is in the statement, so another
/// account's task is not merely filtered out of the answer — it is never read.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    // The order is `stvdy_core::tasks::order_tasks`'s, written out: due first with the
    // undated last, then priority, then position, then `created_at`, then the id. The id
    // breaks every tie, because a batch insert shares `created_at` to the microsecond and
    // an order that stopped short of the id would come back shuffled (A196).
    let sql = format!(
        "select {} from tasks
          where user_id = $1 and deleted_at is null
            and ($2::text is null or status = $2)
            and ($3::uuid is null or workspace_id = $3)
            and ($4::uuid is null or parent_id = $4)
            and ($5::timestamptz is null or due_at < $5)
            and ($6::timestamptz is null or due_at >= $6)
          order by due_at asc nulls last, priority asc, position asc, created_at asc, id asc
          limit $7",
        task_columns(None)
    );
    let rows: Vec<TaskView> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user.user_id)
        .bind(query.status.as_deref())
        .bind(query.workspace_id)
        .bind(query.parent_id)
        .bind(query.due_before)
        .bind(query.due_after)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("listing tasks: {error}")))?;

    Ok(Json(json!({ "tasks": rows })))
}

/// `POST /tasks`. 201 `{task}`, 409 when the id is already taken.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<Value>,
) -> AppResult<(StatusCode, Json<Value>)> {
    // A8: the client mints the id offline, so a body may carry one; the server generates
    // one only when it does not.
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .map(|raw| {
            Uuid::parse_str(raw)
                .map_err(|_| Reject::validation("id is not a uuid").into_app_error())
        })
        .transpose()?
        .unwrap_or_else(Uuid::now_v7);

    let task = validate_task(id, &body, true).map_err(Reject::into_app_error)?;

    // The PK is global, so an id already in use would surface as a 500 from the insert.
    // It is checked without the `user_id` predicate on purpose: the answer is the same
    // whether the row is the caller's or another account's — 409, "this id is taken" —
    // and a client that generated a v7 uuid has not learned anything it did not already
    // have. An id is not a secret; the row is.
    let taken: bool = sqlx::query_scalar("select exists (select 1 from tasks where id = $1)")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("checking a task id: {error}")))?;
    if taken {
        return Err(AppError::Conflict(
            "that task id is already in use".to_string(),
        ));
    }

    let mut row = task.fields();
    row.insert("id".to_string(), json!(id));
    row.insert("user_id".to_string(), json!(user.user_id));
    row.insert(
        "title".to_string(),
        json!(task.title.clone().unwrap_or_default()),
    );
    row.entry("priority".to_string())
        .or_insert_with(|| json!(DEFAULT_PRIORITY));
    row.entry("status".to_string())
        .or_insert_with(|| json!("todo"));
    row.entry("position".to_string())
        .or_insert_with(|| json!(0));

    let created = insert_task(&state, &row).await?;
    Ok((StatusCode::CREATED, Json(json!({ "task": created }))))
}

/// `PATCH /tasks/:id`. 200 `{task}`, 404 for another account's row or a tombstone.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    let task = validate_task(id, &body, false).map_err(Reject::into_app_error)?;

    // One transaction, and the row is locked while it is read: the merge below is a
    // read-then-write, and two devices patching the same task would otherwise each write a
    // row built from a state the other had already replaced.
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a task edit: {error}"))
        })?;

    let stored: Option<Value> = sqlx::query_scalar(
        "select to_jsonb(t) from tasks t where t.id = $1 and t.user_id = $2 \
                            and t.deleted_at is null for update",
    )
    .bind(id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a task: {error}")))?;

    let Some(stored) = stored else {
        return Err(AppError::NotFound);
    };

    let mut merged = stored
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("a task row is not a JSON object")))?;
    // `deleted_at` is not a REST-writable field: deleting is `DELETE`, and restoring is a
    // client's sync concern. Dropping it here is what makes that true even when the body
    // names it.
    for (key, value) in task.fields() {
        if key == "deleted_at" {
            continue;
        }
        merged.insert(key, value);
    }

    let updated = update_task(&mut tx, id, user.user_id, &Value::Object(merged)).await?;
    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a task edit: {error}")))?;

    Ok(Json(json!({ "task": updated })))
}

/// `DELETE /tasks/:id`. 204, and the tombstone is the whole subtree (A197).
///
/// Postgres's `on delete cascade` would fire the children's DELETE triggers on a real
/// delete, but a tombstone is an UPDATE, so the children have to be reached explicitly. The
/// recursive walk is bounded by `deleted_at is null` on both arms, which is also what makes
/// a second delete of the same parent a no-op rather than a re-tombstone.
pub async fn delete_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a task delete: {error}"))
        })?;

    let affected = sqlx::query(
        "with recursive subtree as (
             select id from tasks
              where id = $1 and user_id = $2 and deleted_at is null
             union all
             select child.id from tasks child
               join subtree parent on child.parent_id = parent.id
              where child.user_id = $2 and child.deleted_at is null
         )
         update tasks set deleted_at = now()
          where id in (select id from subtree) and user_id = $2",
    )
    .bind(id)
    .bind(user.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting a task: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a task delete: {error}"))
    })?;

    if affected.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct QuickCaptureBody {
    pub text: String,
}

/// `POST /tasks/quick-capture`: one line in, a draft out, and **no task** (A193).
///
/// The order is `require` first and the reservation second, so a plan that does not grant
/// the feature gets the refusal it can act on rather than a charge it cannot.
pub async fn quick_capture(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<QuickCaptureBody>,
) -> AppResult<Json<Value>> {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::Validation("text is required".to_string()));
    }
    if text.chars().count() > MAX_CAPTURE_CHARS {
        return Err(AppError::Validation(format!(
            "the text is longer than {MAX_CAPTURE_CHARS} characters"
        )));
    }

    // The matrix's own answer, not a literal: free has `quick_capture = false`, so this is
    // 403 `plan_required` with the plan that grants it.
    entitlements::require(&state, &user, Feature::QuickCapture).await?;

    let cost = state.config.plans.credits.task_breakdown;
    let settings = crate::settings::load_or_create(&state.pool, user.user_id).await?;

    // `now` and `timezone` are declared `Caller` by the module and filled from `ChatVars`
    // by `compose_task`, so they are not passed here: a second copy of the clock would be a
    // second answer to what time it is.
    let mut vars: BTreeMap<&'static str, String> = BTreeMap::new();
    vars.insert("capture_text", text.clone());
    // A192: the caller's language, never the source material's — a capture has no source
    // material, and the optional default would silently answer an Indonesian line in
    // English.
    vars.insert("output_language", settings.language.clone());

    let timezone = user.timezone.parse().unwrap_or(chrono_tz::UTC);
    let (spent, draft, prompt_version) = charge_and_generate(
        &state,
        &user,
        QUICK_CAPTURE_ACTION,
        cost,
        ModuleId::QuickCaptureV1,
        vars,
        |raw| capture::parse_draft(raw, timezone),
    )
    .await?;

    Ok(Json(json!({
        "draft": draft,
        "credits_spent": cost,
        "credits_remaining": spent.remaining,
        "prompt_version": prompt_version,
    })))
}

/// `POST /tasks/:id/breakdown`: a proposal out, and **no subtasks** (A195).
///
/// Accepting the proposal is N ordinary `POST /tasks` calls with `parent_id` and `position`
/// — the route below never writes one, which is what makes "dismiss" free and what keeps a
/// proposal from being a plan the student did not agree to.
pub async fn breakdown(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    entitlements::require(&state, &user, Feature::TaskBreakdown).await?;

    let task = owned_task(&state, user.user_id, id).await?;
    let cost = state.config.plans.credits.task_breakdown;
    let settings = crate::settings::load_or_create(&state.pool, user.user_id).await?;

    let mut vars: BTreeMap<&'static str, String> = BTreeMap::new();
    vars.insert("task_title", task.title.clone());
    vars.insert(
        "due_at",
        task.due_at
            .map(|at| at.to_rfc3339())
            .unwrap_or_else(|| NONE.to_string()),
    );
    vars.insert(
        "notes",
        task.notes.clone().unwrap_or_else(|| NONE.to_string()),
    );
    vars.insert("output_language", settings.language.clone());

    let parent_title = task.title.clone();
    let (spent, proposal, prompt_version) = charge_and_generate(
        &state,
        &user,
        BREAKDOWN_ACTION,
        cost,
        ModuleId::TaskBreakdownV1,
        vars,
        |raw| stvdy_core::generation::breakdown::parse_proposal(raw, &parent_title),
    )
    .await?;

    Ok(Json(json!({
        "proposal": proposal,
        "credits_spent": cost,
        "credits_remaining": spent.remaining,
        "prompt_version": prompt_version,
    })))
}

/// The reservation, the commit, the model call, and the settlement both generative routes
/// share.
///
/// The provider call happens **after** the reservation's transaction commits, so it never
/// holds the per-user advisory lock: a model call takes seconds and the lock serialises
/// every write the account makes. The refund on a failed call is the same compensating
/// ledger row the generators write, so a provider failure is not a charge for nothing.
#[allow(clippy::type_complexity)]
async fn charge_and_generate<T>(
    state: &AppState,
    user: &AuthUser,
    action: &str,
    cost: i64,
    module: ModuleId,
    module_vars: BTreeMap<&'static str, String>,
    parse: impl Fn(&str) -> Result<T, GenerationError>,
) -> AppResult<(Spent, T, String)> {
    // The charge and nothing else: there is no reserving row here, because the draft *is*
    // the whole response (A193). `None` for the feature because the matrix prices these
    // actions without counting them — the ledger is the gate, and `require` above already
    // applied the matrix's boolean.
    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a {action} charge: {error}"))
    })?;
    let (spent, ()) = entitlements::reserve_in(
        &mut tx,
        state,
        user,
        None,
        action,
        cost,
        |_conn, _usage_event_id| Box::pin(async { Ok::<(), AppError>(()) }),
    )
    .await?;
    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a {action} charge: {error}"))
    })?;

    let base = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        user.effective_plan(),
        false,
        false,
        true,
        NO_WORKSPACE,
    );

    let generation = generate(state, &base, module, &module_vars, parse).await;

    // The prompt version is attached whether or not the reply was usable: the roadmap's
    // rule is that every model call is attributable, and a call that returned prose was
    // still a call. A failure to record it is a reporting gap, not a reason to fail a
    // request whose answer the student is about to see.
    if let Some(prompt_version) = &generation.prompt_version
        && let Err(error) = credits::record_usage(
            &state.pool,
            Some(spent.usage_event_id),
            Some(generation.usage.in_tokens),
            Some(generation.usage.out_tokens),
            generation.usage.provider_cost_usd,
            prompt_version,
        )
        .await
    {
        tracing::error!(%error, "recording a {action}'s provider usage failed");
    }

    match generation.outcome {
        Ok(value) => Ok((spent, value, generation.prompt_version.unwrap_or_default())),
        Err(error) => {
            if let Err(refund_error) = credits::refund(
                &state.pool,
                user.user_id,
                cost,
                spent.usage_event_id,
                "refund",
            )
            .await
            {
                tracing::error!(%refund_error, "refunding a failed {action}");
            }
            Err(error)
        }
    }
}

/// One generative call: what it produced, what it cost, and which prompt produced it.
///
/// `prompt_version` is `None` only when the prompt could not be composed at all — there is
/// then no version to record, and no model call happened.
struct Generation<T> {
    outcome: Result<T, AppError>,
    usage: Usage,
    prompt_version: Option<String>,
}

/// Composes the prompt, calls the model, and parses the reply, retrying **once** on a parse
/// failure.
///
/// The retry rule is `generation::is_retryable`'s — the same rule the deck, quiz and report
/// generators apply — so a drifted prompt fails the same way on every module. An empty or
/// unreachable provider is not retried: it answered the question, and the answer was
/// "nothing".
async fn generate<T>(
    state: &AppState,
    base: &ChatVars,
    module: ModuleId,
    module_vars: &BTreeMap<&'static str, String>,
    parse: impl Fn(&str) -> Result<T, GenerationError>,
) -> Generation<T> {
    // Neither module has sources, so the chunk list is empty and `{{sources}}` renders as
    // nothing: a capture is the student's own line and a breakdown is their own task.
    let composed = match compose_task(module, base, module_vars, &[]) {
        Ok(composed) => composed,
        Err(error) => {
            return Generation {
                outcome: Err(AppError::Internal(anyhow::anyhow!(
                    "composing the prompt: {error}"
                ))),
                usage: Usage::default(),
                prompt_version: None,
            };
        }
    };

    let Some(provider) = state.llm.as_ref() else {
        return Generation {
            outcome: Err(AppError::Provider(
                "no model provider is configured".to_string(),
            )),
            usage: Usage::default(),
            prompt_version: Some(composed.prompt_version),
        };
    };

    let request = ChatRequest {
        model: String::new(),
        messages: vec![ChatMsg {
            role: Role::System,
            content: composed.text.clone(),
        }],
        tools: Vec::new(),
        temperature: TEMPERATURE,
        max_tokens: MAX_TOKENS,
        json_mode: true,
    };

    let first = match provider.complete(request.clone()).await {
        Ok(first) => first,
        Err(error) => {
            return Generation {
                outcome: Err(AppError::Provider(format!("calling the model: {error}"))),
                usage: Usage::default(),
                prompt_version: Some(composed.prompt_version),
            };
        }
    };
    // The first call is billed whether or not its reply is usable, so its numbers are kept
    // for every path below.
    let mut usage = first.usage.clone();

    let outcome = match parse(&first.text) {
        Ok(value) => Ok(value),
        Err(error) if is_retryable(&error) => {
            let mut retry = request;
            retry.messages.push(ChatMsg {
                role: Role::Assistant,
                content: first.text.clone(),
            });
            retry.messages.push(ChatMsg {
                role: Role::User,
                content: CORRECTION.to_string(),
            });
            match provider.complete(retry).await {
                Ok(second) => {
                    usage.in_tokens += second.usage.in_tokens;
                    usage.out_tokens += second.usage.out_tokens;
                    usage.provider_cost_usd =
                        sum_costs(usage.provider_cost_usd, second.usage.provider_cost_usd);
                    parse(&second.text).map_err(|error| {
                        AppError::Provider(format!(
                            "the model did not return usable JSON after a retry: {error}"
                        ))
                    })
                }
                Err(error) => Err(AppError::Provider(format!(
                    "retrying the model call: {error}"
                ))),
            }
        }
        Err(error) => Err(AppError::Provider(format!(
            "the model did not return usable JSON: {error}"
        ))),
    };

    Generation {
        outcome,
        usage,
        prompt_version: Some(composed.prompt_version),
    }
}

/// Two provider reports added together, with "unknown" staying unknown.
///
/// `None` means the provider did not report a price, and adding an unknown to a known one
/// must not turn it into the known one: a retry whose first call had a cost we cannot read
/// has an unknown total, and recording the second call's price would understate the margin
/// report.
fn sum_costs(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first + second),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

/// Inserts a task from a full field map, returning the stored row.
async fn insert_task(state: &AppState, row: &Map<String, Value>) -> AppResult<TaskView> {
    let sql = format!(
        "insert into tasks
            (id, user_id, workspace_id, parent_id, title, notes, due_at, estimate_min,
             priority, status, position)
         select id, user_id, workspace_id, parent_id, title, notes, due_at, estimate_min,
                priority, status, position
           from jsonb_populate_record(null::tasks, $1::jsonb)
         returning {}",
        task_columns(None)
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(Value::Object(row.clone()))
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            // A dangling `parent_id` or `workspace_id` is the body's fault, not a server
            // fault, so it must not surface as a 500.
            if crate::sync::apply::client_error(&error).is_some() {
                AppError::Validation("the workspace or parent does not exist".to_string())
            } else {
                AppError::Internal(anyhow::anyhow!("creating a task: {error}"))
            }
        })
}

/// Writes the merged row back and returns it.
async fn update_task(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    user_id: Uuid,
    merged: &Value,
) -> AppResult<TaskView> {
    let sql = format!(
        "update tasks t set
            workspace_id = v.workspace_id,
            parent_id    = v.parent_id,
            title        = v.title,
            notes        = v.notes,
            due_at       = v.due_at,
            estimate_min = v.estimate_min,
            priority     = v.priority,
            status       = v.status,
            position     = v.position
           from jsonb_populate_record(null::tasks, $2::jsonb) v
          where t.id = $1 and t.user_id = $3 and t.deleted_at is null
         returning {}",
        task_columns(Some("t"))
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(merged.clone())
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| {
            if crate::sync::apply::client_error(&error).is_some() {
                AppError::Validation("the workspace or parent does not exist".to_string())
            } else {
                AppError::Internal(anyhow::anyhow!("updating a task: {error}"))
            }
        })?
        .ok_or(AppError::NotFound)
}

/// The task, or 404 when it is not this account's or is a tombstone.
pub async fn owned_task(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<TaskView> {
    let sql = format!(
        "select {} from tasks
          where id = $1 and user_id = $2 and deleted_at is null",
        task_columns(None)
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a task: {error}")))?
        .ok_or(AppError::NotFound)
}
