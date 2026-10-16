//! `POST /workspaces/:id/reports/outline`, `GET|PATCH /reports/:id`,
//! `POST /reports/:id/generate`, `GET /reports/:id/export`.
//!
//! Two things here are the contract and are easy to get wrong.
//!
//! **The outline is synchronous and the sections are not.** The outline is one short call
//! whose result the student edits before anything else happens, so it is made in this
//! route (decision 12); a section is per-index, retryable and long enough to outlive a
//! request, so it is a `generate_report_section` job.
//!
//! **The row is the reservation.** `create_outline` counts this month's reports, checks the
//! plan, charges and inserts the reserving row in **one transaction** holding the per-user
//! advisory lock, exactly as `quizzes::routes::create_quiz` does. Counting outside that
//! transaction reads a count a concurrent request has not committed yet, and a report that
//! is still generating has no section text at all — so a count that ignored it would read
//! zero and the monthly cap could be exceeded by asking twice in a row.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::Deserialize;
// `Row` is required for `row.get::<T, _>("column")`.
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::config::ExportFormat;
use stvdy_core::entitlements::Feature;
use stvdy_core::generation::report::{ReportOutline, parse_outline};
use stvdy_core::prompts::PromptChunk;

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::reports::generate::{self, OutlineRequest};
use crate::state::AppState;
use crate::{credits, jobs, workspaces};

/// The action name in `usage_events` for the outline call, so a cost report can separate
/// the two report steps from each other and from every other generator.
pub(crate) const OUTLINE_ACTION: &str = "report_outline";

/// The action name in `usage_events` for one section call. One row per section, because
/// that is what was bought: a report's cost is not one number.
pub(crate) const SECTION_ACTION: &str = "report_section";

/// The most sources one report may draw on. Bounded for the same reason a quiz's list is:
/// the prompt has a chunk budget.
const MAX_SOURCES: usize = 20;

/// The longest topic, style or length the route accepts.
///
/// Bounded rather than unbounded because all three are prompt variables: a topic of ten
/// thousand characters is a prompt the model was not given a budget for, and the request
/// would be charged before anyone noticed.
const MAX_TOPIC_CHARS: usize = 200;
const MAX_SETTING_CHARS: usize = 120;

/// The longest stored report title, after the outline's own title is accepted.
const MAX_TITLE_CHARS: usize = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{id}/reports", get(list_reports))
        .route("/workspaces/{id}/reports/outline", post(create_outline))
        .route(
            "/reports/{id}",
            get(get_report).patch(patch_report).delete(delete_report),
        )
        .route("/reports/{id}/generate", post(generate_section))
        .route("/reports/{id}/export", get(export_report))
}

