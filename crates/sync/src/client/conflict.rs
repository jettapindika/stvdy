//! The conflict table, as data.
//!
//! The roadmap's table is normative and this is its single encoding: the server's apply
//! reads it, the client's reaction reads it, and neither can drift into a second
//! convention (A175). The *decision* is still the server's (A150) — the client only reacts
//! to a `MutationResult`.

use crate::proto::Entity;

/// What the conflict rules say about one entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Insert; a duplicate id is ignored rather than overwriting. `flashcard_reviews`,
    /// `quiz_attempts`, `study_sessions`.
    AppendOnly,
    /// Record-level last-write-wins on server receive time, with a field-level merge when
    /// the two writers' changed field sets are disjoint. `tasks`, `events`.
    LwwWithFieldMerge,
    /// Record-level last-write-wins. `user_settings`.
    Lww,
}

/// The rule for one entity.
///
/// Exhaustive over [`Entity`], so a new entity cannot join the protocol without a rule.
/// The server-owned entities are never *applied* — `crate::proto::Entity::is_client_writable`
/// is the allowlist the server checks first — but they need an arm, and `Lww` is the
/// answer that changes nothing if one ever did reach an apply: it overwrites rather than
/// merging, which is what a server-owned row would want anyway.
pub fn rule_for(entity: Entity) -> Rule {
    match entity {
        Entity::FlashcardReview | Entity::QuizAttempt | Entity::StudySession => Rule::AppendOnly,
        Entity::Task | Entity::Event => Rule::LwwWithFieldMerge,
        Entity::Workspace
        | Entity::Source
        | Entity::Message
        | Entity::Deck
        | Entity::Flashcard
        | Entity::Quiz
        | Entity::QuizQuestion
        | Entity::Report
        | Entity::Cheatsheet
        | Entity::Goal
        | Entity::UserSettings
        | Entity::PendingAction => Rule::Lww,
    }
}
