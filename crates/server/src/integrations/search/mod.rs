//! The search tiers and the composite that walks them (A3).
//!
//! Each tier is its own module and is skipped when unconfigured, so a deployment
//! without a key for one does not fail at the first query with a 401.

pub mod academic;
pub mod composite;
pub mod exa;
pub mod searxng;
pub mod tavily;

pub use composite::{CompositeSearch, SearchOutcome};
