//! `DELETE /account` and `GET /account/export` (A355).
//!
//! The spec pins both routes and the shape of the promise: *"Account deletion: in-app
//! `Delete account` … that deletes or anonymizes all user data, revokes sessions, and
//! cancels pending jobs. There are no recurring charges to cancel; document what happens to
//! remaining paid time. Also `Export my data` (JSON + Markdown + ICS)."*
//!
//! Three decisions follow from the pinned text, and each one is visible in the code rather
//! than in a comment about intent:
//!
//!  * **The request is not the purge.** `users.deleted_at` is the column and the spec calls
//!    the delete *scheduled*, so [`request_deletion`] marks the account and schedules the
//!    purge; `jobs::handlers::purge_accounts` does the removing. The window between the two
//!    is `ACCOUNT_DELETION_GRACE_DAYS` and it is the job's `run_at`, so there is exactly one
//!    place that decides when an account is purged (A355).
//!  * **Everything that is a credential is revoked now, not at purge time.** Sessions,
//!    devices, the ICS feed token, pending login tickets and verification tokens all die at
//!    the moment of the request. A window in which a signed-in device still works would make
//!    the confirmation a lie, and the feed token is a capability URL with no session at all —
//!    leaving it live would keep serving the student's calendar after they deleted it.
//!  * **Login is refused for a marked account**, distinguishably (`account_deleted`), so the
//!    student is told they asked for this and when it ends rather than being shown a wrong
//!    password. `auth::routes::login`, `auth::routes::signup` and `auth::google::resolve_user`
//!    each carry that refusal.
//!
//! The export is **bounded, not streamed**: `EXPORT_MAX_ROWS` per collection, with the
//! truncated collections named in the document. Bounding is the honest choice here because
//! the alternative — a chunked writer over fifteen queries — buys a response shape no client
//! in this product reads (both clients hand the bytes straight to a file) at the cost of a
//! second streaming path to get right. The student is told what was cut, which a stream
//! cannot tell them.

use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Rows carried per collection by `GET /account/export`.
///
/// One number for every collection rather than a per-table budget: a per-table map is a
/// second thing to keep in step with the query list, and the property that matters is "the
/// response is bounded", not "each table got a fair share". Five thousand rows is a few
/// megabytes of JSON for the largest table this product writes (flashcards, messages), which
/// is a document a client can hold in memory on a phone — the number the alternative
/// (streaming) exists to avoid having to pick.
pub const EXPORT_MAX_ROWS: i64 = 5_000;

/// The two routes. Merged into the authenticated group by `routes::router`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/account", delete(request_deletion))
        .route("/account/export", get(export))
}

