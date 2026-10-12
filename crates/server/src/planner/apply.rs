//! `POST /plans/accept`: the client's blocks, validated and written — or none of them.
//!
//! The client may drag; the server still checks (A264). A block that overlaps `busy`, leaves
//! the study window, or pushes its day past `max_study_min_per_day` is a 400 naming the
//! index — not a silently dropped row, because a preview that accepted the drag and then
//! wrote something else is worse than a refusal.
//!
//! **The pinned block's goal is resolved here, and an ambiguous lookup is a refusal.** A
//! pinned block arrives from the pure module with `goal_id: Uuid::nil()` and the topic
//! `"Pinned"` — contract § 6's `Interval` carries no goal attribution and adding one would be
//! a contract change. The server supplied those intervals, so it maps each back to the row
//! that produced it by its exact `(starts_at, ends_at)`. If more than one committed block
//! shares that range the answer is not knowable, and a block attributed to the wrong goal is
//! a plan that studies the wrong subject — so it refuses rather than guessing.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use uuid::Uuid;

use stvdy_core::planner::slots;
use stvdy_core::planner::{BlockKind, Interval, PlannerInput};

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::planner::{EVENT_LOOKAHEAD_DAYS, EVENT_LOOKBACK_DAYS};
use crate::state::AppState;

/// The topic a pinned block carries out of the pure module (A265).
pub const PINNED_TOPIC: &str = "Pinned";

/// The most blocks one accept may carry. Bounded because each one is a row and a validation,
/// and a plan is a week of study blocks rather than a client's arbitrary list.
const MAX_BLOCKS: usize = 400;

/// The longest a block's topic may be, matching the goal title's own bound: it becomes an
/// event title, and a title a row cannot render is a title the student cannot read.
const MAX_TOPIC_CHARS: usize = 200;

/// One block the client accepted, as it sends it back.
///
/// `deny_unknown_fields` is what makes a drifted client fail loudly: a field the server does
/// not read would be silently dropped and the student's adjustment with it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptBlock {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub goal_id: Uuid,
    pub topic: String,
    /// `"review"` (case-insensitively) for a review-reserved block, anything else for new
    /// material. A string rather than the core enum because the client round-trips the JSON
    /// the proposal carried, and accepting both spellings is cheaper than making the two
    /// ends agree on one.
    #[serde(default)]
    pub kind: String,
}

impl AcceptBlock {
    fn is_review(&self) -> bool {
        self.kind.eq_ignore_ascii_case("review")
    }

    /// Whether this block came out of the planner's pinned set (A265).
    fn is_pinned(&self) -> bool {
        self.goal_id.is_nil() && self.topic == PINNED_TOPIC
    }

    fn minutes(&self) -> i64 {
        (self.ends_at - self.starts_at).num_minutes()
    }
}

/// The body of `POST /plans/accept`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptBody {
    pub proposal_id: Uuid,
    pub blocks: Vec<AcceptBlock>,
}

/// One accepted block after its goal has been resolved.
struct Resolved {
    index: usize,
    goal_id: Uuid,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    title: String,
    /// True when the block is a committed block the planner pinned, so its event already
    /// exists and must be **kept exactly** rather than written again.
    reuse: bool,
}

