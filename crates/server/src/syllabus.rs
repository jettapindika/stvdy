//! The syllabus import: the upload, the extraction draft, and the confirm that is the only
//! writer of the rows the student ends up with.
//!
//! **Nothing reaches `events`, `tasks` or `goals` until the student confirms.** The
//! extraction is a model's reading of a document, and a wrong reading written straight
//! through is the failure the review screen exists to prevent — so the extraction lands in
//! `syllabus_drafts`, the student reads it with the source line each item came from, and
//! [`confirm`] is the one place a kept item becomes a row.
//!
//! The upload writes a `sources` row with `kind = 'syllabus'` and enqueues `ingest_source`,
//! which already owns extract → chunk → embed → status. A syllabus adds a step to that
//! pipeline, not a second job kind: the roadmap's § 8 list is fixed and has no syllabus kind
//! (A250). The extraction runs in the job rather than in this route because a whole document
//! is not a route-sized call — the contrast is `create_outline`, which is one short call over
//! selected chunks and does run in its route.
//!
//! The confirm reuses [`crate::events::insert_event`] and [`crate::tasks::insert_task`], the
//! same statements `POST /events` and `POST /tasks` use, and it expands a class meeting's
//! rule with [`stvdy_core::calendar::expand`] — Phase 10's engine, the one the ICS import
//! already uses. Two recurrence engines eventually disagree about a `BYDAY` rule, and the
//! disagreement shows up as a class meeting on the wrong day.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::json;
use uuid::Uuid;

use stvdy_core::entitlements::Feature;
use stvdy_core::generation::syllabus::{
    ReviewItem, ReviewKind, SyllabusFile, expand_meeting, parse_syllabus, review_items,
};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::entitlements::{ReadOnlyRow, require_writable_row};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The action the import is recorded and priced under: the matrix's `[credits]` key.
pub const SYLLABUS_ACTION: &str = "syllabus_import";

/// The source kind the upload writes. Already legal in `sources.kind` (`0007` documents
/// `pdf | pptx | docx | url | text | web | syllabus`), so the upload writes a row and no
/// enum changes.
const SYLLABUS_KIND: &str = "syllabus";

/// How far ahead a kept meeting's rule is expanded at confirm time.
///
/// The same six months the ICS feed renders (`calendar::expand::HORIZON_DAYS`), stated here
/// rather than imported so the two can be read side by side: a class series reaching past
/// this is one the planner will not look at either.
const CONFIRM_HORIZON_DAYS: i64 = 180;

/// The longest a student's title for the upload may be.
const MAX_TITLE_CHARS: usize = 200;

/// One gigabyte, the same ceiling `sources::routes::create` applies: no plan allows it, and
/// it exists only to stop a client streaming forever before the plan check runs.
const HARD_MAX_BYTES: usize = 1024 * 1024 * 1024;

pub fn routes(body_limit_bytes: usize) -> Router<AppState> {
    Router::new()
        .route("/workspaces/{id}/syllabus", post(create))
        .route("/syllabus/{id}/draft", get(draft))
        .route("/syllabus/{id}/confirm", post(confirm))
        .layer(DefaultBodyLimit::max(body_limit_bytes))
}

/// The object key for a syllabus upload.
///
/// The same shape `sources::routes::object_key` builds, and for the same two reasons: the
/// user prefix is what stops a key being guessed into another account, and the source id
/// makes a retry overwrite rather than accumulate. Written here rather than shared because
/// A250 makes this route write its own `sources` row, and reaching into the sources route for
/// one format string would couple the two upload paths that decision keeps apart.
fn object_key(user_id: Uuid, source_id: Uuid) -> String {
    format!("users/{user_id}/sources/{source_id}.pdf")
}

/// A title from pasted text: the first non-empty line, trimmed to something a list row can
/// render. The rule `sources::routes::first_line_title` applies to a pasted note.
fn first_line_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Syllabus");
    let trimmed: String = line.chars().take(80).collect();
    if trimmed.is_empty() {
        "Syllabus".to_string()
    } else {
        trimmed
    }
}

