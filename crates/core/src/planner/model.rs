//! The planner's value types. Contract § 6 of the master roadmap names these; this module
//! writes them in the field order and with the names that contract uses, so a reader can
//! hold the two side by side.
//!
//! `PlanProposal` serializes but does not deserialize (A264): the accept path validates the
//! blocks the client sends against the invariants rather than trusting a round-tripped
//! proposal.

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

/// Everything the planner reads. Pure data: no row, no handle, no clock. `now` is a field
/// rather than a call, which is what makes the whole algorithm testable against a fixed
/// instant and is what contract § 6 means by "same input ⇒ same output".
#[derive(Debug, Clone, PartialEq)]
pub struct PlannerInput {
    pub now: DateTime<Utc>,
    pub tz: Tz,
    pub settings: StudySettings,
    pub goals: Vec<GoalInput>,
    pub due_review_min: i64,
    pub busy: Vec<Interval>,
    pub pinned: Vec<Interval>,
    pub config: PlannerConfig,
}

/// The student's own study-window choices, read from `user_settings` (A256).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StudySettings {
    pub start_hour: u32,
    pub end_hour: u32,
    pub max_study_min_per_day: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoalInput {
    pub id: Uuid,
    pub title: String,
    pub target_date: DateTime<Utc>,
    /// The step-5 tie-break. Not a clock reading: it is the row's own `created_at`.
    pub created_at: DateTime<Utc>,
    pub topics: Vec<Topic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Topic {
    pub source_id: Option<Uuid>,
    pub name: String,
    pub remaining_min: i64,
    /// `None` when the topic has no quiz attempts in the last 14 days (A258).
    pub accuracy: Option<f64>,
}

/// A half-open instant range `[starts_at, ends_at)`. The unit of busy time, of a pinned
/// block and of a free slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// The six tunables contract § 6 fixes. The seventh, `max_horizon_days`, is a bound the
/// pure module cannot receive (A256/A259) and is a constant in `slots` instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannerConfig {
    pub block_len_min: i64,
    pub break_min: i64,
    pub deadline_buffer_min: i64,
    pub revisit_days: i64,
    pub spread_k: f64,
    pub balancer_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlanProposal {
    pub blocks: Vec<ProposedBlock>,
    pub day_load: Vec<DayLoad>,
    pub deficit_min: i64,
    pub warnings: Vec<PlanWarning>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ProposedBlock {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub goal_id: Uuid,
    pub topic: String,
    pub kind: BlockKind,
    /// True for a block that came in through `PlannerInput::pinned` (A265).
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum BlockKind {
    NewMaterial,
    Review,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DayLoad {
    pub date: NaiveDate,
    pub planned_min: i64,
    pub capacity_min: i64,
}

/// Exactly two variants, and the enum is closed (A262). A third is a contract change.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum PlanWarning {
    Overloaded { days: Vec<NaiveDate> },
    PastDeadline { goal_id: Uuid },
}
