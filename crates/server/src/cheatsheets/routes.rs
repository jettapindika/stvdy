//! `POST /workspaces/:id/cheatsheets`, `GET /cheatsheets/:id`,
//! `GET /cheatsheets/:id/html`, `GET /cheatsheets/:id/pdf`,
//! `POST /cheatsheets/:id/re-render`.
//!
//! `create_cheatsheet` follows the quiz path exactly: verified email, owned workspace, owned
//! and ready sources, then **one transaction** holding the per-user advisory lock for the
//! charge and the row that reserves the document. The row *is* the reservation, which is what
//! Phase 5 fixed: a charge written before its row can be orphaned by a crash, and a document
//! whose work is in flight has no artifacts to count, so a limit checked outside the
//! transaction reads zero.
//!
//! There is no monthly count here, and that is deliberate rather than an omission: the plan
//! matrix has no cheatsheet allowance (`reports_per_month` is the report's, and counting a
//! cheatsheet against it would invent a policy the matrix does not state). A cheatsheet is
//! bounded by credits, which is the price the matrix *does* state. See A119.
//!
//! The two document routes answer bytes, not JSON: the free plan prints the HTML the student
//! already has, and a download is what `apiRaw` in the client expects. The plan gate is the
//! same one the report export uses.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
// `Row` is required for `row.get::<T, _>("column")`.
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::config::ExportFormat;
use stvdy_core::entitlements::Feature;

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::jobs::handlers::cheatsheet as cheatsheet_job;
use crate::state::AppState;
use crate::{jobs, workspaces};

/// The action name in `usage_events` and the ledger.
///
/// The cheatsheet is priced as one `report_section`: the matrix has no `cheatsheet` key, and
/// the roadmap says the pair a cheatsheet costs is the report's outline-plus-section rather
/// than a price invented here. It is a single generation, so it is the section's price and
/// not the outline's.
pub(crate) const GENERATE_ACTION: &str = "generate_cheatsheet";

/// The depths `prompts/cheatsheet.v1.md` documents. Stored as text, so this is the only
/// place the set is enforced.
const DEPTHS: [&str; 3] = ["brief", "normal", "thorough"];

/// The most sources one cheatsheet may draw on. Bounded for the same reason a quiz's list
/// is: the prompt has a chunk budget.
const MAX_SOURCES: usize = 20;

/// The longest scope, which is also the document's title.
const MAX_SCOPE_CHARS: usize = 200;

/// How far into the future an exam date may be set.
///
/// Not a business rule so much as a typo guard: a year mistyped as 2062 puts a document in
/// the student's list under a date that will never arrive, and the prompt then tells the
/// model to write for it.
const MAX_EXAM_DATE_YEARS: i32 = 10;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{id}/cheatsheets",
            post(create_cheatsheet).get(list_cheatsheets),
        )
        .route(
            "/cheatsheets/{id}",
            get(get_cheatsheet)
                .delete(delete_cheatsheet)
                .patch(rename_cheatsheet),
        )
        // The document itself, not a JSON envelope: the free plan prints the HTML it already
        // has, which only works if this route serves something a browser can open, and the
        // preview frame consumes it as `srcdoc`.
        .route("/cheatsheets/{id}/html", get(cheatsheet_html))
        .route("/cheatsheets/{id}/pdf", get(cheatsheet_pdf))
        .route("/cheatsheets/{id}/re-render", post(re_render))
}

#[derive(Debug, Deserialize)]
pub struct CreateCheatsheetBody {
    /// The exam or topic the sheet is for, e.g. "UTS Jaringan Komputer". The module's
    /// `{{scope}}`.
    pub scope: String,
    /// `YYYY-MM-DD`, or absent when the student has no date for it.
    #[serde(default)]
    pub exam_date: Option<NaiveDate>,
    pub depth: String,
    pub source_ids: Vec<Uuid>,
}