#[derive(Debug, Deserialize)]
pub struct CreateOutlineBody {
    /// What the report is about. The module's `{{topic}}`.
    pub topic: String,
    /// The audience and register, `{{style}}`.
    pub style: String,
    /// The length target, `{{length}}`.
    pub length: String,
    pub source_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateBody {
    /// The outline section to write. Regenerating an index replaces that section's text
    /// and nothing else, so a student can fix one section without paying for the rest.
    pub index: usize,
}

#[derive(Debug, serde::Serialize)]
struct SectionJson {
    heading: String,
    /// Null until the section has been written, which is a different state from
    /// written-and-empty: the screen offers "Generate" for the first and "Regenerate" for
    /// the second.
    body_md: Option<String>,
    /// The server's flag, so progress survives a reload. The job outlives the tab that
    /// started it, so a client-side flag would show "idle" for work still running on the
    /// other device.
    generating: bool,
    error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct ReportJson {
    id: Uuid,
    workspace_id: Uuid,
    title: String,
    status: String,
    error: Option<String>,
    /// The editable outline, passed through as the JSON that was stored: it is the
    /// student's document and re-encoding it here could only lose a field.
    outline: Option<serde_json::Value>,
    body_md: Option<String>,
    sections: Vec<SectionJson>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// `POST /workspaces/:id/reports/outline`. 201 with the report in status `draft`.
///
/// The outline is generated here rather than by a job, and the charge is the outline's
/// alone (3 credits): the sections are charged one at a time when the student asks for
/// them, so an outline they discard costs only the outline.
pub async fn create_outline(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateOutlineBody>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let topic = body.topic.trim().to_string();
    if topic.is_empty() || topic.chars().count() > MAX_TOPIC_CHARS {
        return Err(AppError::Validation(format!(
            "the topic must be between 1 and {MAX_TOPIC_CHARS} characters"
        )));
    }
    let style = body.style.trim().to_string();
    let length = body.length.trim().to_string();
    for (name, value) in [("style", &style), ("length", &length)] {
        if value.is_empty() || value.chars().count() > MAX_SETTING_CHARS {
            return Err(AppError::Validation(format!(
                "{name} must be between 1 and {MAX_SETTING_CHARS} characters"
            )));
        }
    }
    if body.source_ids.is_empty() || body.source_ids.len() > MAX_SOURCES {
        return Err(AppError::Validation(format!(
            "a report is generated from between 1 and {MAX_SOURCES} sources"
        )));
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

    let workspace_name = workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    // Every source must be this user's, in this workspace, and finished ingesting:
    // an outline drawn from a source whose chunks do not exist yet has no labels to cite,
    // which is the one thing the outline exists to carry.
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
        AppError::Internal(anyhow::anyhow!("checking the report's sources: {error}"))
    })?;
    if ready != source_ids.len() as i64 {
        return Err(AppError::Validation(
            "every source must be one of this workspace's, and finished indexing".to_string(),
        ));
    }

    // The chunks the prompt is built from, loaded before the charge so a source set with
    // nothing indexed is a 400 rather than a 502 after the credits were spent.
    let chunks: Vec<PromptChunk> =
        crate::chunks::select_for_generation(&state.pool, &source_ids, user.user_id)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("loading the report's chunks: {error}"))
            })?;
    if chunks.is_empty() {
        return Err(AppError::Validation(
            "the selected sources have no indexed text to propose an outline from".to_string(),
        ));
    }

    let cost = state.config.plans.credits.report_outline;

    // The count, the charge and the reserving row commit together, because the limit
    // depends on the count. `reserve_in` is that sequence — the per-user advisory lock,
    // the matrix decision against a snapshot read under the lock, the charge, then this
    // closure's row — defined once in `crate::entitlements` and shared with the deck and
    // quiz reservations rather than copied a third time.
    //
    // The feature is `Report`: it counts `reports_per_month` and prices `report_outline`.
    // The section calls are priced separately (`report_section`) and do not consume the
    // monthly allowance, which is why only this route carries the counted feature.
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a report: {error}")))?;

    // Bound before the closure so `async move` captures references rather than the values
    // the handler still needs below (the outline request, the job payload).
    let user_id = user.user_id;
    let topic_ref = topic.as_str();
    let source_ids_ref = source_ids.as_slice();
    let style_ref = style.as_str();
    let length_ref = length.as_str();

    let (spent, (report_id, created_at, updated_at)) = crate::entitlements::reserve_in(
        &mut tx,
        &state,
        &user,
        Some(Feature::Report),
        OUTLINE_ACTION,
        cost,
        |conn, usage_event_id| {
            Box::pin(async move {
                // The report row *is* the reservation the decision's count read, so it is
                // written in the same transaction as the charge: a failure here rolls the
                // charge back with it, so there is no charge without a report and nothing
                // to compensate for by hand. The title starts as the topic and is replaced
                // by the model's title once the outline parses, because the student's own
                // words are a better placeholder than an empty string.
                let row: (Uuid, Option<DateTime<Utc>>, DateTime<Utc>) = sqlx::query_as(
                    "insert into reports
                        (user_id, workspace_id, title, status, source_ids, style, length,
                         credits_spent, usage_event_id)
                     values ($1, $2, $3, 'generating', $4, $5, $6, $7, $8)
                     returning id, created_at, updated_at",
                )
                .bind(user_id)
                .bind(workspace_id)
                .bind(topic_ref)
                .bind(source_ids_ref)
                .bind(style_ref)
                .bind(length_ref)
                .bind(cost as i32)
                .bind(usage_event_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("creating the report: {error}"))
                })?;
                Ok(row)
            })
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing the report: {error}")))?;

    let request = OutlineRequest {
        topic: &topic,
        style: &style,
        length: &length,
        source_ids: &source_ids,
        workspace_name: &workspace_name,
        chunks: &chunks,
    };

    let generation =
        match generate::propose_outline(&state, &user, spent.usage_event_id, &request).await {
            Ok(generation) => generation,
            Err(failure) => {
                // The charge is reversed and the report is left terminal, so a model that
                // cannot produce an outline costs the student nothing. `fail_and_refund` is the
                // same path a failed section takes, so the two cannot drift.
                crate::jobs::handlers::report::fail_and_refund(
                    &state,
                    report_id,
                    user.user_id,
                    cost as i32,
                    &failure.message,
                )
                .await;
                return Err(AppError::Provider(failure.message));
            }
        };

    // The outline is stored as the object the module's schema describes, built from the
    // parsed struct so a field the prompt documents and the struct requires cannot be
    // dropped on the way to `jsonb`.
    let outline = outline_json(&generation.outline);
    let title = bounded_title(&generation.outline.title, &topic);

    sqlx::query(
        "update reports
            set outline = $2, title = $3, status = 'draft', error = null, updated_at = now()
          where id = $1",
    )
    .bind(report_id)
    .bind(&outline)
    .bind(&title)
    .execute(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("storing the report's outline: {error}"))
    })?;

    // The provider's token counts and cost are attached to the row the spend created. A
    // reporting gap must not fail a request whose outline is already stored, so a failure
    // here is logged and the response still goes out.
    if let Err(error) = credits::record_usage(
        &state.pool,
        Some(spent.usage_event_id),
        Some(generation.in_tokens),
        Some(generation.out_tokens),
        generation.provider_cost_usd,
        &generation.prompt_version,
    )
    .await
    {
        tracing::error!(%error, %report_id, "could not attach the outline's provider usage");
    }

    let report = ReportJson {
        id: report_id,
        workspace_id,
        title,
        status: "draft".to_string(),
        error: None,
        outline: Some(outline),
        body_md: None,
        sections: sections_json(&generation.outline, &[], None, None),
        // `created_at` is nullable in `0011_reports.sql`; `updated_at` is not, and a row
        // cannot have been updated before it was created.
        created_at: created_at.unwrap_or(updated_at),
        updated_at,
    };

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "report": report })),
    ))
}

