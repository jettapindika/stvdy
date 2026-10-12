//! The agent turn: the one place a tool call becomes an effect.
//!
//! The chat route holds no tool logic (A271). It hands every `StreamEvent::ToolCall` to
//! [`handle`], which asks the pure [`gate`] what the call may do and then does exactly that:
//!
//!  * [`GateDecision::Execute`] — a READ tool runs here and now, through the same query the
//!    REST route runs ([`run_read`]), and its result goes back to the model as a
//!    `tool_result`;
//!  * [`GateDecision::Propose`] — a WRITE tool becomes one `pending_actions` row and one SSE
//!    `action` event. **Nothing is written to a domain table**: the only thing that turns a
//!    proposal into a row is `POST /actions/:id/confirm`, which is why a poisoned document
//!    cannot cause a write by construction (A279);
//!  * [`GateDecision::Refuse`] — the model is told why, so it can say so in the student's own
//!    language. Dropping the call instead (which is what the route did before this phase)
//!    leaves the model answering as if it had succeeded.
//!
//! The turn's cost is here rather than in the route for the same reason: `agent_turn` is
//! charged **once per turn, at the first tool call** (A282), which is a fact about the turn
//! that only the code seeing the calls can know. A spend that refuses the turn becomes
//! `Refusal::InsufficientCredits` or `Refusal::UsageLimit` — constructed here, because a
//! balance and a spent provider-cost ceiling are database facts and `gate` reads nothing —
//! and reaches the model through the same `tool_result` path as every other refusal.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use stvdy_core::agent::gate::{GateContext, GateDecision, Refusal, gate, read_result, tool_result};
use stvdy_core::agent::tools::{ToolArgs, ToolName};
use stvdy_core::traits::llm::{ChatMsg, ToolCall, ToolSpec};

use stvdy_core::entitlements::Feature;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The action name the agent turn is charged under, which is also its price key.
///
/// `CreditAction::AgentTurn` and `config/plans.toml`'s `agent_turn = 2` already exist, and
/// charging nothing would leave a priced action with no reader (A282).
const AGENT_TURN: &str = "agent_turn";

/// How far `get_upcoming_deadlines` may look when the call names nothing. The tool's own
/// `DEFAULT_DEADLINE_DAYS` is the same number and is what the schema tells the model, so this
/// is only the fallback for a caller that reached here without parsing.
const DEFAULT_DEADLINE_DAYS: i64 = 7;

/// The tools this plan may be offered (A272): the four READ tools always, the six WRITE tools
/// only when `Feature::AgentAction` is granted.
///
/// A free account is therefore offered no write tool at all, so a model cannot propose an
/// action the server would refuse — which is what makes `Refusal::NotOnPlan` a hallucination
/// guard rather than an everyday path.
pub async fn tool_specs(state: &AppState, user: &AuthUser) -> Vec<ToolSpec> {
    stvdy_core::agent::tools::specs(agent_actions_granted(state, user).await)
}

/// Whether the plan grants the six WRITE tools.
///
/// **The only caller is [`tool_specs`]**, so the wire's tool list and this answer cannot
/// disagree (A272). The answer comes from `entitlements::grants`, which asks the same engine
/// `require` does — reading `plans.get(plan).agent_actions` here would be a second reader of a
/// plan field, which is the second source of truth A149 forbids and the sweep's
/// `no_file_outside_the_entitlements_layer_reads_a_plan_limit` fails on.
pub async fn agent_actions_granted(state: &AppState, user: &AuthUser) -> bool {
    crate::entitlements::grants(state, user, Feature::AgentAction).await
}

/// What one tool call produced.
pub enum AgentOutcome {
    /// A READ tool ran; the result goes back to the model as a `tool_result`.
    Answered(ChatMsg),
    /// A WRITE tool became a proposal; the card is streamed and the model is told.
    Proposed {
        pending_action_id: Uuid,
        result: ChatMsg,
    },
    /// The gate refused; the model is told why and can say so.
    Refused(ChatMsg),
}

