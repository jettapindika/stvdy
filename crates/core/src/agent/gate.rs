//! The one decision point: may this tool call run?
//!
//! A WRITE tool never executes. It becomes a proposal a human confirms, on any device
//! (A271), and this module is where that is enforced rather than remembered: [`gate`]
//! returns [`GateDecision::Propose`] for every [`ToolKind::Write`] tool, and
//! [`GateDecision::Execute`] is only reachable from the read arm. A write that the same
//! code path could execute as a read one is the defect this phase exists to prevent, and
//! the shape here makes it unrepresentable instead of discouraged —
//! `crates/core/tests/agent_gate.rs::no_write_tool_is_ever_executed` walks every write
//! against every combination of plan, provider and turn position and asserts the decision
//! is never `Execute`.
//!
//! `gate` reads nothing: no config, no entitlements, no database, no clock. The two
//! questions that need the world — does the plan grant agent actions, and does the provider
//! support tools — arrive already answered in [`GateContext`], exactly as
//! `crate::entitlements::check_with` takes the matrix rather than loading it.
//!
//! **A refusal is a decision, not an error.** It reaches the model as a [`ChatMsg`]
//! through [`tool_result`], so the model can tell the student what happened. Dropping the
//! call instead — which is what the chat route did before this phase — leaves the model
//! answering as if it had succeeded, and the student never learns either.

use crate::agent::tools::{ToolArgs, ToolName, parse_args};
use crate::traits::{ChatMsg, Role, ToolCall};

/// How many WRITE calls one turn may propose.
///
/// Three is what one screen can show before the student is dismissing instead of deciding
/// (A278). A fourth is [`Refusal::ProposalLimit`], which reaches the model as a
/// `tool_result` so it can say "I've prepared three things for you to confirm; ask me again
/// for the rest". The cap is what stops a poisoned document, or a looping model, from
/// filling the confirmation surface with proposals the student has to dismiss one by one.
pub const MAX_PROPOSALS_PER_TURN: u8 = 3;

/// What the gate needs to know, already decided by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateContext {
    /// `LlmProvider::supports_tools()` — false means no call could have been legitimate.
    pub supports_tools: bool,
    /// `Feature::AgentAction`, already decided by the engine (A272).
    pub agent_actions: bool,
    /// How many WRITE calls this turn has already proposed.
    pub proposals_so_far: u8,
}

/// The verdict for one call. Pure: no IO, no clock, no database.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum GateDecision {
    /// A READ tool: run it now and hand the result back to the model.
    Execute { tool: ToolName, args: ToolArgs },
    /// A WRITE tool: create the proposal and wait for a human. Never executed here.
    Propose { tool: ToolName, args: ToolArgs },
    /// The call does not run, and the model is told why.
    Refuse { tool: String, reason: Refusal },
}

/// Why a call did not run.
///
/// The first five are `gate`'s and are pure. The last two are constructed by the *server*,
/// because a balance and a spent provider-cost ceiling are database facts and `gate` reads
/// nothing (A271/A282) — they are in this enum anyway because they have to reach the model
/// through the same `tool_result` path as every other refusal.
///
/// `InsufficientCredits` and `UsageLimit` are deliberately two variants rather than one: the
/// spend refuses a turn for either reason, and "you are out of credits" and "your plan's
/// monthly AI usage limit is reached" are different sentences about different facts. A single
/// variant would force the model to tell the student the wrong one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Refusal {
    /// The provider cannot call tools, so nothing it emitted could have been honoured.
    ToolsUnsupported,
    /// The registry has no such tool. A hallucinated name is a refusal the model can read,
    /// never a dropped call.
    UnknownTool,
    /// The six WRITE tools need `Feature::AgentAction` (A272).
    NotOnPlan { tool: ToolName },
    /// The arguments do not satisfy the tool's contract; `message` names the field.
    Invalid { message: String },
    /// The turn has already proposed [`MAX_PROPOSALS_PER_TURN`] things (A278).
    ProposalLimit { max: u8 },
    /// The `agent_turn` spend was refused for the balance. Constructed by the server, never
    /// by [`gate`].
    InsufficientCredits { needed: i64, available: i64 },
    /// The `agent_turn` spend was refused because the account has reached its plan's monthly
    /// provider-cost ceiling. Constructed by the server, never by [`gate`].
    ///
    /// Distinct from [`Refusal::InsufficientCredits`] because it is a different fact: the
    /// balance may be healthy and the ceiling still reached, and a student told "you need more
    /// credits" for a usage limit they cannot lift by buying is told the wrong remedy.
    UsageLimit,
}

impl Refusal {
    /// What the model is told. Plain, and never a stack trace: it is read by a model that
    /// has to explain it to a student in their own language.
    ///
    /// The plan-related message states the fact and stops. It does not say "offer an
    /// upgrade", because on iOS and Android `config/billing_policy.toml` forbids a purchase
    /// call to action: *how* the model phrases a locked feature is the prompt's business
    /// (`{{plan_gate_instruction_derived}}`), and this is only the fact it phrases.
    pub fn message(&self) -> String {
        match self {
            Refusal::ToolsUnsupported => {
                "tools are not available on this model provider, so no tool call can be honoured"
                    .to_string()
            }
            Refusal::UnknownTool => {
                "there is no tool with that name; call one of the tools you were offered"
                    .to_string()
            }
            Refusal::NotOnPlan { tool } => {
                format!(
                    "{} is not available on the user's current plan",
                    tool.as_str()
                )
            }
            Refusal::Invalid { message } => message.clone(),
            Refusal::ProposalLimit { max } => format!(
                "this turn has already prepared {max} things to confirm; ask the user to \
                 confirm or dismiss those before proposing more"
            ),
            Refusal::InsufficientCredits { needed, available } => format!(
                "this action costs {needed} credits and the user has {available}; tell the \
                 user they need more credits before it can run"
            ),
            Refusal::UsageLimit => {
                "the user's plan has reached its monthly AI usage limit; tell the user it \
                 resets before this action can run, and do not tell them to buy credits for it"
                    .to_string()
            }
        }
    }
}

