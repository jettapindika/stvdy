//! The client engine over an in-memory store and a scripted transport.
//!
//! Both doubles live in `stvdy_sync::client::testing` and are shared with the convergence
//! suite (`crates/server/tests/sync_convergence.rs`), which runs this same engine against
//! the real routes. What is asserted here is the engine's own behaviour — paging, the
//! transaction per page, deferral, the four failure reactions, the outbox order and the
//! status derivation — so a defect can be located without a database.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use serde_json::json;
use stvdy_sync::client::testing::{
    InMemoryStore, ScriptedTransport, delete, page, patch_mutation, upsert, upsert_without_row,
};
use stvdy_sync::client::{Engine, LocalStore, Retry, SyncState, TransportError, plan};
use stvdy_sync::proto::{
    DEFAULT_LIMIT, Entity, Mutation, MutationResult, Op, PullQuery, PushResponse,
};
use uuid::Uuid;

fn engine_over(transport: &ScriptedTransport) -> Engine<InMemoryStore, ScriptedTransport> {
    Engine::new(InMemoryStore::new(), transport.clone(), Uuid::now_v7())
}

/// One pass pulls until `has_more` is false, applies every page, and leaves the cursor at
/// the last seq — the property the whole client exists for.
#[tokio::test]
async fn a_pass_pulls_until_has_more_is_false_and_advances_the_cursor() {
    let workspace = Uuid::now_v7();
    let source = Uuid::now_v7();
    let task = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    transport.answer_pull(page(
        vec![
            upsert(
                Entity::Workspace,
                workspace,
                1,
                json!({ "id": workspace, "name": "Kuliah" }),
            ),
            upsert(
                Entity::Source,
                source,
                2,
                json!({ "id": source, "workspace_id": workspace, "title": "Materi" }),
            ),
        ],
        9,
        true,
    ));
    transport.answer_pull(page(
        vec![upsert(
            Entity::Task,
            task,
            9,
            json!({ "id": task, "title": "Tugas" }),
        )],
        9,
        false,
    ));

    let mut engine = engine_over(&transport);
    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Synced);

    assert_eq!(engine.store().cursor().unwrap(), 9);
    assert_eq!(
        transport.queries(),
        vec![
            PullQuery {
                cursor: 0,
                offset: 0,
                limit: DEFAULT_LIMIT,
            },
            // The second page asks for the anchor the first one returned, at the offset it
            // has already read: the paging the anchor exists for (A164).
            PullQuery {
                cursor: 9,
                offset: 2,
                limit: DEFAULT_LIMIT,
            },
        ]
    );
    for (entity, id) in [
        (Entity::Workspace, workspace),
        (Entity::Source, source),
        (Entity::Task, task),
    ] {
        assert!(
            engine.store().row(entity, id).is_some(),
            "{entity:?} did not land"
        );
    }
}

/// The apply is one transaction with `defer_foreign_keys` on, so a page whose child
/// arrives before its parent still lands. The store records the transaction count, so
/// "one transaction" is asserted rather than assumed.
#[tokio::test]
async fn a_page_is_applied_in_one_transaction_with_deferred_foreign_keys() {
    let workspace = Uuid::now_v7();
    let source = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    // The child first, on purpose: the foreign key is checked at the end of the
    // transaction, by which point its parent is in the same page.
    transport.answer_pull(page(
        vec![
            upsert(
                Entity::Source,
                source,
                2,
                json!({ "id": source, "workspace_id": workspace, "title": "Materi" }),
            ),
            upsert(
                Entity::Workspace,
                workspace,
                1,
                json!({ "id": workspace, "name": "Kuliah" }),
            ),
        ],
        2,
        false,
    ));

    let mut engine = engine_over(&transport);
    engine.sync_once().await.unwrap();

    let store = engine.store();
    assert_eq!(store.transactions(), 1, "one page is one transaction");
    assert!(store.deferred().is_empty(), "nothing needed deferring");
    assert!(store.row(Entity::Source, source).is_some());
    assert!(store.row(Entity::Workspace, workspace).is_some());
}