/// The turn's own accounting, threaded through every call by the caller.
///
/// The charge happens at the **first** handled call and never again (A282), which is why the
/// flag lives here rather than being derived from the call: a turn with three tool calls pays
/// `agent_turn` once, and a turn with none pays nothing beyond `chat_message`.
#[derive(Debug)]
pub struct Turn {
    /// Whether this turn is running on the account's own provider key (A302).
    ///
    /// A BYOK turn is not charged at all — `chat_message` is skipped for it in the chat route,
    /// and charging `agent_turn` here would make the agent the one thing a BYOK account still
    /// pays platform credits for. The tool the server ran is real work, but it is work the
    /// student's own key paid for, which is what `usage_events.byok` records.
    own_key: bool,
    /// Set once the `agent_turn` charge has been attempted, so a second call does not pay
    /// again — including when the attempt failed, because retrying a balance check per call
    /// would make the price of a turn depend on how many tools the model chose.
    charged: bool,
    /// Why the charge was refused, once it has been.
    ///
    /// The charge is attempted once (above), so a later call in the same turn skips the spend
    /// — and without remembering the reason, that skip would read as "already paid" and the
    /// call would run: a turn that could not afford its first tool call would run its second
    /// for free. The reason is the **turn's**, not the call's, because the charge is: if the
    /// turn cannot afford `agent_turn`, it cannot afford any tool it goes on to call, and the
    /// model gets the same sentence it got the first time.
    refusal: Option<Refusal>,
    /// What the charge added, for the `usage` event: `chat_message + agent_turn` (A282).
    pub extra_cost: i64,
    /// The balance after the charge, when it succeeded.
    pub remaining: Option<i64>,
    /// How many WRITE calls this turn has proposed, which is what the gate's
    /// `MAX_PROPOSALS_PER_TURN` reads (A278).
    pub proposals: u8,
    /// Whether any call was handled at all, which is what makes a tool-only turn persist its
    /// message (A281).
    pub handled: bool,
}

impl Turn {
    pub fn new(own_key: bool) -> Self {
        Self {
            own_key,
            charged: false,
            refusal: None,
            extra_cost: 0,
            remaining: None,
            proposals: 0,
            handled: false,
        }
    }
}

/// The one place a tool call becomes an effect.
///
/// `turn_message_id` is the **user** message of this turn, not the assistant's: the assistant
/// message is persisted after the stream (A281), so it has no id yet when a proposal is
/// written. The user message is the turn's own anchor, and it exists before the model is
/// called.
pub async fn handle(
    state: &AppState,
    user: &AuthUser,
    workspace_id: Uuid,
    turn_message_id: Uuid,
    call: &ToolCall,
    ctx: &mut GateContext,
    turn: &mut Turn,
) -> AppResult<AgentOutcome> {
    let decision = gate(call, ctx);

    // The refusal is a decision, not an error (A271): it is answered, not dropped. Two of the
    // seven refusals can be decided here rather than by the gate — the balance and the plan's
    // monthly ceiling are database facts, and `gate` reads nothing — so the check below
    // replaces the decision when the turn cannot afford its own tool call.
    let decision = if matches!(decision, GateDecision::Refuse { .. }) {
        decision
    } else {
        match charge_turn(state, user, turn).await {
            Ok(()) => decision,
            Err(reason) => GateDecision::Refuse {
                tool: call.name.clone(),
                reason,
            },
        }
    };

    match &decision {
        GateDecision::Refuse { .. } => {
            // Not counted as handled: a refused call produced nothing, and a turn that only
            // had refusals has no card to hang a message on. Its `tool_result` still reaches
            // the model, which is the point.
            Ok(AgentOutcome::Refused(tool_result(call, &decision)))
        }
        GateDecision::Execute { tool, args } => {
            turn.handled = true;
            let result = run_read(state, user, workspace_id, *tool, args).await?;
            Ok(AgentOutcome::Answered(read_result(call, *tool, &result)))
        }
        GateDecision::Propose { tool, args } => {
            turn.handled = true;
            turn.proposals = turn.proposals.saturating_add(1);
            ctx.proposals_so_far = turn.proposals;
            let pending_action_id =
                insert_proposal(state, user, *tool, args, turn_message_id).await?;
            tracing::info!(
                pending_action_id = %pending_action_id,
                tool = tool.as_str(),
                user_id = %user.user_id,
                "a write tool proposed an action"
            );
            Ok(AgentOutcome::Proposed {
                pending_action_id,
                result: tool_result(call, &decision),
            })
        }
    }
}

