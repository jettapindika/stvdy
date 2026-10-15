//! The tool registry: the ten tools the spec names, the read/write split that decides
//! which of them may ever run, and the argument contracts the model is held to.
//!
//! Every test here is a contract test rather than a behaviour test, because the registry
//! *is* the contract: the names are the prompt's names, the split is the phase's safety
//! property, and the schemas are the only thing the model is told.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use stvdy_core::agent::tools::{
    CreateEventArgs, CreateFlashcardsArgs, CreateQuizArgs, CreateTaskArgs, EventKind,
    FindFreeSlotsArgs, GetDueFlashcardsArgs, GetUpcomingDeadlinesArgs, ListTasksArgs,
    QuizDifficulty, QuizTypes, ScheduleStudyBlockArgs, ToolArgs, ToolKind, ToolName,
    UpdateTaskArgs, bounds, parse_args, specs,
};
use stvdy_core::tasks::TaskStatus;
use uuid::Uuid;

/// `prompts/base.v3.md`, compiled in: the prompt and the registry are one contract, and
/// this is the test that fails when they drift.
const PROMPT: &str = include_str!("../../../prompts/base.v3.md");

/// The tool names as `prompts/base.v3.md`'s `<tools_and_actions>` section writes them.
///
/// The section names them in one sentence, and that sentence is the list; the rest of the
/// block is prose about READ and WRITE, which is why this reads the sentence rather than
/// the block (the block also carries `{{plan_gate_instruction_derived}}`, which would look
/// like a tool name to a looser parse).
fn prompt_tool_names() -> Vec<String> {
    let sentence = PROMPT
        .lines()
        .find(|line| line.starts_with("You may have tools to manage"))
        .expect("base.v3.md names the tools in its <tools_and_actions> section");
    let list = sentence
        .split_once(':')
        .expect("the sentence names the tools after a colon")
        .1;
    list.split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
        .filter(|name| name.contains('_'))
        .map(str::to_string)
        .collect()
}

/// The JSON Schema of one tool, from the list a plan that has agent actions is offered.
fn schema(tool: ToolName) -> Value {
    specs(true)
        .into_iter()
        .find(|spec| spec.name == tool.as_str())
        .unwrap_or_else(|| panic!("{tool:?} has no spec"))
        .parameters
}

/// The registry and the prompt cannot drift.
#[test]
fn the_registry_is_the_prompts_own_list() {
    let registered: Vec<String> = ToolName::ALL
        .iter()
        .map(|tool| tool.as_str().to_string())
        .collect();
    assert_eq!(registered, prompt_tool_names());
    assert_eq!(registered.len(), 10, "the spec's list is ten tools");

    // Every name round-trips, so a call can be parsed from the wire and written back.
    for tool in ToolName::ALL {
        assert_eq!(ToolName::from_str(tool.as_str()), Some(tool));
    }
    assert_eq!(ToolName::from_str("confirm_action"), None);
}

/// The split is the whole phase: four tools read, six write, and the write set is exactly
/// the six that change a row.
#[test]
fn the_write_set_is_the_six_that_change_a_row() {
    let write: Vec<&str> = ToolName::ALL
        .iter()
        .filter(|tool| tool.is_write())
        .map(|tool| tool.as_str())
        .collect();
    assert_eq!(
        write,
        [
            "create_task",
            "update_task",
            "create_event",
            "schedule_study_block",
            "create_flashcards",
            "create_quiz"
        ]
    );

    let read: Vec<&str> = ToolName::ALL
        .iter()
        .filter(|tool| !tool.is_write())
        .map(|tool| tool.as_str())
        .collect();
    assert_eq!(
        read,
        [
            "list_tasks",
            "find_free_slots",
            "get_due_flashcards",
            "get_upcoming_deadlines"
        ]
    );

    // `kind()` is the only place the split lives, and it is a `matches!`, so this asserts
    // the two ways of asking agree rather than asserting a second list.
    for tool in ToolName::ALL {
        let expected = if tool.is_write() {
            ToolKind::Write
        } else {
            ToolKind::Read
        };
        assert_eq!(tool.kind(), expected);
    }
}