/// A row whose parent is still missing after the whole pass is deferred, not dropped, and
/// is retried on the next pull — the roadmap's rule.
#[tokio::test]
async fn a_row_whose_parent_is_missing_is_deferred_and_retried() {
    let workspace = Uuid::now_v7();
    let source = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    // The child alone: its parent has not been delivered yet.
    transport.answer_pull(page(
        vec![upsert(
            Entity::Source,
            source,
            2,
            json!({ "id": source, "workspace_id": workspace, "title": "Materi" }),
        )],
        2,
        false,
    ));
    // The parent arrives on the next pass.
    transport.answer_pull(page(
        vec![upsert(
            Entity::Workspace,
            workspace,
            3,
            json!({ "id": workspace, "name": "Kuliah" }),
        )],
        3,
        false,
    ));

    let mut engine = engine_over(&transport);
    engine.sync_once().await.unwrap();
    assert!(
        engine.store().row(Entity::Source, source).is_none(),
        "a child must not be written before its parent"
    );
    assert_eq!(
        engine
            .store()
            .deferred()
            .iter()
            .map(|change| (change.entity, change.id))
            .collect::<Vec<_>>(),
        vec![(Entity::Source, source)],
        "the row is deferred, not dropped"
    );

    engine.sync_once().await.unwrap();
    assert!(engine.store().row(Entity::Workspace, workspace).is_some());
    assert!(
        engine.store().row(Entity::Source, source).is_some(),
        "the deferred row must be retried and land"
    );
    assert!(engine.store().deferred().is_empty());
}

/// A 410 wipes the local state, resets the cursor to 0 and re-pulls the whole history in
/// the same pass (the exit gate's resync).
#[tokio::test]
async fn a_410_resyncs_from_zero_without_surfacing_an_error() {
    let gone = Uuid::now_v7();
    let kept = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    // The first pass reads the account as it stands.
    transport.answer_pull(page(
        vec![
            upsert(
                Entity::Task,
                gone,
                3,
                json!({ "id": gone, "title": "Lama" }),
            ),
            upsert(
                Entity::Task,
                kept,
                4,
                json!({ "id": kept, "title": "Tetap" }),
            ),
        ],
        4,
        false,
    ));
    // The second is told its cursor is older than retained history, and the resync it
    // triggers reads the whole history again from zero.
    transport.fail_pull(TransportError::Stale);
    transport.answer_pull(page(
        vec![upsert(
            Entity::Task,
            kept,
            4,
            json!({ "id": kept, "title": "Tetap" }),
        )],
        4,
        false,
    ));

    let mut engine = engine_over(&transport);
    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Synced);
    assert_eq!(engine.store().cursor().unwrap(), 4);

    // The 410 is not an error the user sees: the pass resyncs and reports a normal state.
    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Synced);
    assert_eq!(engine.store().cursor().unwrap(), 4);
    assert_eq!(
        transport.queries()[2],
        PullQuery {
            cursor: 0,
            offset: 0,
            limit: DEFAULT_LIMIT,
        },
        "the resync must start from zero"
    );
    assert!(
        engine.store().row(Entity::Task, kept).is_some(),
        "the retained history must arrive"
    );
    assert!(
        engine.store().row(Entity::Task, gone).is_none(),
        "the wipe is real: a row the resync did not deliver is gone"
    );
}