/// Charges `agent_turn` once per turn, at the first tool call (A282).
///
/// `Err(reason)` is the spend's refusal, carried out as a [`Refusal`] rather than a tuple so
/// the two ways the spend can refuse — a balance that cannot cover the price, and the plan's
/// monthly provider-cost ceiling — stay two distinct sentences. The caller turns it into
/// `GateDecision::Refuse`. The daily cap slots are both empty: `agent_turn` is not a counted
/// allowance in the matrix, and passing the chat cap would refuse the tool call for a
/// *message* limit the message spend has already checked.
async fn charge_turn(state: &AppState, user: &AuthUser, turn: &mut Turn) -> Result<(), Refusal> {
    // A turn that was refused stays refused, and this check comes **before** the
    // `charged` one: the charge is attempted once, so a later call in the same turn skips the
    // spend — and if the skip were read as "already paid", the call would run. That is exactly
    // how an unaffordable turn wrote a proposal: the first call was refused, the model was told
    // and called the tool again, and the second call passed the `charged` guard and proposed.
    if let Some(reason) = turn.refusal.clone() {
        return Err(reason);
    }
    if turn.charged {
        return Ok(());
    }
    turn.charged = true;

    // A BYOK turn pays nothing: the chat route already skips `chat_message` for it (A302), and
    // charging `agent_turn` here would make the agent the one thing a BYOK account still buys
    // platform credits for. The tool did run, and the usage row's `byok` flag is what records
    // that the work happened on the account's own key.
    if turn.own_key {
        tracing::debug!("agent turn on the account's own key: nothing charged");
        return Ok(());
    }

    let cost = state
        .config
        .plans
        .credits
        .cost(stvdy_core::config::CreditAction::AgentTurn);
    if cost <= 0 {
        // A config with `agent_turn = 0` is a free agent turn, and `credits::spend` refuses a
        // zero-credit spend as a programming error rather than writing a meaningless ledger
        // row. The matrix's own value is 2, so this is the guard for a deliberate edit.
        return Ok(());
    }

    match crate::credits::spend(
        state,
        user.user_id,
        user.effective_plan(),
        AGENT_TURN,
        cost,
        [None, None],
        // Never `true`: the own-key case returns above without spending at all, so this call
        // is always a platform charge.
        false,
    )
    .await
    {
        Ok(spent) => {
            turn.extra_cost += cost;
            turn.remaining = Some(spent.remaining);
            Ok(())
        }
        // **Every** entitlement refusal refuses the turn, which is what this arm being on the
        // class rather than on one code buys. The arm this replaced named
        // `insufficient_credits` alone, so `limit_reached` — the provider-cost ceiling, which
        // `spend_in` checks *before* it reads a balance — fell through to the "real failure"
        // arm below, the turn continued as if it had paid, and the tool ran anyway.
        //
        // The two kinds stay distinct because they are two different sentences: a spent
        // balance and a reached usage ceiling are not the same fact, and the model is the one
        // that has to say the true one to the student.
        Err(AppError::Entitlement {
            code: "insufficient_credits",
            ..
        }) => {
            // `AppError::Entitlement` deliberately carries `limit`/`used` and not a balance,
            // so the numbers come from `entitlements::snapshot`. One extra read on the path
            // that is about to tell the student to top up is worth an accurate sentence.
            let available = crate::entitlements::snapshot(state, user)
                .await
                .map(|snapshot| snapshot.credits.total())
                .unwrap_or(0);
            let reason = Refusal::InsufficientCredits {
                needed: cost,
                available,
            };
            turn.refusal = Some(reason.clone());
            Err(reason)
        }
        // The ceiling, and the honest reading of any other entitlement refusal: the account
        // cannot buy this turn. Refusing is the safe direction, because the alternative — what
        // the old catch-all did — is running the call.
        Err(AppError::Entitlement { code, .. }) => {
            tracing::info!(%code, "the agent turn was refused for the account's plan");
            turn.refusal = Some(Refusal::UsageLimit);
            Err(Refusal::UsageLimit)
        }
        Err(error) => {
            // Anything else is a real failure — a lock timeout, a database error — and
            // swallowing it into a refusal would hide it behind a message the model reads as
            // "you are poor". It is recorded and the turn continues without the charge, which
            // is the same direction `record_usage` failures take: the answer is already being
            // produced and losing the charge is a reporting gap, not a reason to fail a turn
            // the student is watching.
            tracing::error!(%error, "charging the agent turn failed");
            Ok(())
        }
    }
}