/// A free plan is offered no write tool, and the read tools are offered on every plan:
/// refusing a read would make the assistant worse on free while protecting nothing.
#[test]
fn the_specs_are_filtered_by_the_plan() {
    let free = specs(false);
    assert_eq!(free.len(), 4);
    assert!(
        free.iter()
            .all(|spec| ToolName::from_str(&spec.name).is_some_and(|tool| !tool.is_write())),
        "a free plan must be offered no write tool"
    );

    let all = specs(true);
    assert_eq!(all.len(), 10);
    assert_eq!(
        all.iter()
            .map(|spec| spec.name.clone())
            .collect::<Vec<String>>(),
        ToolName::ALL
            .iter()
            .map(|tool| tool.as_str().to_string())
            .collect::<Vec<String>>(),
        "the wire list is the registry's order, which is the prompt's order"
    );
}

/// Every spec's `parameters` is a valid JSON-Schema object with `additionalProperties:
/// false`, which is what tells the model that an extra key is a mistake.
#[test]
fn every_spec_is_a_closed_schema() {
    for spec in specs(true) {
        let name = &spec.name;
        assert!(!spec.description.is_empty(), "{name} has no description");
        let schema = &spec.parameters;
        assert_eq!(schema["type"], json!("object"), "{name}");
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{name} must refuse an unknown argument"
        );
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} has no properties"));
        assert!(!properties.is_empty(), "{name} describes no arguments");
        for required in schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} has no required list"))
        {
            let required = required.as_str().expect("a required entry is a name");
            assert!(
                properties.contains_key(required),
                "{name} requires {required} without describing it"
            );
        }
        for (field, property) in properties {
            assert!(
                property.get("type").is_some() || property.get("enum").is_some(),
                "{name}: {field} has no type"
            );
        }
    }
}

/// A schema's `enum` is the type's own list, so the model is told the values the parser
/// accepts rather than a list someone typed twice.
#[test]
fn the_schema_enums_are_the_types_own_values() {
    let kind = &schema(ToolName::CreateEvent)["properties"]["kind"];
    assert_eq!(
        kind["enum"],
        json!(
            EventKind::ALL
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<&str>>()
        )
    );
    assert_eq!(
        kind["enum"],
        json!(["class", "exam", "study_block", "deadline"]),
        "the wire spelling of a study block is the kind the server stores"
    );

    let quiz = schema(ToolName::CreateQuiz);
    assert_eq!(
        quiz["properties"]["difficulty"]["enum"],
        json!(
            QuizDifficulty::ALL
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<&str>>()
        )
    );
    assert_eq!(
        quiz["properties"]["types"]["enum"],
        json!(
            QuizTypes::ALL
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<&str>>()
        )
    );

    let statuses: Vec<String> = [TaskStatus::Todo, TaskStatus::Doing, TaskStatus::Done]
        .iter()
        .map(|status| {
            serde_json::to_value(status)
                .expect("a status serialises")
                .as_str()
                .expect("a status is a string")
                .to_string()
        })
        .collect();
    assert_eq!(statuses, ["todo", "doing", "done"]);
    assert_eq!(
        schema(ToolName::ListTasks)["properties"]["status"]["enum"],
        json!(statuses)
    );
    assert_eq!(
        schema(ToolName::UpdateTask)["properties"]["status"]["enum"],
        json!(statuses)
    );
}

