//! The composite tier walk. Filled in by Phase 3 Task 3 Step 4.

use stvdy_core::traits::search::SearchHit;

/// What a composite search did, so one `usage_events` row can carry the tier that
/// answered plus the ones that did not (A3/A40).
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// The tier that returned results, if any did.
    pub answered_by: Option<String>,
    /// Tiers consulted and found empty, erroring, or unreachable. This is what
    /// makes silent degradation of the free path visible in the admin report.
    pub unresponsive: Vec<String>,
    /// Whether the results came from the cache, which decides whether the request
    /// is charged (A40).
    pub cache_hit: bool,
}

/// Placeholder until Task 3 lands.
pub struct CompositeSearch;