/// `DELETE /account`. Marks the account, revokes its credentials, cancels its jobs, and
/// schedules the purge.
///
/// **Idempotent**: a second call on a marked account is not an error, and it answers the same
/// facts — the date the account was marked and the date its data goes. That matters because
/// the client's retry is the only thing standing between a dropped response and a student
/// who taps the button again, and a 409 there would read as "deletion failed".
pub async fn request_deletion(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Value>> {
    let outcome = mark_deleted(&state, user.user_id).await?;

    tracing::info!(
        user_id = %user.user_id,
        sessions_revoked = outcome.sessions_revoked,
        jobs_cancelled = outcome.jobs_cancelled,
        already_marked = outcome.already_marked,
        "account deletion requested"
    );

    Ok(Json(json!({
        "deleted_at": outcome.deleted_at,
        "purges_at": outcome.purges_at,
        // Told rather than inferred: the confirmation copy says remaining prepaid time is
        // forfeited (A350), and this is the row that makes it a fact about *this* account
        // rather than a sentence about the product.
        "forfeited_period_end": outcome.forfeited_period_end,
        "sessions_revoked": outcome.sessions_revoked,
        "jobs_cancelled": outcome.jobs_cancelled,
        "already_deleted": outcome.already_marked,
    })))
}

/// What [`mark_deleted`] did, and the two dates the response is made of.
#[derive(Debug)]
pub struct Deletion {
    pub deleted_at: DateTime<Utc>,
    pub purges_at: DateTime<Utc>,
    /// The end of the paid period the account was inside, if any. Forfeited, and stated.
    pub forfeited_period_end: Option<DateTime<Utc>>,
    pub sessions_revoked: u64,
    pub jobs_cancelled: u64,
    /// True when the account was already marked, which is what makes a retry a no-op.
    pub already_marked: bool,
}

/// Marks the account and revokes everything that could act for it.
///
/// One transaction, so a request that revokes the sessions and then fails to schedule the
/// purge cannot leave an account that is unreachable *and* never removed. The purge's `run_at`
/// is derived from the `deleted_at` this statement wrote, so the two cannot disagree about the
/// window.
pub async fn mark_deleted(state: &AppState, user_id: Uuid) -> AppResult<Deletion> {
    let now = state.clock.now();
    let grace = chrono::Duration::days(state.config.account_deletion_grace_days);

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a deletion: {error}"))
        })?;

    // `and deleted_at is null` is what makes the second call a no-op *and* leaves the first
    // date alone: a retry must not push the purge window forward, or a client that retried
    // would extend its own account's life.
    let marked: Option<DateTime<Utc>> = sqlx::query_scalar(
        "update users set deleted_at = $2
          where id = $1 and deleted_at is null
      returning deleted_at",
    )
    .bind(user_id)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("marking the account: {error}")))?;

    let already_marked = marked.is_none();
    let deleted_at = match marked {
        Some(at) => at,
        None => sqlx::query_scalar("select deleted_at from users where id = $1")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("reading the deletion date: {error}"))
            })?
            // The caller is authenticated against this row, so it exists. A `None` here is a
            // row deleted between the extractor and this statement — impossible, since the
            // purge anonymizes rather than deletes — and 401 is the honest answer.
            .ok_or(AppError::Unauthorized)?,
    };

    // The paid period the student is forfeiting. Read before anything is revoked, so the
    // answer describes the account as it was when they confirmed.
    let forfeited_period_end: Option<DateTime<Utc>> = sqlx::query_scalar(
        "select current_period_end from subscriptions
          where user_id = $1 and status in ('active', 'grace')
          order by current_period_end desc limit 1",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the paid period: {error}")))?;

    // Every session family. `revoked_at is null` so the count is what this call did, not what
    // the table holds — a retry must report 0 rather than pretending to have revoked again.
    let sessions_revoked = sqlx::query(
        "update auth_sessions set revoked_at = $2 where user_id = $1 and revoked_at is null",
    )
    .bind(user_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking the sessions: {error}")))?
    .rows_affected();

    // Devices too, not only the sessions that hang off them: the extractor refuses a request
    // whose device is revoked, and a device row left live is a row `resolve_device` would
    // reuse if the account were ever restored inside the window.
    sqlx::query("update devices set revoked_at = $2 where user_id = $1 and revoked_at is null")
        .bind(user_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("revoking the devices: {error}")))?;

    // The feed token is a **capability URL with no session behind it** (A242): revoking
    // sessions does not touch it, and `GET /cal/{token}` would keep serving this account's
    // calendar to anyone holding the link until the purge. Clearing it here is what makes the
    // deletion effective immediately for the one route that carries no credential.
    sqlx::query("update users set ics_feed_token = null where id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("clearing the feed token: {error}")))?;

    // Credentials that are not sessions: a verification link and a login ticket both mint
    // access to the account, and both outlive the request that created them.
    for statement in [
        "delete from email_verification_tokens where user_id = $1",
        "delete from login_tickets where user_id = $1",
    ] {
        sqlx::query(statement)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("clearing credentials: {error}"))
            })?;
    }

    // Queued and running jobs. A running job cannot be stopped mid-flight — it is already
    // executing — but its handler either finds nothing to write (the account is marked, and
    // every per-account handler loads the account with `deleted_at is null`) or writes into a
    // row the purge is about to remove. Marking it `failed` is the cancellation the status
    // vocabulary has; there is no `canceled` state, and inventing one would be a fifth value
    // every reader of `jobs.status` would have to learn.
    //
    // Two payload shapes are covered. The per-account kinds carry `user_id`; `send_push`
    // carries only `reminder_id` (it is per reminder, not per account), so it is matched
    // through the account's own reminders.
    let jobs_cancelled = sqlx::query(
        "update jobs set status = 'failed', error = 'account deleted'
          where status in ('queued', 'running')
            and (payload->>'user_id' = $1::text
                 or (kind = 'send_push'
                     and payload->>'reminder_id' in
                         (select id::text from reminders where user_id = $1)))",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("cancelling the jobs: {error}")))?
    .rows_affected();

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a deletion: {error}")))?;

    let purges_at = deleted_at + grace;

    // Scheduled **after** the commit, and idempotent by `dedupe_key`: a retry re-enqueues with
    // the same key and `enqueue_at`'s `on conflict do update` leaves the existing `run_at`
    // alone. So the window is the first request's, and a job that already ran is not re-run.
    crate::jobs::enqueue_at(
        &state.pool,
        &state.jobs,
        "purge_deleted_accounts",
        json!({ "user_id": user_id }),
        Some(&purge_dedupe_key(user_id)),
        purges_at,
    )
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("scheduling the purge: {error}")))?;

    Ok(Deletion {
        deleted_at,
        purges_at,
        forfeited_period_end,
        sessions_revoked,
        jobs_cancelled,
        already_marked,
    })
}