/// `POST /workspaces/:id/syllabus` (multipart: `file` or `text`, optional `title`).
///
/// 202 `{ draft_id, source_id }`. Takes `VerifiedUser`, not `AuthUser`: ingestion is one of
/// the four things A43 keeps locked until the address is confirmed, and this route spends
/// eight credits and then a model call.
///
/// The workspace check is [`require_writable_row`], the same call
/// `POST /workspaces/:id/sources` makes: a workspace a downgrade flagged read-only cannot
/// take a new import, and a foreign workspace id is a 404 rather than a 403.
pub async fn create(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    require_writable_row(&state, &user, ReadOnlyRow::Workspace, workspace_id).await?;

    let mut file: Option<Vec<u8>> = None;
    let mut text: Option<String> = None;
    let mut title: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Validation(format!("reading the upload: {error}")))?
    {
        match field.name().unwrap_or_default() {
            "title" => {
                title = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| AppError::Validation(error.to_string()))?,
                );
            }
            "text" => {
                text = Some(
                    field
                        .text()
                        .await
                        .map_err(|error| AppError::Validation(error.to_string()))?,
                );
            }
            "file" => {
                let filename = field.file_name().unwrap_or("syllabus").to_string();
                // The extension is the check, not the client's content type: a browser sends
                // whatever the OS guessed, and a `.zip` labelled `application/pdf` would claim
                // a kind the extractor cannot read. The same rule `sources::routes` applies.
                if !filename.to_lowercase().ends_with(".pdf") {
                    return Err(AppError::Validation(
                        "a syllabus file must be a .pdf; paste the text instead".to_string(),
                    ));
                }
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|error| AppError::Validation(format!("reading the file: {error}")))?;
                if bytes.len() > HARD_MAX_BYTES {
                    return Err(AppError::Validation("the file is too large".to_string()));
                }
                file = Some(bytes.to_vec());
            }
            // `kind` is accepted and ignored: the path already says what this is, and a
            // client that echoes the `sources` upload's shape must not be refused for a field
            // that carries no decision here.
            _ => {}
        }
    }

    // The media type the ingest job dispatches on, decided before the body is consumed.
    let mime = if file.is_some() {
        "application/pdf"
    } else {
        "text/plain"
    };

    let body: Vec<u8> = match (file, text) {
        (Some(bytes), _) => bytes,
        (None, Some(text)) if !text.trim().is_empty() => {
            title.get_or_insert_with(|| first_line_title(&text));
            text.into_bytes()
        }
        _ => {
            return Err(AppError::Validation(
                "a syllabus needs either a .pdf file or the text field".to_string(),
            ));
        }
    };

    let title = title
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| AppError::Validation("the upload needs a title".to_string()))?;
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(AppError::Validation(format!(
            "the title must be at most {MAX_TITLE_CHARS} characters"
        )));
    }

    let cost = state.config.plans.credits.syllabus_import;
    let source_id = Uuid::now_v7();
    let size = body.len() as i64;

    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a syllabus import: {error}"))
    })?;

    // The count, the charge and the reserving rows commit together, because the monthly
    // limit depends on the count: `syllabus_imports_total` on free is 1, and a row written
    // after the commit is a row the next request's count cannot see.
    let user_id = user.user_id;
    let title_ref = title.as_str();
    let body_ref = body.as_slice();
    let state_ref = &state;

    let (spent, draft_id) = crate::entitlements::reserve_in(
        &mut tx,
        &state,
        &user,
        Some(Feature::SyllabusImport),
        SYLLABUS_ACTION,
        cost,
        |conn, _usage_event_id| {
            Box::pin(async move {
                // The bytes go first: a row pointing at an object that was never stored is a
                // source that fails every retry with "reading the source file", which reads
                // as a storage outage rather than as a failed upload.
                let key = object_key(user_id, source_id);
                state_ref
                    .storage
                    .put(&key, bytes::Bytes::copy_from_slice(body_ref), mime)
                    .await
                    .map_err(|error| {
                        AppError::Provider(format!("storing the syllabus: {error}"))
                    })?;

                sqlx::query(
                    "insert into sources
                       (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
                     values ($1, $2, $3, $4, $5, $6, $7, $8, 'pending')",
                )
                .bind(source_id)
                .bind(user_id)
                .bind(workspace_id)
                .bind(SYLLABUS_KIND)
                .bind(title_ref)
                .bind(&key)
                .bind(size)
                .bind(mime)
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("recording the syllabus source: {error}"))
                })?;

                // The draft is created here rather than by the job so the client has an id to
                // poll the moment the upload is accepted, and so a job that never runs leaves
                // a draft stuck in `extracting` — a state the client can see and report,
                // rather than a 404 that looks like the upload was lost.
                let draft_id: Uuid = sqlx::query_scalar(
                    "insert into syllabus_drafts (user_id, workspace_id, source_id, status)
                     values ($1, $2, $3, 'extracting')
                     returning id",
                )
                .bind(user_id)
                .bind(workspace_id)
                .bind(source_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("creating the syllabus draft: {error}"))
                })?;

                Ok(draft_id)
            })
        },
    )
    .await?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing the syllabus import: {error}"))
    })?;

    crate::jobs::enqueue(
        &state.pool,
        &state.jobs,
        "ingest_source",
        json!({ "source_id": source_id }),
        None,
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing the extraction: {error}")))?;

    tracing::info!(
        %source_id,
        %draft_id,
        %workspace_id,
        cost,
        usage_event_id = %spent.usage_event_id,
        "syllabus accepted"
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "draft_id": draft_id, "source_id": source_id })),
    ))
}

