//! The planner server: the only place a `PlannerInput` is built, and the only place a
//! `PlanProposal` becomes rows.
//!
//! `crates/core/src/planner` is pure — no database, no clock, no IO, no network (A256). This
//! module is the other half of that split: it reads the goal, the busy calendar, the due
//! cards and the quiz weaknesses, calls the one model that estimates minutes, and hands the
//! pure module a value. The effort estimate is the **only** model call in the path; the
//! placement is `stvdy_core::planner::propose`, which reads no clock.
//!
//! Two decisions live here rather than in the pure module, both because the pure module
//! cannot read `config/plans.toml`:
//!
//!  * **The horizon is the server's policy.** `slots::MAX_HORIZON_DAYS` mirrors
//!    `[planner].max_horizon_days` as a backstop, and [`input_for`] clips every goal's target
//!    date to `[today, today + max_horizon_days]` before composing the input. A test binds
//!    the constant to the shipped config value, because a mirrored constant that can drift
//!    silently is the "two sources of truth" class this project keeps catching.
//!  * **A pinned block's goal is resolved here.** Contract § 6's `Interval` carries no goal
//!    attribution, so the pure module emits a pinned block with a nil goal id and the topic
//!    `"Pinned"`; [`apply`] maps each back by its exact `(starts_at, ends_at)` and refuses
//!    when that lookup is ambiguous, because a block attributed to the wrong goal is a plan
//!    that studies the wrong subject.

pub mod apply;
pub mod routes;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

use stvdy_core::config::{PlanId, PlannerMode, PlannerTunables};
use stvdy_core::planner::progress::TopicAttempts;
use stvdy_core::planner::{GoalInput, Interval, PlannerConfig, PlannerInput, StudySettings, Topic};
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role};

use crate::jobs::Failure;
use crate::state::AppState;

/// The minutes one due flashcard is worth (A260).
///
/// A flashcard is a sub-minute item and a minute is the smallest unit every other number in
/// the planner uses, so one minute per card is the honest rounding. Documented as a constant
/// rather than a tunable because it is a unit conversion, not a policy.
pub const DUE_CARD_MIN: i64 = 1;

/// The model temperature for an effort estimate: low, because the reply is a set of numbers
/// rather than a piece of writing.
const EFFORT_TEMPERATURE: f32 = 0.2;

/// Enough for one entry per topic plus a reason each. The contract's own bound
/// (`MAX_ESTIMATE_MIN`) is what keeps the *numbers* finite; this keeps the call finite.
const EFFORT_MAX_TOKENS: u32 = 2000;

/// What the model is told when its first reply was not usable.
const EFFORT_CORRECTION: &str = "Your previous reply was not a valid JSON object matching the schema. Reply with only \
     the JSON object, with no prose before or after it and no markdown fences.";

/// The `user_settings` row, as the planner reads it (A256).
#[derive(Debug, Clone, Copy)]
pub struct StudySettingsRow {
    pub start_hour: u32,
    pub end_hour: u32,
    pub max_study_min_per_day: i64,
    pub block_len_min: i64,
    pub break_min: i64,
}

impl Default for StudySettingsRow {
    /// The schema's own defaults (`0012`), used when an account has no settings row yet.
    /// Not a policy: the columns are `not null default`, so a row that exists always has
    /// them, and this is what a user created before the row was written reads as.
    fn default() -> Self {
        Self {
            start_hour: 8,
            end_hour: 22,
            max_study_min_per_day: 240,
            block_len_min: 50,
            break_min: 10,
        }
    }
}

impl From<StudySettingsRow> for StudySettings {
    fn from(row: StudySettingsRow) -> Self {
        StudySettings {
            start_hour: row.start_hour,
            end_hour: row.end_hour,
            max_study_min_per_day: row.max_study_min_per_day,
        }
    }
}