/// A default the schema documents is the default the parser applies, so the model that
/// omits a field gets what the schema said it would.
#[test]
fn a_documented_default_is_the_default_applied() {
    let uuid = Uuid::new_v4();
    let parsed = parse_args(
        ToolName::CreateQuiz,
        &json!({ "title": "Chapter 3 quiz", "workspace_id": uuid,
                 "source_ids": [uuid], "count": 10 }),
    )
    .expect("a call may omit the fields the schema gives a default");
    match parsed {
        ToolArgs::CreateQuiz(args) => {
            assert_eq!(
                serde_json::to_value(args.difficulty).expect("a difficulty serialises"),
                schema(ToolName::CreateQuiz)["properties"]["difficulty"]["default"]
            );
            assert_eq!(
                serde_json::to_value(args.types).expect("a types value serialises"),
                schema(ToolName::CreateQuiz)["properties"]["types"]["default"]
            );
        }
        other => panic!("expected a create_quiz, got {other:?}"),
    }

    let parsed = parse_args(ToolName::ListTasks, &json!({}))
        .expect("a read with no filter is the whole list, bounded");
    match parsed {
        ToolArgs::ListTasks(args) => assert_eq!(
            json!(args.limit),
            schema(ToolName::ListTasks)["properties"]["limit"]["default"]
        ),
        other => panic!("expected a list_tasks, got {other:?}"),
    }
}

/// The schema and the struct it describes cannot drift: a field added to one and not the
/// other is a model that is told about an argument the parser refuses, or an argument the
/// model is never told about.
///
/// Each fixture names *every* field, which is what makes the comparison total: the structs
/// skip an absent optional field when they serialise, so an all-`Some` instance is the
/// shape that must match the schema exactly.
#[test]
fn every_schema_property_is_a_field_of_its_struct() {
    let uuid = Uuid::new_v4();
    let at = "2026-10-13T19:00:00+07:00"
        .parse::<DateTime<Utc>>()
        .expect("the fixture timestamp is RFC 3339 with an offset");
    let fixtures: Vec<(ToolName, ToolArgs)> = vec![
        (
            ToolName::CreateTask,
            ToolArgs::CreateTask(CreateTaskArgs {
                title: "Read chapter 3".into(),
                notes: Some("pages 40-60".into()),
                due_at: Some(at),
                estimate_min: Some(45),
                priority: Some(1),
                workspace_id: Some(uuid),
                goal_id: Some(uuid),
            }),
        ),
        (
            ToolName::UpdateTask,
            ToolArgs::UpdateTask(UpdateTaskArgs {
                task_id: uuid,
                title: Some("Read chapter 4".into()),
                notes: Some("pages 61-80".into()),
                due_at: Some(at),
                estimate_min: Some(30),
                priority: Some(2),
                status: Some(TaskStatus::Doing),
            }),
        ),
        (
            ToolName::ListTasks,
            ToolArgs::ListTasks(ListTasksArgs {
                status: Some(TaskStatus::Todo),
                due_before: Some(at),
                limit: 20,
            }),
        ),
        (
            ToolName::CreateEvent,
            ToolArgs::CreateEvent(CreateEventArgs {
                title: "Physics exam".into(),
                kind: EventKind::Exam,
                starts_at: at,
                ends_at: at,
                workspace_id: Some(uuid),
                task_id: Some(uuid),
            }),
        ),
        (
            ToolName::FindFreeSlots,
            ToolArgs::FindFreeSlots(FindFreeSlotsArgs {
                from: at,
                to: at,
                duration_min: 60,
            }),
        ),
        (
            ToolName::ScheduleStudyBlock,
            ToolArgs::ScheduleStudyBlock(ScheduleStudyBlockArgs {
                title: "Study: chapter 3".into(),
                starts_at: at,
                duration_min: 50,
                task_id: Some(uuid),
                goal_id: Some(uuid),
                workspace_id: Some(uuid),
            }),
        ),
        (
            ToolName::CreateFlashcards,
            ToolArgs::CreateFlashcards(CreateFlashcardsArgs {
                title: "Chapter 3 cards".into(),
                workspace_id: uuid,
                source_ids: vec![uuid],
                count: 10,
            }),
        ),
        (
            ToolName::CreateQuiz,
            ToolArgs::CreateQuiz(CreateQuizArgs {
                title: "Chapter 3 quiz".into(),
                workspace_id: uuid,
                source_ids: vec![uuid],
                count: 10,
                difficulty: QuizDifficulty::Medium,
                types: QuizTypes::Mixed,
            }),
        ),
        (
            ToolName::GetDueFlashcards,
            ToolArgs::GetDueFlashcards(GetDueFlashcardsArgs {
                deck_id: Some(uuid),
                limit: 20,
            }),
        ),
        (
            ToolName::GetUpcomingDeadlines,
            ToolArgs::GetUpcomingDeadlines(GetUpcomingDeadlinesArgs { days: 7, limit: 20 }),
        ),
    ];
    assert_eq!(
        fixtures.len(),
        ToolName::ALL.len(),
        "every tool needs a fixture, or a new tool's schema goes unchecked"
    );

    for (tool, args) in fixtures {
        let parameters = schema(tool);
        let described: Vec<String> = parameters["properties"]
            .as_object()
            .expect("a schema describes its properties")
            .keys()
            .cloned()
            .collect();
        let mut fields: Vec<String> = serde_json::to_value(&args)
            .expect("the arguments serialise")
            .as_object()
            .expect("a call's arguments are an object")
            .keys()
            .cloned()
            .collect();
        let mut described = described;
        fields.sort();
        described.sort();
        assert_eq!(fields, described, "{tool:?}: schema and struct disagree");
    }
}

