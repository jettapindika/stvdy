//! The queue: its order, its batch size, and the backoff a failure leaves behind.

use std::time::Duration;

use crate::proto::Mutation;

/// How many mutations one push carries.
///
/// A device that has been offline for a week has a long outbox, and one request must not
/// carry all of it: the engine pushes batch after batch until the queue is empty.
pub const BATCH: usize = 100;

/// The next push's worth of the outbox, in enqueue order.
pub fn next_batch(pending: &[Mutation], max: usize) -> &[Mutation] {
    &pending[..pending.len().min(max)]
}

/// Bounded exponential backoff for a pass that failed transiently.
///
/// The bound is what keeps a flapping network from becoming a hot loop against the
/// server, and the monotonicity is what keeps it from being a random one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retry {
    failures: u32,
}

impl Retry {
    /// The first failure's wait.
    pub const BASE: Duration = Duration::from_millis(500);
    /// The cap: long enough that an unreachable server is not hammered, short enough that
    /// a device which comes back does not wait for one.
    pub const MAX: Duration = Duration::from_secs(60);
    /// The largest doubling the cap needs; past it the cap is already binding.
    const MAX_SHIFT: u32 = 8;

    pub fn new() -> Self {
        Self { failures: 0 }
    }

    /// How many consecutive failures there have been since the last pass that reached the
    /// server.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Zero after a pass that reached the server; otherwise [`Retry::BASE`] doubling per
    /// consecutive failure, capped at [`Retry::MAX`].
    pub fn delay(&self) -> Duration {
        if self.failures == 0 {
            return Duration::ZERO;
        }
        let shift = (self.failures - 1).min(Self::MAX_SHIFT);
        Self::BASE.saturating_mul(1u32 << shift).min(Self::MAX)
    }

    /// Records one more failure.
    pub fn advance(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    /// Records a pass that reached the server.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}