/// The `[planner]` tunables with the student's own three knobs composed in (A255/A256/A257).
///
/// This is the **one** place a `PlannerMode` becomes a `PlannerConfig`, which is what makes
/// "balancer and weak-topic blocks on Power" structural rather than a copy decision a screen
/// could get wrong — and why a free account cannot get a balanced plan by calling the route
/// directly.
///
/// `Manual` never reaches the planner: `Feature::PlanGeneration` is already
/// `planner != manual`, so the route refuses with 403 `plan_required` / `min_plan: pro`
/// before a proposal row exists. It maps to the same config as `Auto` rather than panicking,
/// because a total function is what a caller with a `PlannerMode` needs.
pub fn config_from(
    settings: &StudySettingsRow,
    mode: PlannerMode,
    tunables: &PlannerTunables,
) -> PlannerConfig {
    let (balancer_enabled, spread_k) = match mode {
        PlannerMode::Manual | PlannerMode::Auto => (false, 0.0),
        PlannerMode::AutoBalancerWeaknessBlocks => (true, tunables.spread_k),
    };

    PlannerConfig {
        block_len_min: settings.block_len_min,
        break_min: settings.break_min,
        deadline_buffer_min: tunables.deadline_buffer_min,
        revisit_days: tunables.revisit_days,
        spread_k,
        balancer_enabled,
    }
}

/// A calendar row, as the busy computation reads it.
#[derive(Debug, Clone, Copy)]
pub struct EventRow {
    pub kind: EventKind,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// The two kinds of event the planner treats differently.
///
/// A `study_block` is the planner's own output coming back: a block that has already started
/// is busy (the student is sitting in it), and one that has not is pinned (A265). Everything
/// else — a class, an exam, a deadline, a synced external row — is busy in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    StudyBlock,
    Busy,
}

impl EventKind {
    /// The `events.kind` values that are *not* the planner's own output.
    ///
    /// `study_block` is the one value the planner writes; every other value `events.kind`
    /// documents (`class | exam | deadline | external`) is time the student is already
    /// committed to.
    pub fn of(kind: &str) -> Self {
        if kind == "study_block" {
            Self::StudyBlock
        } else {
            Self::Busy
        }
    }
}

/// A task row with a due date, as the busy computation reads it.
#[derive(Debug, Clone, Copy)]
pub struct TaskRow {
    pub due_at: DateTime<Utc>,
}

/// Busy time: every event passed in, plus every task that is due.
///
/// The caller passes only the events that are busy — [`input_for`] queries
/// `kind <> 'study_block'` and adds the goal's already-started blocks separately (A265) — so
/// this function is the merge and nothing else. The tasks contribute an *instant*, not a
/// duration: `tasks` has `estimate_min` but no scheduled range, so the honest reading is that
/// a task's deadline is time the student has spoken for. A one-minute interval is what makes
/// that expressible in the half-open vocabulary the planner uses.
pub fn busy_from(events: &[EventRow], tasks: &[TaskRow]) -> Vec<Interval> {
    let mut intervals: Vec<Interval> = events
        .iter()
        .map(|event| Interval {
            starts_at: event.starts_at,
            ends_at: event.ends_at,
        })
        .chain(tasks.iter().map(|task| Interval {
            starts_at: task.due_at,
            ends_at: task.due_at + Duration::minutes(1),
        }))
        .collect();
    // Sorted here as well as inside the planner, so a caller that logs the busy set sees the
    // same order the planner merges: two orders for one list is how a debugging session
    // starts with "the planner ignored a block that is clearly there".
    intervals.sort_by_key(|interval| (interval.starts_at, interval.ends_at));
    intervals
}

/// The goal's sources as topics, in a canonical order, with no estimates yet.
///
/// A topic *is* a source: the effort prompt's `topics_json` names the student's own material,
/// and `reconcile` maps the reply back by name. `remaining_min` is 0 here because the estimate
/// is the model's and arrives later; [`input_for`] fills it.
pub async fn topics_for(
    state: &AppState,
    user_id: Uuid,
    source_ids: &[Uuid],
    now: DateTime<Utc>,
) -> Result<Vec<Topic>, anyhow::Error> {
    if source_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "select id, title from sources
          where id = any($1) and user_id = $2 and deleted_at is null
          order by created_at, id",
    )
    .bind(source_ids)
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let since = now - Duration::days(ATTEMPT_WINDOW_DAYS);
    let accuracy = accuracy_by_source(state, user_id, source_ids, since).await?;

    Ok(rows
        .into_iter()
        .map(|(id, title)| Topic {
            source_id: Some(id),
            name: title,
            remaining_min: 0,
            accuracy: accuracy.get(&id).copied().flatten(),
        })
        .collect())
}

/// The window the planner weighs a topic's weakness over, matching the prep-progress card's
/// own 14 days (spec § Prep progress): a weakness is a recent fact, and an accuracy from last
/// term is not evidence about this exam.
pub const ATTEMPT_WINDOW_DAYS: i64 = 14;