/// `GET /workspaces/:id/reports`, newest first, tombstones excluded.
///
/// Not in the spec's route list, and added for the same reason the quiz list was: without
/// it a report is unreachable after a page reload, and the Studio panel cannot poll the
/// ones still generating.
pub async fn list_reports(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    let rows = sqlx::query(
        "select id, workspace_id, title, status, error, outline, body_md, sections_md,
                generating_index, created_at, updated_at
           from reports
          where user_id = $1 and workspace_id = $2 and deleted_at is null
          order by sort_order asc, updated_at desc, id",
    )
    .bind(user.user_id)
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing reports: {error}")))?;

    let reports: Vec<ReportJson> = rows.iter().map(report_from_row).collect();
    Ok(Json(serde_json::json!({ "reports": reports })))
}

/// `GET /reports/:id`.
pub async fn get_report(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(report_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let report = owned_report(&state, user.user_id, report_id).await?;
    Ok(Json(serde_json::json!({ "report": report })))
}

/// `PATCH /reports/:id`: replaces the outline.
///
/// The whole outline is replaced rather than patched field by field, because it is
/// editable: an edit the generator could not have produced is still valid input as long as
/// it parses, and a field-level patch would need a second shape to describe a rename, a
/// reorder and a delete.
///
/// The body's `outline` is validated by the **core contract** — the incoming JSON is
/// rendered back to text and parsed by `parse_outline`, which is the same function the
/// model's reply goes through. A second validator here would be a second answer to what an
/// outline is, and the two would disagree about a section the student can see.
pub async fn patch_report(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(report_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> AppResult<Json<serde_json::Value>> {
    // Ownership first, and deliberately before the body is validated: a non-owner must get a
    // 404 for the id rather than a 400 describing a document they cannot see, which would
    // tell them the id exists.
    let row = load_report_row(&state, user.user_id, report_id).await?;

    let incoming = body
        .get("outline")
        .ok_or_else(|| AppError::Validation("the body must carry an outline".to_string()))?;
    let text = serde_json::to_string(incoming)
        .map_err(|error| AppError::Validation(format!("the outline is not JSON: {error}")))?;
    let outline = parse_outline(&text).map_err(|error| {
        AppError::Validation(format!("the outline does not match the contract: {error}"))
    })?;

    let previous_outline: Option<serde_json::Value> = row.get("outline");
    let previous_body: Option<String> = row.get("body_md");
    let previous_texts: Vec<Option<String>> = row
        .get::<Option<serde_json::Value>, _>("sections_md")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    // The section texts follow their heading, and only when the heading is unambiguous:
    // a heading that appears once in the old outline and once in the new one is the same
    // section, so a reorder keeps its text. Anything else — a rename, a duplicate, a
    // deletion — loses that section's text rather than attributing it to a heading the
    // model never wrote it for, which is the failure that would reach a printed report.
    let old_headings = headings_of(&previous_outline);
    let new_headings: Vec<String> = outline.sections.iter().map(|s| s.heading.clone()).collect();
    let texts = align_sections(&old_headings, &previous_texts, &new_headings);
    let body_md = compose_body_md(&new_headings, &texts);
    let outline_json = outline_json(&outline);
    let title = bounded_title(&outline.title, &new_headings.join(" "));

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning the edit: {error}")))?;

    // The roadmap's conflict rule: the latest edit wins and the losing body is kept as a
    // recoverable copy. Written before the update, so the revision is the text that was
    // there, not the text that replaced it.
    if let Some(previous) = previous_body.as_deref()
        && !previous.trim().is_empty()
        && Some(previous) != body_md.as_deref()
    {
        sqlx::query(
            "insert into report_revisions (report_id, user_id, body_md) values ($1, $2, $3)",
        )
        .bind(report_id)
        .bind(user.user_id)
        .bind(previous)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("storing the replaced body: {error}"))
        })?;
    }

    sqlx::query(
        "update reports
            set outline = $2, title = $3, sections_md = $4, body_md = $5, updated_at = now()
          where id = $1 and user_id = $6 and deleted_at is null",
    )
    .bind(report_id)
    .bind(&outline_json)
    .bind(&title)
    .bind(serde_json::Value::from(texts))
    .bind(&body_md)
    .bind(user.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("saving the outline: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing the edit: {error}")))?;

    let report = owned_report(&state, user.user_id, report_id).await?;
    Ok(Json(serde_json::json!({ "report": report })))
}

