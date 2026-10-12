//! The ten tools the assistant may call, and the argument contract each one has.
//!
//! This module is pure: a name and a JSON object in, a typed call or a refusal out. No
//! database, no clock, no model. That is what makes the registry testable, and it is what
//! lets the gate ([`super::gate`]) decide *whether* a call may run without knowing what
//! running it means.
//!
//! **The read/write split is the safety property.** [`ToolKind::Read`] tools may execute
//! inline; a [`ToolKind::Write`] tool only ever becomes a proposal a human confirms
//! (A270/A271). The split is a property of the type rather than a convention a caller
//! remembers: [`ToolName::kind`] is the one place it is decided, it is a `matches!`, and
//! every decision point matches on [`ToolKind`].
//!
//! **The registry and the prompt are one contract.** `prompts/base.v2.md`'s
//! `<tools_and_actions>` section names exactly these ten tools, and
//! `crates/core/tests/agent_tools.rs` fails if the two drift. Adding an eleventh tool is
//! therefore a contract change and not a code change (A270).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::tasks::{MAX_ESTIMATE_MIN, MAX_PRIORITY, MIN_PRIORITY, TaskError, TaskStatus};
use crate::traits::ToolSpec;

/// The most flashcards one proposal may ask for.
///
/// Tighter than the deck route's own ceiling (`flashcards::routes::MAX_REQUESTED_CARDS`),
/// because an agent proposal is one card on a confirmation surface: the prompt tells the
/// model to keep proposals small, and this is the bound that makes "small" enforceable
/// rather than advisory.
pub const MAX_FLASHCARDS: u32 = 30;

/// The most questions one proposed quiz may ask for.
pub const MAX_QUIZ_QUESTIONS: u32 = 20;

/// The most rows any read tool returns. A list is context for the model, not a screen:
/// a model handed 10_000 tasks spends its window on rows nobody asked about.
pub const MAX_LIST: u32 = 50;

/// The longest study block a proposal may schedule, and the shortest.
///
/// Four hours is one sitting, and five minutes is the shortest block worth a calendar row.
/// These are the block's bounds and not the event's: an event may be an exam, and an exam
/// is longer than a study session.
pub const MAX_SLOT_MINUTES: u32 = 240;
pub const MIN_SLOT_MINUTES: u32 = 5;

/// The horizon `get_upcoming_deadlines` may look over, in days.
pub const MAX_DEADLINE_DAYS: u32 = 60;

/// The longest event `create_event` may write, in minutes.
pub const MAX_EVENT_MINUTES: u32 = 8 * 60;

/// The `limit` a read uses when the call does not name one.
pub const DEFAULT_LIMIT: u32 = 20;

/// The horizon `get_upcoming_deadlines` uses when the call does not name one.
pub const DEFAULT_DEADLINE_DAYS: u32 = 7;

/// The priority a task takes when the call does not name one.
///
/// The column's own default (`migrations/0012_...sql`), used only where a value is needed
/// to run [`crate::tasks::validate`] over the fields a call *did* name: it is in range by
/// construction, so it never decides anything.
const DEFAULT_PRIORITY: i32 = 2;

/// Whether a tool reads or writes. The whole phase turns on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Runs immediately; its result goes back to the model.
    Read,
    /// Never runs: it becomes a `pending_actions` row and waits for a human (A271).
    Write,
}

/// The tools the spec names, in the order `prompts/base.v2.md` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    CreateTask,
    UpdateTask,
    ListTasks,
    CreateEvent,
    FindFreeSlots,
    ScheduleStudyBlock,
    CreateFlashcards,
    CreateQuiz,
    GetDueFlashcards,
    GetUpcomingDeadlines,
}

