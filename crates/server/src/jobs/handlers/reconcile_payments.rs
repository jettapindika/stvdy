//! `reconcile_payments`: the hourly sweep that compares our rows with the gateway's (A301).
//!
//! Two mismatches, both `review` rows rather than silent edits: a gateway transaction we
//! have no payment for, and one of ours the gateway does not know. A sweep that could
//! change a payment's status would be a second activation path with a schedule, which is
//! exactly what `billing::apply` exists to prevent — so this pass never writes a status.
//!
//! A gateway that cannot be reached is **not** a failure: the sweep learned nothing, the
//! next hour asks again, and flagging every payment during an outage would bury the real
//! mismatches in noise.

use crate::billing::poll;
use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, _job: &ClaimedJob) -> Result<(), Failure> {
    let started = std::time::Instant::now();
    let flagged = poll::reconcile(state)
        .await
        .map_err(|error| Failure::transient(format!("reconciling payments: {error}")))?;

    tracing::info!(
        flagged,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "reconcile_payments finished"
    );
    Ok(())
}