/// `POST /reports/:id/generate`: writes one section.
///
/// 202, because the work is a job: the response says the section is in flight and the
/// client polls `GET /reports/:id`. The charge is one `report_section` per call, and the
/// row records the index in flight so a second device cannot start a second section of the
/// same report while the first is being written.
pub async fn generate_section(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(report_id): Path<Uuid>,
    Json(body): Json<GenerateBody>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let row = load_report_row(&state, user.user_id, report_id).await?;
    let status: String = row.get("status");
    let outline: Option<serde_json::Value> = row.get("outline");
    let section_count = outline
        .as_ref()
        .map(|value| headings_of(&Some(value.clone())).len())
        .unwrap_or(0);

    if body.index >= section_count {
        return Err(AppError::Validation(format!(
            "the report has {section_count} sections; index {} is outside the outline",
            body.index
        )));
    }
    if status == "generating" {
        // One section at a time, and the status is the marker: two concurrent writes would
        // both rewrite `body_md` from a row they read before the other wrote it.
        return Err(AppError::Conflict(
            "this report already has a section in flight".to_string(),
        ));
    }

    let cost = state.config.plans.credits.report_section;

    // The charge and the status flip commit together, so a report can never be `generating`
    // without a charge or charged without being in flight.
    //
    // The feature is `None`: a section costs credits and consumes **no** monthly allowance
    // (`reports_per_month` is spent once, by the outline), and the contract's `Feature`
    // enum has no variant for "a section of an existing report". `reserve_in` still takes
    // the lock and still charges; the ledger is the whole gate here.
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a section: {error}")))?;

    let (_, ()) = crate::entitlements::reserve_in(
        &mut tx,
        &state,
        &user,
        None,
        SECTION_ACTION,
        cost,
        |conn, usage_event_id| {
            // Bound outside the `async move` so the closure copies a `Uuid` rather than
            // moving the caller's `VerifiedUser`, which the job enqueue below still needs.
            let user_id = user.user_id;
            Box::pin(async move {
                // `usage_event_id` is replaced with this spend's row: the job attaches the
                // provider's tokens and cost to the section call that is running, not to
                // the outline call that created the report.
                let claimed = sqlx::query(
                    "update reports
                        set status = 'generating', generating_index = $2, error = null,
                            credits_spent = $3, usage_event_id = $4, updated_at = now()
                      where id = $1 and user_id = $5 and deleted_at is null
                        and status in ('draft', 'ready', 'failed')",
                )
                .bind(report_id)
                .bind(body.index as i32)
                .bind(cost as i32)
                .bind(usage_event_id)
                .bind(user_id)
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("claiming the section: {error}"))
                })?;

                if claimed.rows_affected() == 0 {
                    // Another device started a section between the read above and this
                    // write. The charge is dropped with the transaction, so the loser pays
                    // nothing.
                    return Err(AppError::Conflict(
                        "this report already has a section in flight".to_string(),
                    ));
                }
                Ok(())
            })
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing the section: {error}")))?;

    // Enqueued after the commit, so a failure here leaves a report the user can see failing
    // rather than a charge with no trace.
    if let Err(error) = jobs::enqueue(
        &state.pool,
        &state.jobs,
        "generate_report_section",
        serde_json::json!({ "report_id": report_id, "index": body.index }),
        None,
    )
    .await
    {
        crate::jobs::handlers::report::fail_and_refund(
            &state,
            report_id,
            user.user_id,
            cost as i32,
            "the generation job could not be queued",
        )
        .await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "enqueueing the section job: {error}"
        )));
    }

    let report = owned_report(&state, user.user_id, report_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "report": report })),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub format: String,
}

