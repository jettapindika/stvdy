//! `sync_calendar`: one connection, one pass.
//!
//! The kind is claimed by the same worker as everything else, and the payload is the
//! connection rather than the account: two Google accounts for one student are two rows and
//! therefore two jobs, so a pass that failed on one cannot starve the other.
//!
//! **A pass that could not reach the provider is not a job failure.** A Google grant that has
//! been revoked cannot be un-revoked by retrying, and a feed URL the guard refuses will be
//! refused again; both mark the connection and *succeed*, because a job that failed would
//! retry five times with backoff and record the same error five times (A247). What is
//! transient — a transport error, a 5xx — is a `Failure::transient`, and the retry is the
//! whole remedy.

use serde_json::Value;
use uuid::Uuid;

use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    // `user_id` first, through the shared accessor: a payload without one is a *permanent*
    // failure, and a handler that guessed would sync the wrong account's calendar.
    let user_id = job.user_id()?;
    let connection_id = connection_id_of(&job.payload)?;

    let Some(connection) = crate::calendar::sync::load(&state.pool, connection_id)
        .await
        .map_err(|error| Failure::transient(format!("loading the connection: {error}")))?
    else {
        // The row is gone (the student deleted it, or the account was purged). Not a failure:
        // no retry brings a deleted row back, and a job that failed would keep the worker busy
        // for half a minute over a row that no longer exists.
        tracing::info!(%connection_id, "a calendar sync ran for a connection that no longer exists");
        return Ok(());
    };

    // The payload's owner and the row's owner must agree. They always do — the enqueuer writes
    // both — so a mismatch is a corrupt row, and syncing it would write one account's classes
    // into another's `events`.
    if connection.user_id != user_id {
        return Err(Failure::permanent(
            "the connection belongs to a different account than the job payload names",
        ));
    }

    match connection.provider.as_str() {
        "ics" => {
            crate::calendar::sync::sync_ics(state, &connection)
                .await
                .map_err(|error| Failure::transient(format!("the ICS import failed: {error}")))?;
            Ok(())
        }
        // `sync_google` answers every non-retryable condition itself — a `410` re-lists fully
        // and succeeds, a refused grant revokes the connection and succeeds, a provider answer
        // this build cannot use is recorded and succeeds — so the only error that reaches here
        // is one a retry is worth, which is why it is transient rather than permanent.
        "google" => {
            crate::calendar::sync::sync_google(state, &connection)
                .await
                .map_err(|error| Failure::transient(format!("the Google sync failed: {error}")))?;
            Ok(())
        }
        // No such provider exists in this build. Permanent rather than transient, because no
        // retry invents a provider client — and the arm is not a refusal of `google`, which
        // the arm above answers.
        other => Err(Failure::permanent(format!(
            "no sync implementation for the {other} provider"
        ))),
    }
}

/// The `connection_id` a calendar job carries.
///
/// A separate parse from [`ClaimedJob::user_id`] because the failure message differs: this one
/// names the connection, which is what an operator reading `jobs.error` needs.
fn connection_id_of(payload: &Value) -> Result<Uuid, Failure> {
    payload
        .get("connection_id")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid connection_id"))
}
