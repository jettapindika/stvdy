//! The confirm path: what a confirmed proposal actually does.
//!
//! **This file contains no DML.** Every arm below builds a body and calls the same service
//! function the REST route calls — `tasks::create_in`, `tasks::update_in`,
//! `events::create_in`, `flashcards::routes::create_in`, `quizzes::routes::create_in` — so a
//! confirmed task is the row `POST /tasks` would have written, with the same field rules and
//! the same `change_log` entry. `crates/server/tests/actions.rs::the_executor_contains_no_dml`
//! asserts the absence mechanically, because a second implementation of "create a task" is a
//! second set of validation and the two diverge the first time either is edited (A276).
//!
//! That is not a hypothetical. Phase 9's offline gate found four defects of exactly this
//! class, every one of them two write paths disagreeing: a push delete that did not cascade,
//! a restore that did not restore, a calendar-owned event writable through push, and a local
//! apply that did not cascade. The confirm path is a second writer by nature — it is invoked
//! from a card rather than from a form — which is why the sharing is enforced by a test
//! rather than left to review.
//!
//! [`run`] is called **inside the claim's transaction** (A275), so a failure here rolls the
//! claim back: a `confirmed` row can never be a row that did nothing. The one piece of work
//! that cannot be in that transaction is a generation job's enqueue — a job row for a deck
//! whose insert rolled back is a job that can never succeed — so it is returned as [`Job`]
//! and queued by the caller after the commit.

use serde_json::{Map, Value, json};
use sqlx::PgTransaction;
use uuid::Uuid;

use stvdy_core::agent::tools::{EventKind, ToolArgs, ToolName};

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// What one execution produced.
pub struct Executed {
    /// The row that was written, as the route's own JSON shape, so the card can show what
    /// the student just got.
    pub result: Value,
    /// Work that has to happen **after** the claim's transaction commits.
    pub deferred: Option<Deferred>,
}

/// A generation job that cannot be queued inside the claim's transaction.
///
/// A job row and the row it names cannot share a transaction: rolling the claim back would
/// leave a job for a deck that does not exist. So the arm records what to queue and the
/// caller queues it after the commit — through the module's own `enqueue_generation`, which
/// is the same function its REST route calls, refund included (A276). There is no
/// compensation logic here: a second refund path would be a second answer to what a failed
/// generation refunds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deferred {
    Flashcards {
        deck_id: Uuid,
        user_id: Uuid,
        cost: i64,
    },
    Quiz {
        quiz_id: Uuid,
        user_id: Uuid,
        cost: i64,
    },
}