/// A 401 on this device is `Revoked`: the store is wiped including the outbox, because a
/// pending offline write from a revoked device must not be replayed by the next account
/// that signs in on this machine (A186).
#[tokio::test]
async fn a_401_wipes_the_cache_and_the_outbox() {
    let task = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    transport.answer_pull(page(
        vec![upsert(
            Entity::Task,
            task,
            4,
            json!({ "id": task, "title": "Tugas" }),
        )],
        4,
        false,
    ));

    let mut engine = engine_over(&transport);
    engine.sync_once().await.unwrap();
    assert!(engine.store().row(Entity::Task, task).is_some());

    engine
        .enqueue(patch_mutation(
            Entity::Task,
            task,
            json!({ "title": "Belum terkirim" }),
            None,
        ))
        .unwrap();
    assert_eq!(engine.store().outbox_len().unwrap(), 1);

    transport.fail_push(TransportError::Revoked);
    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Revoked);
    assert_eq!(engine.status().unwrap(), SyncState::Revoked);
    assert_eq!(
        engine.store().outbox_len().unwrap(),
        0,
        "a pending write from a revoked device must not survive the wipe"
    );
    assert!(
        engine.store().row(Entity::Task, task).is_none(),
        "the cache is wiped"
    );
    assert_eq!(engine.store().cursor().unwrap(), 0);
}

/// A network failure leaves the outbox intact and the status `offline`; nothing is
/// dropped and the next pass retries in order.
#[tokio::test]
async fn a_network_failure_keeps_the_outbox_in_order() {
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    transport.fail_push(TransportError::Network("the socket closed".to_string()));

    let mut engine = engine_over(&transport);
    let one = patch_mutation(Entity::Task, first, json!({ "title": "Satu" }), None);
    let two = patch_mutation(Entity::Task, second, json!({ "title": "Dua" }), None);
    let ids = [one.mutation_id, two.mutation_id];
    engine.enqueue(one).unwrap();
    engine.enqueue(two).unwrap();

    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Offline);
    assert_eq!(engine.status().unwrap(), SyncState::Offline);
    assert_eq!(
        engine
            .store()
            .pending(10)
            .unwrap()
            .iter()
            .map(|mutation| mutation.mutation_id)
            .collect::<Vec<_>>(),
        ids.to_vec(),
        "nothing is dropped and the order holds"
    );
    assert_eq!(
        engine.store().cursor().unwrap(),
        0,
        "a failed push ends the pass before the pull"
    );
    assert!(
        engine.next_delay() > Duration::ZERO,
        "a failure must leave a backoff behind"
    );
}

/// The outbox drains in enqueue order, one mutation per call to the store, and a mutation
/// that is `rejected` is removed with its reason recorded while the rest continue (the
/// server decides; the client only reacts).
#[tokio::test]
async fn the_outbox_drains_in_order_and_a_rejection_does_not_stop_it() {
    let mutations: Vec<Mutation> = (0..3)
        .map(|index| {
            patch_mutation(
                Entity::Task,
                Uuid::now_v7(),
                json!({ "title": format!("Tugas {index}") }),
                None,
            )
        })
        .collect();
    let mutation_ids: Vec<Uuid> = mutations.iter().map(|m| m.mutation_id).collect();
    let task_ids: Vec<Uuid> = mutations.iter().map(|m| m.id).collect();

    let transport = ScriptedTransport::new();
    transport.answer_push(PushResponse {
        results: vec![
            MutationResult::Applied { seq: 1 },
            MutationResult::Rejected {
                code: "read_only_entity".to_string(),
                message: "this row is written by the server".to_string(),
            },
            MutationResult::Applied { seq: 2 },
        ],
    });
    transport.answer_pull(page(vec![], 0, false));

    let mut engine = engine_over(&transport);
    for mutation in mutations {
        engine.enqueue(mutation).unwrap();
    }

    assert_eq!(
        engine.sync_once().await.unwrap(),
        SyncState::NeedsAttention {
            code: "read_only_entity".to_string()
        }
    );
    assert_eq!(
        engine.store().outbox_len().unwrap(),
        0,
        "the queue drains; one rejection does not stall the rest"
    );

    let requests = transport.requests();
    assert_eq!(requests.len(), 1, "one batch carried all three");
    assert_eq!(
        requests[0]
            .mutations
            .iter()
            .map(|mutation| mutation.id)
            .collect::<Vec<_>>(),
        task_ids,
        "the batch keeps enqueue order"
    );
    assert_eq!(
        engine
            .store()
            .resolved()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        mutation_ids,
        "every mutation is resolved individually, in order"
    );
    assert_eq!(
        engine.store().resolved()[1].1,
        MutationResult::Rejected {
            code: "read_only_entity".to_string(),
            message: "this row is written by the server".to_string(),
        },
        "the rejection is kept with the server's own code"
    );
}