/// The draft row, as the review screen reads it.
struct DraftRow {
    id: Uuid,
    workspace_id: Uuid,
    source_id: Uuid,
    status: String,
    extraction: Option<serde_json::Value>,
    error: Option<String>,
}

async fn load_draft(state: &AppState, user_id: Uuid, draft_id: Uuid) -> AppResult<DraftRow> {
    let row: Option<(
        Uuid,
        Uuid,
        Uuid,
        String,
        Option<serde_json::Value>,
        Option<String>,
    )> = sqlx::query_as(
        "select id, workspace_id, source_id, status, extraction, error
               from syllabus_drafts
              where id = $1 and user_id = $2",
    )
    .bind(draft_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a draft: {error}")))?;

    // 404 for another account's draft, never 403: a 403 would confirm the row exists.
    row.map(
        |(id, workspace_id, source_id, status, extraction, error)| DraftRow {
            id,
            workspace_id,
            source_id,
            status,
            extraction,
            error,
        },
    )
    .ok_or(AppError::NotFound)
}

/// The timezone a user's own rows are read in. `users.timezone` is validated at signup, so
/// the fallback is unreachable rather than a policy: it exists so a stored value the tz
/// database does not know cannot turn every read into a 500.
fn zone(timezone: &str) -> Tz {
    timezone.parse().unwrap_or(chrono_tz::Asia::Jakarta)
}

/// The review items a stored extraction maps to.
///
/// Re-derived on every read rather than stored: the extraction is what the model said, and
/// the list is a function of it plus the student's zone. Storing both would be two answers to
/// one question, and the second would be the one that goes stale when the mapping changes.
fn items_of(draft: &DraftRow, timezone: &str) -> AppResult<Vec<ReviewItem>> {
    let Some(extraction) = &draft.extraction else {
        return Ok(Vec::new());
    };
    let file: SyllabusFile = serde_json::from_value(extraction.clone()).map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "the stored extraction is unreadable: {error}"
        ))
    })?;
    review_items(&file, zone(timezone)).map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "the stored extraction does not map to review items: {error}"
        ))
    })
}

/// One review item, as JSON.
///
/// The `confidence` is deliberately absent (A261): the honesty rules forbid showing a
/// confidence score, so the list arrives already ordered by it and the client shows the
/// evidence quote instead — which is what actually lets the student check the item.
fn item_json(item: &ReviewItem) -> serde_json::Value {
    json!({
        "index": item.index,
        "kind": item.kind.as_str(),
        "title": item.title,
        "evidence": item.evidence,
        "needs_check": item.needs_check,
        "when": item.when,
        "rrule": item.rrule,
        "start_time": item.start_time.map(|(h, m)| format!("{h:02}:{m:02}")),
        "end_time": item.end_time.map(|(h, m)| format!("{h:02}:{m:02}")),
        "topics": item.topics,
        "weight_percent": item.weight_percent,
    })
}

