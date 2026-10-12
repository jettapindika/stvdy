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

    /// The columns of this entity's table that travel to a client, in the order a
    /// pull ships them.
    ///
    /// This is the pull's leak surface, and it is a positive list on purpose: a
    /// column added to a table and not added here is *not* shipped, so the failure
    /// mode of forgetting is a missing field rather than a leaked one (A168).
    ///
    /// Three exclusions are load-bearing rather than tidy:
    ///   - `user_id` is the caller's own and redundant on every row.
    ///   - `sources.file_key` and `sources.preview_key` are internal object-storage
    ///     keys whose client-facing form is a signed URL (`GET /sources/:id/file`),
    ///     so shipping them would write a storage layout into every device's cache.
    ///   - a cheatsheet's `blocks_key`/`html_key`/`pdf_key` are the same kind of
    ///     internal key: the client fetches the document through its route, not by
    ///     reading the key out of the row.
    ///
    /// `updated_at` is included because the client needs it for tombstone handling
    /// and the "Last synced" display; `created_at` and `deleted_at` for the same
    /// reason. `user_settings` has no `id` column — the row *is* the user — so its
    /// list starts at the setting fields.
    pub fn payload_columns(self) -> &'static [&'static str] {
        match self {
            Entity::Workspace => &["id", "name", "created_at", "updated_at", "deleted_at"],
            Entity::Source => &[
                "id",
                "workspace_id",
                "kind",
                "title",
                "url",
                "file_size",
                "mime",
                "status",
                "error",
                "read_only",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Message => &[
                "id",
                "workspace_id",
                "role",
                "content",
                "citations",
                "used_web",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Deck => &[
                "id",
                "workspace_id",
                "title",
                "source_ids",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Flashcard => &[
                "id",
                "deck_id",
                "front",
                "back",
                "chunk_id",
                "ease",
                "interval_days",
                "due_at",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::FlashcardReview => &[
                "id",
                "flashcard_id",
                "rating",
                "reviewed_at",
                "device_id",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Quiz => &[
                "id",
                "workspace_id",
                "title",
                "difficulty",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::QuizQuestion => &[
                "id",
                "quiz_id",
                "kind",
                "prompt",
                "options",
                "answer",
                "explanation",
                "chunk_id",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::QuizAttempt => &[
                "id",
                "quiz_id",
                "question_id",
                "user_answer",
                "correct",
                "answered_at",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Report => &[
                "id",
                "workspace_id",
                "title",
                "outline",
                "body_md",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Cheatsheet => &[
                "id",
                "workspace_id",
                "title",
                "scope",
                "exam_date",
                "depth",
                "source_ids",
                "status",
                "error",
                "template_version",
                "page_count",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Event => &[
                "id",
                "workspace_id",
                "kind",
                "title",
                "starts_at",
                "ends_at",
                "external_id",
                "etag",
                "connection_id",
                "source",
                "task_id",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Task => &[
                "id",
                "workspace_id",
                "parent_id",
                "title",
                "notes",
                "due_at",
                "estimate_min",
                "priority",
                "status",
                // A196: the order a breakdown proposed, and the client's to set. Without it
                // in this list a push of it is silently dropped by `write_columns` and a
                // pull never ships it, so subtasks come back shuffled on the second device.
                "position",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::Goal => &[
                "id",
                "workspace_id",
                "title",
                "target_date",
                "source_ids",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::StudySession => &[
                "id",
                "workspace_id",
                "task_id",
                "started_at",
                "duration_min",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
            Entity::UserSettings => &[
                "study_start_hour",
                "study_end_hour",
                "max_study_min_per_day",
                "block_len_min",
                "break_min",
                "reminder_channels",
                // A202: a choice made on the phone is the choice the laptop uses, which is
                // what a synced settings row is for. Absent from this list, a push of any of
                // them is dropped by `write_columns` and a pull never ships them.
                "quiet_start_hour",
                "quiet_end_hour",
                "language",
                "theme",
                "updated_at",
            ],
            Entity::PendingAction => &[
                "id",
                "tool",
                "args",
                "status",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
        }
    }

    /// How this entity's mutations carry their data on the push side.
    ///
    /// The wire type is unchanged (`payload: Option<Value>`); this says which shape
    /// the value has, and it lives here so both ends read one copy (A176).
    pub fn payload_shape(self) -> PayloadShape {
        match self {
            // A task and an event are edited concurrently from several devices, so a
            // mutation carries the patch and the row the client edited from, which is
            // what makes the disjointness of two writers' changed sets computable
            // (A176/A177).
            Entity::Task | Entity::Event => PayloadShape::PatchWithBase,
            // Everything else is written whole: an append-only row is immutable, and
            // a settings row is a single account-wide record.
            _ => PayloadShape::Row,
        }
    }
}

/// The shape of a push mutation's `payload`, per entity (A176).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadShape {
    /// The full row.
    Row,
    /// `{"patch": {…}, "base": {…}|null}`: the changed keys and the row they were
    /// changed from, with `base: null` meaning create.
    PatchWithBase,
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