/// The goal's topics as JSON, for the response body.
pub fn goal_topics(topics: &[Topic]) -> Vec<serde_json::Value> {
    topics
        .iter()
        .map(|topic| {
            serde_json::json!({
                "source_id": topic.source_id,
                "name": topic.name,
                "remaining_min": topic.remaining_min,
                // `null`, never 0: a topic with no attempts has no accuracy, and the
                // honesty rules forbid inventing one (A258).
                "accuracy": topic.accuracy,
            })
        })
        .collect()
}

/// One topic's attempts over the window, for `prep_progress`.
pub async fn topic_attempts(
    state: &AppState,
    user_id: Uuid,
    source_ids: &[Uuid],
    since: DateTime<Utc>,
) -> crate::error::AppResult<Vec<TopicAttempts>> {
    if source_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "select s.title,
                count(a.id)::bigint as attempts,
                coalesce(sum(case when a.correct then 1 else 0 end), 0)::bigint as correct
           from sources s
           left join chunks c on c.source_id = s.id
           left join quiz_questions q
                  on q.chunk_id = c.id and q.user_id = $2 and q.deleted_at is null
           left join quiz_attempts a
                  on a.question_id = q.id and a.user_id = $2
                 and a.deleted_at is null and a.answered_at >= $3
          where s.id = any($1) and s.user_id = $2 and s.deleted_at is null
          group by s.id, s.title
          order by s.title",
    )
    .bind(source_ids)
    .bind(user_id)
    .bind(since)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        crate::error::AppError::Internal(anyhow::anyhow!("reading the goal's attempts: {error}"))
    })?;

    Ok(rows
        .into_iter()
        .map(|(topic, attempts, correct)| TopicAttempts {
            topic,
            attempts,
            correct,
        })
        .collect())
}

/// The due cards across the decks drawn from the goal's sources.
///
/// Scoped to the goal's own material rather than to the account: a plan for one exam should
/// not absorb another course's review queue, and the deck's `source_ids` is the link the
/// schema already has. A goal with no sources therefore has no reviews to reserve, which is
/// the honest reading of "this goal's material".
pub async fn due_cards_for(
    state: &AppState,
    user_id: Uuid,
    source_ids: &[Uuid],
    now: DateTime<Utc>,
) -> Result<i64, anyhow::Error> {
    if source_ids.is_empty() {
        return Ok(0);
    }

    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from flashcards f
           join decks d on d.id = f.deck_id
          where f.user_id = $1 and f.deleted_at is null and f.due_at <= $2
            and d.deleted_at is null and d.source_ids && $3",
    )
    .bind(user_id)
    .bind(now)
    .bind(source_ids)
    .fetch_one(&state.pool)
    .await?;

    Ok(count)
}

/// `accuracy` per source id, or `None` for a source with no attempts in the window.
async fn accuracy_by_source(
    state: &AppState,
    user_id: Uuid,
    source_ids: &[Uuid],
    since: DateTime<Utc>,
) -> Result<std::collections::BTreeMap<Uuid, Option<f64>>, anyhow::Error> {
    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "select s.id,
                count(a.id)::bigint as attempts,
                coalesce(sum(case when a.correct then 1 else 0 end), 0)::bigint as correct
           from sources s
           left join chunks c on c.source_id = s.id
           left join quiz_questions q
                  on q.chunk_id = c.id and q.user_id = $2 and q.deleted_at is null
           left join quiz_attempts a
                  on a.question_id = q.id and a.user_id = $2
                 and a.deleted_at is null and a.answered_at >= $3
          where s.id = any($1) and s.user_id = $2 and s.deleted_at is null
          group by s.id",
    )
    .bind(source_ids)
    .bind(user_id)
    .bind(since)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, attempts, correct)| {
            let accuracy = (attempts > 0).then(|| correct as f64 / attempts as f64);
            (id, accuracy)
        })
        .collect())
}