/// Validates the client's blocks and writes them, or writes none of them (A264).
///
/// Returns the ids of the events the plan now consists of, in the order they were accepted,
/// so the client can select the rows it just created without a second read.
pub async fn accept(
    state: &AppState,
    user: &AuthUser,
    proposal_id: Uuid,
    blocks: Vec<AcceptBlock>,
) -> AppResult<Vec<Uuid>> {
    if blocks.is_empty() {
        return Err(AppError::Validation(
            "an accepted plan needs at least one block".to_string(),
        ));
    }
    if blocks.len() > MAX_BLOCKS {
        return Err(AppError::Validation(format!(
            "a plan may carry at most {MAX_BLOCKS} blocks"
        )));
    }

    let now = state.clock.now();
    let tz = crate::syllabus::zone(&user.timezone);

    let settings = crate::planner::settings_for(state, user.user_id).await?;
    let tunables = state.config.plans.planner_tunables;
    let mode = state.config.plans.get(user.effective_plan()).planner;
    let config = crate::planner::config_from(&settings, mode, &tunables);

    // The validation input: the real busy calendar, no pinned set (the accepted blocks *are*
    // the pinned set here), and the student's own window. Its `goals` is empty because this
    // is a validation rather than a plan.
    let validation = PlannerInput {
        now,
        tz,
        settings: settings.into(),
        goals: Vec::new(),
        due_review_min: 0,
        busy: Vec::new(),
        pinned: Vec::new(),
        config,
    };

    // Resolve every block's goal first, so a refusal names the block and nothing has been
    // written. A pinned block resolves through the committed row it came from; anything else
    // must name a goal the caller owns.
    let mut resolved: Vec<Resolved> = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        if block.ends_at <= block.starts_at {
            return Err(AppError::Validation(format!(
                "block {index}: ends_at must be after starts_at"
            )));
        }
        if block.topic.chars().count() > MAX_TOPIC_CHARS {
            return Err(AppError::Validation(format!(
                "block {index}: the topic must be at most {MAX_TOPIC_CHARS} characters"
            )));
        }

        let (goal_id, reuse) = if block.is_pinned() {
            (
                resolve_pinned(state, user.user_id, block.starts_at, block.ends_at).await?,
                true,
            )
        } else {
            (block.goal_id, false)
        };
        let goal = crate::goals::owned_goal(state, user.user_id, goal_id).await?;

        // A review-reserved block says "Review" and names its goal, so the calendar shows
        // what the session is for rather than a block that reads as new material (A269).
        // New material is named by its topic, which is the source the material came from.
        let title = if block.is_review() {
            format!("Review: {}", goal.title)
        } else {
            block.topic.clone()
        };

        resolved.push(Resolved {
            index,
            goal_id,
            starts_at: block.starts_at,
            ends_at: block.ends_at,
            title,
            reuse,
        });
    }

    // The blocked set: every event in the window, minus the committed blocks this accept is
    // keeping (which necessarily overlap themselves). A block may not overlap anything that
    // is not part of this plan.
    let kept: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>)> = resolved
        .iter()
        .filter(|block| block.reuse)
        .map(|block| (block.goal_id, block.starts_at, block.ends_at))
        .collect();
    let kept_events = kept_event_ids(state, user.user_id, &kept).await?;
    let mut blocked = blocked_intervals(state, user.user_id, now).await?;
    blocked.retain(|interval| {
        !kept
            .iter()
            .any(|(_, start, end)| interval.starts_at == *start && interval.ends_at == *end)
    });

    // The three contract invariants, per block: inside the study window, not overlapping
    // busy time, and within the day's maximum.
    let mut per_day: BTreeMap<chrono::NaiveDate, i64> = BTreeMap::new();
    for block in &resolved {
        let day = block.starts_at.with_timezone(&tz).date_naive();
        let window = slots::window_for(&validation, day).ok_or_else(|| {
            AppError::Validation(format!(
                "block {}: {} has no study window in this timezone",
                block.index, day
            ))
        })?;
        if block.starts_at < window.starts_at || block.ends_at > window.ends_at {
            return Err(AppError::Validation(format!(
                "block {}: {}–{} is outside the study window on {day}",
                block.index, block.starts_at, block.ends_at
            )));
        }
        if let Some(hit) = blocked
            .iter()
            .find(|busy| block.starts_at < busy.ends_at && busy.starts_at < block.ends_at)
        {
            return Err(AppError::Validation(format!(
                "block {}: {}–{} overlaps busy time ({}–{})",
                block.index, block.starts_at, block.ends_at, hit.starts_at, hit.ends_at
            )));
        }

        let load = per_day.entry(day).or_insert(0);
        *load += (block.ends_at - block.starts_at).num_minutes();
        if *load > settings.max_study_min_per_day {
            return Err(AppError::Validation(format!(
                "block {}: {day} would hold {} minutes, over the {} minute daily maximum",
                block.index, *load, settings.max_study_min_per_day
            )));
        }
    }

    // One transaction: the proposal's status change and every block commit together, or the
    // student is left with half a plan and a proposal that says `accepted`.
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning an accept: {error}")))?;

    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update plan_proposals set status = 'accepted', updated_at = now()
          where id = $1 and user_id = $2 and status = 'ready'
         returning id",
    )
    .bind(proposal_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("claiming the proposal: {error}")))?;

    let Some(_) = claimed else {
        // Either it is another account's, it does not exist, or it is not `ready` — a second
        // accept of the same plan lands here, which is the idempotence the endpoint promises.
        // Distinguished below so the client is told which.
        let status: Option<String> =
            sqlx::query_scalar("select status from plan_proposals where id = $1 and user_id = $2")
                .bind(proposal_id)
                .bind(user.user_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("reading the proposal: {error}"))
                })?;
        return match status {
            None => Err(AppError::NotFound),
            Some(status) => Err(AppError::Conflict(format!(
                "this proposal is {status} and cannot be accepted again"
            ))),
        };
    };

    let mut event_ids: Vec<Uuid> = Vec::with_capacity(resolved.len());
    for block in &resolved {
        if block.reuse {
            // Kept exactly: the committed block already exists, so its id is returned and
            // nothing is written. Writing it again would double the student's calendar.
            if let Some((_, _, _, id)) = kept_events.iter().find(|(goal, start, end, _)| {
                *goal == block.goal_id && *start == block.starts_at && *end == block.ends_at
            }) {
                event_ids.push(*id);
                continue;
            }
        }

        let id: Uuid = sqlx::query_scalar(
            "insert into events
               (id, user_id, workspace_id, kind, title, starts_at, ends_at, source, goal_id)
             select $1, $2, g.workspace_id, 'study_block', $3, $4, $5, 'app', $6
               from goals g where g.id = $6 and g.user_id = $2
             returning id",
        )
        .bind(Uuid::now_v7())
        .bind(user.user_id)
        .bind(&block.title)
        .bind(block.starts_at)
        .bind(block.ends_at)
        .bind(block.goal_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("writing a study block: {error}")))?
        .ok_or_else(|| {
            AppError::Validation(format!("block {}: the goal does not exist", block.index))
        })?;
        event_ids.push(id);
    }

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing an accept: {error}")))?;

    tracing::info!(
        %proposal_id,
        user_id = %user.user_id,
        blocks = event_ids.len(),
        "plan accepted"
    );
    Ok(event_ids)
}