/// `GET /reports/:id/export?format=md|pdf`.
///
/// The format is checked against the plan's `export_formats` **before** anything is
/// produced, and the refusal is `403 plan_required` with `min_plan`: a client that has been
/// told about a purchase it must not offer is the review risk the policy exists for, so the
/// answer names the plan rather than the price.
pub async fn export_report(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(report_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> AppResult<Response> {
    let format = match query.format.as_str() {
        "md" => ExportFormat::Md,
        "pdf" => ExportFormat::Pdf,
        other => {
            return Err(AppError::Validation(format!(
                "{other} is not an export format for a report; use md or pdf"
            )));
        }
    };

    // The format is a plan feature, not a quantity: the refusal is 403
    // `plan_required` with `min_plan` when a higher plan has it and this client may be
    // told so, and 403 `feature_locked` on a store-policy device where no purchase call to
    // action is allowed (A143). Both are decided from the device row, never the body.
    crate::entitlements::require(&state, &user, Feature::ExportFormat(format)).await?;

    let ReportJson { title, body_md, .. } = owned_report(&state, user.user_id, report_id).await?;
    let Some(body_md) = body_md else {
        return Err(AppError::Conflict(
            "this report has no generated sections to export yet".to_string(),
        ));
    };

    match format {
        ExportFormat::Md => Ok((
            [
                (
                    header::CONTENT_TYPE,
                    "text/markdown; charset=utf-8".to_string(),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}.md\"", file_stem(&title)),
                ),
            ],
            body_md,
        )
            .into_response()),
        ExportFormat::Pdf => {
            let pdf = crate::reports::render::render_report_pdf(&state, &title, &body_md).await?;
            Ok((
                [
                    (header::CONTENT_TYPE, "application/pdf".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{}.pdf\"", file_stem(&title)),
                    ),
                ],
                pdf,
            )
                .into_response())
        }
        // The route's two formats are named above; this arm exists because the enum is
        // open and a future format must be added here deliberately rather than falling
        // through to a document the student did not ask for.
        other => Err(AppError::Validation(format!(
            "{} is not an export format for a report",
            format!("{other:?}").to_lowercase()
        ))),
    }
}