/// Writes the proposal a WRITE tool produced (A271).
///
/// `expires_at` is bound from [`crate::actions::PENDING_ACTION_TTL_MIN`] rather than written
/// as a literal interval, because the migration that adds the column deliberately carries no
/// SQL default (A273) — this constant is the only place the number lives.
///
/// `message_id` anchors the card to the turn that produced it (A281): the client renders the
/// proposal beside the message it came from, and without the anchor a confirmed card has no
/// thread to appear in.
async fn insert_proposal(
    state: &AppState,
    user: &AuthUser,
    tool: ToolName,
    args: &ToolArgs,
    turn_message_id: Uuid,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    let args = serde_json::to_value(args).map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "serializing a proposal's arguments: {error}"
        ))
    })?;

    sqlx::query(
        "insert into pending_actions
           (id, user_id, tool, args, status, message_id, expires_at)
         values ($1, $2, $3, $4, 'pending', $5, now() + ($6 * interval '1 minute'))",
    )
    .bind(id)
    .bind(user.user_id)
    .bind(tool.as_str())
    .bind(args)
    .bind(turn_message_id)
    .bind(crate::actions::PENDING_ACTION_TTL_MIN)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing a proposal: {error}")))?;

    Ok(id)
}

/// A READ tool's execution. One arm per read tool, each calling the query the REST route
/// calls — no second implementation, for the same reason A276 exists.
pub async fn run_read(
    state: &AppState,
    user: &AuthUser,
    workspace_id: Uuid,
    tool: ToolName,
    args: &ToolArgs,
) -> AppResult<serde_json::Value> {
    match args {
        ToolArgs::ListTasks(args) => {
            // The status goes through `TaskStatus::as_str`, the same string the column stores
            // and the schema enumerates, so a call cannot filter on a spelling the database
            // does not have.
            let query = crate::tasks::ListQuery {
                status: args.status.map(|status| status.as_str().to_string()),
                workspace_id: None,
                parent_id: None,
                due_before: args.due_before,
                due_after: None,
                limit: Some(i64::from(args.limit)),
            };
            let tasks = crate::tasks::list_in(&state.pool, user.user_id, &query).await?;
            Ok(serde_json::json!({ "tasks": tasks }))
        }
        ToolArgs::FindFreeSlots(args) => {
            free_slots(state, user, args.from, args.to, args.duration_min).await
        }
        ToolArgs::GetDueFlashcards(args) => {
            let now = state.clock.now();
            let cards = crate::flashcards::routes::due_in(
                &state.pool,
                user.user_id,
                args.deck_id,
                now,
                i64::from(args.limit),
            )
            .await?;
            Ok(serde_json::json!({ "cards": cards, "now": now }))
        }
        ToolArgs::GetUpcomingDeadlines(args) => {
            upcoming_deadlines(state, user, workspace_id, args.days, args.limit).await
        }
        // A WRITE tool cannot reach here: the gate returns `Propose` for one and the caller
        // stores a row instead of running anything. The arm is a `500` naming the tool rather
        // than an `unreachable!`, because the alternative to a loud bug is a silent no-op on
        // a write.
        ToolArgs::CreateTask(_)
        | ToolArgs::UpdateTask(_)
        | ToolArgs::CreateEvent(_)
        | ToolArgs::ScheduleStudyBlock(_)
        | ToolArgs::CreateFlashcards(_)
        | ToolArgs::CreateQuiz(_) => Err(AppError::Internal(anyhow::anyhow!(
            "{} is a write tool and has no read path",
            tool.as_str()
        ))),
    }
}