/// The arguments the parser accepts are exactly the ones the model is told about: a call
/// built from the schema parses, which is what makes the schema a contract and not prose.
#[test]
fn the_schemas_fixtures_parse() {
    let uuid = Uuid::new_v4();
    let calls: Vec<(ToolName, Value)> = vec![
        (
            ToolName::CreateTask,
            json!({ "title": "Read chapter 3", "due_at": "2026-10-13T19:00:00+07:00" }),
        ),
        (
            ToolName::UpdateTask,
            json!({ "task_id": uuid, "status": "done" }),
        ),
        (ToolName::ListTasks, json!({ "status": "todo", "limit": 5 })),
        (
            ToolName::CreateEvent,
            json!({ "title": "Physics exam", "kind": "exam",
                    "starts_at": "2026-10-14T09:00:00+07:00",
                    "ends_at": "2026-10-14T11:00:00+07:00" }),
        ),
        (
            ToolName::FindFreeSlots,
            json!({ "from": "2026-10-13T08:00:00+07:00",
                    "to": "2026-10-13T20:00:00+07:00", "duration_min": 60 }),
        ),
        (
            ToolName::ScheduleStudyBlock,
            json!({ "title": "Study: chapter 3", "starts_at": "2026-10-13T19:00:00+07:00",
                    "duration_min": 50 }),
        ),
        (
            ToolName::CreateFlashcards,
            json!({ "title": "Chapter 3 cards", "workspace_id": uuid,
                    "source_ids": [uuid], "count": 10 }),
        ),
        (
            ToolName::CreateQuiz,
            json!({ "title": "Chapter 3 quiz", "workspace_id": uuid,
                    "source_ids": [uuid], "count": 10, "difficulty": "hard" }),
        ),
        (ToolName::GetDueFlashcards, json!({ "limit": 10 })),
        (ToolName::GetUpcomingDeadlines, json!({ "days": 3 })),
    ];
    for (tool, args) in calls {
        assert!(
            parse_args(tool, &args).is_ok(),
            "{tool:?} refused its own schema's arguments: {args}"
        );
    }
}