impl ToolName {
    /// The spec's list, in the order `prompts/base.v2.md` names it.
    pub const ALL: [ToolName; 10] = [
        ToolName::CreateTask,
        ToolName::UpdateTask,
        ToolName::ListTasks,
        ToolName::CreateEvent,
        ToolName::FindFreeSlots,
        ToolName::ScheduleStudyBlock,
        ToolName::CreateFlashcards,
        ToolName::CreateQuiz,
        ToolName::GetDueFlashcards,
        ToolName::GetUpcomingDeadlines,
    ];

    /// The name on the wire, which is the name in the prompt and the name stored in
    /// `pending_actions.tool`.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolName::CreateTask => "create_task",
            ToolName::UpdateTask => "update_task",
            ToolName::ListTasks => "list_tasks",
            ToolName::CreateEvent => "create_event",
            ToolName::FindFreeSlots => "find_free_slots",
            ToolName::ScheduleStudyBlock => "schedule_study_block",
            ToolName::CreateFlashcards => "create_flashcards",
            ToolName::CreateQuiz => "create_quiz",
            ToolName::GetDueFlashcards => "get_due_flashcards",
            ToolName::GetUpcomingDeadlines => "get_upcoming_deadlines",
        }
    }

    /// The tool a name means, or `None` for a name the registry does not have.
    ///
    /// `Option` rather than `Result` because an unknown name is not an error to propagate:
    /// a model that hallucinated `confirm_action` gets a refusal it can read (A271), and
    /// the gate is the one that decides that. The `allow` is the price of the pinned
    /// signature — the standard trait's `Result` would make every caller map the miss it
    /// is going to handle anyway.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(name: &str) -> Option<ToolName> {
        ToolName::ALL
            .iter()
            .copied()
            .find(|tool| tool.as_str() == name)
    }

    /// Whether this tool reads or writes. The only place the split is decided.
    pub fn kind(self) -> ToolKind {
        match self {
            ToolName::ListTasks
            | ToolName::FindFreeSlots
            | ToolName::GetDueFlashcards
            | ToolName::GetUpcomingDeadlines => ToolKind::Read,
            ToolName::CreateTask
            | ToolName::UpdateTask
            | ToolName::CreateEvent
            | ToolName::ScheduleStudyBlock
            | ToolName::CreateFlashcards
            | ToolName::CreateQuiz => ToolKind::Write,
        }
    }

    /// `true` for a tool that may only propose.
    pub fn is_write(self) -> bool {
        self.kind() == ToolKind::Write
    }
}

/// The event kinds a proposal may write.
///
/// `external` is deliberately absent: that kind marks a row imported from a calendar feed
/// (`events.source` is `google` or `ics`), and a tool that could write one would put a
/// fabricated import in the student's calendar — a row no feed would ever reconcile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Class,
    Exam,
    StudyBlock,
    Deadline,
}

impl EventKind {
    /// Every kind a proposal may name, which is also the schema's `enum`.
    pub const ALL: [EventKind; 4] = [
        EventKind::Class,
        EventKind::Exam,
        EventKind::StudyBlock,
        EventKind::Deadline,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Class => "class",
            EventKind::Exam => "exam",
            EventKind::StudyBlock => "study_block",
            EventKind::Deadline => "deadline",
        }
    }
}

/// The quiz difficulties the generator accepts.
///
/// The set is the contract: `crates/server/src/quizzes/routes.rs::DIFFICULTIES` is the
/// enforcement for the REST path and this is the same three values, so a call that reaches
/// a proposal is one the route would also accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuizDifficulty {
    Easy,
    Medium,
    Hard,
}

impl QuizDifficulty {
    pub const ALL: [QuizDifficulty; 3] = [
        QuizDifficulty::Easy,
        QuizDifficulty::Medium,
        QuizDifficulty::Hard,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            QuizDifficulty::Easy => "easy",
            QuizDifficulty::Medium => "medium",
            QuizDifficulty::Hard => "hard",
        }
    }
}

/// The question types the generator accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuizTypes {
    Mcq,
    Short,
    Mixed,
}

impl QuizTypes {
    pub const ALL: [QuizTypes; 3] = [QuizTypes::Mcq, QuizTypes::Short, QuizTypes::Mixed];

