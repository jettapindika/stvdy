//! The gate: the one place that decides whether a tool call runs, proposes, or is refused.
//!
//! The cases here are the contract's own cases rather than a sample of them: the refusal
//! order is documented, so it is asserted; the read/write split is the phase's safety
//! property, so every write is walked; and a refusal has to reach the model, so the
//! `tool_result` is read rather than assumed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use stvdy_core::agent::gate::{
    GateContext, GateDecision, MAX_PROPOSALS_PER_TURN, Refusal, gate, read_result, tool_result,
};
use stvdy_core::agent::tools::{ToolArgs, ToolName};
use stvdy_core::traits::{Role, ToolCall};

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: "call_1".to_string(),
        name: name.to_string(),
        arguments,
    }
}

fn ctx(supports_tools: bool, agent_actions: bool, proposals_so_far: u8) -> GateContext {
    GateContext {
        supports_tools,
        agent_actions,
        proposals_so_far,
    }
}

/// The arguments a well-formed call to each tool carries. The read cases are the ones the
/// schema documents, so a refusal below is about the gate and never about the parser.
fn arguments(tool: ToolName) -> Value {
    match tool {
        ToolName::CreateTask => json!({ "title": "Read chapter 3" }),
        ToolName::UpdateTask => json!({
            "task_id": "6a1f1e5e-1a1f-4b3a-9d1f-2b7c8e9f0a1b",
            "status": "done"
        }),
        ToolName::ListTasks => json!({ "limit": 5 }),
        ToolName::CreateEvent => json!({
            "title": "Physics exam", "kind": "exam",
            "starts_at": "2026-10-14T09:00:00+07:00",
            "ends_at": "2026-10-14T11:00:00+07:00"
        }),
        ToolName::FindFreeSlots => json!({
            "from": "2026-10-13T08:00:00+07:00",
            "to": "2026-10-13T20:00:00+07:00",
            "duration_min": 60
        }),
        ToolName::ScheduleStudyBlock => json!({
            "title": "Study: chapter 3",
            "starts_at": "2026-10-13T19:00:00+07:00",
            "duration_min": 50
        }),
        ToolName::CreateFlashcards => json!({
            "title": "Chapter 3 cards",
            "workspace_id": "6a1f1e5e-1a1f-4b3a-9d1f-2b7c8e9f0a1b",
            "source_ids": ["6a1f1e5e-1a1f-4b3a-9d1f-2b7c8e9f0a1b"],
            "count": 10
        }),
        ToolName::CreateQuiz => json!({
            "title": "Chapter 3 quiz",
            "workspace_id": "6a1f1e5e-1a1f-4b3a-9d1f-2b7c8e9f0a1b",
            "source_ids": ["6a1f1e5e-1a1f-4b3a-9d1f-2b7c8e9f0a1b"],
            "count": 10
        }),
        ToolName::GetDueFlashcards => json!({ "limit": 10 }),
        ToolName::GetUpcomingDeadlines => json!({ "days": 3 }),
    }
}

/// The check order is the contract, and each case is reachable: a provider with no tool
/// support, a name the registry does not have, a write on a plan without AgentAction, and
/// arguments that do not parse.
#[test]
fn every_refusal_has_one_reason_and_they_are_checked_in_order() {
    // 1. A provider that cannot call tools: nothing it asked for could have been
    //    legitimate, so no later question is asked.
    assert_eq!(
        gate(
            &call("create_task", json!({ "title": "Read chapter 3" })),
            &ctx(false, true, 0)
        ),
        GateDecision::Refuse {
            tool: "create_task".to_string(),
            reason: Refusal::ToolsUnsupported,
        }
    );

    // 2. A name the registry does not have, before anything else about the call.
    assert_eq!(
        gate(&call("confirm_action", json!({})), &ctx(true, true, 0)),
        GateDecision::Refuse {
            tool: "confirm_action".to_string(),
            reason: Refusal::UnknownTool,
        }
    );

    // 3. A write on a plan without AgentAction is refused *before* its arguments are
    //    judged: this call also has an empty title, and the plan is the answer the model
    //    can act on.
    assert_eq!(
        gate(
            &call("create_task", json!({ "title": "" })),
            &ctx(true, false, 0)
        ),
        GateDecision::Refuse {
            tool: "create_task".to_string(),
            reason: Refusal::NotOnPlan {
                tool: ToolName::CreateTask
            },
        }
    );

    // 4. Arguments that do not parse, with the field named so the model can correct them.
    match gate(
        &call(
            "create_task",
            json!({ "title": "Read chapter 3", "due_at": "tomorrow" }),
        ),
        &ctx(true, true, 0),
    ) {
        GateDecision::Refuse {
            reason: Refusal::Invalid { message },
            tool,
        } => {
            assert_eq!(tool, "create_task");
            assert!(message.contains("due_at"), "{message}");
        }
        other => panic!("invalid arguments must be refused, got {other:?}"),
    }

    // 5. The fourth write in one turn, with the limit named in the message the model
    //    repeats to the student.
    match gate(
        &call("create_task", json!({ "title": "Read chapter 3" })),
        &ctx(true, true, MAX_PROPOSALS_PER_TURN),
    ) {
        GateDecision::Refuse {
            reason: Refusal::ProposalLimit { max },
            ..
        } => {
            assert_eq!(max, MAX_PROPOSALS_PER_TURN);
            assert!(
                Refusal::ProposalLimit { max }
                    .message()
                    .contains(&max.to_string()),
                "the refusal names the limit"
            );
        }
        other => panic!("a fourth proposal must be refused, got {other:?}"),
    }
}