/// Backoff is bounded and monotonic, so a flapping network does not turn into a hot loop
/// against the server.
#[test]
fn retry_backoff_is_monotonic_and_capped() {
    let mut retry = Retry::new();
    assert_eq!(
        retry.delay(),
        Duration::ZERO,
        "a pass that reached the server does not wait"
    );

    let mut delays = Vec::new();
    for _ in 0..40 {
        retry.advance();
        delays.push(retry.delay());
    }
    assert_eq!(delays[0], Retry::BASE, "the first failure waits the base");
    assert!(
        delays.windows(2).all(|pair| pair[0] <= pair[1]),
        "the backoff must never go backwards: {delays:?}"
    );
    assert_eq!(
        *delays.last().unwrap(),
        Retry::MAX,
        "and it must reach the cap: {delays:?}"
    );
    assert!(delays.iter().all(|delay| *delay <= Retry::MAX));

    retry.reset();
    assert_eq!(retry.delay(), Duration::ZERO);
    assert_eq!(retry.failures(), 0);
}

/// Status is derived, never asserted: `synced` requires an empty outbox, a successful pull
/// and a known `last_synced_at` (A187). This is gate 7(b) as a test — a status the app
/// cannot compute must not be shown.
#[tokio::test]
async fn synced_requires_an_empty_outbox_and_a_successful_pass() {
    let task = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    let mut engine = engine_over(&transport);

    // A device that has never completed a pass is not "Synced".
    assert_eq!(engine.status().unwrap(), SyncState::Syncing);

    transport.answer_pull(page(
        vec![upsert(
            Entity::Task,
            task,
            1,
            json!({ "id": task, "title": "Tugas" }),
        )],
        1,
        false,
    ));
    engine.sync_once().await.unwrap();
    assert_eq!(engine.status().unwrap(), SyncState::Synced);

    // A write enqueued after the pass makes it unsynced again: no status reads "Synced"
    // while the outbox is non-empty.
    engine
        .enqueue(patch_mutation(
            Entity::Task,
            task,
            json!({ "title": "Baru" }),
            None,
        ))
        .unwrap();
    assert_eq!(engine.status().unwrap(), SyncState::Syncing);

    // A rejected mutation is `needs_attention`, with the server's code.
    transport.answer_push(PushResponse {
        results: vec![MutationResult::Rejected {
            code: "validation".to_string(),
            message: "the payload carries no field for this entity".to_string(),
        }],
    });
    transport.answer_pull(page(vec![], 1, false));
    assert_eq!(
        engine.sync_once().await.unwrap(),
        SyncState::NeedsAttention {
            code: "validation".to_string()
        }
    );
    assert_eq!(
        engine.status().unwrap(),
        SyncState::NeedsAttention {
            code: "validation".to_string()
        }
    );
}

/// `data: null` on an `upsert` is applied as a delete, because a row this device cannot
/// see must never be created on it (A188).
#[tokio::test]
async fn a_null_payload_is_applied_as_a_delete() {
    let task = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    transport.answer_pull(page(
        vec![upsert(
            Entity::Task,
            task,
            1,
            json!({ "id": task, "title": "Tugas" }),
        )],
        1,
        false,
    ));
    // The wire shape: an `upsert` whose row this device cannot see ships `data: null`,
    // which deserialises to no payload at all.
    transport.answer_pull(page(
        vec![upsert_without_row(Entity::Task, task, 2)],
        2,
        false,
    ));

    let mut engine = engine_over(&transport);
    engine.sync_once().await.unwrap();
    assert!(engine.store().row(Entity::Task, task).is_some());

    engine.sync_once().await.unwrap();
    assert!(
        engine.store().row(Entity::Task, task).is_none(),
        "a row this device cannot see must never exist on it"
    );

    // And the normalisation is the planner's, not an accident of the store.
    let set = plan(&[upsert_without_row(Entity::Task, task, 2)]);
    assert_eq!(set.writes().len(), 1);
    assert_eq!(set.writes()[0].op, Op::Delete);
}