/// Everything the planner reads for one goal, as a value.
///
/// Returns `Result<_, Failure>` rather than `AppResult<_, _>` on purpose: the classification
/// **is** the job's contract. A transport failure is transient and worth the worker's retry;
/// a goal that has been deleted, a reply `reconcile` refuses twice, or a missing provider are
/// permanent, and mapping them through `AppError` would collapse the two — a transient
/// failure classified permanent would refund a plan the student could have retried.
///
/// `now` is read here, once, from `state.clock`. Nothing below this call reads a clock, which
/// is what makes a proposal reproducible against a fixed instant.
pub async fn input_for(
    state: &AppState,
    user_id: Uuid,
    goal_id: Uuid,
) -> Result<PlannerInput, Failure> {
    let now = state.clock.now();

    let goal = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Vec<Uuid>,
        ),
    >(
        "select id, title, target_date, created_at, coalesce(source_ids, '{}') from goals
          where id = $1 and user_id = $2 and deleted_at is null",
    )
    .bind(goal_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the goal: {error}")))?
    .ok_or_else(|| Failure::permanent("the goal no longer exists"))?;
    let (id, title, target_date, created_at, source_ids) = goal;

    let user = load_user(state, user_id).await?;
    let settings = load_settings(state, user_id).await?;
    let tz: Tz = user
        .timezone
        .parse()
        .map_err(|_| Failure::permanent("the account's timezone is not a known zone"))?;

    let plan = crate::entitlements::effective_plan(&user.plan_id, user.subscription_status);
    let mode = state.config.plans.get(plan).planner;
    let tunables = state.config.plans.planner_tunables;
    let config = config_from(&settings, mode, &tunables);

    // The server is the policy for the horizon (A255): the pure module reads no config, so it
    // carries `slots::MAX_HORIZON_DAYS` as a backstop and this is where the goal is clipped to
    // it. A goal dated five years out would otherwise make the module's day loop unbounded —
    // the roadmap's own risk register calls that "a real server hang" — and the backstop would
    // silently produce a different plan from the one the config asks for.
    let horizon_end = now + Duration::days(tunables.max_horizon_days);
    let target_date = target_date.clamp(now, horizon_end);

    let mut topics = topics_for(state, user_id, &source_ids, now)
        .await
        .map_err(|error| Failure::transient(format!("loading the goal's topics: {error}")))?;
    let estimates = estimate_effort(state, &user, &title, target_date, &topics).await?;
    // `reconcile` returns the estimates in the caller's order and refuses a reply that drops,
    // invents or duplicates a topic, so the pairing is positional and total.
    for (topic, (_, minutes)) in topics.iter_mut().zip(estimates) {
        topic.remaining_min = minutes;
    }

    let (busy, pinned) = load_calendar(state, user_id, now, horizon_end).await?;
    let due_cards = due_cards_for(state, user_id, &source_ids, now)
        .await
        .map_err(|error| Failure::transient(format!("counting the due cards: {error}")))?;

    Ok(PlannerInput {
        now,
        tz,
        settings: settings.into(),
        goals: vec![GoalInput {
            id,
            title,
            target_date,
            // The row's own `created_at`, which is the step-5 tie-break — a fact about the
            // row, not a clock reading. It is `default now()` and `not null` in effect, so the
            // fallback is unreachable rather than a policy.
            created_at: created_at.unwrap_or(now),
            topics,
        }],
        due_review_min: due_cards * DUE_CARD_MIN,
        busy,
        pinned,
        config,
    })
}