/// The purge job's `dedupe_key`. One per account, forever: the account is purged once, and a
/// second enqueue — from a retried request or from the scheduler's backstop pass — must be a
/// no-op rather than a second pass over the same rows.
pub fn purge_dedupe_key(user_id: Uuid) -> String {
    format!("purge_deleted_accounts:{user_id}")
}

/// The `format` `GET /account/export` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Markdown,
    Ics,
}

impl ExportFormat {
    fn parse(raw: &str) -> AppResult<Self> {
        match raw {
            "json" => Ok(Self::Json),
            "md" => Ok(Self::Markdown),
            "ics" => Ok(Self::Ics),
            other => Err(AppError::Validation(format!(
                "{other} is not an export format; use json, md or ics"
            ))),
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Markdown => "text/markdown; charset=utf-8",
            Self::Ics => "text/calendar; charset=utf-8",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
            Self::Ics => "ics",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    /// `json` (the default), `md` or `ics`. Not a plan feature: `ExportFormat` in the
    /// entitlements matrix governs the *report* and *cheatsheet* exports, and the spec's
    /// deletion rule puts `Export my data` on every plan — a student on free must be able to
    /// take their data out before they leave.
    pub format: Option<String>,
}

/// `GET /account/export`. The caller's own data, in one of three documents.
///
/// Every collection is read with an explicit `user_id = $1` predicate — the cross-tenant
/// assertion in `crates/server/tests/account.rs` is the reason `EXPORT_COLLECTIONS` is a
/// literal list of statements rather than a loop over table names: a missing predicate is the
/// worst possible version of this bug, and a list you can read is a list you can check.
pub async fn export(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ExportQuery>,
) -> AppResult<Response> {
    let format = match query.format.as_deref() {
        None => ExportFormat::Json,
        Some(raw) => ExportFormat::parse(raw)?,
    };

    let body = match format {
        ExportFormat::Json => serde_json::to_vec_pretty(
            &export_json(&state.pool, user.user_id).await?,
        )
        .map_err(|error| AppError::Internal(anyhow::anyhow!("serializing the export: {error}")))?,
        ExportFormat::Markdown => export_markdown(&state.pool, user.user_id)
            .await?
            .into_bytes(),
        ExportFormat::Ics => export_ics(&state, user.user_id).await?.into_bytes(),
    };

    tracing::info!(user_id = %user.user_id, format = ?format, bytes = body.len(), "account export");

    Ok((
        [
            (header::CONTENT_TYPE, format.content_type().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"stvdy-export.{}\"",
                    format.extension()
                ),
            ),
        ],
        body,
    )
        .into_response())
}

/// Every collection the export carries, as `(key, statement)`.
///
/// The statements select whole rows (`select *`) rather than a column list, deliberately: this
/// is the student's own data, so *every* column of their row belongs in it, and a hand-written
/// list would silently omit the next column a migration adds — which is a data export that
/// stops being complete without anyone noticing. `to_jsonb` is applied to the row, so the
/// document tracks the schema.
///
/// `deleted_at is null` excludes tombstones: a row the student already deleted is not part of
/// what they are asking for. It is not a filter on the *account* — a marked account cannot
/// reach this route at all.
const EXPORT_COLLECTIONS: &[(&str, &str)] = &[
    (
        "workspaces",
        "select * from workspaces where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "sources",
        "select * from sources where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "messages",
        "select * from messages where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "decks",
        "select * from decks where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "flashcards",
        "select * from flashcards where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "flashcard_reviews",
        "select * from flashcard_reviews where user_id = $1 order by reviewed_at, id",
    ),
    (
        "quizzes",
        "select * from quizzes where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "quiz_questions",
        "select * from quiz_questions where user_id = $1 order by quiz_id, created_at, id",
    ),
    (
        "quiz_attempts",
        "select * from quiz_attempts where user_id = $1 order by answered_at, id",
    ),
    (
        "reports",
        "select * from reports where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "report_revisions",
        "select * from report_revisions where user_id = $1 order by replaced_at, id",
    ),
    (
        "cheatsheets",
        "select * from cheatsheets where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "tasks",
        "select * from tasks where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "goals",
        "select * from goals where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "study_sessions",
        "select * from study_sessions where user_id = $1 and deleted_at is null order by started_at, id",
    ),
    (
        "events",
        "select * from events where user_id = $1 and deleted_at is null order by starts_at, id",
    ),
    (
        "reminders",
        "select * from reminders where user_id = $1 order by fire_at, id",
    ),
    (
        "pending_actions",
        "select * from pending_actions where user_id = $1 and deleted_at is null order by created_at, id",
    ),
    (
        "syllabus_drafts",
        "select * from syllabus_drafts where user_id = $1 order by created_at, id",
    ),
    (
        "plan_proposals",
        "select * from plan_proposals where user_id = $1 order by created_at, id",
    ),
    (
        "calendar_connections",
        "select * from calendar_connections where user_id = $1 order by id",
    ),
    (
        "devices",
        "select * from devices where user_id = $1 order by created_at, id",
    ),
    (
        "subscriptions",
        "select * from subscriptions where user_id = $1 order by created_at, id",
    ),
    (
        "payments",
        "select * from payments where user_id = $1 order by created_at, id",
    ),
    (
        "credit_ledger",
        "select * from credit_ledger where user_id = $1 order by created_at, id",
    ),
    (
        "usage_events",
        "select * from usage_events where user_id = $1 order by created_at, id",
    ),
    (
        "user_settings",
        "select * from user_settings where user_id = $1",
    ),
];

/// The account row, with an **explicit** column list because this is the one place the export
/// must omit something: `password_hash`, `google_sub` and `ics_feed_token` are credentials,
/// not data, and a data export that hands back a password hash is a leak with a download
/// button.
const EXPORT_ACCOUNT: &str = "select id, email, timezone, plan_id, email_verified, \
                              email_verified_at, student_verified, student_email, \
                              is_admin, created_at, deleted_at, purged_at \
                                from users where id = $1";

/// The JSON document: the account, one array per collection, and what was cut.
pub async fn export_json(pool: &PgPool, user_id: Uuid) -> AppResult<Value> {
    let account: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select to_jsonb(t) from ({EXPORT_ACCOUNT}) t"
    )))
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the account: {error}")))?;

    let mut document = serde_json::Map::new();
    document.insert("exported_at".to_string(), json!(Utc::now()));
    document.insert("account".to_string(), account);
    document.insert(
        "row_limit".to_string(),
        json!({
            "per_collection": EXPORT_MAX_ROWS,
            "note": "each collection is capped; `truncated` names the ones that hit the cap",
        }),
    );

    let mut truncated: Vec<&str> = Vec::new();
    for (key, statement) in EXPORT_COLLECTIONS {
        let rows = collection(pool, statement, user_id, key, &mut truncated).await?;
        document.insert((*key).to_string(), rows);
    }
    document.insert("truncated".to_string(), json!(truncated));

    Ok(Value::Object(document))
}