/// Runs one confirmed action (A276).
///
/// `tx` is the caller's transaction and not a pool: the type is the reminder that the
/// execution and the claim have to commit or roll back together (A275).
pub async fn run(
    tx: &mut PgTransaction<'_>,
    state: &AppState,
    user: &AuthUser,
    tool: ToolName,
    args: &ToolArgs,
) -> AppResult<Executed> {
    match args {
        ToolArgs::CreateTask(args) => {
            // The tool's arguments are not a task body: `goal_id` is not a `tasks` column
            // and `validate_task` refuses a field a client may not write, so the bridge
            // below names exactly the columns a task has. Dropping `goal_id` here is
            // deliberate and is the one place the tool contract and the row contract
            // differ; a task's goal link is Phase 11's column and not this phase's.
            let mut body = Map::new();
            body.insert("title".to_string(), json!(args.title));
            insert_optional(&mut body, "notes", args.notes.as_ref());
            insert_optional(&mut body, "due_at", args.due_at.as_ref());
            insert_optional(&mut body, "estimate_min", args.estimate_min.as_ref());
            insert_optional(&mut body, "priority", args.priority.as_ref());
            insert_optional(&mut body, "workspace_id", args.workspace_id.as_ref());

            let task = crate::tasks::create_in(&mut *tx, user, Value::Object(body)).await?;
            Ok(Executed {
                result: json!({ "task": task }),
                deferred: None,
            })
        }
        ToolArgs::UpdateTask(args) => {
            let mut body = Map::new();
            insert_optional(&mut body, "title", args.title.as_ref());
            insert_optional(&mut body, "notes", args.notes.as_ref());
            insert_optional(&mut body, "due_at", args.due_at.as_ref());
            insert_optional(&mut body, "estimate_min", args.estimate_min.as_ref());
            insert_optional(&mut body, "priority", args.priority.as_ref());
            // The status is serialised from the type, so the string that reaches
            // `validate_task` is the one `tasks.status` stores and the one the tool schema
            // enumerates — not a hand-written literal that could drift from either.
            if let Some(status) = args.status {
                body.insert("status".to_string(), json!(status));
            }

            let task = crate::tasks::update_in(tx, user, args.task_id, Value::Object(body)).await?;
            Ok(Executed {
                result: json!({ "task": task }),
                deferred: None,
            })
        }
        ToolArgs::CreateEvent(args) => {
            let event = crate::events::create_in(
                &mut *tx,
                user,
                event_body(
                    &args.title,
                    args.kind,
                    args.starts_at,
                    args.ends_at,
                    args.workspace_id,
                    args.task_id,
                ),
            )
            .await?;
            Ok(Executed {
                result: json!({ "event": event }),
                deferred: None,
            })
        }
        ToolArgs::ScheduleStudyBlock(args) => {
            // The block's end is derived from its start and its length, which is why the
            // tool's contract names `duration_min` rather than `ends_at`: two timestamps
            // can disagree, and a model that sent an end before the start would be
            // proposing a block with a negative length. The derivation happens here, at the
            // one place the two contracts meet.
            let ends_at = args.starts_at + chrono::Duration::minutes(i64::from(args.duration_min));
            let event = crate::events::create_in(
                &mut *tx,
                user,
                event_body(
                    &args.title,
                    EventKind::StudyBlock,
                    args.starts_at,
                    ends_at,
                    args.workspace_id,
                    args.task_id,
                ),
            )
            .await?;
            Ok(Executed {
                result: json!({ "event": event }),
                deferred: None,
            })
        }
        ToolArgs::CreateFlashcards(args) => {
            let body = crate::flashcards::routes::CreateDeckBody {
                title: args.title.clone(),
                source_ids: args.source_ids.clone(),
                count: args.count,
            };
            let (deck, cost) = crate::flashcards::routes::create_in(
                &mut *tx,
                state,
                user,
                args.workspace_id,
                body,
            )
            .await?;
            Ok(Executed {
                result: json!({ "deck": deck }),
                deferred: Some(Deferred::Flashcards {
                    deck_id: deck.id,
                    user_id: user.user_id,
                    cost,
                }),
            })
        }
        ToolArgs::CreateQuiz(args) => {
            let body = crate::quizzes::routes::CreateQuizBody {
                title: args.title.clone(),
                source_ids: args.source_ids.clone(),
                count: args.count,
                // Both enums serialise lowercase, byte-identical to the route's
                // `DIFFICULTIES` and `TYPE_SETTINGS` arrays, so the route's own `contains`
                // checks pass without a translation table and without hand-built strings.
                difficulty: serde_json::to_value(args.difficulty)
                    .map_err(|error| {
                        AppError::Internal(anyhow::anyhow!("serializing a difficulty: {error}"))
                    })?
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                types: serde_json::to_value(args.types)
                    .map_err(|error| {
                        AppError::Internal(anyhow::anyhow!("serializing a quiz type: {error}"))
                    })?
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            };
            let (quiz, cost) =
                crate::quizzes::routes::create_in(&mut *tx, state, user, args.workspace_id, body)
                    .await?;
            Ok(Executed {
                result: json!({ "quiz": quiz }),
                deferred: Some(Deferred::Quiz {
                    quiz_id: quiz.id,
                    user_id: user.user_id,
                    cost,
                }),
            })
        }
        // A READ tool cannot reach here: the gate returns `Execute` for one and
        // `chat::agent::handle` runs it inline, so a read arm in this match would be dead
        // code. It is a `500` naming the tool rather than an `unreachable!`, because the
        // alternative to a loud bug is a silent no-op on a confirm.
        ToolArgs::ListTasks(_)
        | ToolArgs::FindFreeSlots(_)
        | ToolArgs::GetDueFlashcards(_)
        | ToolArgs::GetUpcomingDeadlines(_) => Err(AppError::Internal(anyhow::anyhow!(
            "{} is a read tool and has no confirm path",
            tool.as_str()
        ))),
    }
}

/// The `events` body a confirmed event or study block becomes.
///
/// One builder for both arms, because `schedule_study_block` is a `create_event` whose kind
/// is fixed and whose end is derived: two literal bodies would be two places for the column
/// set to drift from `validate_event`'s.
fn event_body(
    title: &str,
    kind: EventKind,
    starts_at: chrono::DateTime<chrono::Utc>,
    ends_at: chrono::DateTime<chrono::Utc>,
    workspace_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Value {
    let mut body = Map::new();
    body.insert("title".to_string(), json!(title));
    body.insert("kind".to_string(), json!(kind.as_str()));
    body.insert("starts_at".to_string(), json!(starts_at));
    body.insert("ends_at".to_string(), json!(ends_at));
    insert_optional(&mut body, "workspace_id", workspace_id.as_ref());
    insert_optional(&mut body, "task_id", task_id.as_ref());
    Value::Object(body)
}

/// Adds a field the call named. An absent field is absent from the body rather than null,
/// which is what lets `validate_task` and `validate_event` tell "not named" from "named as
/// null" — the distinction a partial write depends on.
fn insert_optional<T: serde::Serialize>(
    body: &mut Map<String, Value>,
    key: &str,
    value: Option<&T>,
) {
    if let Some(value) = value
        && let Ok(value) = serde_json::to_value(value)
    {
        body.insert(key.to_string(), value);
    }
}

/// Queues the work a confirmed proposal left behind, through the module's own function.
///
/// Separate from [`run`] because it runs **after** the claim's commit, and a thin dispatcher
/// rather than an implementation: each arm calls the `enqueue_generation` its REST route
/// calls, so the job payload, the job kind and the refund-on-failure are the route's and
/// there is one of each (A276).
pub async fn queue(state: &AppState, deferred: Deferred) -> AppResult<()> {
    match deferred {
        Deferred::Flashcards {
            deck_id,
            user_id,
            cost,
        } => crate::flashcards::routes::enqueue_generation(state, deck_id, user_id, cost).await,
        Deferred::Quiz {
            quiz_id,
            user_id,
            cost,
        } => crate::quizzes::routes::enqueue_generation(state, quiz_id, user_id, cost).await,
    }
}