/// `GET /syllabus/:id/draft` — the extraction and the list the review screen renders.
///
/// 409 `conflict` while the extraction is still running, because that is not a failure and
/// the client should poll again. A `failed` extraction is **200** with `status: "failed"` and
/// the error in the body rather than a 4xx: this endpoint's job is to report the draft's
/// state, and a poll that turns an expected state into an HTTP error forces every client to
/// treat a normal outcome as an exception it must unwrap before it can show the message.
pub async fn draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path(draft_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let row = load_draft(&state, user.user_id, draft_id).await?;

    match row.status.as_str() {
        "extracting" => Err(AppError::Conflict(
            "the syllabus is still being read".to_string(),
        )),
        "failed" => Ok(Json(json!({
            "draft_id": row.id,
            "source_id": row.source_id,
            "status": row.status,
            "error": row.error,
            "items": [],
        }))),
        _ => {
            let items = items_of(&row, &user.timezone)?;
            Ok(Json(json!({
                "draft_id": row.id,
                "source_id": row.source_id,
                "status": row.status,
                "error": row.error,
                "course": row
                    .extraction
                    .as_ref()
                    .and_then(|value| value.get("course"))
                    .cloned(),
                "items": items.iter().map(item_json).collect::<Vec<_>>(),
            })))
        }
    }
}

/// The confirm body.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmBody {
    pub items: Vec<ConfirmItem>,
}

/// One item the student kept or dropped.
///
/// Every field is the student's edit of what the model extracted, and `None` means "keep what
/// the model said" rather than "clear it": the review screen sends only what the student
/// changed, and a `None` that cleared a title would silently blank an item they accepted.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmItem {
    /// The `ReviewItem::index` the student saw. An index that is not in the draft is a 400,
    /// never a silently ignored row: a client that sent a stale index would otherwise get a
    /// success response for a confirm that dropped an item the student meant to keep.
    pub index: usize,
    pub keep: bool,
    pub title: Option<String>,
    /// The student's date for the item, replacing what the model extracted.
    pub when: Option<String>,
    pub rrule: Option<String>,
    pub topics: Option<Vec<String>>,
    /// The workspace sources a kept exam's goal draws its topics from. Absent means every
    /// live source in the workspace, which is what a student who picked none means.
    pub source_ids: Option<Vec<Uuid>>,
}

/// `POST /syllabus/:id/confirm` — the only writer of the rows a syllabus produces.
///
/// One transaction: the draft must be `ready` and the caller's, the status becomes
/// `confirmed`, and every kept item's rows are written in the same transaction through the
/// same statements the REST routes use ([`crate::events::insert_event`],
/// [`crate::tasks::insert_task`]). A partial confirm is the failure mode this prevents: some
/// events written, the draft still `ready`, and a retry that doubles the student's calendar.
///
/// A draft that is already `confirmed` is a 409 rather than a second set of rows, which is
/// what makes the endpoint safe to retry after a timeout.
pub async fn confirm(
    State(state): State<AppState>,
    user: AuthUser,
    Path(draft_id): Path<Uuid>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<serde_json::Value>> {
    let row = load_draft(&state, user.user_id, draft_id).await?;
    if row.status != "ready" {
        return Err(AppError::Conflict(format!(
            "this syllabus is {} and cannot be confirmed",
            row.status
        )));
    }

    let items = items_of(&row, &user.timezone)?;
    let by_index: BTreeMap<usize, &ReviewItem> =
        items.iter().map(|item| (item.index, item)).collect();

    // Every index is checked before anything is written, so a body naming an index the draft
    // does not have is a 400 and not a half-written confirm. Duplicates are refused for the
    // same reason: one index naming two rows is a client bug whose outcome would be two
    // events for one class.
    let mut seen = BTreeSet::new();
    for item in &body.items {
        if !by_index.contains_key(&item.index) {
            return Err(AppError::Validation(format!(
                "item {} is not in this syllabus",
                item.index
            )));
        }
        if !seen.insert(item.index) {
            return Err(AppError::Validation(format!(
                "item {} appears twice in the confirm",
                item.index
            )));
        }
    }

    let tz = zone(&user.timezone);
    let now = state.clock.now();
    let mut written = Written::default();

    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a syllabus confirm: {error}"))
    })?;

    // The status is re-read under the row lock before anything is written, so two concurrent
    // confirms cannot both pass the check above and write the rows twice.
    let locked: Option<String> = sqlx::query_scalar(
        "select status from syllabus_drafts
          where id = $1 and user_id = $2 and status = 'ready' for update",
    )
    .bind(draft_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("locking the draft: {error}")))?;
    if locked.is_none() {
        return Err(AppError::Conflict(
            "this syllabus has already been confirmed".to_string(),
        ));
    }

    for item in body.items.iter().filter(|item| item.keep) {
        let source = by_index
            .get(&item.index)
            .copied()
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("an index vanished mid-confirm")))?;
        match source.kind {
            ReviewKind::Meeting => {
                write_meeting(&mut tx, &user, &row, source, item, tz, now, &mut written).await?;
            }
            ReviewKind::Deadline => {
                write_deadline(&mut tx, &user, &row, source, item, &mut written).await?;
            }
            ReviewKind::Exam => {
                write_exam(&mut tx, &user, &row, source, item, &mut written).await?;
            }
        }
    }

    sqlx::query(
        "update syllabus_drafts set status = 'confirmed', updated_at = now() where id = $1",
    )
    .bind(draft_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("confirming the draft: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing the syllabus confirm: {error}"))
    })?;

    tracing::info!(
        %draft_id,
        events = written.events,
        tasks = written.tasks,
        goals = written.goals,
        "syllabus confirmed"
    );

    Ok(Json(json!({
        "draft_id": draft_id,
        "status": "confirmed",
        "events": written.events,
        "tasks": written.tasks,
        "goals": written.goals,
    })))
}