    pub fn as_str(self) -> &'static str {
        match self {
            QuizTypes::Mcq => "mcq",
            QuizTypes::Short => "short",
            QuizTypes::Mixed => "mixed",
        }
    }
}

/// The bounds every tool's arguments are held to, in one place.
///
/// The checks below read these and so do the JSON schemas, so the number the model is told
/// and the number it is refused by are the same number. A bound that lived only in the
/// schema would be a lie the parser never enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolBounds {
    pub max_flashcards: u32,
    pub max_quiz_questions: u32,
    pub max_list: u32,
    pub max_slot_minutes: u32,
    pub min_slot_minutes: u32,
    pub max_deadline_days: u32,
    pub max_event_minutes: u32,
}

static BOUNDS: ToolBounds = ToolBounds {
    max_flashcards: MAX_FLASHCARDS,
    max_quiz_questions: MAX_QUIZ_QUESTIONS,
    max_list: MAX_LIST,
    max_slot_minutes: MAX_SLOT_MINUTES,
    min_slot_minutes: MIN_SLOT_MINUTES,
    max_deadline_days: MAX_DEADLINE_DAYS,
    max_event_minutes: MAX_EVENT_MINUTES,
};

/// The bounds, for the checks and the schemas that share them.
pub fn bounds() -> &'static ToolBounds {
    &BOUNDS
}

/// A call whose arguments do not satisfy the tool's contract.
///
/// One variant, because the caller's next step is the same for all of them: the message
/// becomes a `Refusal::Invalid` and is handed to the model, which is why every message
/// names the field and the bound rather than the type.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Invalid(String),
}

/// The `create_task` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTaskArgs {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(
        default,
        deserialize_with = "due_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_min: Option<i32>,
    /// `1` high, `2` normal, `3` low, as `tasks.priority` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
    /// The goal the task serves, when the student named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<Uuid>,
}

/// The `update_task` arguments. Every field but the id is optional, and at least one has to
/// be named: a proposal that changes nothing is a confirmation the student cannot use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTaskArgs {
    pub task_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(
        default,
        deserialize_with = "due_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_min: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
}

/// The `list_tasks` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListTasksArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(
        default,
        deserialize_with = "due_before",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_before: Option<DateTime<Utc>>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// The `create_event` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEventArgs {
    pub title: String,
    pub kind: EventKind,
    #[serde(deserialize_with = "starts_at")]
    pub starts_at: DateTime<Utc>,
    #[serde(deserialize_with = "ends_at")]
    pub ends_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Uuid>,
}

/// The `find_free_slots` arguments. A question about the calendar, not a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindFreeSlotsArgs {
    #[serde(deserialize_with = "from")]
    pub from: DateTime<Utc>,
    #[serde(deserialize_with = "to")]
    pub to: DateTime<Utc>,
    /// How long the slot has to be. The block bounds, because this answers "when can I
    /// study", which is the same question `schedule_study_block` answers.
    pub duration_min: u32,
}

/// The `schedule_study_block` arguments.
///
/// The block's end is derived from its start and its length rather than named: two
/// timestamps can disagree, and a model that sent `ends_at` before `starts_at` would be
/// proposing a block with a negative length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleStudyBlockArgs {
    pub title: String,
    #[serde(deserialize_with = "starts_at")]
    pub starts_at: DateTime<Utc>,
    pub duration_min: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
}

/// The `create_flashcards` arguments. The workspace is required because a deck belongs to
/// one (`decks.workspace_id` is `not null`), and the proposal outlives the chat that made
/// it: the confirm may happen on another device, in another workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFlashcardsArgs {
    pub title: String,
    pub workspace_id: Uuid,
    pub source_ids: Vec<Uuid>,
    pub count: u32,
}