/// Every event in the window that blocks a new block: classes, exams, synced rows, tasks that
/// are due, and committed study blocks.
///
/// Read over a bounded window rather than the whole table, and over the same lookback the
/// planner uses, so the two agree about what "busy" means.
async fn blocked_intervals(
    state: &AppState,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> AppResult<Vec<Interval>> {
    let from = now - Duration::days(EVENT_LOOKBACK_DAYS);
    let to = now + Duration::days(EVENT_LOOKAHEAD_DAYS);
    let events: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "select starts_at, ends_at from events
          where user_id = $1 and deleted_at is null and ends_at > $2 and starts_at < $3
          order by starts_at, ends_at, id",
    )
    .bind(user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the calendar: {error}")))?;

    let tasks: Vec<(DateTime<Utc>,)> = sqlx::query_as(
        "select due_at from tasks
          where user_id = $1 and deleted_at is null and due_at is not null
            and due_at >= $2 and due_at < $3
          order by due_at, id",
    )
    .bind(user_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the tasks: {error}")))?;

    Ok(events
        .into_iter()
        .map(|(starts_at, ends_at)| Interval { starts_at, ends_at })
        .chain(tasks.into_iter().map(|(due_at,)| Interval {
            starts_at: due_at,
            ends_at: due_at + Duration::minutes(1),
        }))
        .collect())
}

/// The committed blocks this accept is keeping, by exact range.
///
/// Read over the caller's own `study_block` rows with a goal attached: a block with no goal is
/// not one a plan can keep, because there is no goal to attribute it to.
async fn kept_event_ids(
    state: &AppState,
    user_id: Uuid,
    kept: &[(Uuid, DateTime<Utc>, DateTime<Utc>)],
) -> AppResult<Vec<(Uuid, DateTime<Utc>, DateTime<Utc>, Uuid)>> {
    if kept.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>, Uuid)> = sqlx::query_as(
        "select goal_id, starts_at, ends_at, id from events
          where user_id = $1 and kind = 'study_block' and deleted_at is null
            and goal_id is not null
          order by starts_at, id",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the kept blocks: {error}")))?;

    Ok(rows
        .into_iter()
        .filter(|(goal, start, end, _)| {
            kept.iter()
                .any(|(k, s, e)| k == goal && s == start && e == end)
        })
        .collect())
}

/// The goal a pinned block came from, by its exact `(starts_at, ends_at)`.
///
/// **A refusal, not a guess, when the lookup is ambiguous.** The pure module emits a pinned
/// block with a nil goal id because contract § 6's `Interval` carries no goal attribution, so
/// the only thing that can say which goal a pinned block belongs to is the committed row it
/// was read from. Two committed blocks sharing one exact range make that unknowable, and a
/// block attributed to the wrong goal is a plan that studies the wrong subject — so the
/// accept refuses and the student keeps the plan they have.
pub async fn resolve_pinned(
    state: &AppState,
    user_id: Uuid,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
) -> AppResult<Uuid> {
    let goals: Vec<Option<Uuid>> = sqlx::query_scalar(
        "select goal_id from events
          where user_id = $1 and kind = 'study_block' and deleted_at is null
            and starts_at = $2 and ends_at = $3",
    )
    .bind(user_id)
    .bind(starts_at)
    .bind(ends_at)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("resolving a pinned block's goal: {error}"))
    })?;

    match goals.as_slice() {
        [Some(goal_id)] => Ok(*goal_id),
        // No committed block at that range: the proposal does not describe the account's
        // calendar any more, so there is nothing to keep exactly.
        [] => Err(AppError::Conflict(
            "this proposal no longer matches the calendar it was built from; replan it".to_string(),
        )),
        // More than one: the answer is not knowable from the range alone.
        _ => Err(AppError::Conflict(
            "more than one committed block occupies that time; replan instead of accepting"
                .to_string(),
        )),
    }
}

/// The block kind a title is written for, kept beside [`AcceptBlock::is_review`] so the two
/// spellings cannot drift.
pub fn kind_of(block: &AcceptBlock) -> BlockKind {
    if block.is_review() {
        BlockKind::Review
    } else {
        BlockKind::NewMaterial
    }
}
