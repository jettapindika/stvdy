//! Replanning: the same algorithm, with more of its input already decided.
//!
//! Contract step 2 subtracts `busy ∪ pinned`, so a pinned block occupies its slot and no new
//! block can overlap it; `propose` emits the pinned intervals as blocks so the client can
//! show them unchanged (A265). `replan` is that same function under the name the server's
//! replan route calls, so a replan can never drift from a first plan.

use super::model::{PlanProposal, PlannerInput};
use super::propose;

/// `propose`, with `PlannerInput::pinned` kept exactly. Everything else is identical: replan
/// is not a second algorithm (A265).
pub fn replan(input: &PlannerInput) -> PlanProposal {
    propose(input)
}
