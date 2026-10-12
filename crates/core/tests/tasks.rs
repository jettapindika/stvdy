//! The task rules: the order two devices must agree on, the local due window, and the tree.
//!
//! Every case here is a rule the Tasks screen and the Today screen both depend on, and the
//! ones worth pinning are the ones that fail *silently*: an order with a tiebreak that is not
//! total, a due window computed in UTC for a Jakarta student, and a subtree that leaks a
//! grandchild into a one-level list.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

use stvdy_core::tasks::{
    MAX_ESTIMATE_MIN, MAX_PRIORITY, MAX_TITLE, MIN_PRIORITY, TaskError, TaskRow, TaskStatus,
    due_on, order_tasks, subtasks_of, validate,
};

fn jakarta() -> Tz {
    "Asia/Jakarta".parse().unwrap()
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

/// A live, undated, normal-priority task: the state every case starts from, so a case states
/// only the field it is about.
fn task(id: u128) -> TaskRow {
    TaskRow {
        id: Uuid::from_u128(id),
        parent_id: None,
        title: format!("Tugas {id}"),
        due_at: None,
        estimate_min: None,
        priority: 2,
        position: 0,
        status: TaskStatus::Todo,
        created_at: at("2026-10-11T00:00:00Z"),
        deleted_at: None,
    }
}

fn ids(rows: &[TaskRow]) -> Vec<Uuid> {
    rows.iter().map(|row| row.id).collect()
}

fn borrowed_ids(rows: &[&TaskRow]) -> Vec<Uuid> {
    rows.iter().map(|row| row.id).collect()
}

/// A batch insert shares `created_at` to the microsecond, so ordering by time alone is a
/// random order. The id breaks every tie, which is what makes the order total: two devices
/// that hold the same rows put them in the same sequence without talking to each other.
#[test]
fn tasks_with_the_same_position_and_timestamp_still_order_deterministically() {
    let mut rows = vec![
        TaskRow {
            priority: 3,
            ..task(3)
        },
        TaskRow {
            priority: 1,
            ..task(1)
        },
    ];
    order_tasks(&mut rows);
    assert_eq!(ids(&rows), vec![Uuid::from_u128(1), Uuid::from_u128(3)]);

    // Identical in every field the order reads but the id: the id is the last word.
    let mut rows = vec![task(9), task(2)];
    order_tasks(&mut rows);
    assert_eq!(ids(&rows), vec![Uuid::from_u128(2), Uuid::from_u128(9)]);
}

/// Due first, then priority, then position: the student's next action is at the top, and a
/// task with no due date sorts after every dated one rather than to the top.
#[test]
fn the_order_is_due_then_priority_then_position_and_undated_tasks_come_last() {
    let mut rows = vec![
        TaskRow {
            priority: 1,
            ..task(1)
        },
        TaskRow {
            due_at: Some(at("2026-10-12T00:00:00Z")),
            priority: 3,
            ..task(2)
        },
        TaskRow {
            due_at: Some(at("2026-10-11T00:00:00Z")),
            priority: 3,
            ..task(3)
        },
        TaskRow {
            due_at: Some(at("2026-10-11T00:00:00Z")),
            priority: 1,
            ..task(4)
        },
    ];
    order_tasks(&mut rows);
    assert_eq!(
        ids(&rows),
        vec![
            Uuid::from_u128(4),
            Uuid::from_u128(3),
            Uuid::from_u128(2),
            Uuid::from_u128(1)
        ]
    );
}

/// Position orders subtasks of one parent, which is what the breakdown's `position` is for
/// (A196): a batch inserted in the model's order must come back in the model's order.
#[test]
fn position_orders_two_rows_that_agree_on_due_and_priority() {
    let due = at("2026-10-11T00:00:00Z");
    let mut rows = vec![
        TaskRow {
            due_at: Some(due),
            position: 5,
            ..task(1)
        },
        TaskRow {
            due_at: Some(due),
            position: 1,
            ..task(2)
        },
    ];
    order_tasks(&mut rows);
    assert_eq!(ids(&rows), vec![Uuid::from_u128(2), Uuid::from_u128(1)]);
}

/// `due_on` is a local-calendar question, and this is the case a UTC implementation gets
/// wrong: 06:00 on the 16th in Jakarta is 23:00 on the 15th in UTC, so a UTC comparison puts
/// the task on the wrong day for every user east of Greenwich — which is most of them.
///
/// The key is the one the client computes (`app/src/features/today/day.ts`, through `Intl`
/// with an explicit `timeZone`), so this also pins that the two sides agree on the format.
#[test]
fn due_on_uses_the_students_local_day_not_utc() {
    let rows = [TaskRow {
        due_at: Some(at("2026-10-16T06:00:00+07:00")),
        ..task(1)
    }];

    assert_eq!(due_on(&rows, jakarta(), "2026-10-16").len(), 1);
    assert_eq!(
        due_on(&rows, jakarta(), "2026-10-15").len(),
        0,
        "the UTC day is the 15th and must not be the answer"
    );
    assert_eq!(due_on(&rows, chrono_tz::UTC, "2026-10-15").len(), 1);
}

/// A task due yesterday is still due today — it is the first thing the student must do, and
/// dropping it from "today" is how a deadline disappears. The caller labels it overdue from
/// its own due date; `due_on` returns it rather than deciding what to call it.
#[test]
fn overdue_tasks_are_included_and_tomorrows_are_not() {
    let rows = vec![
        TaskRow {
            due_at: Some(at("2026-10-15T03:00:00Z")),
            ..task(1)
        },
        TaskRow {
            due_at: Some(at("2026-10-16T03:00:00Z")),
            ..task(2)
        },
        TaskRow {
            due_at: Some(at("2026-10-17T03:00:00Z")),
            ..task(3)
        },
    ];
    let due = due_on(&rows, jakarta(), "2026-10-16");
    assert_eq!(
        borrowed_ids(&due),
        vec![Uuid::from_u128(1), Uuid::from_u128(2)],
        "yesterday and today, in order, and not tomorrow"
    );
}

/// A done task is not outstanding, and a tombstoned one is not there at all. Both would be
/// counted as work still to do by a due window that only compared dates.
#[test]
fn a_done_or_deleted_row_is_absent_from_the_due_window() {
    let rows = vec![
        TaskRow {
            due_at: Some(at("2026-10-16T03:00:00Z")),
            status: TaskStatus::Done,
            ..task(1)
        },
        TaskRow {
            due_at: Some(at("2026-10-16T03:00:00Z")),
            deleted_at: Some(at("2026-10-16T04:00:00Z")),
            ..task(2)
        },
        TaskRow {
            due_at: Some(at("2026-10-16T03:00:00Z")),
            ..task(3)
        },
    ];
    assert_eq!(
        borrowed_ids(&due_on(&rows, jakarta(), "2026-10-16")),
        vec![Uuid::from_u128(3)]
    );
}

/// A task with no due date is not due on any day: an undated task is the case where the
/// window must not invent a date for it.
#[test]
fn an_undated_row_is_never_in_the_due_window() {
    let rows = vec![task(1)];
    assert!(due_on(&rows, jakarta(), "2026-10-16").is_empty());
}

/// The due window is in `order_tasks` order, so the list the student sees is the same order
/// the Tasks screen shows rather than whatever the rows happened to arrive in.
#[test]
fn the_due_window_is_in_order_tasks_order() {
    let rows = vec![
        TaskRow {
            due_at: Some(at("2026-10-16T09:00:00Z")),
            priority: 3,
            ..task(1)
        },
        TaskRow {
            due_at: Some(at("2026-10-16T09:00:00Z")),
            priority: 1,
            ..task(2)
        },
    ];
    assert_eq!(
        borrowed_ids(&due_on(&rows, jakarta(), "2026-10-16")),
        vec![Uuid::from_u128(2), Uuid::from_u128(1)]
    );
}

/// Subtasks are one level, which is what the UI shows and what the spec asks for: a
/// grandchild is not returned as a child of the root, or the tree would nest without a bound.
#[test]
fn subtasks_of_returns_one_level() {
    let root = Uuid::from_u128(1);
    let child = Uuid::from_u128(2);
    let rows = vec![
        task(1),
        TaskRow {
            parent_id: Some(root),
            ..task(2)
        },
        TaskRow {
            parent_id: Some(child),
            ..task(3)
        },
    ];
    assert_eq!(
        borrowed_ids(&subtasks_of(&rows, root)),
        vec![child],
        "the grandchild belongs to its own parent, not to the root"
    );
    assert!(subtasks_of(&rows, Uuid::from_u128(99)).is_empty());
}

/// The tree drops what the list drops: a tombstoned child never shows under its parent, which
/// is what keeps a deleted subtask from reappearing on the next read.
#[test]
fn a_deleted_child_is_absent_from_the_tree() {
    let root = Uuid::from_u128(1);
    let rows = vec![
        task(1),
        TaskRow {
            parent_id: Some(root),
            deleted_at: Some(at("2026-10-11T05:00:00Z")),
            ..task(2)
        },
        TaskRow {
            parent_id: Some(root),
            ..task(3)
        },
    ];
    assert_eq!(
        borrowed_ids(&subtasks_of(&rows, root)),
        vec![Uuid::from_u128(3)]
    );
}

/// The children are ordered by the same rule as everything else, so a breakdown's order is
/// the order the student reads.
#[test]
fn subtasks_are_in_order_tasks_order() {
    let root = Uuid::from_u128(1);
    let rows = vec![
        task(1),
        TaskRow {
            parent_id: Some(root),
            position: 2,
            ..task(2)
        },
        TaskRow {
            parent_id: Some(root),
            position: 0,
            ..task(3)
        },
        TaskRow {
            parent_id: Some(root),
            position: 1,
            ..task(4)
        },
    ];
    assert_eq!(
        borrowed_ids(&subtasks_of(&rows, root)),
        vec![Uuid::from_u128(3), Uuid::from_u128(4), Uuid::from_u128(2)]
    );
}

/// The field rules, and the message each failure carries. A validator whose message does not
/// name the violation is a 400 the student cannot act on, and the push path's rejection
/// reason is the same string.
#[test]
fn the_field_rules_name_what_is_wrong() {
    let id = Uuid::from_u128(1);
    let ok = |title: &str, priority: i32, estimate: Option<i32>| {
        validate(id, None, title, priority, estimate)
    };

    assert!(ok("Tugas kripto", MIN_PRIORITY, Some(1)).is_ok());
    assert!(ok("Tugas kripto", MAX_PRIORITY, Some(MAX_ESTIMATE_MIN)).is_ok());

    assert!(matches!(ok("   ", 2, None), Err(TaskError::TitleEmpty)));
    assert!(matches!(ok("", 2, None), Err(TaskError::TitleEmpty)));
    assert!(matches!(
        ok(&"a".repeat(MAX_TITLE + 1), 2, None),
        Err(TaskError::TitleTooLong)
    ));
    assert!(ok(&"a".repeat(MAX_TITLE), 2, None).is_ok());

    assert!(matches!(ok("Tugas", 0, None), Err(TaskError::Priority)));
    assert!(matches!(ok("Tugas", 4, None), Err(TaskError::Priority)));
    assert!(matches!(ok("Tugas", 2, Some(0)), Err(TaskError::Estimate)));
    assert!(matches!(
        ok("Tugas", 2, Some(MAX_ESTIMATE_MIN + 1)),
        Err(TaskError::Estimate)
    ));

    assert!(matches!(
        validate(id, Some(id), "Tugas", 2, None),
        Err(TaskError::SelfParent)
    ));
    assert!(validate(id, Some(Uuid::from_u128(2)), "Tugas", 2, None).is_ok());

    // Every message names the bound it enforces rather than saying "invalid".
    let message = ok("   ", 2, None).unwrap_err().to_string();
    assert!(message.contains("title"), "{message}");
    let message = ok("Tugas", 7, None).unwrap_err().to_string();
    assert!(message.contains("1") && message.contains("3"), "{message}");
    let message = ok("Tugas", 2, Some(9999)).unwrap_err().to_string();
    assert!(message.contains("1440"), "{message}");
}

/// The status serialises the way the column stores it, because the payload a client sends is
/// the string `todo | doing | done` and the SPA's filter reads the same three.
#[test]
fn the_status_serialises_lowercase() {
    assert_eq!(serde_json::to_value(TaskStatus::Todo).unwrap(), "todo");
    assert_eq!(serde_json::to_value(TaskStatus::Doing).unwrap(), "doing");
    assert_eq!(serde_json::to_value(TaskStatus::Done).unwrap(), "done");
    assert_eq!(
        serde_json::from_value::<TaskStatus>(serde_json::json!("doing")).unwrap(),
        TaskStatus::Doing
    );
    assert!(serde_json::from_value::<TaskStatus>(serde_json::json!("Doing")).is_err());
}