/// `DELETE /reports/:id`: tombstone the report. Credits for a `generating` report are
/// zeroed so `fail_and_refund` cannot refund a deliberately deleted item; a `failed`
/// report's charge is left for the boot sweep. Report revisions are left intact.
pub async fn delete_report(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(report_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // Verify ownership first (returns 404 for missing/other-account/already-deleted).
    owned_report(&state, user.user_id, report_id).await?;

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("starting transaction: {error}"))
        })?;

    sqlx::query(
        "update reports
            set deleted_at = now(), updated_at = now(),
                credits_spent = case when status = 'generating' then 0 else credits_spent end
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(report_id)
    .bind(user.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting the report: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing report deletion: {error}"))
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// The report, or 404 when it is not this user's. Tombstones are excluded, so a deleted
/// report is indistinguishable from one that never existed: the id must not confirm that
/// another account's report exists.
async fn owned_report(state: &AppState, user_id: Uuid, report_id: Uuid) -> AppResult<ReportJson> {
    let row = load_report_row(state, user_id, report_id).await?;
    Ok(report_from_row(&row))
}

async fn load_report_row(
    state: &AppState,
    user_id: Uuid,
    report_id: Uuid,
) -> AppResult<sqlx::postgres::PgRow> {
    sqlx::query(
        "select id, workspace_id, title, status, error, outline, body_md, sections_md,
                generating_index, created_at, updated_at
           from reports
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(report_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a report: {error}")))?
    .ok_or(AppError::NotFound)
}

fn report_from_row(row: &sqlx::postgres::PgRow) -> ReportJson {
    let outline: Option<serde_json::Value> = row.get("outline");
    let texts: Vec<Option<String>> = row
        .get::<Option<serde_json::Value>, _>("sections_md")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let status: String = row.get("status");
    let generating_index: Option<i32> = row.get("generating_index");
    let error: Option<String> = row.get("error");
    let created_at: Option<DateTime<Utc>> = row.get("created_at");
    let updated_at: DateTime<Utc> = row.get("updated_at");

    let sections = sections_json_of(
        &outline,
        &texts,
        &status,
        generating_index,
        error.as_deref(),
    );

    ReportJson {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        title: row.get("title"),
        sections,
        status,
        error,
        outline,
        body_md: row.get("body_md"),
        created_at: created_at.unwrap_or(updated_at),
        updated_at,
    }
}

/// The per-section view: the outline's headings and purposes, the text each section has,
/// and the two flags the screen needs.
///
/// `generating` is true only for the section the row says is in flight, and the report's
/// error is shown on that same section rather than on the report: the student needs to know
/// *which* section failed, and the report as a whole is still theirs.
fn sections_json_of(
    outline: &Option<serde_json::Value>,
    texts: &[Option<String>],
    status: &str,
    generating_index: Option<i32>,
    error: Option<&str>,
) -> Vec<SectionJson> {
    let headings = headings_of(outline);
    headings
        .into_iter()
        .enumerate()
        .map(|(index, heading)| {
            let generating = status == "generating" && generating_index == Some(index as i32);
            SectionJson {
                heading,
                body_md: texts.get(index).cloned().flatten(),
                generating,
                error: if status == "failed" && generating_index == Some(index as i32) {
                    error.map(str::to_string)
                } else {
                    None
                },
            }
        })
        .collect()
}

fn sections_json(
    outline: &ReportOutline,
    texts: &[Option<String>],
    status: Option<&str>,
    error: Option<&str>,
) -> Vec<SectionJson> {
    let value = outline_json(outline);
    sections_json_of(&Some(value), texts, status.unwrap_or("draft"), None, error)
}

/// The outline's section headings, in order.
///
/// Read from the stored JSON rather than from the struct: the route writes the object the
/// module's schema describes and the client reads it back, so the JSON *is* the outline and
/// re-parsing it into a struct on every read would be a second decode of the same bytes.
fn headings_of(outline: &Option<serde_json::Value>) -> Vec<String> {
    outline
        .as_ref()
        .and_then(|value| value.get("sections"))
        .and_then(|sections| sections.as_array())
        .map(|sections| {
            sections
                .iter()
                .map(|section| {
                    section
                        .get("heading")
                        .and_then(|heading| heading.as_str())
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The outline as the JSON the client reads and the module's schema describes.
///
/// Built from the parsed struct rather than by passing the model's raw text through: the
/// struct is what was validated, so a field the prompt documents cannot be dropped on the
/// way to `jsonb`, and a field the model invented cannot arrive at the client.
fn outline_json(outline: &ReportOutline) -> serde_json::Value {
    serde_json::json!({
        "title": outline.title,
        "sections": outline
            .sections
            .iter()
            .map(|section| serde_json::json!({
                "heading": section.heading,
                "purpose": section.purpose,
                "labels": section.labels,
                "gaps": section.gaps,
            }))
            .collect::<Vec<_>>(),
    })
}

/// Keeps a stored title inside the column's expectations.
///
/// `reports.title` is `text` and unbounded, but the title is what the report is listed and
/// printed under, so it is capped rather than trusted: a model that returned its whole
/// outline as the title would otherwise produce a list row that cannot be read.
fn bounded_title(title: &str, fallback: &str) -> String {
    let trimmed = title.trim();
    let source = if trimmed.is_empty() {
        fallback.trim()
    } else {
        trimmed
    };
    source.chars().take(MAX_TITLE_CHARS).collect()
}

/// The section texts after an outline edit.
///
/// A heading that appears exactly once in the old outline and exactly once in the new one
/// is the same section, so its text follows it — which is what makes a reorder keep the
/// work the student paid for. Every other case (a rename, a duplicate heading, a deletion,
/// a new section) leaves that section unwritten, and that is deliberate: attributing one
/// section's text to another heading produces a report that reads as if the model wrote
/// something it did not, and nothing on the page would say so.
fn align_sections(
    old_headings: &[String],
    old_texts: &[Option<String>],
    new_headings: &[String],
) -> Vec<Option<String>> {
    new_headings
        .iter()
        .map(|heading| {
            let mut matches = old_headings
                .iter()
                .enumerate()
                .filter(|(_, old)| *old == heading);
            let first = matches.next()?;
            if matches.next().is_some() {
                // The heading is ambiguous in the old outline, so there is no way to tell
                // which text this section is.
                return None;
            }
            old_texts.get(first.0).cloned().flatten()
        })
        .collect()
}

/// The report as stored Markdown: each written section under its heading, in outline order.
///
/// Derived from the section texts rather than accumulated as the sections arrive, so the
/// document and the per-section view cannot disagree, and a reorder re-emits the document
/// in the new order. `None` when no section has text, which is what makes `body_md` mean
/// "there is a document" rather than "the field exists".
pub(crate) fn compose_body_md(headings: &[String], texts: &[Option<String>]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (index, heading) in headings.iter().enumerate() {
        let Some(Some(text)) = texts.get(index) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        parts.push(format!("## {}\n\n{text}", heading.trim()));
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("\n\n"))
}

/// A filename stem from a title: the characters a filesystem and a `content-disposition`
/// header both accept, and nothing else.
///
/// `pub(crate)` because the cheatsheet's PDF route uses the same rule: two derivations would
/// be two chances for one of them to emit a quote or a slash into a header.
pub(crate) fn file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches('-').to_string();
    if stem.is_empty() {
        "report".to_string()
    } else {
        stem.chars().take(60).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stvdy_core::generation::report::MAX_REPORT_SECTIONS;

    fn headings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// The outline contract is enforced on the edit path too, and by the same function the
    /// model's reply goes through. A second validator here would be a second answer to what
    /// an outline is.
    #[test]
    fn an_outline_edit_is_validated_by_the_core_contract() {
        let error = parse_outline(
            r#"{"title":"T","sections":[{"heading":"","purpose":"p","labels":[],"gaps":false}]}"#,
        )
        .expect_err("an empty heading must be refused");
        assert!(error.to_string().contains("heading"), "{error}");

        // And the field the SPA round-trips is required, not defaulted: an edit that drops
        // `labels` cannot be stored, because the section call cites them.
        let error = parse_outline(
            r#"{"title":"T","sections":[{"heading":"h","purpose":"p","gaps":false}]}"#,
        )
        .expect_err("labels is required");
        assert!(error.to_string().contains("labels"), "{error}");

        let outline = parse_outline(
            r#"{"title":"T","sections":[{"heading":"h","purpose":"p","labels":[1],"gaps":true}]}"#,
        )
        .expect("a complete outline parses");
        assert_eq!(outline.sections.len(), 1);
        assert_eq!(MAX_REPORT_SECTIONS, 12);
    }

    /// A reorder keeps the text of the section that moved, because the heading is what
    /// identifies a section: index-aligned texts would silently re-label a section's prose
    /// with the neighbouring heading, which is the failure that reaches a printed report.
    #[test]
    fn a_reordered_outline_keeps_each_sections_text() {
        let old = headings(&["Pendahuluan", "Isi", "Kesimpulan"]);
        let texts = vec![
            Some("satu".to_string()),
            Some("dua".to_string()),
            Some("tiga".to_string()),
        ];

        let aligned = align_sections(
            &old,
            &texts,
            &headings(&["Isi", "Pendahuluan", "Kesimpulan"]),
        );
        assert_eq!(
            aligned,
            vec![
                Some("dua".to_string()),
                Some("satu".to_string()),
                Some("tiga".to_string())
            ]
        );
    }

    /// A rename loses that section's text rather than keeping it under the new heading: the
    /// text was written for the old purpose, and a report that prints it under a heading the
    /// model never wrote for is a lie about what it says.
    #[test]
    fn a_renamed_section_is_unwritten_and_a_deleted_one_is_dropped() {
        let old = headings(&["Pendahuluan", "Isi"]);
        let texts = vec![Some("satu".to_string()), Some("dua".to_string())];

        let aligned = align_sections(&old, &texts, &headings(&["Pendahuluan", "Isi baru"]));
        assert_eq!(aligned, vec![Some("satu".to_string()), None]);

        // A deleted section's text is gone from the document entirely, and the surviving
        // section keeps its own.
        let aligned = align_sections(&old, &texts, &headings(&["Isi"]));
        assert_eq!(aligned, vec![Some("dua".to_string())]);
    }

    /// Two sections with the same heading are ambiguous, so neither inherits a text: there
    /// is no way to tell which one it was written for.
    #[test]
    fn a_duplicated_heading_does_not_inherit_a_text() {
        let old = headings(&["Isi", "Isi"]);
        let texts = vec![Some("satu".to_string()), Some("dua".to_string())];

        let aligned = align_sections(&old, &texts, &headings(&["Isi", "Isi"]));
        assert_eq!(aligned, vec![None, None]);
    }

    /// The stored document is the sections under their headings, and it is `None` while
    /// nothing has been written — which is what makes "there is a document" and "the field
    /// exists" two different things.
    #[test]
    fn the_body_is_the_written_sections_in_outline_order() {
        let headings = headings(&["Satu", "Dua", "Tiga"]);
        let texts = vec![Some("isi satu".to_string()), None, Some("  ".to_string())];
        assert_eq!(
            compose_body_md(&headings, &texts).as_deref(),
            Some("## Satu\n\nisi satu")
        );

        let texts = vec![None, Some("  isi dua  ".to_string()), None];
        assert_eq!(
            compose_body_md(&headings, &texts).as_deref(),
            Some("## Dua\n\nisi dua"),
            "a section's text is trimmed, and the heading is the outline's"
        );

        assert_eq!(compose_body_md(&headings, &[None, None, None]), None);
    }

    /// A title is capped and falls back, so a model that returns its whole outline as the
    /// title cannot produce an unreadable list row.
    #[test]
    fn a_title_is_bounded_and_falls_back() {
        assert_eq!(bounded_title("  Ringkasan  ", "topik"), "Ringkasan");
        assert_eq!(bounded_title("   ", "topik"), "topik");
        let long = "x".repeat(500);
        assert_eq!(
            bounded_title(&long, "topik").chars().count(),
            MAX_TITLE_CHARS
        );
    }

    /// A filename is derived from the title without trusting it: a title containing a quote
    /// or a slash must not become a header injection or a path.
    #[test]
    fn an_export_filename_is_derived_safely() {
        assert_eq!(file_stem("Jaringan Komputer"), "Jaringan-Komputer");
        assert_eq!(file_stem("???"), "report");

        // The property, rather than one spelling of it: nothing that could end a quoted
        // header value or walk a path survives.
        let stem = file_stem("a\"/../b; filename=\"x\"");
        for forbidden in ['"', '/', '\\', ';', '.'] {
            assert!(
                !stem.contains(forbidden),
                "{forbidden:?} must not survive into a filename: {stem}"
            );
        }
        assert!(stem.starts_with('a') && stem.contains('b'), "{stem}");
        assert!(stem.len() <= 60, "a filename is bounded: {stem}");
    }
}