#[derive(Debug, serde::Serialize)]
struct CheatsheetJson {
    id: Uuid,
    workspace_id: Uuid,
    title: String,
    scope: String,
    exam_date: Option<NaiveDate>,
    depth: String,
    source_ids: Vec<Uuid>,
    status: String,
    error: Option<String>,
    /// Which `templates/cheatsheet.*.html` rendered it, so an old one is visible.
    template_version: Option<String>,
    /// The pages the renderer reported for the print. Null until the PDF exists — and a PDF
    /// is plan-gated, so on the free plan it stays null rather than being estimated.
    page_count: Option<i32>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// `POST /workspaces/:id/cheatsheets`. 201 with the cheatsheet in status `generating`.
pub async fn create_cheatsheet(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateCheatsheetBody>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let scope = body.scope.trim().to_string();
    if scope.is_empty() || scope.chars().count() > MAX_SCOPE_CHARS {
        return Err(AppError::Validation(format!(
            "the scope must be between 1 and {MAX_SCOPE_CHARS} characters"
        )));
    }
    if !DEPTHS.contains(&body.depth.as_str()) {
        return Err(AppError::Validation(format!(
            "depth must be one of {}",
            DEPTHS.join(", ")
        )));
    }
    if body.source_ids.is_empty() || body.source_ids.len() > MAX_SOURCES {
        return Err(AppError::Validation(format!(
            "a cheatsheet is generated from between 1 and {MAX_SOURCES} sources"
        )));
    }
    if let Some(exam_date) = body.exam_date {
        let limit = state.clock.now().date_naive()
            + chrono::Duration::days(i64::from(MAX_EXAM_DATE_YEARS) * 366);
        if exam_date > limit {
            return Err(AppError::Validation(
                "the exam date is too far in the future; check the year".to_string(),
            ));
        }
    }

    // Deduplicated before it is validated or stored, for the same reason the quiz path
    // deduplicates: `count(*)` counts rows and `len()` counts the request, so a client that
    // sent one source twice would be refused with "every source must be one of this
    // workspace's", which is false. First-seen order is kept.
    let mut source_ids: Vec<Uuid> = Vec::with_capacity(body.source_ids.len());
    for id in &body.source_ids {
        if !source_ids.contains(id) {
            source_ids.push(*id);
        }
    }

    workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    // Every source must be this user's, in this workspace, and finished ingesting:
    // a cheatsheet built from a source whose chunks do not exist yet would be an empty
    // document with no error anywhere.
    let ready: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources
          where id = any($1) and workspace_id = $2 and user_id = $3
            and status = 'ready' and deleted_at is null",
    )
    .bind(&source_ids)
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "checking the cheatsheet's sources: {error}"
        ))
    })?;
    if ready != source_ids.len() as i64 {
        return Err(AppError::Validation(
            "every source must be one of this workspace's, and finished indexing".to_string(),
        ));
    }

    let cost = state.config.plans.credits.report_section;

    // The charge and the reserving row commit together. `reserve_in` is that sequence —
    // the per-user advisory lock, the matrix decision against a snapshot read under the
    // lock, the charge, then this closure's row — defined once in `crate::entitlements`
    // and shared with the deck, quiz and report reservations.
    //
    // The feature is `None`: the matrix has no monthly cheatsheet allowance, so there is
    // nothing to count. The cost is the `report_section` price because a cheatsheet is one
    // generation call of the same size, and a separate price for it would be a second
    // number to keep in step with the first.
    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a cheatsheet: {error}"))
        })?;

    // Bound before the closure so `async move` captures references rather than the values
    // the handler still needs below (the document's JSON body, the job payload).
    let user_id = user.user_id;
    let scope_ref = scope.as_str();
    let depth_ref = body.depth.as_str();
    let source_ids_ref = source_ids.as_slice();
    let exam_date = body.exam_date;

    let (_, (cheatsheet_id, created_at, updated_at)) = crate::entitlements::reserve_in(
        &mut tx,
        &state,
        &user,
        None,
        GENERATE_ACTION,
        cost,
        |conn, usage_event_id| {
            Box::pin(async move {
                // The row *is* the reservation, so it is written in the same transaction as
                // the charge: a failure here rolls the charge back with it, so there is no
                // charge without a document and nothing to compensate for by hand.
                let row: (Uuid, Option<DateTime<Utc>>, DateTime<Utc>) = sqlx::query_as(
                    "insert into cheatsheets
                        (user_id, workspace_id, title, scope, exam_date, depth, source_ids,
                         status, credits_spent, usage_event_id)
                     values ($1, $2, $3, $4, $5, $6, $7, 'generating', $8, $9)
                     returning id, created_at, updated_at",
                )
                .bind(user_id)
                .bind(workspace_id)
                .bind(scope_ref)
                .bind(scope_ref)
                .bind(exam_date)
                .bind(depth_ref)
                .bind(source_ids_ref)
                .bind(cost as i32)
                .bind(usage_event_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("creating the cheatsheet: {error}"))
                })?;
                Ok(row)
            })
        },
    )
    .await?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing the cheatsheet: {error}"))
    })?;

    // Enqueued after the commit, so a failure here leaves a document the user can see failing
    // rather than a charge with no trace.
    if let Err(error) = jobs::enqueue(
        &state.pool,
        &state.jobs,
        GENERATE_ACTION,
        serde_json::json!({ "cheatsheet_id": cheatsheet_id }),
        None,
    )
    .await
    {
        cheatsheet_job::fail_and_refund(
            &state,
            cheatsheet_id,
            user.user_id,
            cost as i32,
            "the generation job could not be queued",
        )
        .await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "enqueueing the generation job: {error}"
        )));
    }

    let cheatsheet = CheatsheetJson {
        id: cheatsheet_id,
        workspace_id,
        title: scope.clone(),
        scope,
        exam_date: body.exam_date,
        depth: body.depth,
        source_ids,
        status: "generating".to_string(),
        error: None,
        template_version: None,
        page_count: None,
        created_at: created_at.unwrap_or(updated_at),
        updated_at,
    };

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "cheatsheet": cheatsheet })),
    ))
}

