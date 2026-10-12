//! `plan_goal`: the one model call, then the pure planner, then the row.
//!
//! The call estimates minutes and nothing else (`planner_effort.v1.md` says so in its own
//! text, and `reconcile` refuses a reply that answers a scheduling question anyway). The
//! placement is `stvdy_core::planner::propose`, which has no clock and no IO.
//!
//! `now` is read **once**, inside `planner::input_for`, and put in the `PlannerInput`; nothing
//! below reads a clock. That is what makes a proposal reproducible against a fixed instant,
//! which is what contract § 6 means by "same input ⇒ same output".

use sqlx::Row;
use uuid::Uuid;

use crate::jobs::{self, ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let proposal_id: Uuid = job
        .payload
        .get("proposal_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid proposal_id"))?;

    let row = sqlx::query(
        "select id, user_id, goal_id, status, credits_spent
           from plan_proposals where id = $1",
    )
    .bind(proposal_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the proposal: {error}")))?;

    let Some(row) = row else {
        // Deleted between enqueue and claim: nothing to do, and not an error.
        return Ok(());
    };

    // Idempotent: a retried job whose first attempt already wrote the proposal must not write
    // a second plan, and a proposal the enqueue path already failed must not be resurrected
    // into `ready`. The status is the marker.
    let status: String = row.get("status");
    if status != "planning" {
        return Ok(());
    }
    let user_id: Uuid = row.get("user_id");
    let goal_id: Uuid = row.get("goal_id");
    let credits_spent: i32 = row.get("credits_spent");

    let input = match crate::planner::input_for(state, user_id, goal_id).await {
        Ok(input) => input,
        Err(failure) => {
            // A permanent failure — a deleted goal, a missing provider, a reply `reconcile`
            // refused twice — has no retry left that could change it, so the proposal is
            // marked failed and the charge comes back. A transient one leaves the row
            // `planning` for the worker's retry.
            let final_attempt = !failure.retryable || job.attempts >= jobs::MAX_ATTEMPTS;
            if final_attempt {
                fail_and_refund(state, proposal_id, user_id, credits_spent, &failure.message).await;
            }
            return Err(failure);
        }
    };

    // The pure planner. No clock, no IO, no database: a value in, a value out.
    let proposal = stvdy_core::planner::propose(&input);

    // `deficit_min` and `warnings` are lifted out of the serialized proposal because the list
    // screen reads them without parsing the whole thing, and because a query for "which
    // proposals could not fit" is a query.
    let proposal_json = serde_json::to_value(&proposal)
        .map_err(|error| Failure::permanent(format!("serializing the proposal: {error}")))?;
    let warnings = serde_json::to_value(&proposal.warnings)
        .map_err(|error| Failure::permanent(format!("serializing the warnings: {error}")))?;

    let updated = sqlx::query(
        "update plan_proposals
            set status = 'ready', proposal = $2, deficit_min = $3, warnings = $4,
                updated_at = now()
          where id = $1 and status = 'planning'",
    )
    .bind(proposal_id)
    .bind(&proposal_json)
    .bind(proposal.deficit_min as i32)
    .bind(&warnings)
    .execute(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("storing the proposal: {error}")))?;

    if updated.rows_affected() == 0 {
        // Another attempt won the race and already stored it; nothing to do.
        return Ok(());
    }

    tracing::info!(
        %proposal_id,
        %goal_id,
        blocks = proposal.blocks.len(),
        deficit_min = proposal.deficit_min,
        "plan ready"
    );
    Ok(())
}

/// Marks the proposal failed and gives back exactly what was charged, once.
///
/// The guard is `credits_spent > 0`, consumed by the claim that zeroes it: a second call (a
/// retried job, the enqueue-failure path in the route, or the boot sweep) refunds nothing
/// because there is nothing left to refund.
///
/// The proposal is left terminal in every case. When the refund insert cannot be written, an
/// infrastructure failure rather than a business one, the charge is kept (`credits_spent > 0`,
/// "charged and not delivered, refund pending") and the status change still commits, because
/// this is only ever called on the final attempt: a rollback would leave the proposal
/// `planning` with no retry left to resolve it. `credits::refund_pending(PlanProposal)`
/// recovers the charge at the next boot.
pub async fn fail_and_refund(
    state: &AppState,
    proposal_id: Uuid,
    user_id: Uuid,
    credits: i32,
    message: &str,
) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %proposal_id, "could not begin the plan failure transaction");
            return;
        }
    };

    let claimed: Option<i32> = sqlx::query_scalar(
        "update plan_proposals
            set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where id = $1 and credits_spent > 0
         returning credits_spent",
    )
    .bind(proposal_id)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if claimed.is_none() {
        // Either the proposal was already failed, or it never had a charge. Make sure the
        // status is right either way, then stop.
        if let Err(error) = sqlx::query(
            "update plan_proposals set status = 'failed', error = $2, updated_at = now()
              where id = $1 and status = 'planning'",
        )
        .bind(proposal_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %proposal_id, "could not mark the proposal failed");
        }
    }

    if claimed.is_some() {
        // The ledger row is issued inside the same transaction as the claim that zeroed
        // `credits_spent`, so a crash between the two cannot charge a student for a plan they
        // never received with no retry left to fix it. The refund runs under a savepoint
        // because Postgres aborts a transaction on any failed statement: without it the
        // recovery below could not run, and the only way to close the transaction would be a
        // rollback that also undid the status change.
        if let Err(error) = sqlx::query("savepoint refund").execute(&mut *tx).await {
            tracing::error!(%error, %proposal_id, "could not open the refund savepoint");
            return;
        }

        if let Err(error) =
            crate::credits::refund_in(&mut tx, user_id, i64::from(credits), proposal_id, "refund")
                .await
        {
            // A refund that cannot be written is an infrastructure failure, and the job that
            // reached here has no retry left. Rolling the whole transaction back would leave
            // the proposal `planning` forever, charged, and polled by the client without end,
            // so the status change is committed with `credits_spent` restored to the charged
            // amount: the proposal is terminal, and the boot sweep recovers the charge.
            tracing::error!(%error, %proposal_id, "could not refund a failed plan; the charge stays pending for recovery");

            // Undo only the failed insert attempt. The claim that marked the proposal failed
            // stands, and `credits_spent` is put back explicitly rather than by rolling the
            // claim back: `credits_spent > 0` is then exactly "charged and not delivered,
            // refund pending", which is what the boot sweep looks for.
            if let Err(rollback_error) = sqlx::query("rollback to savepoint refund")
                .execute(&mut *tx)
                .await
            {
                tracing::error!(%rollback_error, %proposal_id, "could not roll back the failed refund insert");
                return;
            }
            if let Err(restore_error) =
                sqlx::query("update plan_proposals set credits_spent = $2 where id = $1")
                    .bind(proposal_id)
                    .bind(credits)
                    .execute(&mut *tx)
                    .await
            {
                tracing::error!(%restore_error, %proposal_id, "could not restore the pending charge");
                return;
            }
            if let Err(commit_error) = tx.commit().await {
                tracing::error!(%commit_error, %proposal_id, "could not commit the plan failure");
                return;
            }
            return;
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %proposal_id, "could not commit the plan failure");
        return;
    }

    if claimed.is_some() {
        tracing::warn!(%proposal_id, credits, "plan generation failed permanently; credits refunded");
    }
}
