//! `poll_payment`: the backstop for a client that is not watching (A301).
//!
//! A thin shell over [`crate::billing::poll::poll_pending`], and deliberately thin: the
//! pass calls `billing::apply::apply_paid` — the *same* function the checkout screen's
//! poll calls — so a payment recovered by this sweep and one recovered by the client
//! cannot differ. All this module adds is the job's failure policy, which is the one thing
//! the pass cannot decide for itself.
//!
//! **A gateway that is down is transient; a payment the gateway does not know is not.** The
//! pass already writes `review` for the second (asking again would ask forever) and leaves
//! the row `pending` for the first, so the failure this handler can still see is a
//! database problem, and that is worth a retry.
//!
//! It is a backstop rather than the fast path: the bridge never pushes, so the checkout
//! screen's own poll asks the gateway on every tick, and this pass exists for the student
//! who paid and then closed the tab.

use crate::billing::poll;
use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, _job: &ClaimedJob) -> Result<(), Failure> {
    let started = std::time::Instant::now();
    let applied = poll::poll_pending(state)
        .await
        .map_err(|error| Failure::transient(format!("polling pending payments: {error}")))?;

    tracing::info!(
        applied,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "poll_payment finished"
    );
    Ok(())
}