/// A read on a free plan executes: `agent_actions = false` gates writes, not reads.
#[test]
fn a_read_executes_without_agent_actions() {
    match gate(
        &call("list_tasks", json!({ "limit": 5 })),
        &ctx(true, false, 0),
    ) {
        GateDecision::Execute {
            tool: ToolName::ListTasks,
            args: ToolArgs::ListTasks(args),
        } => assert_eq!(args.limit, 5),
        other => panic!("a read must execute, got {other:?}"),
    }

    // A read is not a proposal, so the cap does not touch it: a turn that has already
    // proposed three things can still look at the calendar.
    assert!(matches!(
        gate(
            &call("list_tasks", json!({})),
            &ctx(true, false, MAX_PROPOSALS_PER_TURN)
        ),
        GateDecision::Execute { .. }
    ));
}

/// A write on a plan without AgentAction is refused with `NotOnPlan`, never executed —
/// and the same call proposes once the plan grants it, so the refusal is about the plan
/// and not about the call.
#[test]
fn a_write_without_agent_actions_is_refused() {
    for tool in ToolName::ALL.iter().copied().filter(|tool| tool.is_write()) {
        let call = call(tool.as_str(), arguments(tool));
        let decision = gate(&call, &ctx(true, false, 0));
        match decision {
            GateDecision::Refuse {
                reason: Refusal::NotOnPlan { tool: refused },
                ..
            } => assert_eq!(refused, tool),
            other => panic!("{tool:?} must be refused on a free plan, got {other:?}"),
        }

        match gate(&call, &ctx(true, true, 0)) {
            GateDecision::Propose {
                tool: proposed,
                args: _,
            } => assert_eq!(proposed, tool),
            other => panic!("{tool:?} must propose on a plan that grants it, got {other:?}"),
        }
    }
}

/// No write tool can be *executed* by the gate, on any plan and at any point in a turn.
/// This is the property the phase exists for: a WRITE tool that the same code path could
/// execute as a READ one is the defect, and it is unrepresentable here.
#[test]
fn no_write_tool_is_ever_executed() {
    for tool in ToolName::ALL.iter().copied().filter(|tool| tool.is_write()) {
        let call = call(tool.as_str(), arguments(tool));
        for supports_tools in [true, false] {
            for agent_actions in [true, false] {
                for proposals_so_far in 0..=MAX_PROPOSALS_PER_TURN {
                    let decision =
                        gate(&call, &ctx(supports_tools, agent_actions, proposals_so_far));
                    assert!(
                        !matches!(decision, GateDecision::Execute { .. }),
                        "{tool:?} was executed with {supports_tools}/{agent_actions}/{proposals_so_far}"
                    );
                }
            }
        }
    }
}

/// The fourth write in a turn is refused with the limit in the message.
#[test]
fn the_fourth_proposal_is_refused_and_the_limit_is_named() {
    for already in 0..MAX_PROPOSALS_PER_TURN {
        assert!(
            matches!(
                gate(
                    &call("create_task", json!({ "title": "Read chapter 3" })),
                    &ctx(true, true, already)
                ),
                GateDecision::Propose { .. }
            ),
            "{already} proposals so far must still allow one more"
        );
    }
    assert_eq!(
        MAX_PROPOSALS_PER_TURN, 3,
        "three is what one screen can show"
    );
}

