//! The wire contract between the server and the native shell. These tests pin
//! the properties later phases depend on: a bijective entity/table mapping, a
//! dense dependency ranking, and the pushable-entity allowlist.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{TimeZone, Utc};
use serde_json::json;
use stvdy_sync::proto::{
    Change, Entity, ErrorCode, Mutation, MutationResult, Op, PullResponse, PushRequest,
    PushResponse,
};
use uuid::Uuid;

#[test]
fn every_entity_round_trips_through_its_table_name() {
    for entity in Entity::ALL {
        let table = entity.table_name();
        assert_eq!(
            Entity::from_table_name(table),
            Some(entity),
            "{table} did not map back to {entity:?}"
        );
    }
}

#[test]
fn table_names_are_unique() {
    let mut names: Vec<&str> = Entity::ALL.iter().map(|e| e.table_name()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "two entities share a table name");
}

#[test]
fn dependency_ranks_put_parents_before_children() {
    let rank = |e: Entity| e.dependency_rank();
    assert!(rank(Entity::Workspace) < rank(Entity::Source));
    assert!(rank(Entity::Source) < rank(Entity::Deck));
    assert!(rank(Entity::Deck) < rank(Entity::Flashcard));
    assert!(rank(Entity::Flashcard) < rank(Entity::FlashcardReview));
    assert!(rank(Entity::Quiz) < rank(Entity::QuizQuestion));
    assert!(rank(Entity::QuizQuestion) < rank(Entity::QuizAttempt));
    assert!(rank(Entity::Workspace) < rank(Entity::Cheatsheet));
    assert!(rank(Entity::Source) < rank(Entity::Cheatsheet));
    assert!(rank(Entity::Task) < rank(Entity::Event));
    // Ranks are a dense 1..=N sequence so initial sync can just sort by them.
    let mut ranks: Vec<u8> = Entity::ALL.iter().map(|e| e.dependency_rank()).collect();
    ranks.sort_unstable();
    assert_eq!(ranks, (1..=Entity::ALL.len() as u8).collect::<Vec<_>>());
}

#[test]
fn only_the_offline_scope_is_client_writable() {
    // Compare order-independently: `Entity::ALL` is in declaration order, which
    // is not the order of the writable subset, and the assertion should also
    // survive a future enum reordering.
    let mut writable: Vec<&str> = Entity::ALL
        .into_iter()
        .filter(|e| e.is_client_writable())
        .map(|e| e.table_name())
        .collect();
    writable.sort_unstable();

    let mut expected = vec![
        "tasks",
        "events",
        "flashcard_reviews",
        "quiz_attempts",
        "study_sessions",
        "user_settings",
    ];
    expected.sort_unstable();

    assert_eq!(
        writable, expected,
        "the pushable set must be exactly the spec's offline-write scope"
    );
}

#[test]
fn server_owned_entities_reject_client_pushes() {
    // Security-relevant: messages are persisted by the chat endpoint, so a
    // client push would skip the credit spend and allow forged assistant turns.
    for entity in [
        Entity::Message,
        Entity::Source,
        Entity::Flashcard,
        Entity::Deck,
        Entity::Quiz,
        Entity::QuizQuestion,
        Entity::Report,
        Entity::Cheatsheet,
        Entity::Goal,
        Entity::Workspace,
        Entity::PendingAction,
    ] {
        assert!(
            !entity.is_client_writable(),
            "{entity:?} must not be client-writable"
        );
        assert!(
            !entity.accepts_push_op(Op::Upsert),
            "{entity:?} must reject upserts"
        );
        assert!(
            !entity.accepts_push_op(Op::Delete),
            "{entity:?} must reject deletes"
        );
    }
}

#[test]
fn append_only_entities_accept_upserts_but_not_deletes() {
    for entity in [
        Entity::FlashcardReview,
        Entity::QuizAttempt,
        Entity::StudySession,
    ] {
        assert!(entity.is_append_only());
        assert!(entity.accepts_push_op(Op::Upsert));
        assert!(
            !entity.accepts_push_op(Op::Delete),
            "{entity:?} is append-only"
        );
    }
}

#[test]
fn mutable_offline_entities_accept_both_ops_and_settings_has_no_tombstone() {
    for entity in [Entity::Task, Entity::Event] {
        assert!(entity.accepts_push_op(Op::Upsert));
        assert!(entity.accepts_push_op(Op::Delete));
    }
    // user_settings is synced but has no deleted_at column.
    assert!(Entity::UserSettings.accepts_push_op(Op::Upsert));
    assert!(!Entity::UserSettings.accepts_push_op(Op::Delete));
}