/// Decide one call.
///
/// The checks run in a fixed order, and the order is the contract (A271): the least
/// specific question that can settle the call is asked first, so the answer the model gets
/// is the one it can act on.
///
/// 1. [`GateContext::supports_tools`] — a provider that cannot call tools makes every call
///    illegitimate, whatever it says;
/// 2. the registry (`ToolName::from_str`) — an unknown name is refused before its arguments
///    are judged, because the arguments of a tool that does not exist mean nothing;
/// 3. the plan ([`GateContext::agent_actions`]) — a WRITE tool on a free plan is refused
///    before its arguments are judged, because "not on your plan" is the answer the student
///    can act on and "your title is empty" is not;
/// 4. [`parse_args`] — the contract, whose message names the field;
/// 5. [`MAX_PROPOSALS_PER_TURN`] — the cap, which a READ tool is not subject to.
///
/// A READ tool that survives 1, 2 and 4 is `Execute`; every WRITE tool that survives is
/// `Propose`, and there is no arm that returns `Execute` for one.
pub fn gate(call: &ToolCall, ctx: &GateContext) -> GateDecision {
    if !ctx.supports_tools {
        return GateDecision::Refuse {
            tool: call.name.clone(),
            reason: Refusal::ToolsUnsupported,
        };
    }
    let Some(tool) = ToolName::from_str(&call.name) else {
        return GateDecision::Refuse {
            tool: call.name.clone(),
            reason: Refusal::UnknownTool,
        };
    };
    if tool.is_write() && !ctx.agent_actions {
        return GateDecision::Refuse {
            tool: call.name.clone(),
            reason: Refusal::NotOnPlan { tool },
        };
    }
    let args = match parse_args(tool, &call.arguments) {
        Ok(args) => args,
        Err(error) => {
            return GateDecision::Refuse {
                tool: call.name.clone(),
                reason: Refusal::Invalid {
                    message: error.to_string(),
                },
            };
        }
    };
    if tool.is_write() {
        if ctx.proposals_so_far >= MAX_PROPOSALS_PER_TURN {
            return GateDecision::Refuse {
                tool: call.name.clone(),
                reason: Refusal::ProposalLimit {
                    max: MAX_PROPOSALS_PER_TURN,
                },
            };
        }
        // The read/write split, in one line: a write proposes, and this is the only place
        // a `Propose` is constructed.
        return GateDecision::Propose { tool, args };
    }
    GateDecision::Execute { tool, args }
}

/// The `tool_result` turn handed back to the model for one decision, so a refusal is
/// answered rather than dropped (A271).
///
/// The call's id is part of the text because a turn can carry several calls: a model that
/// asked for three things has to be able to tell which answer belongs to which request, and
/// the provider boundary this workspace speaks to serialises a message as `role` +
/// `content` only (`crates/server/src/integrations/llm.rs::body`), so there is no
/// `tool_call_id` field to carry it.
///
/// The role is [`Role::User`] for that same reason: [`ChatMsg`] has no tool role, and a
/// tool result is content the model has to read as input, which is what `user` means to
/// that endpoint. Widening [`ChatMsg`] would be a change to a trait the roadmap fixes, for
/// a distinction this adapter cannot carry.
///
/// An `Execute` decision is answered with a placeholder rather than an empty string: a
/// caller that asks for a tool result before the read has run has a bug, and empty content
/// would hide it behind a model that answers as if the tool returned nothing.
pub fn tool_result(call: &ToolCall, decision: &GateDecision) -> ChatMsg {
    let content = match decision {
        GateDecision::Refuse { tool, reason } => format!(
            "The call to `{tool}` ({}) did not run: {}",
            call.id,
            reason.message()
        ),
        GateDecision::Execute { tool, .. } => format!(
            "The call to `{}` ({}) was accepted; its result follows.",
            tool.as_str(),
            call.id
        ),
        GateDecision::Propose { tool, .. } => format!(
            "The call to `{}` ({}) was accepted as a proposal; the user must confirm it \
             before anything is saved.",
            tool.as_str(),
            call.id
        ),
    };
    ChatMsg {
        role: Role::User,
        content,
    }
}

/// The `tool_result` for a READ tool that has run.
///
/// The value is handed over as compact JSON, and the envelope is the same shape as
/// [`tool_result`]'s so the model sees one form for every tool result. Compact rather than
/// pretty because it is tokens, not a screen: the model does not read indentation, and the
/// pretty form of a fifty-row list is a measurable fraction of the window.
pub fn read_result(call: &ToolCall, tool: ToolName, result: &serde_json::Value) -> ChatMsg {
    ChatMsg {
        role: Role::User,
        content: format!(
            "The call to `{}` ({}) returned:\n{result}",
            tool.as_str(),
            call.id
        ),
    }
}
