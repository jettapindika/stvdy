//! The task rules: the order, the local due window, the tree, and the field bounds.
//!
//! Pure data in, pure data out — no database, no serde_json, no clock. The order is defined
//! *here* and repeated in SQL by `crates/server/src/tasks.rs`, with a comment naming
//! [`order_tasks`], because the server sorts in the database and this sorts a vector; two
//! independent orders would put the same rows in two sequences on two screens.
//!
//! Local-date maths goes through `chrono_tz` and never through a fixed offset (A6). A UTC
//! comparison is wrong for every user east of Greenwich, and `Asia/Jakarta` is the default
//! zone: the defect would be invisible in a UTC-only test and visible to most of the users.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

/// The priority range `tasks.priority` accepts. `1` is high.
pub const MIN_PRIORITY: i32 = 1;
pub const MAX_PRIORITY: i32 = 3;

/// The longest estimate a task may carry, in minutes. A day, because an estimate is a
/// single sitting and a task that takes longer than a day is a project.
pub const MAX_ESTIMATE_MIN: i32 = 1440;

/// The title and notes bounds `tasks` stores. Characters, not bytes: the column is `text`,
/// and a limit counted in bytes would cut an Indonesian title short.
pub const MAX_TITLE: usize = 500;
pub const MAX_NOTES: usize = 20_000;

/// A task as the rules see it — no db, no serde, no clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub title: String,
    pub due_at: Option<DateTime<Utc>>,
    pub estimate_min: Option<i32>,
    pub priority: i32,
    /// Where the task sits among its siblings. The client sets it when it accepts a
    /// breakdown, and LWW treats it like any other field (A196).
    pub position: i32,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Todo,
    Doing,
    Done,
}

/// A field rule a task row did not satisfy. Every message names the bound, because it is the
/// reason a push is rejected and the text the client shows.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("a title is required")]
    TitleEmpty,
    #[error("the title is longer than {MAX_TITLE} characters")]
    TitleTooLong,
    #[error("priority must be between {MIN_PRIORITY} and {MAX_PRIORITY}")]
    Priority,
    #[error("the estimate must be between 1 and {MAX_ESTIMATE_MIN} minutes")]
    Estimate,
    #[error("a subtask cannot be its own parent")]
    SelfParent,
}

/// The field rules every writer of a task row applies, whether the row arrives through
/// `POST /tasks` or through a push. One implementation, two callers: a second would be a
/// second answer to "may this row exist".
///
/// `notes` is not checked here — the schema bounds it as text and there is no rule about its
/// content — and the tree rules (one level of subtasks, a parent in the same workspace) are
/// the caller's, because they need rows this function does not see.
pub fn validate(
    id: Uuid,
    parent_id: Option<Uuid>,
    title: &str,
    priority: i32,
    estimate_min: Option<i32>,
) -> Result<(), TaskError> {
    if title.trim().is_empty() {
        return Err(TaskError::TitleEmpty);
    }
    if title.chars().count() > MAX_TITLE {
        return Err(TaskError::TitleTooLong);
    }
    if !(MIN_PRIORITY..=MAX_PRIORITY).contains(&priority) {
        return Err(TaskError::Priority);
    }
    if let Some(minutes) = estimate_min
        && !(1..=MAX_ESTIMATE_MIN).contains(&minutes)
    {
        return Err(TaskError::Estimate);
    }
    if parent_id == Some(id) {
        return Err(TaskError::SelfParent);
    }
    Ok(())
}

/// The one order the Tasks screen and the Today screen both use.
///
/// Due first (undated last), then priority, then position, then `created_at`, then the id.
/// Deterministic to the end: the id breaks every tie, because two tasks can share a due date
/// to the second and a batch insert shares `created_at` to the microsecond, which is exactly
/// how Phase 5's questions came back shuffled (A196, A100). Two devices that hold the same
/// rows therefore put them in the same sequence without talking to each other, which is what
/// "the order is a total order" buys.
///
/// A tombstone is sorted like any other row: this is a sort over the rows a query already
/// filtered (`where deleted_at is null`), and silently reordering a deleted row to the end
/// would hide a caller that forgot the filter instead of failing it.
pub fn order_tasks(rows: &mut [TaskRow]) {
    rows.sort_by_key(order_key);
}

/// The order key, written once because three callers sort by it — this module's two and,
/// through the SQL it is repeated in, the server's `GET /tasks`.
///
/// Due first (undated last), then priority, then position, then `created_at`, then the id.
/// Deterministic to the end: the id breaks every tie, because two tasks can share a due date
/// to the second and a batch insert shares `created_at` to the microsecond, which is exactly
/// how Phase 5's questions came back shuffled (A196, A100). Two devices that hold the same
/// rows therefore put them in the same sequence without talking to each other, which is what
/// "the order is a total order" buys.
fn order_key(row: &TaskRow) -> (u8, DateTime<Utc>, i32, i32, DateTime<Utc>, Uuid) {
    let (dated, due) = match row.due_at {
        // An undated task is not the most urgent thing the student has, so "no due date"
        // sorts after every date rather than before it.
        Some(due) => (0u8, due),
        None => (1u8, DateTime::<Utc>::MIN_UTC),
    };
    (
        dated,
        due,
        row.priority,
        row.position,
        row.created_at,
        row.id,
    )
}

/// Live subtasks of `parent`, in [`order_tasks`] order.
///
/// One level: a grandchild belongs to its own parent and is not returned here, which is the
/// depth the UI shows and the depth the spec asks for. A tombstoned child is absent, so a
/// deleted subtask does not reappear under its parent on the next read.
pub fn subtasks_of(rows: &[TaskRow], parent: Uuid) -> Vec<&TaskRow> {
    let mut children: Vec<&TaskRow> = rows
        .iter()
        .filter(|row| row.parent_id == Some(parent) && row.deleted_at.is_none())
        .collect();
    children.sort_by_key(|row| order_key(row));
    children
}

/// The tasks the student must do on their local calendar day `date_key` (`YYYY-MM-DD`),
/// plus the overdue ones, in [`order_tasks`] order.
///
/// `due_at` is converted through `tz`, never through a fixed offset: 06:00 on the 16th in
/// Jakarta is 23:00 on the 15th in UTC, so a UTC comparison puts the task on the wrong day.
/// Overdue rows are returned rather than dropped — a deadline that passed is the first thing
/// the student must do, and the caller labels it overdue from its own due date.
///
/// A done or tombstoned row is not outstanding and is absent. A `date_key` that is not a date
/// has no due tasks: the key comes from a date this codebase formatted, and there is no day
/// to compare against.
pub fn due_on<'a>(rows: &'a [TaskRow], tz: Tz, date_key: &str) -> Vec<&'a TaskRow> {
    let Some(day) = chrono::NaiveDate::parse_from_str(date_key, "%Y-%m-%d").ok() else {
        return Vec::new();
    };

    let mut due: Vec<&TaskRow> = rows
        .iter()
        .filter(|row| row.deleted_at.is_none() && row.status != TaskStatus::Done)
        .filter(|row| {
            row.due_at
                .is_some_and(|due_at| due_at.with_timezone(&tz).date_naive() <= day)
        })
        .collect();

    due.sort_by_key(|row| order_key(row));
    due
}