/// The one refusal `gate` never produces, and the reason it is still in the enum: the
/// server constructs it when the `agent_turn` spend fails, and it has to reach the model
/// through the same `tool_result` path as every other refusal (A271/A282).
#[test]
fn an_insufficient_balance_is_a_refusal_the_model_can_repeat() {
    let decision = GateDecision::Refuse {
        tool: "create_task".to_string(),
        reason: Refusal::InsufficientCredits {
            needed: 2,
            available: 0,
        },
    };
    let msg = tool_result(&call("create_task", json!({})), &decision);
    assert_eq!(msg.role, Role::User);
    assert!(msg.content.contains("create_task"), "{}", msg.content);
    assert!(msg.content.contains('2'), "{}", msg.content);
    assert!(msg.content.contains('0'), "{}", msg.content);
}

/// A refusal is a `ChatMsg` the model can read, and it names the tool so the model does
/// not confuse it with a different call.
#[test]
fn a_refusal_is_answered_not_dropped() {
    let reasons: [Refusal; 6] = [
        Refusal::ToolsUnsupported,
        Refusal::UnknownTool,
        Refusal::NotOnPlan {
            tool: ToolName::CreateTask,
        },
        Refusal::Invalid {
            message: "due_at must be an RFC 3339 timestamp with an offset".to_string(),
        },
        Refusal::ProposalLimit {
            max: MAX_PROPOSALS_PER_TURN,
        },
        Refusal::InsufficientCredits {
            needed: 2,
            available: 1,
        },
    ];
    for reason in reasons {
        let expected = reason.message();
        assert!(!expected.trim().is_empty(), "{reason:?} has no message");
        let decision = GateDecision::Refuse {
            tool: "create_task".to_string(),
            reason,
        };
        let msg = tool_result(
            &call("create_task", json!({ "title": "Read chapter 3" })),
            &decision,
        );
        assert_eq!(msg.role, Role::User);
        assert!(
            msg.content.contains("create_task"),
            "the tool result must name the call it answers: {}",
            msg.content
        );
        assert!(
            msg.content.contains(&expected),
            "the refusal's reason must reach the model: {}",
            msg.content
        );
    }
}

/// A read that ran is answered with what it found, in the same envelope as a refusal, so
/// the model sees one shape for every tool result — and the call's id, so a turn with
/// several calls can tell them apart.
#[test]
fn a_reads_result_is_answered_with_its_data() {
    let result = json!({ "tasks": [{ "id": "6a1f1e5e-1a1f-4b3a-9d1f-2b7c8e9f0a1b", "title": "Read chapter 3" }] });
    let msg = read_result(
        &call("list_tasks", json!({ "limit": 5 })),
        ToolName::ListTasks,
        &result,
    );
    assert_eq!(msg.role, Role::User);
    assert!(msg.content.contains("list_tasks"), "{}", msg.content);
    assert!(msg.content.contains("call_1"), "{}", msg.content);
    assert!(msg.content.contains("Read chapter 3"), "{}", msg.content);
    assert!(
        msg.content.contains(&result.to_string()),
        "the result is handed over as compact JSON: {}",
        msg.content
    );
}

/// The call's id reaches the model in a refusal too, because a turn can carry several
/// calls and the model has to know which one it is being told about.
#[test]
fn a_refusal_names_the_call_it_answers() {
    let decision = GateDecision::Refuse {
        tool: "create_task".to_string(),
        reason: Refusal::ToolsUnsupported,
    };
    let msg = tool_result(&call("create_task", json!({})), &decision);
    assert!(msg.content.contains("call_1"), "{}", msg.content);
}

/// The gate is pure: the same call and context give the same decision, and the decision
/// serialises in a fixed order so two runs can be compared byte for byte.
#[test]
fn the_gate_is_deterministic() {
    let call = call(
        "create_task",
        json!({
            "title": "Read chapter 3",
            "due_at": "2026-10-13T19:00:00+07:00",
            "priority": 1
        }),
    );
    let context = ctx(true, true, 0);
    let first = gate(&call, &context);
    let second = gate(&call, &context);
    assert_eq!(first, second);

    let text = serde_json::to_string(&first).expect("a decision serialises");
    assert_eq!(
        text,
        serde_json::to_string(&second).expect("a decision serialises")
    );
    assert!(
        text.find("\"tool\"").unwrap() < text.find("\"args\"").unwrap(),
        "the fields keep their declared order: {text}"
    );
    assert!(
        text.find("\"title\"").unwrap() < text.find("\"due_at\"").unwrap()
            && text.find("\"due_at\"").unwrap() < text.find("\"priority\"").unwrap(),
        "the arguments keep their declared order: {text}"
    );
    assert!(
        text.contains("2026-10-13T12:00:00Z"),
        "the offset is applied before the decision is compared: {text}"
    );
}
