//! The four planner routes: ask for a plan, poll it, accept it, replan it.
//!
//! Planning is a **job, not a route** (A253). The effort estimate is a model call, so
//! `POST /goals/:id/plan` reserves the two credits, writes a `plan_proposals` row in status
//! `planning`, enqueues `plan_goal` and answers 202 with `{ proposal_id }`; `GET /plans/:id` is
//! how the client polls. The contrast with `create_outline` is deliberate and is the same
//! distinction A250 draws: one short call may run in a route, a whole-document call may not.
//!
//! Accept spends nothing: the two credits were charged at plan time (A264). Replan reserves
//! two more, because it is another model call.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use stvdy_core::entitlements::Feature;

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The action a plan generation is recorded and priced under: the matrix's `[credits]` key.
pub const PLAN_ACTION: &str = "plan_generation";

/// The job kind the two generating routes enqueue.
pub const PLAN_JOB: &str = "plan_goal";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/goals/{id}/plan", post(create_proposal))
        .route("/plans/{id}", get(proposal))
        .route("/plans/accept", post(accept))
        .route("/plan/replan", post(replan))
}

/// `POST /goals/:id/plan` — 202 `{ proposal_id }`, two credits reserved.
///
/// `VerifiedUser`, not `AuthUser`: this spends credits and then a model call, and A43 keeps
/// both behind a confirmed address. The order is the entitlements engine's — the count and the
/// charge commit with the reserving row, so a failure rolls the charge back with it and there
/// is no charge without a proposal.
pub async fn create_proposal(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(goal_id): Path<Uuid>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let user = user.0;
    reserve_and_enqueue(&state, &user, goal_id, None).await
}

/// `GET /plans/:id` — the poll. 404 for another account's proposal.
///
/// A proposal in any status is **200**: this endpoint's job is to report the proposal's state,
/// and a poll that turns `failed` into an HTTP error forces every client to treat a normal
/// outcome as an exception it must unwrap before it can show the message.
pub async fn proposal(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = load_proposal(&state, user.user_id, id).await?;
    Ok(Json(json!({ "proposal": row })))
}

/// `POST /plans/accept` — the write (A264).
///
/// Takes `VerifiedUser` (A359), like every route over the student's own study rows: the
/// credits were already spent at plan time and accepting spends nothing more, but "spends
/// nothing" is no longer the test — an unverified account has no workspace to accept a plan
/// into. A second accept of the same proposal is a 409 rather than a second set of events,
/// which is what makes the endpoint safe to retry.
pub async fn accept(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<crate::planner::apply::AcceptBody>,
) -> AppResult<Json<Value>> {
    let ids = crate::planner::apply::accept(&state, &user, body.proposal_id, body.blocks).await?;
    Ok(Json(json!({ "event_ids": ids })))
}

/// The replan body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplanBody {
    pub goal_id: Uuid,
}

/// `POST /plan/replan` — 202 `{ proposal_id }`, two credits reserved (A265).
///
/// A replan is `propose` again with more of its input decided: the goal's committed blocks are
/// split into `pinned` (not yet started, kept exactly) and `busy` (already started, subtracted)
/// by `planner::input_for`. The previous proposal is marked `superseded` in the same
/// transaction that writes the new one, so a client polling the old id is told what happened
/// rather than left with a proposal that will never change.
pub async fn replan(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<ReplanBody>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let user = user.0;
    reserve_and_enqueue(&state, &user, body.goal_id, Some(true)).await
}

