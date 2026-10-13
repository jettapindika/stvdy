//! The per-mutation push decision — Phase 8's route calls this, and nothing else.
//!
//! [`decide`] is a pure function of the snapshot, the entity, the op and the payload:
//! no database, no clock. That is what lets `crates/server/tests/tamper.rs` drive every
//! `docs/THREAT_MODEL.md` §5 row that concerns a client push without a running push
//! route, and it is why Phase 8 re-implements nothing (A150): its route takes the
//! advisory lock, loops the mutations, calls `decide` for each, applies what it allows
//! and writes `mutation_log`.
//!
//! The matrix is handed in (`plans`, `facts`) rather than read here, exactly as
//! `check_with` requires: `config/plans.toml` is the authority, loaded once at startup,
//! and a second copy of it in this module would be a second answer to "what does this
//! plan allow".

use std::time::Instant;

use axum::Json;
use axum::extract::State;
use serde_json::Value;
use stvdy_core::config::PlansFile;
use stvdy_core::entitlements::{Feature, PlanFacts, UsageSnapshot, check_with};
use stvdy_sync::proto::{
    Entity, ErrorCode, Mutation, MutationResult, Op, PushRequest, PushResponse,
};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::log::{self, Replay};
use super::{apply, touch_last_sync};

/// The fields the server derives, **per entity**.
///
/// A card's `ease`, `interval_days` and `due_at` are the output of the server-side SM-2
/// replay; accepting them through a push would let a client schedule its own reviews
/// (roadmap § Conflict rules).
///
/// The rule is entity-scoped rather than a flat list of names, and that matters: a
/// **task** carries a `due_at` the *client* sets (Phase 9 pushes one), so a global rule
/// on the name would refuse every task push that has a due date. Only the card's fields
/// are derived, so only a card's payload is guarded.
///
/// The guard is a second line behind the allowlist — `flashcards` is not client-writable
/// either, so a push of it is refused before this is reached — and it is the thing that
/// protects a future writable entity that carries a derived field.
fn derived_fields(entity: Entity) -> &'static [&'static str] {
    match entity {
        Entity::Flashcard => &["ease", "interval_days", "due_at"],
        _ => &[],
    }
}

/// Why a push was refused before the entitlement engine was consulted.
///
/// Every one of these is the wire code `read_only_entity`: the client's remedy is to
/// write through the dedicated endpoint (or to stop writing), never to upgrade, so the
/// refusal names no plan (A143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The entity is outside the offline-write scope (`Entity::is_client_writable`).
    NotClientWritable,
    /// The op is not acceptable for the entity (`Entity::accepts_push_op`).
    OpNotAccepted,
    /// The payload carries a field the server derives for that entity.
    DerivedField,
    /// The calling device is read-only after a plan change: it may read, not write.
    ReadOnlyAccount,
}

impl Decision {
    /// The wire result, in the exact shape Phase 8 serialises.
    pub fn rejected(self) -> MutationResult {
        MutationResult::Rejected {
            code: ErrorCode::ReadOnlyEntity.as_str().to_string(),
            message: self.message(),
        }
    }

    fn message(self) -> String {
        match self {
            Decision::NotClientWritable => {
                "this row is written by the server, not by a client push".to_string()
            }
            Decision::OpNotAccepted => {
                "this entity does not accept that operation through a push".to_string()
            }
            Decision::DerivedField => {
                "ease, interval_days and due_at are derived by the server".to_string()
            }
            Decision::ReadOnlyAccount => {
                "this device is read-only after your plan change; delete something or sign \
                 another device out to write again"
                    .to_string()
            }
        }
    }
}