/// The `create_quiz` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateQuizArgs {
    pub title: String,
    pub workspace_id: Uuid,
    pub source_ids: Vec<Uuid>,
    pub count: u32,
    #[serde(default = "default_difficulty")]
    pub difficulty: QuizDifficulty,
    #[serde(default = "default_quiz_types")]
    pub types: QuizTypes,
}

/// The `get_due_flashcards` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetDueFlashcardsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deck_id: Option<Uuid>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// The `get_upcoming_deadlines` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetUpcomingDeadlinesArgs {
    #[serde(default = "default_deadline_days")]
    pub days: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// One typed argument contract per tool.
///
/// Untagged, because the variant *is* the arguments: the proposal stores the object the
/// model produced (`pending_actions.args`), and an enum wrapper in that column would be a
/// shape no reader wants. Deserialisation is not derived — the variant is chosen by the
/// call's tool name, which is not in the object — so [`parse_args`] is the only way in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ToolArgs {
    CreateTask(CreateTaskArgs),
    UpdateTask(UpdateTaskArgs),
    ListTasks(ListTasksArgs),
    CreateEvent(CreateEventArgs),
    FindFreeSlots(FindFreeSlotsArgs),
    ScheduleStudyBlock(ScheduleStudyBlockArgs),
    CreateFlashcards(CreateFlashcardsArgs),
    CreateQuiz(CreateQuizArgs),
    GetDueFlashcards(GetDueFlashcardsArgs),
    GetUpcomingDeadlines(GetUpcomingDeadlinesArgs),
}

/// Parse and check one call's arguments.
///
/// The tool name comes from the call rather than the object, so a variant is never guessed.
/// Every failure is a [`ToolError`] whose message names the field, because the model is the
/// reader: it has to be able to correct the call it just made.
pub fn parse_args(tool: ToolName, value: &Value) -> Result<ToolArgs, ToolError> {
    if !value.is_object() {
        return Err(ToolError::Invalid(
            "the arguments must be a JSON object".to_string(),
        ));
    }
    let args = match tool {
        ToolName::CreateTask => ToolArgs::CreateTask(from_value(value)?),
        ToolName::UpdateTask => ToolArgs::UpdateTask(from_value(value)?),
        ToolName::ListTasks => ToolArgs::ListTasks(from_value(value)?),
        ToolName::CreateEvent => ToolArgs::CreateEvent(from_value(value)?),
        ToolName::FindFreeSlots => ToolArgs::FindFreeSlots(from_value(value)?),
        ToolName::ScheduleStudyBlock => ToolArgs::ScheduleStudyBlock(from_value(value)?),
        ToolName::CreateFlashcards => ToolArgs::CreateFlashcards(from_value(value)?),
        ToolName::CreateQuiz => ToolArgs::CreateQuiz(from_value(value)?),
        ToolName::GetDueFlashcards => ToolArgs::GetDueFlashcards(from_value(value)?),
        ToolName::GetUpcomingDeadlines => ToolArgs::GetUpcomingDeadlines(from_value(value)?),
    };
    check(&args)?;
    Ok(args)
}

/// The tool specs a caller may put on the wire.
///
/// `agent_actions` is the entitlement engine's answer (`Feature::AgentAction`, A272) and
/// not a plan id: core does not know what a plan is, and the server already has the answer.
/// A free account is therefore offered the four reads and none of the six writes, so a
/// model cannot propose an action the server would refuse.
pub fn specs(agent_actions: bool) -> Vec<ToolSpec> {
    ToolName::ALL
        .iter()
        .filter(|tool| agent_actions || !tool.is_write())
        .map(|tool| ToolSpec {
            name: tool.as_str().to_string(),
            description: description(*tool).to_string(),
            parameters: schema(*tool),
        })
        .collect()
}