/// The shared body of the two generating routes: reserve, write the proposal, enqueue.
///
/// The order is Phase 5's, which is the worked example (A220): `reserve_in` takes the
/// per-user advisory lock, decides against the matrix, charges, and runs the closure that
/// writes the reserving row — all in one transaction. The job is enqueued **after** the
/// commit, so a failure there leaves a proposal the student can see failing rather than a
/// charge with no trace.
async fn reserve_and_enqueue(
    state: &AppState,
    user: &AuthUser,
    goal_id: Uuid,
    replan: Option<bool>,
) -> AppResult<(StatusCode, Json<Value>)> {
    // The goal must be the caller's before anything is charged: a foreign or deleted goal is a
    // 404, and it must be a 404 rather than a 403 so the endpoint does not confirm that
    // someone else's goal id exists.
    crate::goals::owned_goal(state, user.user_id, goal_id).await?;

    let cost = state.config.plans.credits.plan_generation;
    let user_id = user.user_id;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a plan: {error}")))?;

    let (_, proposal_id) = crate::entitlements::reserve_in(
        &mut tx,
        state,
        user,
        // `Feature::PlanGeneration` is `planner != manual`, which is why the free plan's
        // refusal is a `plan_required` naming `pro` and why A257 needs no second rule.
        Some(Feature::PlanGeneration),
        PLAN_ACTION,
        cost,
        |conn, _usage_event_id| {
            Box::pin(async move {
                // A replan supersedes the goal's outstanding proposal in the same transaction
                // that writes the new one, so there is never a moment with two `ready`
                // proposals for one goal and a client polling the old id is told it moved on.
                if replan.is_some() {
                    sqlx::query(
                        "update plan_proposals set status = 'superseded', updated_at = now()
                          where goal_id = $1 and user_id = $2
                            and status in ('planning', 'ready')",
                    )
                    .bind(goal_id)
                    .bind(user_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| {
                        AppError::Internal(anyhow::anyhow!("superseding the old proposal: {error}"))
                    })?;
                }

                let id: Uuid = sqlx::query_scalar(
                    "insert into plan_proposals (user_id, goal_id, status, credits_spent)
                     values ($1, $2, 'planning', $3)
                     returning id",
                )
                .bind(user_id)
                .bind(goal_id)
                .bind(cost as i32)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("creating the proposal: {error}"))
                })?;
                Ok(id)
            })
        },
    )
    .await?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing the plan request: {error}"))
    })?;

    // Enqueued after the commit so a failure here leaves a proposal the student can see
    // failing rather than a charge with no trace. The refund arm is the same shape the quiz
    // route uses: the proposal is marked `failed` and the two credits come back, once.
    if let Err(error) = crate::jobs::enqueue(
        &state.pool,
        &state.jobs,
        PLAN_JOB,
        json!({ "proposal_id": proposal_id }),
        None,
    )
    .await
    {
        crate::jobs::handlers::plan::fail_and_refund(
            state,
            proposal_id,
            user_id,
            cost as i32,
            "the planning job could not be queued",
        )
        .await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "enqueueing the planning job: {error}"
        )));
    }

    tracing::info!(
        %proposal_id,
        %goal_id,
        user_id = %user.user_id,
        cost,
        "plan requested"
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "proposal_id": proposal_id })),
    ))
}

/// The proposal row, as `GET /plans/:id` serializes it.
#[derive(Debug, serde::Serialize)]
pub struct ProposalView {
    pub id: Uuid,
    pub goal_id: Uuid,
    pub status: String,
    pub proposal: Option<Value>,
    pub deficit_min: Option<i32>,
    pub warnings: Option<Value>,
    pub prompt_version: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The raw `plan_proposals` row, in the column order [`ProposalView`] destructures it.
type ProposalRow = (
    Uuid,
    Uuid,
    String,
    Option<Value>,
    Option<i32>,
    Option<Value>,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

async fn load_proposal(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<ProposalView> {
    let row: Option<ProposalRow> = sqlx::query_as(
        "select id, goal_id, status, proposal, deficit_min, warnings, prompt_version, error,
                created_at, updated_at
           from plan_proposals where id = $1 and user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading a proposal: {error}")))?;

    row.map(
        |(
            id,
            goal_id,
            status,
            proposal,
            deficit_min,
            warnings,
            prompt_version,
            error,
            created_at,
            updated_at,
        )| ProposalView {
            id,
            goal_id,
            status,
            proposal,
            deficit_min,
            warnings,
            prompt_version,
            error,
            created_at,
            updated_at,
        },
    )
    .ok_or(AppError::NotFound)
}