/// A citation label is not an id (A280). The poisoned document's own `[1]` cannot become a
/// `source_id`, and neither can a bare number.
#[test]
fn a_label_is_not_an_id() {
    let uuid = Uuid::new_v4();
    let valid = json!({ "title": "Chapter 3 cards", "workspace_id": uuid,
                        "source_ids": [uuid], "count": 10 });
    assert!(
        parse_args(ToolName::CreateFlashcards, &valid).is_ok(),
        "the fixture must be a valid call, or the refusals below prove nothing"
    );

    for bad in [json!("1"), json!("[1]"), json!(1)] {
        let args = json!({ "title": "Chapter 3 cards", "workspace_id": uuid,
                           "source_ids": [bad], "count": 10 });
        let message = parse_args(ToolName::CreateFlashcards, &args)
            .expect_err("a label is not a source id")
            .to_string();
        assert!(
            message.to_lowercase().contains("uuid"),
            "the refusal must say what an id is: {message}"
        );
    }
}

/// The bounds hold, and they are read from one place: 10_000 flashcards is refused, and
/// the refusal names the maximum.
#[test]
fn the_bounds_are_enforced_and_named() {
    let bound = bounds();
    let uuid = Uuid::new_v4();

    let flashcards = |count: u32| {
        json!({ "title": "Chapter 3 cards", "workspace_id": uuid,
                "source_ids": [uuid], "count": count })
    };
    let refused = parse_args(ToolName::CreateFlashcards, &flashcards(10_000))
        .expect_err("10_000 cards is not a batch")
        .to_string();
    assert!(
        refused.contains(&bound.max_flashcards.to_string()),
        "the refusal names the maximum: {refused}"
    );
    assert!(
        parse_args(
            ToolName::CreateFlashcards,
            &flashcards(bound.max_flashcards)
        )
        .is_ok()
    );
    assert!(parse_args(ToolName::CreateFlashcards, &flashcards(0)).is_err());

    let quiz = |count: u32| {
        json!({ "title": "Chapter 3 quiz", "workspace_id": uuid,
                "source_ids": [uuid], "count": count })
    };
    let refused = parse_args(ToolName::CreateQuiz, &quiz(bound.max_quiz_questions + 1))
        .expect_err("too many questions")
        .to_string();
    assert!(
        refused.contains(&bound.max_quiz_questions.to_string()),
        "{refused}"
    );
    assert!(parse_args(ToolName::CreateQuiz, &quiz(bound.max_quiz_questions)).is_ok());

    let refused = parse_args(ToolName::ListTasks, &json!({ "limit": bound.max_list + 1 }))
        .expect_err("the list has a ceiling")
        .to_string();
    assert!(refused.contains(&bound.max_list.to_string()), "{refused}");
    assert!(parse_args(ToolName::ListTasks, &json!({ "limit": 0 })).is_err());
    assert!(parse_args(ToolName::ListTasks, &json!({ "limit": bound.max_list })).is_ok());

    let block = |minutes: u32| {
        json!({ "title": "Study: chapter 3", "starts_at": "2026-10-13T19:00:00+07:00",
                "duration_min": minutes })
    };
    let refused = parse_args(
        ToolName::ScheduleStudyBlock,
        &block(bound.max_slot_minutes + 1),
    )
    .expect_err("a block is not half a day")
    .to_string();
    assert!(
        refused.contains(&bound.max_slot_minutes.to_string()),
        "{refused}"
    );
    let refused = parse_args(
        ToolName::ScheduleStudyBlock,
        &block(bound.min_slot_minutes - 1),
    )
    .expect_err("a block is not a minute")
    .to_string();
    assert!(
        refused.contains(&bound.min_slot_minutes.to_string()),
        "{refused}"
    );
    assert!(parse_args(ToolName::ScheduleStudyBlock, &block(bound.min_slot_minutes)).is_ok());
    assert!(parse_args(ToolName::ScheduleStudyBlock, &block(bound.max_slot_minutes)).is_ok());

    // The event bound is on the span the call describes, not on either end: 09:00 to 17:00
    // is eight hours and allowed; one minute more is not.
    let event = |ends_at: &str| {
        json!({ "title": "Physics exam", "kind": "exam",
                "starts_at": "2026-10-13T09:00:00+07:00", "ends_at": ends_at })
    };
    assert!(parse_args(ToolName::CreateEvent, &event("2026-10-13T17:00:00+07:00")).is_ok());
    let refused = parse_args(ToolName::CreateEvent, &event("2026-10-13T17:01:00+07:00"))
        .expect_err("an eight-hour event is the ceiling")
        .to_string();
    assert!(
        refused.contains(&bound.max_event_minutes.to_string()),
        "{refused}"
    );
    assert!(
        parse_args(ToolName::CreateEvent, &event("2026-10-13T08:00:00+07:00")).is_err(),
        "an event that ends before it starts is not an event"
    );

    let refused = parse_args(
        ToolName::GetUpcomingDeadlines,
        &json!({ "days": bound.max_deadline_days + 1 }),
    )
    .expect_err("the horizon is bounded")
    .to_string();
    assert!(
        refused.contains(&bound.max_deadline_days.to_string()),
        "{refused}"
    );
    assert!(
        parse_args(
            ToolName::GetUpcomingDeadlines,
            &json!({ "days": bound.max_deadline_days })
        )
        .is_ok()
    );
}