/// The features a push of this entity acquires, if any.
///
/// Exhaustive over `Entity`, so a new entity cannot be added to the protocol without a
/// decision about what a push of it costs. All six pushable entities return `&[]` today:
/// a task, an event, a review, an attempt, a session and a settings row are all free
/// (R1). The day one of them is not, this is the one place that changes.
pub fn push_features(entity: Entity) -> &'static [Feature] {
    match entity {
        // The offline-write scope (spec § PLATFORMS AND ARCHITECTURE): free to push.
        Entity::Task
        | Entity::Event
        | Entity::FlashcardReview
        | Entity::QuizAttempt
        | Entity::StudySession
        | Entity::UserSettings => &[],
        // Everything else is server-owned and is refused by the allowlist below before
        // its features are ever consulted. The arm exists so the match stays exhaustive.
        Entity::Workspace
        | Entity::Source
        | Entity::Message
        | Entity::Deck
        | Entity::Flashcard
        | Entity::Quiz
        | Entity::QuizQuestion
        | Entity::Report
        | Entity::Cheatsheet
        | Entity::Goal
        | Entity::PendingAction => &[],
    }
}

/// The per-mutation decision Phase 8's route calls for every mutation in a batch.
///
/// `Ok(())` means "this mutation may be applied"; Phase 8 then applies it and reports
/// `Applied { seq }`. `Err(MutationResult::Rejected { code, message })` is the exact wire
/// result, so the tamper suite asserts the same value Phase 8 will serialise.
///
/// The guards run in a fixed order, and each is the *most specific* refusal available:
///
/// 1. the allowlist (`Entity::is_client_writable`) — a `Message` or a `Source` is
///    server-owned, and accepting a push of one would skip the credit spend or let a
///    client mark a source `ready`;
/// 2. the op rule (`Entity::accepts_push_op`) — an append-only row has no tombstone;
/// 3. the derived-field guard — a card's `ease`, `interval_days` and `due_at` are the
///    server's;
/// 4. the read-only account state — a device the plan has put over `devices_max` may
///    read, not write;
/// 5. the entity's own field rules — [`validated`], which is the **same** function the REST
///    route calls, so a rule cannot hold on one path and not the other;
/// 6. the entitlement engine, once per feature [`push_features`] names.
///
/// `plans` and `facts` are the matrix and the caller's client facts, passed in because
/// the engine is pure (see the module docs). `device_read_only` is the flag from the
/// calling device's row; it is a parameter rather than a field of `UsageSnapshot`
/// because the snapshot is about the *account*'s counters while this is about the one
/// device making the request.
pub fn decide(
    plans: &PlansFile,
    facts: &PlanFacts,
    snapshot: &UsageSnapshot,
    device_read_only: bool,
    mutation: &Mutation,
) -> Result<(), MutationResult> {
    if !mutation.entity.is_client_writable() {
        return Err(Decision::NotClientWritable.rejected());
    }
    if !mutation.entity.accepts_push_op(mutation.op) {
        return Err(Decision::OpNotAccepted.rejected());
    }
    if carries_derived_field(mutation.entity, mutation.payload.as_ref()) {
        return Err(Decision::DerivedField.rejected());
    }
    if device_read_only {
        return Err(Decision::ReadOnlyAccount.rejected());
    }
    if let Err(reject) = validated(mutation) {
        return Err(MutationResult::Rejected {
            code: reject.code.to_string(),
            message: reject.message,
        });
    }
    for feature in push_features(mutation.entity) {
        if let Err(error) = check_with(plans, facts, snapshot, *feature) {
            let refusal = crate::entitlements::refusal(snapshot.plan, error);
            return Err(MutationResult::Rejected {
                code: refusal.code().to_string(),
                message: refusal.to_string(),
            });
        }
    }
    Ok(())
}

/// `POST /sync/push`: one transaction for the whole batch, one savepoint per mutation.
pub async fn push(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<PushRequest>,
) -> AppResult<Json<PushResponse>> {
    let started = Instant::now();
    let response = apply_batch(&state, &user, &body).await?;

    tracing::info!(
        user_id = %user.user_id,
        device_id = %user.device_id,
        mutations = body.mutations.len(),
        applied = response
            .results
            .iter()
            .filter(|result| matches!(result, MutationResult::Applied { .. }))
            .count(),
        latency_ms = started.elapsed().as_millis() as u64,
        "sync push"
    );

    Ok(Json(response))
}