/// How many rows a confirm produced, so the response and the log say the same thing.
#[derive(Debug, Default, Clone, Copy)]
struct Written {
    events: usize,
    tasks: usize,
    goals: usize,
}

/// The title a kept item is written under: the student's edit when there is one.
fn title_of(source: &ReviewItem, item: &ConfirmItem) -> String {
    item.title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&source.title)
        .to_string()
}

/// The instant a kept item is written at: the student's edit when there is one, and the
/// extraction's own date otherwise.
///
/// `None` with no fallback is a 400 naming the item rather than an invented date: an exam
/// the document never dated cannot become a goal with a `target_date` nobody stated, and the
/// review screen can already ask the student for one.
fn instant_of(
    index: usize,
    raw: Option<&str>,
    fallback: Option<DateTime<Utc>>,
) -> AppResult<DateTime<Utc>> {
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => DateTime::parse_from_rfc3339(raw)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|_| {
                AppError::Validation(format!("item {index}: when is not an ISO 8601 instant"))
            }),
        None => fallback.ok_or_else(|| {
            AppError::Validation(format!(
                "item {index}: this item has no date, so it needs one before it can be kept"
            ))
        }),
    }
}

/// One kept meeting: one `events` row per occurrence (A269).
///
/// The rule is expanded by Phase 10's engine, anchored at the day the student confirmed —
/// which is why an extraction with no date of its own is still enough to write the series.
#[allow(clippy::too_many_arguments)]
async fn write_meeting(
    tx: &mut sqlx::PgConnection,
    user: &AuthUser,
    row: &DraftRow,
    source: &ReviewItem,
    item: &ConfirmItem,
    tz: Tz,
    now: DateTime<Utc>,
    written: &mut Written,
) -> AppResult<()> {
    let rrule = item
        .rrule
        .as_deref()
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .or(source.rrule.as_deref())
        .ok_or_else(|| AppError::Validation(format!("item {}: no class rule", item.index)))?;
    let start = source.start_time.ok_or_else(|| {
        AppError::Validation(format!("item {}: the class has no start time", item.index))
    })?;
    let end = source.end_time.ok_or_else(|| {
        AppError::Validation(format!("item {}: the class has no end time", item.index))
    })?;
    let title = title_of(source, item);

    // A student-supplied `when` is the first occurrence's date; without one the series is
    // anchored at today, and the rule's own `BYDAY` picks the matching weekdays out of it.
    let anchor = match item
        .when
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        Some(raw) => DateTime::parse_from_rfc3339(raw)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|_| {
                AppError::Validation(format!(
                    "item {}: when is not an ISO 8601 instant",
                    item.index
                ))
            })?,
        None => now,
    };

    let occurrences = expand_meeting(
        Uuid::now_v7(),
        &title,
        rrule,
        start,
        end,
        tz,
        anchor,
        CONFIRM_HORIZON_DAYS,
    )
    .map_err(|error| AppError::Validation(format!("item {}: {error}", item.index)))?;

    for occurrence in occurrences {
        crate::events::insert_event(
            &mut *tx,
            json!({
                "id": Uuid::now_v7(),
                "user_id": user.user_id,
                "workspace_id": row.workspace_id,
                "kind": "class",
                "title": title,
                "starts_at": occurrence.starts_at,
                "ends_at": occurrence.ends_at,
                "source": "app",
            }),
        )
        .await?;
        written.events += 1;
    }
    Ok(())
}