/// One collection, capped at [`EXPORT_MAX_ROWS`] and reported when it hit the cap.
///
/// `limit $2 + 1` and then a `pop` is how the cap is *detected* rather than inferred from an
/// equal count: a collection with exactly `EXPORT_MAX_ROWS` rows is not truncated, and a test
/// that asserted on `len() == cap` could not tell the two apart.
async fn collection(
    pool: &PgPool,
    statement: &str,
    user_id: Uuid,
    key: &'static str,
    truncated: &mut Vec<&'static str>,
) -> AppResult<Value> {
    // The `limit` goes inside the subquery, so it bounds the *rows* the aggregate sees. Outside
    // it, it would bound the aggregate — one row — and the export would silently be one array.
    let wrapped = format!(
        "select coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb) from ({statement} limit $2) t"
    );

    // The aggregate decodes as **one** `Value`, not a `Vec<Value>`: `jsonb_agg` returns a
    // single `jsonb` value (an array), and sqlx maps a `jsonb` column to one `Value` — asking
    // for a `Vec` here is a type mismatch at decode time, not an empty result.
    let aggregated: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(wrapped))
        .bind(user_id)
        .bind(EXPORT_MAX_ROWS + 1)
        .fetch_one(pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("reading the {key} collection: {error}"))
        })?;

    let mut rows = aggregated.as_array().cloned().unwrap_or_default();

    if rows.len() as i64 > EXPORT_MAX_ROWS {
        rows.truncate(EXPORT_MAX_ROWS as usize);
        truncated.push(key);
    }
    Ok(Value::Array(rows))
}