/// The batch, in the order the contract fixes:
///
///   1. `select pg_advisory_xact_lock(hashtext($1::text))` — the roadmap's lock-order
///      rule. The per-user lock also lives inside the AFTER trigger, so a batch that
///      wrote rows first would acquire it *after* its row locks and two devices could
///      deadlock (40P01). Taking it first makes the order uniform everywhere.
///   2. `select set_config('app.device_id', $1, true)` — so `origin_device_id` is
///      populated without a device column on every table.
///   3. Per mutation: the replay lookup, [`decide`], the apply under a SAVEPOINT, and the
///      `mutation_log` insert. A failed apply rolls back to its savepoint and the batch
///      continues (A170).
///
/// `touch_last_sync` runs once, after the loop, in the same transaction (A180).
pub async fn apply_batch(
    state: &AppState,
    user: &AuthUser,
    body: &PushRequest,
) -> AppResult<PushResponse> {
    // A171: `origin_device_id` is only worth recording if the claim is checkable, so a
    // body free to name another device is refused before anything is written.
    if body.device_id != user.device_id {
        return Err(AppError::Forbidden);
    }

    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning the push transaction: {error}"))
    })?;

    // 1. The lock first, before a single row is touched.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user.user_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the push lock: {error}")))?;

    // 2. The device, so the triggers can attribute the change.
    sqlx::query("select set_config('app.device_id', $1, true)")
        .bind(user.device_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("setting the device: {error}")))?;

    // The calling device's own read-only flag, read in the same transaction as the
    // decision so the two describe one instant (A149). A missing row reads false: the
    // extractor already proved the device is live.
    let device_read_only: bool =
        sqlx::query_scalar("select read_only from devices where id = $1 and user_id = $2")
            .bind(user.device_id)
            .bind(user.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the device: {error}")))?
            .unwrap_or(false);

    let facts = crate::entitlements::facts(state, user);
    let snapshot = crate::entitlements::snapshot_in(&mut tx, state, user).await?;

    let mut results = Vec::with_capacity(body.mutations.len());
    for mutation in &body.mutations {
        // 3a. A replay returns the stored result and applies nothing.
        match log::replay(&mut tx, user.user_id, mutation.mutation_id).await? {
            Replay::Stored(result) => {
                results.push(result);
                continue;
            }
            // A172: the id is global, so a collision with another account is
            // representable — and returning that account's result would be a
            // cross-tenant read.
            Replay::ForeignUser => {
                results.push(MutationResult::Rejected {
                    code: ErrorCode::Conflict.as_str().to_string(),
                    message: "this mutation id belongs to another account".to_string(),
                });
                continue;
            }
            Replay::Fresh => {}
        }

        // 3a-bis. `user_settings`' primary key is the caller's own id, and the apply keys
        // the row by the session and ignores the mutation's id — so a forged id would be
        // accepted and silently do nothing, which reads as success. Refused here because
        // this is the first place the session's user is known; `decide` stays pure (A200).
        if mutation.entity == Entity::UserSettings
            && mutation.op == Op::Upsert
            && let Some(payload) = mutation.payload.as_ref()
            && let Err(reject) =
                crate::settings::validate_settings_push(user.user_id, mutation.id, payload)
        {
            results.push(MutationResult::Rejected {
                code: reject.code.to_string(),
                message: reject.message,
            });
            continue;
        }

        // 3b. Phase 7's decision, not a copy of it (A150).
        if let Err(result) = decide(
            &state.config.plans,
            &facts,
            &snapshot,
            device_read_only,
            mutation,
        ) {
            results.push(result);
            continue;
        }

        // 3c. The apply, under a savepoint so an apply failure rolls back this mutation
        // alone rather than the batch (A170).
        sqlx::query("savepoint mutation")
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("opening a savepoint: {error}")))?;
        match apply::apply_one(&mut tx, user.user_id, mutation).await {
            Ok(outcome) => {
                sqlx::query("release savepoint mutation")
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| {
                        AppError::Internal(anyhow::anyhow!("releasing a savepoint: {error}"))
                    })?;
                let result = outcome.into_result();
                log::record(
                    &mut tx,
                    user.user_id,
                    user.device_id,
                    mutation.mutation_id,
                    &result,
                )
                .await?;
                results.push(result);
            }
            Err(error) => {
                sqlx::query("rollback to savepoint mutation")
                    .execute(&mut *tx)
                    .await
                    .map_err(|rollback| {
                        AppError::Internal(anyhow::anyhow!(
                            "rolling back a mutation: {rollback} (after {error})"
                        ))
                    })?;
                sqlx::query("release savepoint mutation")
                    .execute(&mut *tx)
                    .await
                    .map_err(|release| {
                        AppError::Internal(anyhow::anyhow!("releasing a savepoint: {release}"))
                    })?;
                tracing::warn!(
                    user_id = %user.user_id,
                    mutation_id = %mutation.mutation_id,
                    %error,
                    "a mutation could not be applied"
                );
                results.push(MutationResult::Rejected {
                    code: ErrorCode::Internal.as_str().to_string(),
                    message: "the mutation could not be applied".to_string(),
                });
            }
        }
    }

    touch_last_sync(&mut tx, user.device_id)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("recording the last sync: {error}")))?;
    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing the push transaction: {error}"))
    })?;

    Ok(PushResponse { results })
}