/// `find_free_slots`: when the student can study between two instants.
///
/// The slot math is `stvdy_core::planner::slots::free_slots` — the planner's own function, and
/// the only implementation of "subtract what is busy from the study window" in the tree. The
/// alternative, re-deriving it from the busy list here, is the divergence class this phase
/// exists to prevent: the agent would tell a student a slot is free and the planner would then
/// place a block somewhere else, or refuse to place one at all.
///
/// The input is composed from the same three sources the planner server's own `input_for`
/// reads — the student's `user_settings` study window, the `[planner]` tunables through
/// [`crate::planner::config_from`], and the calendar — so this is a second *caller* of that
/// composition and not a second composition. `goals` is empty on purpose: a question about
/// the calendar is not a question about a goal, and the planner's allocation steps are not
/// reached by `free_slots`.
async fn free_slots(
    state: &AppState,
    user: &AuthUser,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    duration_min: u32,
) -> AppResult<serde_json::Value> {
    let settings = crate::settings::load_or_create(&state.pool, user.user_id).await?;
    let settings_row = crate::planner::StudySettingsRow {
        start_hour: u32::try_from(settings.study_start_hour).unwrap_or(8),
        end_hour: u32::try_from(settings.study_end_hour).unwrap_or(22),
        max_study_min_per_day: i64::from(settings.max_study_min_per_day),
        block_len_min: i64::from(settings.block_len_min),
        break_min: i64::from(settings.break_min),
    };

    let tz: chrono_tz::Tz = user.timezone.parse().unwrap_or(chrono_tz::Asia::Jakarta);
    let now = state.clock.now();

    // Busy time, from the same two reads the planner server uses: the caller's events in the
    // window, and the tasks due inside it. `busy_from` is that merge, and it is theirs.
    let events = crate::events::list_in(&state.pool, user.user_id, from, to).await?;
    let event_rows: Vec<crate::planner::EventRow> = events
        .iter()
        .map(|event| crate::planner::EventRow {
            kind: crate::planner::EventKind::of(&event.kind),
            starts_at: event.starts_at,
            ends_at: event.ends_at,
        })
        .collect();

    let due_query = crate::tasks::ListQuery {
        status: None,
        workspace_id: None,
        parent_id: None,
        due_before: Some(to),
        due_after: Some(from),
        limit: Some(i64::from(stvdy_core::agent::tools::bounds().max_list)),
    };
    let due_tasks = crate::tasks::list_in(&state.pool, user.user_id, &due_query).await?;
    let task_rows: Vec<crate::planner::TaskRow> = due_tasks
        .iter()
        .filter_map(|task| task.due_at.map(|due_at| crate::planner::TaskRow { due_at }))
        .collect();

    let input = stvdy_core::planner::PlannerInput {
        now,
        tz,
        settings: settings_row.into(),
        // Empty: `free_slots` reads neither, and a goal's demand is a question about a goal.
        goals: Vec::new(),
        due_review_min: 0,
        busy: crate::planner::busy_from(&event_rows, &task_rows),
        // Empty for the same reason the goal's own blocks are not loaded: this answers "when
        // is the student free", and a block they have not accepted is not time they hold.
        pinned: Vec::new(),
        config: crate::planner::config_from(
            &settings_row,
            crate::planner::mode_for(state, user.effective_plan()),
            &state.config.plans.planner_tunables,
        ),
    };

    let wanted = i64::from(duration_min);
    let mut slots: Vec<serde_json::Value> = Vec::new();
    // Day by day in the student's own zone, never a fixed offset: a day boundary is a local
    // fact, and `free_slots` takes a `NaiveDate` in the input's timezone.
    let mut day = from.with_timezone(&tz).date_naive();
    let last = to.with_timezone(&tz).date_naive();
    while day <= last {
        let (found, _capacity) = stvdy_core::planner::slots::free_slots(&input, day);
        for interval in found {
            // The caller asked for a block of a given length, so a shorter free gap is not an
            // answer to the question — reporting it would make the model offer a slot the
            // student cannot fit the work into. The window bounds are applied again because a
            // day's slots may extend past `to`.
            if interval.ends_at - interval.starts_at < Duration::minutes(wanted)
                || interval.starts_at < from
                || interval.ends_at > to
            {
                continue;
            }
            slots.push(serde_json::json!({
                "starts_at": interval.starts_at,
                "ends_at": interval.ends_at,
                "minutes": (interval.ends_at - interval.starts_at).num_minutes(),
            }));
        }
        let Some(next) = day.succ_opt() else { break };
        day = next;
    }

    Ok(serde_json::json!({
        "from": from,
        "to": to,
        "duration_min": duration_min,
        "timezone": user.timezone,
        "slots": slots,
    }))
}