/// The planner keeps only the newest change per row and orders parents before children, so
/// a page's intermediate versions are not written and its children are not deferred behind
/// a parent that is in the same page.
#[test]
fn the_planner_keeps_the_newest_change_per_row_and_orders_parents_first() {
    let task = Uuid::now_v7();
    let workspace = Uuid::now_v7();
    let set = plan(&[
        upsert(
            Entity::Task,
            task,
            7,
            json!({ "id": task, "title": "Lama" }),
        ),
        upsert(
            Entity::Workspace,
            workspace,
            2,
            json!({ "id": workspace, "name": "Kuliah" }),
        ),
        upsert(
            Entity::Task,
            task,
            9,
            json!({ "id": task, "title": "Baru" }),
        ),
    ]);

    assert_eq!(set.writes().len(), 2, "one write per row");
    let writes = set.writes();
    assert_eq!(
        (writes[0].entity, writes[1].entity),
        (Entity::Workspace, Entity::Task),
        "the parent must be written first"
    );
    assert_eq!(writes[1].seq, 9, "and the newest change is the one kept");
}

/// A device whose outbox is empty and whose last pass failed is offline, and the engine
/// retries it with a growing delay rather than a fixed one.
#[tokio::test]
async fn an_offline_pass_backs_off_further_on_each_attempt() {
    let transport = ScriptedTransport::new();
    transport.fail_push(TransportError::Network("down".to_string()));
    transport.fail_push(TransportError::Network("still down".to_string()));

    let mut engine = engine_over(&transport);
    engine
        .enqueue(patch_mutation(
            Entity::Task,
            Uuid::now_v7(),
            json!({ "title": "Tugas" }),
            None,
        ))
        .unwrap();

    engine.sync_once().await.unwrap();
    let first = engine.next_delay();
    engine.sync_once().await.unwrap();
    let second = engine.next_delay();
    assert!(first > Duration::ZERO);
    assert!(second > first, "{second:?} must exceed {first:?}");
}

/// A route-level refusal the request cannot fix is `needs_attention` with the server's own
/// code, and it does not grow the backoff — retrying would repeat it.
#[tokio::test]
async fn a_permanent_refusal_is_reported_with_the_servers_code() {
    let transport = ScriptedTransport::new();
    transport.fail_pull(TransportError::Server {
        status: 403,
        code: "forbidden".to_string(),
    });

    let mut engine = engine_over(&transport);
    assert_eq!(
        engine.sync_once().await.unwrap(),
        SyncState::NeedsAttention {
            code: "forbidden".to_string()
        }
    );
    assert_eq!(engine.next_delay(), Duration::ZERO);

    // A 5xx is transient instead: the device waits and tries again.
    let transport = ScriptedTransport::new();
    transport.fail_pull(TransportError::Server {
        status: 503,
        code: "internal".to_string(),
    });
    let mut engine = engine_over(&transport);
    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Offline);
    assert!(engine.next_delay() > Duration::ZERO);
}

/// A tombstone arrives with no payload, and the row it names is gone from the cache.
#[tokio::test]
async fn a_delete_change_removes_the_row() {
    let task = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    transport.answer_pull(page(
        vec![upsert(
            Entity::Task,
            task,
            1,
            json!({ "id": task, "title": "Tugas" }),
        )],
        1,
        false,
    ));
    transport.answer_pull(page(vec![delete(Entity::Task, task, 2)], 2, false));

    let mut engine = engine_over(&transport);
    engine.sync_once().await.unwrap();
    assert!(engine.store().row(Entity::Task, task).is_some());

    engine.sync_once().await.unwrap();
    assert!(engine.store().row(Entity::Task, task).is_none());
}