/// The entity's own field rules, run for a pushed mutation.
///
/// Every arm is the **same function the REST route calls** — `tasks::validate_task`,
/// `events::validate_event`, `study::validate_session`, `settings::validate_settings_push` —
/// so "may this row exist" has one answer on both paths. A second implementation here would
/// be a second answer, and the two would disagree the first time one of them was edited.
///
/// The three entities with no arm are the ones the spec's offline scope names but this phase
/// does not: a `FlashcardReview` and a `QuizAttempt` were validated by their own routes in
/// Phase 5, and adding a rule here is that phase's decision to make, not this one's.
///
/// The op matters: a `Task`/`Event` **create** is a payload with `base: null` (A176), and a
/// create must state the fields a row needs. A delete carries no payload at all, so it is not
/// validated — a tombstone has no fields to check.
fn validated(mutation: &Mutation) -> Result<(), crate::tasks::Reject> {
    let Some(payload) = mutation.payload.as_ref() else {
        return Ok(());
    };
    if mutation.op == Op::Delete {
        return Ok(());
    }

    match mutation.entity {
        Entity::Task => {
            let is_create = payload.get("base").is_none_or(Value::is_null);
            crate::tasks::validate_task(mutation.id, payload, is_create).map(|_| ())
        }
        Entity::Event => {
            let is_create = payload.get("base").is_none_or(Value::is_null);
            crate::events::validate_event(payload, is_create).map(|_| ())
        }
        // The mutation's own `client_ts` is what the bound compares against (A204): the
        // device's report of when it wrote the row is the field the contract already
        // carries, and reading a clock here would make the decision impure.
        Entity::StudySession => {
            crate::study::validate_session(payload, mutation.client_ts, mutation.client_ts)
                .map(|_| ())
        }
        // `user_settings` is validated in `apply_batch` instead, by
        // `settings::validate_settings_push`: its first rule is that the mutation's id
        // equals the caller's own user id (A200), and `decide` is pure — it has no session
        // to compare against. One validator, one call site, and this arm is the honest
        // statement that the field rules cannot be the whole answer for this entity.
        Entity::UserSettings => Ok(()),
        _ => Ok(()),
    }
}