/// What each tool is for. Exhaustively matched, so a new tool cannot be added without a
/// description — and a description is all the model has to choose with.
fn description(tool: ToolName) -> &'static str {
    match tool {
        ToolName::CreateTask => {
            "Add a task to the user's list. This only proposes: the user confirms before anything is saved."
        }
        ToolName::UpdateTask => {
            "Change one or more fields of an existing task. This only proposes: the user confirms before anything is saved."
        }
        ToolName::ListTasks => {
            "The user's tasks, most urgent first: due date, then priority. Use it to answer what is due and to avoid proposing a duplicate."
        }
        ToolName::CreateEvent => {
            "Add a class, exam or deadline to the user's calendar. This only proposes: the user confirms before anything is saved."
        }
        ToolName::FindFreeSlots => {
            "Find gaps of at least duration_min between the user's existing events, inside their study hours. Use it before proposing a study block."
        }
        ToolName::ScheduleStudyBlock => {
            "Put a study block in the user's calendar. This only proposes: the user confirms before anything is saved."
        }
        ToolName::CreateFlashcards => {
            "Generate flashcards from the workspace's sources into a new deck. This only proposes: the user confirms before anything is saved."
        }
        ToolName::CreateQuiz => {
            "Generate a quiz from the workspace's sources. This only proposes: the user confirms before anything is saved."
        }
        ToolName::GetDueFlashcards => "The cards the user should review now, soonest due first.",
        ToolName::GetUpcomingDeadlines => {
            "Tasks and events with a date in the next `days` days, soonest first."
        }
    }
}