/// A `delete` change for a parent takes the descendants the store holds with it, so the
/// cache does not show orphaned children between the parent's tombstone and theirs (A197).
///
/// The page carries the parent's delete and not the children's, which is the whole reason
/// the store does the walk: the rows it must remove are the ones already in the cache. Two
/// levels are used because the relation is recursive, and a non-nesting entity (an event) is
/// asserted to be untouched by the rule.
#[tokio::test]
async fn a_delete_change_removes_the_descendants_the_store_holds() {
    let parent = Uuid::now_v7();
    let child = Uuid::now_v7();
    let grandchild = Uuid::now_v7();
    let unrelated = Uuid::now_v7();
    let event = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    transport.answer_pull(page(
        vec![
            upsert(
                Entity::Task,
                parent,
                1,
                json!({ "id": parent, "title": "Induk" }),
            ),
            upsert(
                Entity::Task,
                child,
                2,
                json!({ "id": child, "parent_id": parent, "title": "Anak" }),
            ),
            upsert(
                Entity::Task,
                grandchild,
                3,
                json!({ "id": grandchild, "parent_id": child, "title": "Cucu" }),
            ),
            upsert(
                Entity::Task,
                unrelated,
                4,
                json!({ "id": unrelated, "title": "Tugas lain" }),
            ),
            upsert(
                Entity::Event,
                event,
                5,
                json!({ "id": event, "title": "Kelas" }),
            ),
        ],
        5,
        false,
    ));
    // The parent's tombstone, and nothing about its descendants.
    transport.answer_pull(page(vec![delete(Entity::Task, parent, 6)], 6, false));

    let mut engine = engine_over(&transport);
    engine.sync_once().await.unwrap();
    assert_eq!(engine.store().dump().len(), 5, "the subtree is cached");

    engine.sync_once().await.unwrap();
    for (entity, id) in [
        (Entity::Task, parent),
        (Entity::Task, child),
        (Entity::Task, grandchild),
    ] {
        assert!(
            engine.store().row(entity, id).is_none(),
            "{id} must leave the cache with its parent: {:?}",
            engine.store().dump()
        );
    }
    // Not a blanket wipe: a task that was never in the subtree, and the event, stay.
    assert!(engine.store().row(Entity::Task, unrelated).is_some());
    assert!(engine.store().row(Entity::Event, event).is_some());
    assert_eq!(engine.store().dump().len(), 2);
}

/// The local-first write path: a row written locally is readable before it is pushed, and
/// the queued mutation survives until the server answers.
#[tokio::test]
async fn a_local_write_is_readable_before_it_is_pushed() {
    let task = Uuid::now_v7();
    let transport = ScriptedTransport::new();
    let mut engine = engine_over(&transport);
    engine.store_mut().write_local(
        Entity::Task,
        task,
        json!({ "id": task, "title": "Tugas", "notes": "Catatan" }),
    );

    let row = engine
        .store()
        .row(Entity::Task, task)
        .expect("the local row");
    assert_eq!(row["title"], json!("Tugas"));
    assert_eq!(engine.status().unwrap(), SyncState::Syncing);

    // The shell queues the mutation for the row it just wrote.
    engine
        .enqueue(patch_mutation(
            Entity::Task,
            task,
            json!({ "title": "Tugas" }),
            None,
        ))
        .unwrap();
    assert_eq!(engine.store().outbox_len().unwrap(), 1);
    assert_eq!(engine.store().pending(10).unwrap()[0].id, task);

    // With the transport down the row is still readable and the write still queued: that
    // is the whole point of writing locally first.
    transport.fail_push(TransportError::Network("down".to_string()));
    assert_eq!(engine.sync_once().await.unwrap(), SyncState::Offline);
    assert_eq!(engine.store().outbox_len().unwrap(), 1);
    assert_eq!(
        engine.store().row(Entity::Task, task).unwrap()["notes"],
        json!("Catatan")
    );
}