/// Whether a payload carries one of the entity's server-derived fields.
///
/// Only a JSON object can: a payload that is not an object is not a row this protocol
/// understands, and the op's own validation refuses it elsewhere.
fn carries_derived_field(entity: Entity, payload: Option<&Value>) -> bool {
    let fields = derived_fields(entity);
    if fields.is_empty() {
        return false;
    }
    let Some(Value::Object(object)) = payload else {
        return false;
    };
    fields.iter().any(|field| object.contains_key(*field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stvdy_core::config::PlanId;
    use stvdy_core::entitlements::{Credits, SubStatus};
    use stvdy_sync::proto::Op;
    use uuid::Uuid;

    fn plans() -> PlansFile {
        crate::testing::plans()
    }

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot {
            plan: PlanId::Free,
            subscription_status: SubStatus::None,
            credits: Credits::default(),
            storage_bytes: 0,
            workspaces: 0,
            sources: 0,
            devices: 1,
            chat_msgs_today: 0,
            web_searches_today: 0,
            flashcards_this_month: 0,
            quizzes_this_month: 0,
            reports_this_month: 0,
            syllabus_imports: 0,
            google_accounts: 0,
        }
    }

    fn mutation(entity: Entity, op: Op, payload: Option<Value>) -> Mutation {
        Mutation {
            mutation_id: Uuid::now_v7(),
            entity,
            op,
            id: Uuid::now_v7(),
            payload,
            client_ts: chrono::Utc::now(),
        }
    }

    fn decide_one(entity: Entity, op: Op, payload: Option<Value>) -> Result<(), MutationResult> {
        decide(
            &plans(),
            &PlanFacts {
                upgrade_offerable: true,
            },
            &snapshot(),
            false,
            &mutation(entity, op, payload),
        )
    }

    #[test]
    fn every_pushable_entity_is_free_today() {
        // R1: a task, an event, a review, an attempt, a session and a settings row cost
        // nothing to push. The day one of them costs something, this is the assertion
        // that changes.
        for entity in Entity::ALL {
            if entity.is_client_writable() {
                assert!(
                    push_features(entity).is_empty(),
                    "{} is pushable and must be free (R1)",
                    entity.table_name()
                );
            }
        }
    }

    #[test]
    fn a_push_of_a_server_owned_row_is_read_only_entity() {
        for entity in [
            Entity::Message,
            Entity::Source,
            Entity::Flashcard,
            Entity::Deck,
        ] {
            assert!(!entity.is_client_writable());
            let error = decide_one(entity, Op::Upsert, Some(serde_json::json!({ "x": 1 })))
                .expect_err("a server-owned row must be refused");
            assert_eq!(error, Decision::NotClientWritable.rejected());
        }
    }

    #[test]
    fn a_task_payload_may_carry_a_due_date() {
        // The derived-field guard is entity-scoped on purpose: a task's `due_at` is the
        // client's to set (Phase 9 pushes one), so only a card's payload is guarded. A
        // flat name list would refuse every task push with a due date.
        decide_one(
            Entity::Task,
            Op::Upsert,
            Some(serde_json::json!({ "title": "Read", "due_at": "2026-01-01T00:00:00Z" })),
        )
        .expect("a task's due_at is the client's field, not a derived one");
    }

    #[test]
    fn a_card_payload_carrying_derived_fields_is_guarded() {
        assert!(carries_derived_field(
            Entity::Flashcard,
            Some(&serde_json::json!({ "ease": 2.9 }))
        ));
        assert!(!carries_derived_field(
            Entity::Flashcard,
            Some(&serde_json::json!({ "front": "a" }))
        ));
        // And the entity scoping, stated as its own assertion.
        assert!(!carries_derived_field(
            Entity::Task,
            Some(&serde_json::json!({ "due_at": "2026-01-01T00:00:00Z" }))
        ));
    }

    #[test]
    fn a_delete_of_an_append_only_row_is_refused() {
        // An append-only row has no tombstone: the op rule refuses it before the
        // entitlement engine is reached.
        let error = decide_one(Entity::FlashcardReview, Op::Delete, None)
            .expect_err("an append-only row cannot be deleted");
        assert_eq!(error, Decision::OpNotAccepted.rejected());
    }

    #[test]
    fn a_read_only_device_may_not_push() {
        let error = decide(
            &plans(),
            &PlanFacts {
                upgrade_offerable: true,
            },
            &snapshot(),
            true,
            &mutation(
                Entity::Task,
                Op::Upsert,
                Some(serde_json::json!({ "title": "Read" })),
            ),
        )
        .expect_err("a read-only device may read, not write");
        assert_eq!(error, Decision::ReadOnlyAccount.rejected());
    }
}