/// `GET /workspaces/:id/cheatsheets`, newest first, tombstones excluded.
///
/// Added for the same reason the quiz list was: without it a cheatsheet is unreachable after
/// a page reload, and the Studio panel cannot poll the ones still generating.
pub async fn list_cheatsheets(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    let rows = sqlx::query(
        "select id, workspace_id, title, scope, exam_date, depth, source_ids, status, error,
                template_version, page_count, created_at, updated_at
           from cheatsheets
          where user_id = $1 and workspace_id = $2 and deleted_at is null
          order by sort_order asc, created_at desc, id",
    )
    .bind(user.user_id)
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing cheatsheets: {error}")))?;

    let cheatsheets: Vec<CheatsheetJson> = rows.iter().map(cheatsheet_from_row).collect();
    Ok(Json(serde_json::json!({ "cheatsheets": cheatsheets })))
}

/// `GET /cheatsheets/:id`.
pub async fn get_cheatsheet(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(cheatsheet_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let cheatsheet = owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;
    Ok(Json(serde_json::json!({ "cheatsheet": cheatsheet })))
}

/// `GET /cheatsheets/:id/html`: the rendered document itself.
///
/// `text/html`, not a JSON envelope, and the same bytes that were stored: the preview frame
/// renders them in a sandboxed `srcdoc`, and the free plan's remedy for a withheld PDF is to
/// print this document from a browser. A JSON envelope would make both of those impossible.
///
/// Not plan-gated. The plan gates the *PDF*: the HTML is the document the student's credits
/// bought, and withholding it would mean charging for something and delivering nothing.
pub async fn cheatsheet_html(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(cheatsheet_id): Path<Uuid>,
) -> AppResult<Response> {
    owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;
    let key = html_key_of(&state, cheatsheet_id).await?;

    let bytes = state
        .storage
        .get(&key)
        .await
        .map_err(|error| AppError::Provider(format!("reading the document: {error}")))?;

    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            // The document is generated from the student's own material and is served to
            // their own session, so it is private rather than public. `no-store` because it
            // is re-rendered in place: a cached copy would outlive a re-render.
            (header::CACHE_CONTROL, "private, no-store".to_string()),
            // The frame that renders this must be sandboxed, and this is the header that
            // says so for any other consumer: the document contains text a model wrote.
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:"
                    .to_string(),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `GET /cheatsheets/:id/pdf`: the print, plan-gated on `export_formats`.
///
/// The refusal is `403 plan_required` with `min_plan`, before anything is read: a client told
/// about a purchase it must not offer is the review risk the policy exists for.
///
/// The PDF is streamed rather than presigned. A presigned URL is right for a source's
/// original, which a client may want to hand to an external viewer; here the client asks for
/// bytes it will save, and the local backend's signed URL points at `/objects/:id`, which
/// resolves a *source* — a cheatsheet key through it would 404. Streaming is also what makes
/// the plan check unbypassable.
pub async fn cheatsheet_pdf(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(cheatsheet_id): Path<Uuid>,
) -> AppResult<Response> {
    crate::entitlements::require(&state, &user, Feature::ExportFormat(ExportFormat::Pdf)).await?;

    let cheatsheet = owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;
    let key: Option<String> = sqlx::query_scalar(
        "select pdf_key from cheatsheets where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(cheatsheet_id)
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the PDF key: {error}")))?
    .flatten();

    let Some(key) = key else {
        // The document exists and its print does not, which is a state the pipeline produces
        // on purpose (a failed print does not fail the generation). 409 rather than 404: the
        // cheatsheet is there, and the remedy is a re-render.
        return Err(AppError::Conflict(
            "this cheatsheet has no PDF yet; re-render it to produce one".to_string(),
        ));
    };

    let bytes = state
        .storage
        .get(&key)
        .await
        .map_err(|error| AppError::Provider(format!("reading the PDF: {error}")))?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"{}.pdf\"",
                    crate::reports::routes::file_stem(&cheatsheet.title)
                ),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// `POST /cheatsheets/:id/re-render`: re-renders from the stored blocks, with no model call.
///
/// This is the endpoint the whole content-versus-layout split exists for. The blocks are the
/// document; the HTML is a pure function of them plus a template version; so a template fix
/// reaches a cheatsheet the student may already have printed, for free, and without producing
/// *different content* — which re-generating through the model would, at a price.
///
/// It costs nothing and therefore charges nothing: no `spend`, no ledger row, no usage event.
/// A re-render that charged would be a second charge for the same document.
pub async fn re_render(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(cheatsheet_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    // Ownership first, so another account's id is a 404 before anything is read from storage.
    owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;

    let cheatsheet = cheatsheet_job::load_cheatsheet(&state, cheatsheet_id)
        .await
        .map_err(|failure| AppError::Internal(anyhow::anyhow!("{}", failure.message)))?
        .ok_or(AppError::NotFound)?;

    let blocks_key = cheatsheet_job::blocks_key_of(&state, cheatsheet_id)
        .await
        .map_err(|failure| AppError::Conflict(failure.message))?;

    let raw = state
        .storage
        .get(&blocks_key)
        .await
        .map_err(|error| AppError::Provider(format!("reading the stored blocks: {error}")))?;
    let raw = std::str::from_utf8(&raw).map_err(|error| {
        AppError::Internal(anyhow::anyhow!("the blocks are not UTF-8: {error}"))
    })?;

    let (blocks, labels) = crate::cheatsheets::generate::from_stored(raw).map_err(|error| {
        AppError::Conflict(format!("the stored document is unreadable: {error}"))
    })?;

    let (html_key, _rendered) =
        crate::cheatsheets::generate::render_and_store(&state, &cheatsheet, &blocks, &labels)
            .await
            .map_err(|failure| AppError::Provider(failure.message))?;

    sqlx::query(
        "update cheatsheets
            set html_key = $2, template_version = $3, error = null, updated_at = now()
          where id = $1 and user_id = $4",
    )
    .bind(cheatsheet_id)
    .bind(&html_key)
    .bind(crate::cheatsheets::generate::template_version())
    .bind(user.user_id)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("saving the re-render: {error}")))?;

    // The print is re-queued when the renderer is available, so a document whose PDF failed
    // recovers from the same action that fixed its layout. It is enqueued rather than
    // awaited: the student asked for the document, and the print is the slow part.
    if state.renderer.is_some()
        && let Err(error) = jobs::enqueue(
            &state.pool,
            &state.jobs,
            "render_cheatsheet_pdf",
            serde_json::json!({ "cheatsheet_id": cheatsheet_id }),
            None,
        )
        .await
    {
        tracing::error!(%error, %cheatsheet_id, "could not enqueue the print job");
    }

    let cheatsheet = owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;
    Ok(Json(serde_json::json!({ "cheatsheet": cheatsheet })))
}

/// `DELETE /cheatsheets/:id`: tombstone the cheatsheet. Credits for a `generating`
/// cheatsheet are zeroed so `fail_and_refund` cannot refund a deliberately deleted item;
/// a `failed` cheatsheet's charge is left for the boot sweep.
pub async fn delete_cheatsheet(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(cheatsheet_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // Verify ownership first (returns 404 for missing/other-account/already-deleted).
    owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("starting transaction: {error}"))
        })?;

    sqlx::query(
        "update cheatsheets
            set deleted_at = now(), updated_at = now(),
                credits_spent = case when status = 'generating' then 0 else credits_spent end
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(cheatsheet_id)
    .bind(user.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting the cheatsheet: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing cheatsheet deletion: {error}"))
    })?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RenameCheatsheetBody {
    title: String,
}

/// `PATCH /cheatsheets/:id`: rename a cheatsheet.
async fn rename_cheatsheet(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(cheatsheet_id): Path<Uuid>,
    Json(body): Json<RenameCheatsheetBody>,
) -> AppResult<Json<serde_json::Value>> {
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::Validation("title must not be empty".to_string()));
    }
    if title.chars().count() > 200 {
        return Err(AppError::Validation(
            "title must be at most 200 characters".to_string(),
        ));
    }

    owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;

    sqlx::query(
        "update cheatsheets set title = $3, updated_at = now()
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(cheatsheet_id)
    .bind(user.user_id)
    .bind(&title)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("renaming the cheatsheet: {error}")))?;

    let cheatsheet = owned_cheatsheet(&state, user.user_id, cheatsheet_id).await?;
    Ok(Json(serde_json::json!({ "cheatsheet": cheatsheet })))
}

