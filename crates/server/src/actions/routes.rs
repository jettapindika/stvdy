//! The three action routes: list the proposals, confirm one, reject one.
//!
//! `POST /actions/:id/confirm` is the **only** thing in the product that turns a proposal
//! into a row (A271), and it is written so that fact is structural rather than a promise:
//!
//! 1. the plan decision runs first (`Feature::AgentAction`, A272) so an account that cannot
//!    confirm is refused before the row is even looked up;
//! 2. the claim ([`super::claim`]) is a guarded state flip inside the execution's own
//!    transaction, so two taps produce one row (A275);
//! 3. the execution ([`super::execute`]) calls the REST routes' own service functions, so a
//!    confirmed task is the row `POST /tasks` would have written (A276).
//!
//! Reject is deliberately **not** gated: it spends nothing and frees the student from a
//! card, and gating it would leave a downgraded account unable to dismiss a proposal it can
//! no longer confirm.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use uuid::Uuid;

use stvdy_core::agent::tools::{ToolName, parse_args};
use stvdy_core::entitlements::Feature;

use crate::actions::{
    ActionRow, ActionView, RESOLVED_VISIBLE_HOURS, Resolution, claim, execute, expire_stale, find,
    record_error, record_result,
};
use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The most proposals one list answers. The confirmation surface is a screen, not a queue:
/// A278 already caps a turn at three, so a page of fifty is several turns' worth.
const MAX_PENDING: i64 = 50;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/actions/pending", get(pending))
        .route("/actions/{id}/confirm", post(confirm))
        .route("/actions/{id}/reject", post(reject))
}

/// `GET /actions/pending`: what is waiting to be decided, plus what was decided in the last
/// day.
///
/// The resolved window is what lets a card confirmed on device A read "Confirmed" on device
/// B instead of vanishing: the exit gate's cross-device half is a *read* fact, and without
/// this window the second device's only evidence would be a card that disappeared.
///
/// The lazy expiry runs first (A274). A `GET` that writes is not a smell here — the
/// alternative is a scheduler pass over every account's pending actions and the roadmap § 8
/// job-kind list has no expiry kind — and the claim re-checks `expires_at` inside its own
/// transaction, so this is the convenience and not the enforcement.
pub async fn pending(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<serde_json::Value>> {
    expire_stale(&state.pool, user.user_id).await?;

    let sql = format!(
        "select {} from pending_actions
          where user_id = $1 and deleted_at is null
            and (status = 'pending'
                 or (resolved_at is not null and resolved_at > now() - ($2 * interval '1 hour')))
          order by created_at desc nulls last, id desc
          limit $3",
        super::ACTION_COLUMNS
    );
    let rows: Vec<ActionRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user.user_id)
        .bind(RESOLVED_VISIBLE_HOURS)
        .bind(MAX_PENDING)
        .fetch_all(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("listing proposals: {error}")))?;

    let actions: Vec<ActionView> = rows.into_iter().map(ActionView::from).collect();
    Ok(Json(serde_json::json!({ "actions": actions })))
}

/// The confirm body.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmBody {
    /// The Edit half of Confirm / Edit / Dismiss. `None` confirms what was proposed.
    ///
    /// `Some` is re-parsed through the same [`parse_args`] the proposal went through, so an
    /// edit cannot produce arguments a proposal could not (A271/A276): the tool's bounds,
    /// the field rules and the closed `additionalProperties` all apply again. The edit can
    /// therefore only ever *narrow* what the model proposed, never widen it.
    #[serde(default)]
    pub args: Option<serde_json::Value>,
}