/// `get_upcoming_deadlines`: what is due inside the horizon.
///
/// Two reads rather than one union, and both are the routes' own: `tasks::list_in` with a
/// `due_after`/`due_before` pair (which is what `GET /tasks?due_after=&due_before=` answers)
/// and `events::list_in` for the window. A hand-written `union all` would be a third answer to
/// "what is on the student's calendar", and the calendar's overlap rule — an event that
/// straddles the horizon is in it — is exactly the rule a second query gets wrong.
async fn upcoming_deadlines(
    state: &AppState,
    user: &AuthUser,
    workspace_id: Uuid,
    days: u32,
    limit: u32,
) -> AppResult<serde_json::Value> {
    let now = state.clock.now();
    let days = if days == 0 {
        DEFAULT_DEADLINE_DAYS
    } else {
        i64::from(days)
    };
    let horizon = now + Duration::days(days);

    let due_query = crate::tasks::ListQuery {
        status: None,
        workspace_id: None,
        parent_id: None,
        due_before: Some(horizon),
        due_after: Some(now),
        limit: Some(i64::from(limit)),
    };
    let tasks = crate::tasks::list_in(&state.pool, user.user_id, &due_query).await?;
    let events = crate::events::list_in(&state.pool, user.user_id, now, horizon).await?;

    // The workspace is reported rather than used as a filter: a deadline anywhere in the
    // account is upcoming, and a tool that silently narrowed to the workspace the chat happens
    // to be in would answer a different question than the one the student asked.
    Ok(serde_json::json!({
        "now": now,
        "horizon": horizon,
        "workspace_id": workspace_id,
        "days": days,
        "tasks": tasks,
        "events": events,
    }))
}

/// The rows a read tool may return, for the schema's own bound.
///
/// Exposed so a caller can cap a list without re-deriving the number: the schema the model
/// reads and the limit the query applies are the same number (A270's neighbour).
pub fn max_list() -> u32 {
    stvdy_core::agent::tools::bounds().max_list
}
