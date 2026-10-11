use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Every synced entity. `table_name()` must equal the Postgres table name,
/// because `change_log.entity` stores `tg_table_name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entity {
    Workspace,
    Source,
    Message,
    Deck,
    Flashcard,
    FlashcardReview,
    Quiz,
    QuizQuestion,
    QuizAttempt,
    Report,
    Cheatsheet,
    Event,
    Task,
    Goal,
    StudySession,
    UserSettings,
    PendingAction,
}

impl Entity {
    pub const ALL: [Entity; 17] = [
        Entity::Workspace,
        Entity::Source,
        Entity::Message,
        Entity::Deck,
        Entity::Flashcard,
        Entity::FlashcardReview,
        Entity::Quiz,
        Entity::QuizQuestion,
        Entity::QuizAttempt,
        Entity::Report,
        Entity::Cheatsheet,
        Entity::Event,
        Entity::Task,
        Entity::Goal,
        Entity::StudySession,
        Entity::UserSettings,
        Entity::PendingAction,
    ];

    pub fn table_name(self) -> &'static str {
        match self {
            Entity::Workspace => "workspaces",
            Entity::Source => "sources",
            Entity::Message => "messages",
            Entity::Deck => "decks",
            Entity::Flashcard => "flashcards",
            Entity::FlashcardReview => "flashcard_reviews",
            Entity::Quiz => "quizzes",
            Entity::QuizQuestion => "quiz_questions",
            Entity::QuizAttempt => "quiz_attempts",
            Entity::Report => "reports",
            Entity::Cheatsheet => "cheatsheets",
            Entity::Event => "events",
            Entity::Task => "tasks",
            Entity::Goal => "goals",
            Entity::StudySession => "study_sessions",
            Entity::UserSettings => "user_settings",
            Entity::PendingAction => "pending_actions",
        }
    }

    pub fn from_table_name(table: &str) -> Option<Entity> {
        Entity::ALL.into_iter().find(|e| e.table_name() == table)
    }

    /// Dense 1..=17 rank used to order the initial sync so parents land before
    /// children. Lower is applied first.
    pub fn dependency_rank(self) -> u8 {
        match self {
            Entity::Workspace => 1,
            Entity::Source => 2,
            Entity::Deck => 3,
            Entity::Flashcard => 4,
            Entity::Quiz => 5,
            Entity::QuizQuestion => 6,
            Entity::Report => 7,
            // Cheatsheet depends on workspaces + sources only, but appending it
            // last keeps the dense 1..=N property the wire test asserts.
            Entity::Cheatsheet => 17,
            Entity::Task => 8,
            Entity::Goal => 9,
            Entity::Event => 10,
            Entity::StudySession => 11,
            Entity::Message => 12,
            Entity::FlashcardReview => 13,
            Entity::QuizAttempt => 14,
            Entity::UserSettings => 15,
            Entity::PendingAction => 16,
        }
    }

    /// Whether a client may write this entity through `POST /sync/push`.
    ///
    /// This is exactly the spec's offline-write scope. Everything else is
    /// written through its dedicated endpoint. Two entries are security relevant
    /// rather than tidy:
    ///   - `Message` is persisted by `POST /workspaces/:id/chat`; accepting a
    ///     push would skip the credit spend and let a client forge assistant
    ///     turns with fabricated citations.
    ///   - `Source` status/file_key/read_only are server-owned; a client must
    ///     not be able to mark a source `ready` or bypass storage quotas.
    pub fn is_client_writable(self) -> bool {
        matches!(
            self,
            Entity::Task
                | Entity::Event
                | Entity::FlashcardReview
                | Entity::QuizAttempt
                | Entity::StudySession
                | Entity::UserSettings
        )
    }

    /// Append-only entities accept only `Op::Upsert`; their rows are never
    /// tombstoned, and a duplicate id is ignored rather than overwriting.
    pub fn is_append_only(self) -> bool {
        matches!(
            self,
            Entity::FlashcardReview | Entity::QuizAttempt | Entity::StudySession
        )
    }

    /// Whether `op` is acceptable for this entity through `/sync/push`.
    pub fn accepts_push_op(self, op: Op) -> bool {
        if !self.is_client_writable() {
            return false;
        }
        match op {
            Op::Upsert => true,
            // Append-only rows are never deleted, and `user_settings` has no
            // tombstone column at all.
            Op::Delete => !self.is_append_only() && self != Entity::UserSettings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Upsert,
    Delete,
}

/// One row of `change_log` plus the entity's current state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub seq: i64,
    pub entity: Entity,
    pub id: Uuid,
    pub op: Op,
    /// `None` for tombstones.
    pub data: Option<Value>,
}
