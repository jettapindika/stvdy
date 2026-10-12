//! The agent's pure core: what a tool call is, and whether it may run.
//!
//! [`tools`] is the registry: the ten tools the spec names, each with a typed argument
//! contract and a JSON schema, and the [`tools::ToolKind`] split that says which of them
//! read and which of them write. The gate that acts on that split is
//! [`crate::agent::gate`]; it arrives with the decision point itself, and this module
//! re-exports it from there.
//!
//! Everything here is pure: no database, no clock, no network, no model. That is what
//! makes "may this run?" unit-testable, and it is what keeps the answer in one place
//! (A270/A271).

pub mod tools;

pub use tools::{
    EventKind, QuizDifficulty, QuizTypes, ToolArgs, ToolBounds, ToolError, ToolKind, ToolName,
    bounds, parse_args, specs,
};