/// The cheatsheet, or 404 when it is not this user's. Tombstones are excluded, so a deleted
/// cheatsheet is indistinguishable from one that never existed.
async fn owned_cheatsheet(
    state: &AppState,
    user_id: Uuid,
    cheatsheet_id: Uuid,
) -> AppResult<CheatsheetJson> {
    let row = sqlx::query(
        "select id, workspace_id, title, scope, exam_date, depth, source_ids, status, error,
                template_version, page_count, created_at, updated_at
           from cheatsheets
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(cheatsheet_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a cheatsheet: {error}")))?
    .ok_or(AppError::NotFound)?;

    Ok(cheatsheet_from_row(&row))
}

/// The stored HTML key, or a 409 naming the state.
///
/// 409 rather than 404: the cheatsheet exists and is readable, and what is missing is an
/// artifact the generation has not written yet. The client's remedy is to wait for the job
/// or to re-render, which is not what a 404 tells it.
async fn html_key_of(state: &AppState, cheatsheet_id: Uuid) -> AppResult<String> {
    let key: Option<String> =
        sqlx::query_scalar("select html_key from cheatsheets where id = $1 and deleted_at is null")
            .bind(cheatsheet_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the HTML key: {error}")))?
            .flatten();

    key.ok_or_else(|| AppError::Conflict("this cheatsheet has not been rendered yet".to_string()))
}

fn cheatsheet_from_row(row: &sqlx::postgres::PgRow) -> CheatsheetJson {
    let created_at: Option<DateTime<Utc>> = row.get("created_at");
    let updated_at: DateTime<Utc> = row.get("updated_at");

    CheatsheetJson {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        title: row.get("title"),
        scope: row.get("scope"),
        exam_date: row.get("exam_date"),
        depth: row.get("depth"),
        source_ids: row.get("source_ids"),
        status: row.get("status"),
        error: row.get("error"),
        template_version: row.get("template_version"),
        page_count: row.get("page_count"),
        created_at: created_at.unwrap_or(updated_at),
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The depth set is the module's, and an unknown one is refused before anything is
    /// charged: the prompt has no meaning for a depth it does not document.
    #[test]
    fn the_depths_are_the_module_s_own() {
        assert_eq!(DEPTHS, ["brief", "normal", "thorough"]);
        assert!(!DEPTHS.contains(&"deep"));
    }
}
