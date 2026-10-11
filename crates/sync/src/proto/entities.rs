//! Synced entities and the change-log record. Expanded in Task 10.

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