/// The JSON Schema of one tool's arguments.
///
/// Built next to the struct it describes, so a field added to one without the other is a
/// two-line diff a reviewer sees — and `crates/core/tests/agent_tools.rs` fails when the
/// two disagree, in either direction. Every schema is closed
/// (`additionalProperties: false`): an argument the contract does not have is either a
/// model mistake worth surfacing or an injected payload, and both are refused.
fn schema(tool: ToolName) -> Value {
    match tool {
        ToolName::CreateTask => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title"],
            "properties": {
                "title": { "type": "string", "description": "What the task is." },
                "notes": { "type": "string", "description": "Any detail the user gave." },
                "due_at": timestamp("When it is due."),
                "estimate_min": {
                    "type": "integer", "minimum": 1, "maximum": MAX_ESTIMATE_MIN,
                    "description": "How long it takes, in minutes."
                },
                "priority": {
                    "type": "integer", "minimum": MIN_PRIORITY, "maximum": MAX_PRIORITY,
                    "description": "1 high, 2 normal, 3 low."
                },
                "workspace_id": uuid("The workspace it belongs to, when the user named one."),
                "goal_id": uuid("The goal it serves, when the user named one.")
            }
        }),
        ToolName::UpdateTask => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["task_id"],
            "properties": {
                "task_id": uuid("The task to change, from list_tasks."),
                "title": { "type": "string", "description": "A new title." },
                "notes": { "type": "string", "description": "New notes." },
                "due_at": timestamp("A new due date."),
                "estimate_min": {
                    "type": "integer", "minimum": 1, "maximum": MAX_ESTIMATE_MIN,
                    "description": "A new estimate, in minutes."
                },
                "priority": {
                    "type": "integer", "minimum": MIN_PRIORITY, "maximum": MAX_PRIORITY,
                    "description": "A new priority: 1 high, 2 normal, 3 low."
                },
                "status": { "type": "string", "enum": ["todo", "doing", "done"] }
            }
        }),
        ToolName::ListTasks => json!({
            "type": "object",
            "additionalProperties": false,
            "required": [],
            "properties": {
                "status": { "type": "string", "enum": ["todo", "doing", "done"] },
                "due_before": timestamp("Only tasks due at or before this instant."),
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MAX_LIST,
                    "default": DEFAULT_LIMIT
                }
            }
        }),
        ToolName::CreateEvent => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "kind", "starts_at", "ends_at"],
            "properties": {
                "title": { "type": "string", "description": "What the event is." },
                "kind": {
                    "type": "string",
                    "enum": enum_values(&EventKind::ALL.map(|kind| kind.as_str())),
                    "description": "class, exam or deadline. Use schedule_study_block for a study block."
                },
                "starts_at": timestamp("When it starts."),
                "ends_at": timestamp("When it ends."),
                "workspace_id": uuid("The workspace it belongs to, when the user named one."),
                "task_id": uuid("The task it belongs to, when it is one task's deadline.")
            }
        }),
        ToolName::FindFreeSlots => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["from", "to", "duration_min"],
            "properties": {
                "from": timestamp("The start of the window to look in."),
                "to": timestamp("The end of the window to look in."),
                "duration_min": {
                    "type": "integer", "minimum": MIN_SLOT_MINUTES, "maximum": MAX_SLOT_MINUTES,
                    "description": "How long the slot has to be, in minutes."
                }
            }
        }),
        ToolName::ScheduleStudyBlock => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "starts_at", "duration_min"],
            "properties": {
                "title": { "type": "string", "description": "What the block is for." },
                "starts_at": timestamp("When the block starts."),
                "duration_min": {
                    "type": "integer", "minimum": MIN_SLOT_MINUTES, "maximum": MAX_SLOT_MINUTES,
                    "description": "How long the block is, in minutes."
                },
                "task_id": uuid("The task the block works on, when there is one."),
                "goal_id": uuid("The goal the block works towards, when there is one."),
                "workspace_id": uuid("The workspace it belongs to, when the user named one.")
            }
        }),
        ToolName::CreateFlashcards => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "workspace_id", "source_ids", "count"],
            "properties": {
                "title": { "type": "string", "description": "The deck's name." },
                "workspace_id": uuid("The workspace the sources are in."),
                "source_ids": {
                    "type": "array", "minItems": 1, "items": { "type": "string", "format": "uuid" },
                    "description": "The sources to generate from, by id — never a citation label."
                },
                "count": {
                    "type": "integer", "minimum": 1, "maximum": MAX_FLASHCARDS,
                    "description": "How many cards to ask for."
                }
            }
        }),
        ToolName::CreateQuiz => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "workspace_id", "source_ids", "count"],
            "properties": {
                "title": { "type": "string", "description": "The quiz's name." },
                "workspace_id": uuid("The workspace the sources are in."),
                "source_ids": {
                    "type": "array", "minItems": 1, "items": { "type": "string", "format": "uuid" },
                    "description": "The sources to generate from, by id — never a citation label."
                },
                "count": {
                    "type": "integer", "minimum": 1, "maximum": MAX_QUIZ_QUESTIONS,
                    "description": "How many questions to ask for."
                },
                "difficulty": {
                    "type": "string",
                    "enum": enum_values(&QuizDifficulty::ALL.map(|value| value.as_str())),
                    "default": QuizDifficulty::Medium.as_str()
                },
                "types": {
                    "type": "string",
                    "enum": enum_values(&QuizTypes::ALL.map(|value| value.as_str())),
                    "default": QuizTypes::Mixed.as_str()
                }
            }
        }),
        ToolName::GetDueFlashcards => json!({
            "type": "object",
            "additionalProperties": false,
            "required": [],
            "properties": {
                "deck_id": uuid("Only this deck's cards."),
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MAX_LIST,
                    "default": DEFAULT_LIMIT
                }
            }
        }),
        ToolName::GetUpcomingDeadlines => json!({
            "type": "object",
            "additionalProperties": false,
            "required": [],
            "properties": {
                "days": {
                    "type": "integer", "minimum": 1, "maximum": MAX_DEADLINE_DAYS,
                    "default": DEFAULT_DEADLINE_DAYS
                },
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MAX_LIST,
                    "default": DEFAULT_LIMIT
                }
            }
        }),
    }
}

/// A timestamp argument's schema.
///
/// The format is stated in the description as well as the keyword, because the model reads
/// the description and the parser refuses anything without an offset (A280's neighbour: a
/// bare date, a bare number and a citation label are all not timestamps).
fn timestamp(what: &str) -> Value {
    json!({
        "type": "string",
        "format": "date-time",
        "description": format!("{what} RFC 3339 with an offset, e.g. \"2026-10-13T19:00:00+07:00\".")
    })
}

