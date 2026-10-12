//! The task-breakdown proposal contract (`prompts/task_breakdown.v1.md`).
//!
//! A proposal, never a row (A195): the route returns it and the student confirms, edits or
//! dismisses it. The rules the prompt states — 3-8 subtasks, 15-90 minutes each, a total that
//! equals the sum, no subtask that restates the parent — are enforced here, because a model
//! that ignores one produces something the client would otherwise render as a plan.

use crate::generation::{GenerationError, strip_code_fence};
use crate::quiz::normalize;

/// The count the prompt asks for, and what makes "3-8" a contract rather than a suggestion.
/// Fewer than three restates the parent; more than eight is a project plan.
pub const MIN_SUBTASKS: usize = 3;
pub const MAX_SUBTASKS: usize = 8;

/// The single-session range the prompt asks for. Outside it is a warning, not a failure: it is
/// a judgement the student can fix in the proposal list, and rejecting the whole reply over one
/// number costs a second model call for something a single keystroke fixes.
pub const MIN_ESTIMATE_MIN: i64 = 15;
pub const MAX_ESTIMATE_MIN: i64 = 90;

/// The reply, exactly as `prompts/task_breakdown.v1.md` documents it.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that invents a
/// field produces a parse error and a retry rather than a proposal with a silently dropped one.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakdownFile {
    pub subtasks: Vec<ProposedSubtask>,
    pub total_estimate_min: i64,
}

/// One proposed subtask, as the model returned it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedSubtask {
    pub title: String,
    pub estimate_min: i64,
}

/// The proposal, in the order the model wrote it.
///
/// The vector's index *is* the position the caller assigns when it creates the tasks (A196):
/// the order is carried by the sequence rather than by a field, so nothing can re-sort it
/// without visibly rewriting the list. That is what stops a batch insert from coming back
/// shuffled, the way Phase 5's questions did when every row of a generation shared
/// `created_at` to the microsecond and the order fell through to a random uuid (A100).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Proposal {
    pub subtasks: Vec<ProposedSubtask>,
    pub total_estimate_min: i64,
    pub warnings: Vec<String>,
}

/// Parses a breakdown reply into a proposal.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite the
/// contract. It does **not** tolerate prose around the object: guessing which part is the JSON
/// is how a half-parsed proposal ships.
///
/// `parent_title` is the task being broken down. A subtask that restates it is dropped with a
/// warning, and the count and the total are checked *after* the drop — so a reply that only
/// reaches the minimum by counting the parent is refused rather than shipped as a two-item
/// breakdown, and a total that counted the dropped restatement is refused rather than shown
/// beside a list it does not describe.
pub fn parse_proposal(raw: &str, parent_title: &str) -> Result<Proposal, GenerationError> {
    let text = strip_code_fence(raw);
    let file: BreakdownFile = serde_json::from_str(text).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    // A zero-subtask reply is the count rule's business, not an `Empty`: the module asks for
    // 3-8, and a reply outside that range is what the one retry exists to fix. Treating it as
    // `Empty` would make the most degenerate reply the only one that is not retried.
    let mut warnings = Vec::new();
    let parent = normalize(parent_title);
    let mut subtasks = Vec::with_capacity(file.subtasks.len());

    for (index, subtask) in file.subtasks.into_iter().enumerate() {
        let title = subtask.title.trim().to_string();
        if title.is_empty() {
            return Err(GenerationError::Schema(format!(
                "subtask {index}: the title is empty"
            )));
        }
        if normalize(&title) == parent {
            warnings.push(format!(
                "subtask {index}: \"{title}\" restates the task being broken down; dropped"
            ));
            continue;
        }
        if !(MIN_ESTIMATE_MIN..=MAX_ESTIMATE_MIN).contains(&subtask.estimate_min) {
            warnings.push(format!(
                "subtask {index}: the estimate of {} minutes is outside the \
                 {MIN_ESTIMATE_MIN}-{MAX_ESTIMATE_MIN} range the module asks for",
                subtask.estimate_min
            ));
        }
        subtasks.push(ProposedSubtask {
            title,
            estimate_min: subtask.estimate_min,
        });
    }

    if subtasks.len() < MIN_SUBTASKS || subtasks.len() > MAX_SUBTASKS {
        return Err(GenerationError::Schema(format!(
            "the proposal has {} subtasks after dropping restatements of the parent, and the \
             module asks for {MIN_SUBTASKS}-{MAX_SUBTASKS}",
            subtasks.len()
        )));
    }

    // The declared total must agree with the subtasks that remain. A total the model wrote
    // that disagrees with its own list is a number that cannot be shown: the "Suggested …"
    // region displays it, and the alternative — substituting the sum silently — would show
    // the student a number they never saw proposed.
    let sum: i64 = subtasks.iter().map(|subtask| subtask.estimate_min).sum();
    if file.total_estimate_min != sum {
        return Err(GenerationError::Schema(format!(
            "the total is {} minutes but its own subtasks sum to {sum}",
            file.total_estimate_min
        )));
    }

    Ok(Proposal {
        subtasks,
        total_estimate_min: sum,
        warnings,
    })
}
