//! Compiled-in prompt modules.
//!
//! `include_str!` means a missing or renamed prompt file is a build error, not
//! a runtime 500 in front of a user. The version string is the filename stem,
//! and it is what lands in `usage_events.prompt_version` so a quality
//! regression can be attributed to a prompt change rather than guessed at.

/// The chat base prompt. Every chat and assistant turn is composed on top of
/// this text, never on inline prompt strings scattered through the server.
pub const BASE_V1: &str = include_str!("../../../../prompts/base.v1.md");

/// The version stored with every request composed from [`BASE_V1`].
pub const BASE_V1_VERSION: &str = "base.v1";