/// An unknown argument is refused rather than ignored: it is either a model mistake worth
/// surfacing or an injected payload, and both should be refused.
#[test]
fn an_unknown_argument_is_refused() {
    let uuid = Uuid::new_v4();
    let valid = json!({ "title": "Chapter 3 cards", "workspace_id": uuid,
                        "source_ids": [uuid], "count": 10 });
    let mut sneaky = valid.clone();
    sneaky["confirm"] = json!(true);

    let message = parse_args(ToolName::CreateFlashcards, &sneaky)
        .expect_err("an argument the schema does not have is refused")
        .to_string();
    assert!(message.contains("confirm"), "{message}");

    let message = parse_args(ToolName::CreateFlashcards, &json!("create 10 cards"))
        .expect_err("arguments are an object")
        .to_string();
    assert!(message.contains("object"), "{message}");

    // A field the tool does need is still required.
    let message = parse_args(
        ToolName::CreateFlashcards,
        &json!({ "workspace_id": uuid,
                                                                  "source_ids": [uuid],
                                                                  "count": 10 }),
    )
    .expect_err("the title is required")
    .to_string();
    assert!(message.contains("title"), "{message}");
}

/// A naive timestamp is refused, and the message says what shape is expected, because the
/// model is the reader.
#[test]
fn a_timestamp_without_an_offset_is_refused() {
    for bad in [
        json!("2026-10-13"),
        json!("2026-10-13T19:00:00"),
        json!("tomorrow evening"),
        json!(1_760_000_000),
    ] {
        let args = json!({ "title": "Read chapter 3", "due_at": bad });
        let message = parse_args(ToolName::CreateTask, &args)
            .expect_err("a timestamp without an offset is not a timestamp")
            .to_string();
        assert!(
            message.contains("RFC 3339") && message.contains("offset"),
            "the refusal must name the shape the model has to send: {message}"
        );
        assert!(message.contains("due_at"), "and the field: {message}");
    }

    // The offset is applied, not merely accepted: 19:00 in Jakarta is 12:00 UTC.
    let parsed = parse_args(
        ToolName::CreateTask,
        &json!({ "title": "Read chapter 3", "due_at": "2026-10-13T19:00:00+07:00" }),
    )
    .expect("the shape the refusal names is accepted");
    match parsed {
        ToolArgs::CreateTask(args) => assert_eq!(
            args.due_at.map(|due| due.to_rfc3339()),
            Some("2026-10-13T12:00:00+00:00".to_string())
        ),
        other => panic!("expected a create_task, got {other:?}"),
    }
}
