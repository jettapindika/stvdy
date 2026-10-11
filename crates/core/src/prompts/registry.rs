//! Compiled-in prompt modules.
//!
//! `include_str!` means a missing or renamed prompt file is a build error, not
//! a runtime 500 in front of a user. The version string is the filename stem,
//! and it is what lands in `usage_events.prompt_version` so a quality
//! regression can be attributed to a prompt change rather than guessed at.

use super::VarKind;

/// The chat base prompt. Every chat and assistant turn is composed on top of
/// this text, never on inline prompt strings scattered through the server.
pub const BASE_V1: &str = include_str!("../../../../prompts/base.v1.md");

/// The version stored with every request composed from [`BASE_V1`].
pub const BASE_V1_VERSION: &str = "base.v1";

/// The shared strict-JSON preamble every task module is followed by.
pub const JSON_CONTRACT_V1: &str = include_str!("../../../../prompts/json_contract.v1.md");

/// The flashcards module.
pub const FLASHCARDS_V1: &str = include_str!("../../../../prompts/flashcards.v1.md");

/// What `{{output_language}}` means when the caller supplies nothing.
///
/// A study app's cards must be in the language of the student's material. There is
/// no language setting until Phase 9, and translating a deck into English would be
/// a silent content bug for every Indonesian student, so the default names the
/// material rather than picking a language.
pub const SOURCE_LANGUAGE: &str = "the same language as the source material (do not translate)";

/// The `base.v1.md` sections a strict-JSON task keeps.
///
/// The three dropped by name in `prompts/README.md` (`teaching_style`,
/// `tools_and_actions`, `planning_help`) all invite prose in a call that must
/// return one JSON object. `plans_and_billing` and `when_things_go_wrong` are
/// dropped for the same reason: both are instructions about what to *say* to the
/// user, and there is no user-facing turn here. The safety, integrity, privacy and
/// language sections stay: a source document can contain anything, and the model
/// must still refuse to reproduce what it must not.
pub const TASK_BASE_SECTIONS: &[&str] = &[
    "context",
    "user_context_data",
    "language_and_tone",
    "grounding_and_citations",
    "security_untrusted_content",
    "academic_integrity",
    "wellbeing_and_safety",
    "privacy_and_identity",
];

/// A strict-JSON task module: its text, its variables, and the base sections it
/// keeps. One variant per file under `prompts/`, added here and in [`ModuleId::ALL`]
/// when a phase adds a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleId {
    FlashcardsV1,
}

impl ModuleId {
    /// Every module, for the contract sweep in `tests/prompts.rs`.
    pub const ALL: [ModuleId; 1] = [ModuleId::FlashcardsV1];

    /// The version string logged to `usage_events.prompt_version`.
    pub fn version(self) -> &'static str {
        match self {
            ModuleId::FlashcardsV1 => "flashcards.v1",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            ModuleId::FlashcardsV1 => FLASHCARDS_V1,
        }
    }

    /// The variables the module's own text (and the JSON contract) uses.
    pub fn variables(self) -> &'static [(&'static str, VarKind)] {
        match self {
            ModuleId::FlashcardsV1 => &[
                ("count", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
        }
    }

    /// The `base.v1.md` sections this module keeps.
    pub fn base_sections(self) -> &'static [&'static str] {
        match self {
            ModuleId::FlashcardsV1 => TASK_BASE_SECTIONS,
        }
    }
}