/// The goal's busy time and its pinned blocks (A265).
///
/// Busy is every non-`study_block` event overlapping the horizon plus every task due inside
/// it. Pinned is every `study_block` that starts after `now`, kept exactly; a block that has
/// **already started** joins busy instead, because contract step 2 subtracts `busy ∪ pinned`
/// and does not subtract `study_block` rows by default — without that, a replan would
/// cheerfully schedule new material over a session the student is sitting in.
///
/// The pinned set is every future block in the window, not only the planned goal's: a block
/// another goal put in the calendar is time the student has already committed, and keeping it
/// is what stops a replan of goal A from moving goal B's blocks. On accept, each pinned block
/// is attributed back to its own goal by its exact `(starts_at, ends_at)` ([`apply`]), so
/// "pinned" and "whose block this is" are separate questions with separate answers.
async fn load_calendar(
    state: &AppState,
    user_id: Uuid,
    now: DateTime<Utc>,
    horizon_end: DateTime<Utc>,
) -> Result<(Vec<Interval>, Vec<Interval>), Failure> {
    let rows: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "select kind, starts_at, ends_at from events
          where user_id = $1 and deleted_at is null
            and ends_at > $2 and starts_at < $3
          order by starts_at, ends_at, id",
    )
    .bind(user_id)
    .bind(now)
    .bind(horizon_end)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the calendar: {error}")))?;

    let tasks: Vec<(DateTime<Utc>,)> = sqlx::query_as(
        "select due_at from tasks
          where user_id = $1 and deleted_at is null
            and due_at is not null and due_at >= $2 and due_at < $3
          order by due_at, id",
    )
    .bind(user_id)
    .bind(now)
    .bind(horizon_end)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the tasks: {error}")))?;

    let mut busy_events: Vec<EventRow> = Vec::new();
    let mut pinned: Vec<Interval> = Vec::new();
    for (kind, starts_at, ends_at) in rows {
        match EventKind::of(&kind) {
            EventKind::StudyBlock if starts_at > now => {
                pinned.push(Interval { starts_at, ends_at })
            }
            // A study block that has already begun is time the student is in, so it joins
            // the busy set rather than the pinned one (A265).
            EventKind::StudyBlock => busy_events.push(EventRow {
                kind: EventKind::Busy,
                starts_at,
                ends_at,
            }),
            EventKind::Busy => busy_events.push(EventRow {
                kind: EventKind::Busy,
                starts_at,
                ends_at,
            }),
        }
    }

    let task_rows: Vec<TaskRow> = tasks
        .into_iter()
        .map(|(due_at,)| TaskRow { due_at })
        .collect();

    Ok((busy_from(&busy_events, &task_rows), pinned))
}

/// The account facts the prompt's base needs and the plan mapping reads.
pub struct UserRow {
    pub email: String,
    pub timezone: String,
    pub plan_id: String,
    pub subscription_status: stvdy_core::entitlements::SubStatus,
}

async fn load_user(state: &AppState, user_id: Uuid) -> Result<UserRow, Failure> {
    let row = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "select u.email, u.timezone, u.plan_id,
                (select sub.status from subscriptions sub
                  where sub.user_id = u.id
                  order by (sub.status in ('active', 'grace')) desc,
                           sub.current_period_end desc
                  limit 1) as subscription_status
           from users u where u.id = $1 and u.deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the account: {error}")))?
    .ok_or_else(|| Failure::permanent("the account no longer exists"))?;

    Ok(UserRow {
        email: row.0,
        timezone: row.1,
        plan_id: row.2,
        subscription_status: row
            .3
            .as_deref()
            .map(stvdy_core::entitlements::SubStatus::parse)
            .unwrap_or(stvdy_core::entitlements::SubStatus::None),
    })
}

/// How far back the accept validation reads the calendar.
///
/// A day, because a block the client drags is a block in the near future and the planner's
/// own horizon is forward-only; reading the whole table would be a query that grows with the
/// account for a fact about a week.
pub const EVENT_LOOKBACK_DAYS: i64 = 1;

/// How far forward the accept validation reads the calendar. Matches the planner's own
/// horizon bound, so a block the planner could have proposed is a block the validator can
/// check.
pub const EVENT_LOOKAHEAD_DAYS: i64 = 120;

/// The student's own study-window choices (A256), or the schema's defaults when the account
/// has no row yet.
pub async fn settings_for(
    state: &AppState,
    user_id: Uuid,
) -> crate::error::AppResult<StudySettingsRow> {
    load_settings(state, user_id)
        .await
        .map_err(|failure| crate::error::AppError::Internal(anyhow::anyhow!("{}", failure.message)))
}