/// The Markdown document: the same rows, rendered for a person.
///
/// A separate renderer rather than "the JSON in a fenced block": the spec asks for Markdown
/// *and* JSON because they answer different questions — the JSON is for a machine, the
/// Markdown is for the student reading what we hold about them. Sections with no rows are
/// named as empty rather than omitted, so the document cannot read as "we hold nothing" when
/// it means "we did not look".
pub async fn export_markdown(pool: &PgPool, user_id: Uuid) -> AppResult<String> {
    let document = export_json(pool, user_id).await?;
    let truncated: Vec<String> = document["truncated"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("# Your Stvdy data\n\n");
    out.push_str(&format!(
        "Exported {}. Every collection is capped at {} rows; {}.\n\n",
        Utc::now().to_rfc3339(),
        EXPORT_MAX_ROWS,
        if truncated.is_empty() {
            "nothing was cut".to_string()
        } else {
            format!("these were cut: {}", truncated.join(", "))
        }
    ));

    if let Some(account) = document.get("account") {
        out.push_str("## Account\n\n");
        out.push_str(&format!(
            "- Email: {}\n- Plan: {}\n- Timezone: {}\n- Created: {}\n\n",
            string_field(account, "email"),
            string_field(account, "plan_id"),
            string_field(account, "timezone"),
            string_field(account, "created_at"),
        ));
    }

    for (key, _) in EXPORT_COLLECTIONS {
        let heading = heading_for(key);
        out.push_str(&format!("## {heading}\n\n"));
        let empty = Vec::new();
        let rows = document[key].as_array().unwrap_or(&empty);
        if rows.is_empty() {
            out.push_str("_None._\n\n");
            continue;
        }
        for row in rows {
            out.push_str(&format!("- {}\n", markdown_row(key, row)));
        }
        out.push('\n');
    }

    out.push_str(
        "## What is not here\n\n\
         Uploaded files and the text extracted from them stay in object storage and are not \
         part of this document; this export carries their metadata. Chat citations refer to \
         the sources above by id.\n",
    );
    Ok(out)
}

/// The label field for a collection's rows, and the two facts worth showing beside it.
///
/// Hand-written rather than derived from the row, because "which field is this row's name" is
/// a judgement about the domain, not a property of the JSON. A collection with no entry falls
/// back to the row's own `id`, which is honest: a row with no name still exists, and the
/// alternative (omitting it) would make the Markdown disagree with the JSON.
fn markdown_row(key: &str, row: &Value) -> String {
    let (label_field, meta): (&str, &[&str]) = match key {
        "workspaces" => ("name", &["created_at"]),
        "sources" => ("title", &["kind", "status", "file_size"]),
        "messages" => ("role", &["created_at", "workspace_id"]),
        "decks" => ("name", &["created_at"]),
        "flashcards" => ("front", &["due_at", "state"]),
        "quizzes" => ("title", &["status", "created_at"]),
        "reports" => ("title", &["status", "created_at"]),
        "cheatsheets" => ("title", &["status", "exam_date"]),
        "tasks" => ("title", &["status", "due_at", "priority"]),
        "goals" => ("title", &["target_date"]),
        "events" => ("title", &["kind", "starts_at", "ends_at"]),
        "reminders" => ("subject_key", &["kind", "fire_at", "status"]),
        "payments" => ("order_id", &["kind", "status", "amount_idr", "paid_at"]),
        "credit_ledger" => ("reason", &["delta", "bucket", "created_at"]),
        "usage_events" => ("action", &["credits", "created_at"]),
        "subscriptions" => ("plan_id", &["status", "current_period_end"]),
        "devices" => ("name", &["platform", "created_at", "revoked_at"]),
        "study_sessions" => ("started_at", &["duration_min"]),
        "flashcard_reviews" => ("rating", &["reviewed_at"]),
        "quiz_attempts" => ("score", &["created_at"]),
        "pending_actions" => ("tool", &["status", "created_at"]),
        "calendar_connections" => ("provider", &["status", "last_synced_at"]),
        "plan_proposals" => ("status", &["created_at"]),
        "syllabus_drafts" => ("status", &["created_at"]),
        "quiz_questions" => ("prompt", &["kind"]),
        "report_revisions" => ("replaced_at", &["report_id"]),
        "user_settings" => ("user_id", &["language", "theme", "updated_at"]),
        _ => ("id", &[]),
    };

    let label = string_field(row, label_field);
    let facts: Vec<String> = meta
        .iter()
        .filter_map(|field| {
            let value = string_field(row, field);
            (!value.is_empty()).then(|| format!("{field}: {value}"))
        })
        .collect();

    if facts.is_empty() {
        label
    } else {
        format!("{label} ({})", facts.join(", "))
    }
}

/// A row's field as display text, or an empty string when the field is absent or null.
///
/// A missing field renders as empty rather than as `null` or a panic: the Markdown is written
/// from rows whose schema this module does not enumerate, so a column renamed by a migration
/// must degrade to a less informative line rather than to a failed export.
fn string_field(row: &Value, field: &str) -> String {
    match row.get(field) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The heading a collection gets in the Markdown. The key is the JSON's key, so the two
/// documents cannot disagree about which collection is which.
fn heading_for(key: &str) -> String {
    let mut heading = String::with_capacity(key.len());
    let mut capitalise = true;
    for character in key.chars() {
        if character == '_' {
            heading.push(' ');
            capitalise = true;
        } else if capitalise {
            heading.extend(character.to_uppercase());
            capitalise = false;
        } else {
            heading.push(character);
        }
    }
    heading
}

/// The ICS document: the account's own calendar, plus one deadline per task that has a due
/// date — the same shape the feed carries (A241/A248), rendered through the same pure
/// renderer, so an exported calendar and a subscribed one cannot disagree about a `UID` or a
/// `SEQUENCE`.
///
/// `kind = 'external'` rows are excluded, and that is a decision rather than an oversight: an
/// `external` row is a copy of an event that lives in a calendar the student already has, so
/// exporting it would hand them a second copy of their own Google calendar under a `UID` their
/// client has never seen. The JSON collection still carries those rows in full.
pub async fn export_ics(state: &AppState, user_id: Uuid) -> AppResult<String> {
    use stvdy_core::calendar::feed::{FeedEvent, render_feed};

    let timezone = timezone_for(&state.pool, user_id).await?;

    let rows: Vec<ExportEventRow> = sqlx::query_as(
        "select e.id, e.kind, e.title, e.starts_at, e.ends_at,
                coalesce(e.created_at, e.updated_at) as created_at, e.updated_at
           from events e
          where e.user_id = $1 and e.deleted_at is null
            and e.kind = any($2)
         union all
         select t.id, 'deadline', t.title, t.due_at, t.due_at,
                coalesce(t.created_at, t.updated_at), t.updated_at
           from tasks t
          where t.user_id = $1 and t.deleted_at is null and t.due_at is not null
          order by 4 asc, 1 asc",
    )
    .bind(user_id)
    .bind(FEED_KINDS)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the export's events: {error}")))?;

    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(kind) = feed_kind(&row.kind) else {
            continue;
        };
        events.push(FeedEvent {
            id: row.id,
            kind,
            title: row.title,
            starts_at: row.starts_at,
            ends_at: row.ends_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            timezone,
        });
    }

    Ok(render_feed(&events))
}

/// One row of the export's `union all`. A struct rather than a tuple because seven columns
/// positionally is a shape where swapping two of them compiles.
#[derive(sqlx::FromRow)]
struct ExportEventRow {
    id: Uuid,
    kind: String,
    title: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// The kinds an exported calendar carries, the same set the feed carries (A241).
const FEED_KINDS: &[&str] = &["class", "exam", "study_block", "deadline"];

/// The column's value as the feed's kind. A row this build does not understand is dropped
/// rather than rendered under a kind the client would draw wrongly.
fn feed_kind(raw: &str) -> Option<stvdy_core::calendar::feed::FeedKind> {
    use stvdy_core::calendar::feed::FeedKind;
    match raw {
        "class" => Some(FeedKind::Class),
        "exam" => Some(FeedKind::Exam),
        "study_block" => Some(FeedKind::StudyBlock),
        "deadline" => Some(FeedKind::Deadline),
        _ => None,
    }
}

/// The account's IANA zone, through `chrono-tz` and never the machine's (A6).
///
/// The same fallback as `calendar::ics_out`: an unparseable value becomes the spec's default
/// zone, logged, rather than a failed export. A Jakarta deadline rendered in UTC is a day
/// early, and a student exporting their data is owed a document even when one column of their
/// account row is wrong.
async fn timezone_for(pool: &PgPool, user_id: Uuid) -> AppResult<chrono_tz::Tz> {
    let raw: String = sqlx::query_scalar("select timezone from users where id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the timezone: {error}")))?
        // A missing row is the same case as an unparseable one, and it is reached for an
        // account the purge has already anonymized — `timezone` is reset to the default then.
        .unwrap_or_else(|| "Asia/Jakarta".to_string());

    Ok(raw
        .parse::<chrono_tz::Tz>()
        .unwrap_or_else(|_| default_timezone(user_id, &raw)))
}

/// The spec's default zone (A6), used when the account's own value does not parse.
///
/// A typed constant rather than a string to re-parse: `chrono_tz` has no fallible step left once
/// the zone is a value, so the fallback cannot fail and needs no `expect` to say so.
fn default_timezone(user_id: Uuid, raw: &str) -> chrono_tz::Tz {
    tracing::warn!(%user_id, timezone = %raw, "unknown timezone; exporting in Asia/Jakarta");
    chrono_tz::Asia::Jakarta
}