/// `POST /actions/:id/confirm` — the one place a proposal becomes a row.
///
/// The order is the contract: plan, row, claim, execute, record, commit, and only then any
/// work that could not be part of the transaction (a generation job's enqueue). Every step
/// that can fail before the claim leaves the proposal `pending`, so the student can retry.
pub async fn confirm(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<serde_json::Value>> {
    // The plan decision first, and before the row is read: an account without
    // `AgentAction` gets `403 plan_required` whatever the id names, which is what makes the
    // route sweep's driver arm need no per-row body (A272).
    crate::entitlements::require(&state, &user, Feature::AgentAction).await?;

    let Some(row) = find(&state.pool, user.user_id, id).await? else {
        // Another account's proposal is a 404, never a 403: a 403 would confirm the id
        // exists, and the row is not the caller's to learn about.
        return Err(AppError::NotFound);
    };

    let tool = ToolName::from_str(&row.tool).ok_or_else(|| {
        // Unreachable through the API: only the agent writes this column and it writes
        // `ToolName::as_str()`. It is a 500 rather than a 400 because the client cannot
        // have caused it and cannot fix it.
        AppError::Internal(anyhow::anyhow!(
            "pending action {id} names a tool that is not in the registry: {}",
            row.tool
        ))
    })?;

    // The edit, or what the model proposed, through the one parser either way. An invalid
    // edit is a 400 naming the field and the row stays `pending`.
    let proposed = body.args.as_ref().unwrap_or(&row.args);
    let args =
        parse_args(tool, proposed).map_err(|error| AppError::Validation(error.to_string()))?;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a confirm: {error}")))?;

    let claimed = claim(
        &mut tx,
        user.user_id,
        id,
        user.device_id,
        Resolution::Confirmed,
    )
    .await?;
    if claimed.is_none() {
        // Zero rows from the guarded update: someone else resolved it first, or it expired
        // between the read above and here. Either way nothing was written, and the body
        // names the current status so the client's remedy is to re-read the row (A275).
        let current = find(&state.pool, user.user_id, id).await?;
        let status = current
            .map(|row| row.status)
            .unwrap_or_else(|| "unknown".to_string());
        return Err(AppError::Conflict(format!(
            "this proposal is already {status}"
        )));
    }

    // Inside the claim's transaction (A275): a failure here rolls the claim back, so a
    // `confirmed` row can never be a row that did nothing.
    let executed = execute::run(&mut tx, &state, &user, tool, &args).await?;
    record_result(&mut tx, user.user_id, id, &executed.result).await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a confirm: {error}")))?;

    // After the commit, because a job row and the work it names cannot be in the same
    // transaction: the enqueue is what makes the deck generate, and a rollback of the
    // claim would leave a job for a deck that does not exist. A failure here is the one
    // case the transaction cannot cover, so it is recorded on the row rather than hidden —
    // the proposal *was* resolved and the row *was* written, and the card on every device
    // then says why the work did not start.
    if let Some(deferred) = executed.deferred
        && let Err(error) = execute::queue(&state, deferred).await
    {
        record_error(&state.pool, user.user_id, id, &error.to_string()).await?;
        return Err(error);
    }

    tracing::info!(
        pending_action_id = %id,
        tool = tool.as_str(),
        device_id = %user.device_id,
        "a proposal was confirmed"
    );

    // Read back rather than assembled from the claim's `returning`: `resolved_at` is the
    // database's `now()`, and a view built from the process clock would report an instant
    // that is not the one stored.
    let stored = find(&state.pool, user.user_id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(
        serde_json::json!({ "action": ActionView::from(stored) }),
    ))
}

/// `POST /actions/:id/reject`. 200 on the transition, 404 for another account's row, 409
/// for one that is not pending (including an expired one).
///
/// Single-winner like the confirm, and for the same reason: two devices dismissing the same
/// card must agree about what happened, and the second one has to be told rather than
/// silently succeeding.
pub async fn reject(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    if find(&state.pool, user.user_id, id).await?.is_none() {
        return Err(AppError::NotFound);
    }

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a reject: {error}")))?;

    let Some(claimed) = claim(
        &mut tx,
        user.user_id,
        id,
        user.device_id,
        Resolution::Rejected,
    )
    .await?
    else {
        let current = find(&state.pool, user.user_id, id).await?;
        let status = current
            .map(|row| row.status)
            .unwrap_or_else(|| "unknown".to_string());
        return Err(AppError::Conflict(format!(
            "this proposal is already {status}"
        )));
    };

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a reject: {error}")))?;

    Ok(Json(
        serde_json::json!({ "action": ActionView::from(claimed) }),
    ))
}