#[test]
fn change_serializes_with_the_documented_contract() {
    let id = Uuid::now_v7();
    let change = Change {
        seq: 42,
        entity: Entity::Task,
        id,
        op: Op::Upsert,
        data: Some(json!({ "title": "Baca bab 3" })),
    };
    let value = serde_json::to_value(&change).unwrap();
    assert_eq!(value["seq"], 42);
    assert_eq!(value["entity"], "task");
    assert_eq!(value["op"], "upsert");
    assert_eq!(value["id"], id.to_string());

    let back: Change = serde_json::from_value(value).unwrap();
    assert_eq!(back.entity, Entity::Task);
    assert_eq!(back.op, Op::Upsert);
}

#[test]
fn a_tombstone_has_no_data() {
    let change = Change {
        seq: 7,
        entity: Entity::Workspace,
        id: Uuid::now_v7(),
        op: Op::Delete,
        data: None,
    };
    let value = serde_json::to_value(&change).unwrap();
    assert!(value["data"].is_null());
}

#[test]
fn pull_response_round_trips() {
    let response = PullResponse {
        changes: vec![],
        next_cursor: 128,
        has_more: true,
    };
    let text = serde_json::to_string(&response).unwrap();
    assert!(text.contains("\"next_cursor\":128"));
    assert!(text.contains("\"has_more\":true"));
    let back: PullResponse = serde_json::from_str(&text).unwrap();
    assert_eq!(back.next_cursor, 128);
    assert!(back.has_more);
}

#[test]
fn push_request_round_trips_with_client_generated_ids() {
    let client_ts = Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap();
    let mutation_id = Uuid::now_v7();
    let id = Uuid::now_v7();

    let request = PushRequest {
        device_id: Uuid::now_v7(),
        mutations: vec![Mutation {
            mutation_id,
            entity: Entity::StudySession,
            op: Op::Upsert,
            id,
            payload: Some(json!({ "duration_min": 25 })),
            client_ts,
        }],
    };

    let text = serde_json::to_string(&request).unwrap();
    let back: PushRequest = serde_json::from_str(&text).unwrap();

    assert_eq!(back.mutations.len(), 1);
    let m = &back.mutations[0];
    assert_eq!(m.entity, Entity::StudySession);
    assert_eq!(m.op, Op::Upsert);
    assert_eq!(m.mutation_id, mutation_id, "idempotency key must survive");
    assert_eq!(m.id, id);
    // Round-trip fidelity, not a magic constant: the client timestamp must come
    // back exactly, since the server compares it for conflict resolution.
    assert_eq!(m.client_ts, client_ts);
}

#[test]
fn mutation_results_carry_the_rejection_reason() {
    let applied = MutationResult::Applied { seq: 9 };
    assert_eq!(serde_json::to_value(&applied).unwrap()["status"], "applied");

    let rejected = MutationResult::Rejected {
        code: "limit_reached".into(),
        message: "sources_total".into(),
    };
    let value = serde_json::to_value(&rejected).unwrap();
    assert_eq!(value["status"], "rejected");
    assert_eq!(value["code"], "limit_reached");

    let conflict = MutationResult::Conflict {
        winner: json!({ "title": "theirs" }),
    };
    assert_eq!(
        serde_json::to_value(&conflict).unwrap()["status"],
        "conflict"
    );

    let response = PushResponse {
        results: vec![applied, rejected, conflict],
    };
    let back: PushResponse =
        serde_json::from_value(serde_json::to_value(&response).unwrap()).unwrap();
    assert_eq!(back.results.len(), 3);
}

#[test]
fn error_codes_serialize_to_their_wire_strings() {
    // The client switches on these, and they must match AppError::code().
    assert_eq!(
        serde_json::to_value(ErrorCode::ReadOnlyEntity).unwrap(),
        "read_only_entity"
    );
    assert_eq!(
        serde_json::to_value(ErrorCode::ResyncRequired).unwrap(),
        "resync_required"
    );
    assert_eq!(
        serde_json::to_value(ErrorCode::InsufficientCredits).unwrap(),
        "insufficient_credits"
    );
    assert_eq!(ErrorCode::ReadOnlyEntity.as_str(), "read_only_entity");
}