/// A uuid argument's schema.
fn uuid(what: &str) -> Value {
    json!({ "type": "string", "format": "uuid", "description": what })
}

/// An `enum` list from the type's own values, so the schema cannot drift from the parser.
fn enum_values(values: &[&'static str]) -> Value {
    json!(values)
}

/// The argument checks, exhaustively matched: a tool cannot be added without an answer to
/// what its bounds are.
fn check(args: &ToolArgs) -> Result<(), ToolError> {
    match args {
        ToolArgs::CreateTask(args) => {
            check_task_fields(Some(&args.title), args.priority, args.estimate_min)
        }
        ToolArgs::UpdateTask(args) => {
            if args.title.is_none()
                && args.notes.is_none()
                && args.due_at.is_none()
                && args.estimate_min.is_none()
                && args.priority.is_none()
                && args.status.is_none()
            {
                return Err(ToolError::Invalid(
                    "update_task must name at least one field to change".to_string(),
                ));
            }
            check_task_fields(args.title.as_deref(), args.priority, args.estimate_min)
        }
        ToolArgs::ListTasks(args) => check_limit(args.limit),
        ToolArgs::CreateEvent(args) => check_event(args),
        ToolArgs::FindFreeSlots(args) => {
            if args.to <= args.from {
                return Err(ToolError::Invalid("to must be after from".to_string()));
            }
            check_slot(args.duration_min)
        }
        ToolArgs::ScheduleStudyBlock(args) => check_slot(args.duration_min),
        ToolArgs::CreateFlashcards(args) => {
            check_sources(&args.source_ids)?;
            check_count(args.count, MAX_FLASHCARDS, "cards")
        }
        ToolArgs::CreateQuiz(args) => {
            check_sources(&args.source_ids)?;
            check_count(args.count, MAX_QUIZ_QUESTIONS, "questions")
        }
        ToolArgs::GetDueFlashcards(args) => check_limit(args.limit),
        ToolArgs::GetUpcomingDeadlines(args) => {
            if !(1..=MAX_DEADLINE_DAYS).contains(&args.days) {
                return Err(ToolError::Invalid(format!(
                    "days must be between 1 and {}",
                    bounds().max_deadline_days
                )));
            }
            check_limit(args.limit)
        }
    }
}

/// The task field rules, which are `crate::tasks`' and not this module's: a call that names
/// a title goes through [`crate::tasks::validate`], the same function `POST /tasks` and the
/// sync push call, so a proposal cannot carry a row the route would refuse.
///
/// `title` is `None` for a patch that did not name one. `validate` takes a title, and a
/// call that only moves a due date must not be refused for the title it left alone — so the
/// fields that *were* named are checked against the same bounds, which live in
/// `crate::tasks` and are read from there rather than repeated.
fn check_task_fields(
    title: Option<&str>,
    priority: Option<i32>,
    estimate_min: Option<i32>,
) -> Result<(), ToolError> {
    if let Some(title) = title {
        // The tree rules need a row and a parent that do not exist yet: a tool call never
        // sets `parent_id`, and a nil id has no parent, so `SelfParent` cannot fire.
        crate::tasks::validate(
            Uuid::nil(),
            None,
            title,
            priority.unwrap_or(DEFAULT_PRIORITY),
            estimate_min,
        )
        .map_err(|error| ToolError::Invalid(error.to_string()))?;
        return Ok(());
    }
    if let Some(priority) = priority
        && !(MIN_PRIORITY..=MAX_PRIORITY).contains(&priority)
    {
        return Err(ToolError::Invalid(TaskError::Priority.to_string()));
    }
    if let Some(minutes) = estimate_min
        && !(1..=MAX_ESTIMATE_MIN).contains(&minutes)
    {
        return Err(ToolError::Invalid(TaskError::Estimate.to_string()));
    }
    Ok(())
}

fn check_event(args: &CreateEventArgs) -> Result<(), ToolError> {
    if args.ends_at <= args.starts_at {
        return Err(ToolError::Invalid(
            "ends_at must be after starts_at".to_string(),
        ));
    }
    let minutes = (args.ends_at - args.starts_at).num_minutes();
    if minutes > i64::from(MAX_EVENT_MINUTES) {
        return Err(ToolError::Invalid(format!(
            "an event may last at most {} minutes",
            bounds().max_event_minutes
        )));
    }
    Ok(())
}

fn check_limit(limit: u32) -> Result<(), ToolError> {
    if !(1..=MAX_LIST).contains(&limit) {
        return Err(ToolError::Invalid(format!(
            "limit must be between 1 and {}",
            bounds().max_list
        )));
    }
    Ok(())
}

fn check_slot(minutes: u32) -> Result<(), ToolError> {
    if !(MIN_SLOT_MINUTES..=MAX_SLOT_MINUTES).contains(&minutes) {
        return Err(ToolError::Invalid(format!(
            "duration_min must be between {} and {} minutes",
            bounds().min_slot_minutes,
            bounds().max_slot_minutes
        )));
    }
    Ok(())
}

fn check_count(count: u32, max: u32, noun: &str) -> Result<(), ToolError> {
    if count == 0 || count > max {
        return Err(ToolError::Invalid(format!(
            "count must be between 1 and {max} {noun}"
        )));
    }
    Ok(())
}

/// A deck or a quiz is generated from the sources it names, so naming none is a call that
/// would fail later for a reason the model can be told now.
fn check_sources(source_ids: &[Uuid]) -> Result<(), ToolError> {
    if source_ids.is_empty() {
        return Err(ToolError::Invalid(
            "source_ids must name at least one source".to_string(),
        ));
    }
    Ok(())
}

fn from_value<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, ToolError> {
    serde_json::from_value(value.clone()).map_err(|error| ToolError::Invalid(error.to_string()))
}

/// The shape message a timestamp refusal carries.
///
/// It names the field and the shape, and not the parse error: the model has to send an
/// offset, and "invalid character at position 4" would not tell it that.
fn expected_shape<E: serde::de::Error>(field: &str) -> E {
    E::custom(format!(
        "{field} must be an RFC 3339 timestamp with an offset, for example \"2026-10-13T19:00:00+07:00\""
    ))
}

/// The one timestamp parser. `parse_from_rfc3339` requires the offset, which is the point:
/// a bare date or a naive time is refused rather than assumed to be UTC, because assuming
/// would silently place a Jakarta student's 19:00 at 02:00 the next morning.
fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(raw).map(|parsed| parsed.with_timezone(&Utc))
}

fn required_timestamp<'de, D>(deserializer: D, field: &str) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer).map_err(|_| expected_shape(field))?;
    parse_timestamp(&raw).map_err(|_| expected_shape(field))
}

fn optional_timestamp<'de, D>(
    deserializer: D,
    field: &str,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer).map_err(|_| expected_shape(field))?;
    match raw {
        None => Ok(None),
        Some(raw) => parse_timestamp(&raw)
            .map(Some)
            .map_err(|_| expected_shape(field)),
    }
}

fn due_at<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    optional_timestamp(deserializer, "due_at")
}

fn due_before<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    optional_timestamp(deserializer, "due_before")
}

fn starts_at<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    required_timestamp(deserializer, "starts_at")
}

fn ends_at<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    required_timestamp(deserializer, "ends_at")
}

fn from<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    required_timestamp(deserializer, "from")
}

fn to<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    required_timestamp(deserializer, "to")
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

fn default_deadline_days() -> u32 {
    DEFAULT_DEADLINE_DAYS
}

fn default_difficulty() -> QuizDifficulty {
    QuizDifficulty::Medium
}

fn default_quiz_types() -> QuizTypes {
    QuizTypes::Mixed
}