/// The student's own study-window choices (A256), or the schema's defaults when the account
/// has no row yet.
async fn load_settings(state: &AppState, user_id: Uuid) -> Result<StudySettingsRow, Failure> {
    let row = sqlx::query_as::<_, (i32, i32, i32, i32, i32)>(
        "select study_start_hour, study_end_hour, max_study_min_per_day,
                block_len_min, break_min
           from user_settings where user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the study settings: {error}")))?;

    Ok(row
        .map(|(start, end, max, block, brk)| StudySettingsRow {
            // The schema's bounds (`0028`) are `0..=23` and `1..=24`, so the casts are exact;
            // a value outside them cannot exist, and `clamp` is what keeps that true if the
            // constraints are ever relaxed.
            start_hour: start.clamp(0, 23) as u32,
            end_hour: end.clamp(1, 24) as u32,
            max_study_min_per_day: i64::from(max),
            block_len_min: i64::from(block),
            break_min: i64::from(brk),
        })
        .unwrap_or_default())
}

/// The one model call: minutes per topic, and nothing else.
///
/// `planner_effort.v1.md` says in its own text that the scheduler is deterministic code that
/// runs after it, and `reconcile` refuses a reply that answers a scheduling question anyway —
/// a `Schema` error on the drifted shape. A reply that drops, invents or duplicates a topic is
/// retried once and then permanent, because a plan built from a partial estimate would leave
/// a topic the student is examined on with no time at all.
async fn estimate_effort(
    state: &AppState,
    user: &UserRow,
    goal_title: &str,
    target_date: DateTime<Utc>,
    topics: &[Topic],
) -> Result<Vec<(String, i64)>, Failure> {
    if topics.is_empty() {
        return Ok(Vec::new());
    }

    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let plan = crate::entitlements::effective_plan(&user.plan_id, user.subscription_status);
    let base = crate::chat::prompt::base_vars(
        state,
        &user.email,
        &user.timezone,
        plan,
        false,
        false,
        // Task modules compose on v1's trimmed base, which has no `{{sources_only}}`; the
        // value is never read, and `true` is what every other task module passes.
        true,
        "not set",
    );

    let names: Vec<String> = topics.iter().map(|topic| topic.name.clone()).collect();
    let topics_json = serde_json::to_string(
        &topics
            .iter()
            .map(|topic| {
                serde_json::json!({
                    "topic": topic.name,
                    "accuracy": topic.accuracy,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| Failure::permanent(format!("building the topic list: {error}")))?;

    let mut module_vars: std::collections::BTreeMap<&'static str, String> =
        std::collections::BTreeMap::new();
    module_vars.insert("goal_title", goal_title.to_string());
    module_vars.insert(
        "target_date",
        target_date
            .with_timezone(&user.timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC))
            .format("%Y-%m-%d")
            .to_string(),
    );
    module_vars.insert("topics_json", topics_json);

    let composed = stvdy_core::prompts::compose_task(
        stvdy_core::prompts::ModuleId::PlannerEffortV1,
        &base,
        &module_vars,
        &[],
    )
    .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    let request = ChatRequest {
        model: String::new(),
        messages: vec![ChatMsg {
            role: Role::System,
            content: composed.text.clone(),
        }],
        tools: Vec::new(),
        temperature: EFFORT_TEMPERATURE,
        max_tokens: EFFORT_MAX_TOKENS,
        json_mode: true,
    };

    let first = provider
        .complete(request.clone())
        .await
        .map_err(|error| Failure::transient(format!("calling the model: {error}")))?;

    match reconcile_reply(&first.text, &names) {
        Ok(estimates) => Ok(estimates),
        Err(error) if stvdy_core::generation::is_retryable(&error) => {
            let mut retry = request;
            retry.messages.push(ChatMsg {
                role: Role::Assistant,
                content: first.text.clone(),
            });
            retry.messages.push(ChatMsg {
                role: Role::User,
                content: format!("{EFFORT_CORRECTION}\n\nThe problem was: {error}"),
            });
            let second = provider
                .complete(retry)
                .await
                .map_err(|error| Failure::transient(format!("retrying the model call: {error}")))?;
            reconcile_reply(&second.text, &names).map_err(|error| {
                Failure::permanent(format!(
                    "the model did not return a usable estimate after a retry: {error}"
                ))
            })
        }
        Err(error) => Err(Failure::permanent(format!(
            "the model's estimate was unusable: {error}"
        ))),
    }
}

/// Parses and reconciles one reply against the topics that were asked about.
fn reconcile_reply(
    raw: &str,
    requested: &[String],
) -> Result<Vec<(String, i64)>, stvdy_core::generation::GenerationError> {
    let file = stvdy_core::generation::effort::parse_effort(raw)?;
    stvdy_core::generation::effort::reconcile(&file, requested)
}

/// The plan's `PlannerMode`, for a caller that has the plan rather than an `AuthUser`.
///
/// A one-line read, kept as a function so "which plan's planner mode" has one answer: the
/// agent's `find_free_slots` tool and the two plan routes all ask this same question, and a
/// second `plans.get(plan).planner` in a handler is a second place a mode could be read from
/// the wrong plan.
pub fn mode_for(state: &AppState, plan: PlanId) -> PlannerMode {
    state.config.plans.get(plan).planner
}