/// One kept deadline: one `tasks` row (A269) — `due_at` set, `priority` 1, no estimate.
///
/// `priority` 1 is the module's "high": a date the faculty set is not the student's to rank
/// below their own todos, and the Tasks screen sorts by it.
async fn write_deadline(
    tx: &mut sqlx::PgConnection,
    user: &AuthUser,
    row: &DraftRow,
    source: &ReviewItem,
    item: &ConfirmItem,
    written: &mut Written,
) -> AppResult<()> {
    let due_at = instant_of(item.index, item.when.as_deref(), source.when)?;

    crate::tasks::insert_task(
        &mut *tx,
        json!({
            "id": Uuid::now_v7(),
            "user_id": user.user_id,
            "workspace_id": row.workspace_id,
            "title": title_of(source, item),
            "due_at": due_at,
            "priority": 1,
            "status": "todo",
            "position": 0,
        }),
    )
    .await?;
    written.tasks += 1;
    Ok(())
}

/// One kept exam: one `goals` row (A269) — `target_date` set, `source_ids` the workspace's
/// sources.
///
/// `goals.rs` does not exist yet, so this is the only writer of a goal row today; when the
/// goals routes land they become the second caller of the same statement and this moves
/// beside them.
async fn write_exam(
    tx: &mut sqlx::PgConnection,
    user: &AuthUser,
    row: &DraftRow,
    source: &ReviewItem,
    item: &ConfirmItem,
    written: &mut Written,
) -> AppResult<()> {
    let target_date = instant_of(item.index, item.when.as_deref(), source.when)?;

    let source_ids = match item.source_ids.as_ref().filter(|ids| !ids.is_empty()) {
        Some(ids) => {
            // Every id must be this workspace's, or the goal would carry topics the student
            // cannot see — and the planner would schedule material from another workspace.
            let owned: i64 = sqlx::query_scalar(
                "select count(*)::bigint from sources
                  where id = any($1) and workspace_id = $2 and user_id = $3
                    and deleted_at is null",
            )
            .bind(ids)
            .bind(row.workspace_id)
            .bind(user.user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("checking the exam's sources: {error}"))
            })?;
            if owned != ids.len() as i64 {
                return Err(AppError::Validation(format!(
                    "item {}: every source must be one of this workspace's",
                    item.index
                )));
            }
            ids.clone()
        }
        // Absent means every live source in the workspace: the student picked none, and a
        // goal with no sources has no topics for the planner to schedule.
        None => sqlx::query_scalar(
            "select id from sources
              where workspace_id = $1 and user_id = $2 and deleted_at is null
              order by created_at, id",
        )
        .bind(row.workspace_id)
        .bind(user.user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("listing the workspace's sources: {error}"))
        })?,
    };

    sqlx::query(
        "insert into goals (id, user_id, workspace_id, title, target_date, source_ids)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(user.user_id)
    .bind(row.workspace_id)
    .bind(title_of(source, item))
    .bind(target_date)
    .bind(&source_ids)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("creating the exam goal: {error}")))?;
    written.goals += 1;
    Ok(())
}

/// Parses a model reply into the extraction, for the ingest job.
///
/// The re-export is what keeps the job from naming `stvdy_core` for the one call it makes:
/// the contract is this feature's, and the module that owns the route owns the parse.
pub(crate) fn parse_extraction(
    raw: &str,
) -> Result<SyllabusFile, stvdy_core::generation::GenerationError> {
    parse_syllabus(raw)
}
