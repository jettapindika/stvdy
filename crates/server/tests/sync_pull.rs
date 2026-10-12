//! `GET /sync/pull`: the cursor rules, the anchor, and the isolation rule that is this
//! phase's named risk.
//!
//! Every case here is driven through the real route rather than against `load_page`,
//! because the thing under test is the statement's `user_id` scope and the handler's
//! cursor handling, neither of which a unit call would exercise.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

/// Signs up, verifies, and returns `(token, user_id, device_id)`.
async fn account(app: &axum::Router, harness: &TestState, email: &str) -> (String, Uuid, Uuid) {
    let token = signed_up(app, harness, email).await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the user row");
    let device_id: Uuid = sqlx::query_scalar("select id from devices where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the device row");
    (token, user_id, device_id)
}

async fn insert_workspace(pool: &PgPool, user_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, $3)")
        .bind(id)
        .bind(user_id)
        .bind(name)
        .execute(pool)
        .await
        .expect("insert workspace");
    id
}

async fn insert_source(pool: &PgPool, user_id: Uuid, workspace_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into sources (id, user_id, workspace_id, kind, title, status)
         values ($1, $2, $3, 'text', $4, 'ready')",
    )
    .bind(id)
    .bind(user_id)
    .bind(workspace_id)
    .bind(title)
    .execute(pool)
    .await
    .expect("insert source");
    id
}

async fn insert_task(pool: &PgPool, user_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, $3)")
        .bind(id)
        .bind(user_id)
        .bind(title)
        .execute(pool)
        .await
        .expect("insert task");
    id
}

fn changes(body: &serde_json::Value) -> &Vec<serde_json::Value> {
    body["changes"].as_array().expect("a changes array")
}

/// The order the roadmap fixes: an initial pass is dependency rank first, then seq, so
/// a client's cache has every parent before its children (A164).
#[sqlx::test(migrations = "../../migrations")]
async fn the_initial_pass_is_rank_ordered(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "rank@example.test").await;

    // Inserted in an order that is *not* rank order, so a seq-ordered pass would fail
    // this: the task (rank 8) first, then the workspace (rank 1), then the source (2).
    let task = insert_task(&harness.state.pool, user_id, "Tugas").await;
    let workspace = insert_workspace(&harness.state.pool, user_id, "Kuliah").await;
    let source = insert_source(&harness.state.pool, user_id, workspace, "Materi").await;

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mine: Vec<String> = changes(&body)
        .iter()
        .filter_map(|change| change["id"].as_str())
        .filter(|id| {
            [task, workspace, source]
                .iter()
                .any(|known| known.to_string() == *id)
        })
        .map(str::to_string)
        .collect();
    assert_eq!(
        mine,
        vec![workspace.to_string(), source.to_string(), task.to_string()],
        "a parent must precede its child: workspace (1), source (2), task (8)"
    );
}

/// A change committed while the pass is running must not shift a row out of the pass.
/// This is the defect the anchor exists for: a new rank-1 workspace inserted between two
/// pages moves every later row one position right, and unanchored offset paging would
/// skip the row that fell across the boundary — then advance the cursor past it, losing
/// it permanently.
#[sqlx::test(migrations = "../../migrations")]
async fn a_write_during_the_initial_pass_cannot_shift_a_row_out_of_it(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "anchor@example.test").await;

    let workspace = insert_workspace(&harness.state.pool, user_id, "Kuliah").await;
    let source = insert_source(&harness.state.pool, user_id, workspace, "Materi").await;
    let task = insert_task(&harness.state.pool, user_id, "Tugas").await;

    let (status, page1) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0&limit=2",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["has_more"], true, "{page1}");
    let anchor = page1["next_cursor"].as_i64().expect("an anchor");

    // A rank-1 row inserted mid-pass. It sorts before everything the client has already
    // read, which is exactly the shift an unanchored offset page would stumble on.
    let late = insert_workspace(&harness.state.pool, user_id, "Pindah").await;

    let (status, page2) = json(
        &app,
        "GET",
        &format!("/sync/pull?cursor={anchor}&offset=2&limit=2"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(
        page2["next_cursor"].as_i64(),
        Some(anchor),
        "the anchor must not move between pages of one pass: {page2}"
    );
    assert_eq!(page2["has_more"], false, "{page2}");

    // Every row of the pass set is delivered exactly once, and the mid-pass write is not
    // in it — it has a seq above the anchor, so it arrives on the next incremental pull.
    let mut seen: Vec<Uuid> = changes(&page1)
        .iter()
        .chain(changes(&page2).iter())
        .filter_map(|change| {
            change["id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
        })
        .collect();
    seen.sort();
    let mut expected = vec![workspace, source, task];
    expected.sort();
    assert_eq!(seen, expected, "the pass must cover every row once");
    assert!(
        !changes(&page2)
            .iter()
            .any(|change| change["id"] == late.to_string()),
        "a row committed after the anchor must not enter the pass"
    );

    let (status, incremental) = json(
        &app,
        "GET",
        &format!("/sync/pull?cursor={anchor}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{incremental}");
    assert!(
        changes(&incremental)
            .iter()
            .any(|change| change["id"] == late.to_string()),
        "the mid-pass write must arrive on the next incremental pull: {incremental}"
    );
}

/// The named risk of this phase, as a test. A pull for A contains no seq that belongs
/// to B, and no row id that B owns.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pull_never_contains_another_accounts_seq(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (a_token, a_user, _) = account(&app, &harness, "a@example.test").await;
    let (_, b_user, _) = account(&app, &harness, "b@example.test").await;

    let a_workspace = insert_workspace(&harness.state.pool, a_user, "A").await;
    let b_workspace = insert_workspace(&harness.state.pool, b_user, "B").await;
    let b_task = insert_task(&harness.state.pool, b_user, "B's task").await;

    let b_seqs: Vec<i64> = sqlx::query_scalar("select seq from change_log where user_id = $1")
        .bind(b_user)
        .fetch_all(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &a_token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for change in changes(&body) {
        let seq = change["seq"].as_i64().unwrap();
        assert!(
            !b_seqs.contains(&seq),
            "A's pull contains B's seq {seq}: {body}"
        );
        assert_ne!(
            change["id"].as_str(),
            Some(b_workspace.to_string().as_str()),
            "A's pull contains B's workspace"
        );
        assert_ne!(
            change["id"].as_str(),
            Some(b_task.to_string().as_str()),
            "A's pull contains B's task"
        );
    }
    assert!(
        changes(&body)
            .iter()
            .any(|c| c["id"] == a_workspace.to_string()),
        "A's own change must be present: {body}"
    );
}

/// The same risk one layer down: a planted `change_log` row for A whose entity_id is
/// B's task must not produce B's row. It produces `data: null`, which the client
/// applies as a delete (A188), never a cross-tenant read.
#[sqlx::test(migrations = "../../migrations")]
async fn the_payload_loader_cannot_be_pointed_at_another_users_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (a_token, a_user, _) = account(&app, &harness, "plant-a@example.test").await;
    let (_, b_user, _) = account(&app, &harness, "plant-b@example.test").await;

    let b_task = insert_task(&harness.state.pool, b_user, "B's secret").await;

    // A change_log row for A that names B's task. The trigger would never write this;
    // it is the pathological row the isolation rule has to survive.
    sqlx::query(
        "insert into change_log (user_id, entity, entity_id, op)
         values ($1, 'tasks', $2, 'upsert')",
    )
    .bind(a_user)
    .bind(b_task)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &a_token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let planted = changes(&body)
        .iter()
        .find(|change| change["id"] == b_task.to_string())
        .expect("the planted change is delivered");
    assert_eq!(
        planted["data"],
        serde_json::Value::Null,
        "B's row must never be loaded for A: {planted}"
    );
    assert_eq!(planted["op"], "upsert", "{planted}");
}

/// `410 resync_required` is the contract's code and it is already the error's mapping
/// (A166): no new variant, no new body shape.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cursor_below_the_watermark_is_410_resync_required(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "stale@example.test").await;

    let first = insert_workspace(&harness.state.pool, user_id, "One").await;
    insert_workspace(&harness.state.pool, user_id, "Two").await;

    let seqs: Vec<i64> =
        sqlx::query_scalar("select seq from change_log where user_id = $1 order by seq")
            .bind(user_id)
            .fetch_all(&harness.state.pool)
            .await
            .unwrap();
    let stale_cursor = seqs[0];
    let watermark = seqs[seqs.len() - 1];

    sqlx::query(
        "insert into change_log_watermark (user_id, min_retained_seq, compacted_at)
         values ($1, $2, now())",
    )
    .bind(user_id)
    .bind(watermark)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        &format!("/sync/pull?cursor={stale_cursor}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["code"], "resync_required", "{body}");

    // A cursor of 0 is never stale, so a full resync is always available (A165).
    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        changes(&body).iter().any(|c| c["id"] == first.to_string()),
        "the surviving history must still arrive from zero: {body}"
    );
}

/// A tombstone is `op: "delete"` with `data: null`, and the seq of a tombstone is
/// returned like any other change.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tombstone_carries_no_payload(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "tombstone@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Dihapus").await;
    sqlx::query("update tasks set deleted_at = now() where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let tombstone = changes(&body)
        .iter()
        .find(|change| change["id"] == task.to_string() && change["op"] == "delete")
        .expect("a delete change");
    assert_eq!(tombstone["data"], serde_json::Value::Null, "{tombstone}");
}

/// Paging: `limit` rows, `has_more` true while more remain, and `next_cursor` equal to
/// the last seq of the page.
#[sqlx::test(migrations = "../../migrations")]
async fn a_page_is_limited_and_has_more_says_whether_to_ask_again(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "paging@example.test").await;

    for index in 0..3 {
        insert_task(&harness.state.pool, user_id, &format!("Task {index}")).await;
    }

    let (status, page1) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0&limit=2",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(changes(&page1).len(), 2, "{page1}");
    assert_eq!(page1["has_more"], true, "{page1}");

    let anchor = page1["next_cursor"].as_i64().unwrap();
    let (status, page2) = json(
        &app,
        "GET",
        &format!("/sync/pull?cursor={anchor}&offset=2&limit=2"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["has_more"], false, "{page2}");

    // The incremental walk reports the last seq of the page and stops when it is empty.
    let last_seq = page2["changes"].as_array().unwrap().last().unwrap()["seq"]
        .as_i64()
        .unwrap();
    let (status, empty) = json(
        &app,
        "GET",
        &format!("/sync/pull?cursor={last_seq}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert!(changes(&empty).is_empty(), "{empty}");
    assert_eq!(empty["has_more"], false, "{empty}");
    assert_eq!(
        empty["next_cursor"].as_i64(),
        Some(last_seq),
        "a quiet account's cursor must not move: {empty}"
    );
}

/// Pulling writes `devices.last_sync_at` (A180), which is the only source of the
/// Devices screen's "Last synced".
#[sqlx::test(migrations = "../../migrations")]
async fn a_pull_records_when_this_device_last_synced(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, _, device_id) = account(&app, &harness, "lastsync@example.test").await;

    let before: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select last_sync_at from devices where id = $1")
            .bind(device_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(before.is_none(), "nothing wrote last_sync_at before a pull");

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select last_sync_at from devices where id = $1")
            .bind(device_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(after.is_some(), "a pull must record the device's last sync");
}

/// A `user_settings` change is keyed by the user id, because that is what
/// `log_change_by_user` writes into `entity_id` (A179).
#[sqlx::test(migrations = "../../migrations")]
async fn a_settings_change_is_keyed_by_the_user_id(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "settings@example.test").await;

    // The account has no settings row until something writes one, so the test creates
    // it; the update is what produces the change this test reads.
    sqlx::query("insert into user_settings (user_id) values ($1)")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update user_settings set study_start_hour = 6 where user_id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let settings = changes(&body)
        .iter()
        .find(|change| change["entity"] == "user_settings")
        .expect("a settings change");
    assert_eq!(
        settings["id"],
        user_id.to_string(),
        "a settings change is keyed by the user id: {settings}"
    );
    assert_eq!(settings["data"]["study_start_hour"], 6, "{settings}");
    assert!(
        settings["data"].get("user_id").is_none(),
        "the payload allowlist excludes user_id: {settings}"
    );
}

/// The fields Phase 9 added ship in a pull, which is the other half of the same allowlist.
///
/// A push that lands a column and a pull that never ships it is a device that writes a value
/// it cannot read back on any other device: the settings a phone chose would stay on the
/// phone, and a breakdown's subtask order would not survive the trip. Asserted through the
/// real route rather than against the list, because the list is only interesting for what it
/// does to a payload.
#[sqlx::test(migrations = "../../migrations")]
async fn the_fields_phase_nine_added_ship_in_a_pull(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "new-columns@example.test").await;

    let task = insert_task(&harness.state.pool, user_id, "Kerjakan latihan").await;
    sqlx::query("update tasks set position = 2 where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    sqlx::query("insert into user_settings (user_id) values ($1)")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "update user_settings
            set quiet_start_hour = 22, quiet_end_hour = 7, language = 'id', theme = 'dark'
          where user_id = $1",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let task_change = changes(&body)
        .iter()
        .find(|change| change["id"] == task.to_string())
        .expect("the task change");
    assert_eq!(
        task_change["data"]["position"], 2,
        "`position` must ship, or a breakdown's order dies on the second device: {task_change}"
    );

    let settings = changes(&body)
        .iter()
        .find(|change| change["entity"] == "user_settings")
        .expect("the settings change");
    for (field, expected) in [
        ("quiet_start_hour", serde_json::json!(22)),
        ("quiet_end_hour", serde_json::json!(7)),
        ("language", serde_json::json!("id")),
        ("theme", serde_json::json!("dark")),
    ] {
        assert_eq!(
            settings["data"][field], expected,
            "{field} must ship, or the choice never leaves the device that made it: {settings}"
        );
    }
}

/// A limit of 0 is a 400, and a limit over the cap is clamped rather than refused.
#[sqlx::test(migrations = "../../migrations")]
async fn the_page_size_is_validated_and_clamped(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "limit@example.test").await;
    insert_task(&harness.state.pool, user_id, "Task").await;

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0&limit=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "validation", "{body}");

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0&limit=100000",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!changes(&body).is_empty(), "{body}");

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0&offset=-1",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// Every change of a live row in one page carries the row.
///
/// The payload is a property of the **row**, not of the change: a row created and edited
/// inside one pass set appears several times in one page, and each of those changes must
/// ship the row. A `data: null` on an `upsert` is A188's signal that the row is *not
/// visible to this account*, which the client applies as a delete — so handing the payload
/// to only the first change of a row would make every later change read as a row the
/// device may not see, and the client would delete a row the account still has.
#[sqlx::test(migrations = "../../migrations")]
async fn every_change_of_a_live_row_in_one_page_carries_the_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, user_id, _) = account(&app, &harness, "payload@example.test").await;

    // One row, three changes: an insert and two edits, all inside the pass set.
    let task = insert_task(&harness.state.pool, user_id, "Pertama").await;
    sqlx::query("update tasks set title = 'Kedua' where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update tasks set title = 'Ketiga' where id = $1")
        .bind(task)
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mine: Vec<&serde_json::Value> = changes(&body)
        .iter()
        .filter(|change| change["id"] == task.to_string())
        .collect();
    assert_eq!(
        mine.len(),
        3,
        "the row has three changes in the pass: {body}"
    );
    for change in &mine {
        assert_eq!(change["op"], "upsert", "{change}");
        assert_ne!(
            change["data"],
            serde_json::Value::Null,
            "a live row's change must carry the row: {change}"
        );
        assert_eq!(
            change["data"]["title"], "Ketiga",
            "and the payload is the row as it stands: {change}"
        );
    }
}

/// The null signal A188 defines survives that: a row this account cannot load still ships
/// `data: null`, so a device never creates a row it may not see.
///
/// The two halves belong together. The naive repair for the case above — making every
/// payload non-null — would delete the signal and let a device invent a row from another
/// account's `change_log` row.
#[sqlx::test(migrations = "../../migrations")]
async fn a_row_that_cannot_be_loaded_still_ships_a_null_payload(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let (token, a_user, _) = account(&app, &harness, "null-a@example.test").await;
    let (_, b_user, _) = account(&app, &harness, "null-b@example.test").await;

    let b_task = insert_task(&harness.state.pool, b_user, "B's secret").await;
    // Two changes for one id, neither loadable for A: both must ship the null.
    sqlx::query(
        "insert into change_log (user_id, entity, entity_id, op)
         values ($1, 'tasks', $2, 'upsert'), ($1, 'tasks', $2, 'upsert')",
    )
    .bind(a_user)
    .bind(b_task)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "GET",
        "/sync/pull?cursor=0",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let planted: Vec<&serde_json::Value> = changes(&body)
        .iter()
        .filter(|change| change["id"] == b_task.to_string())
        .collect();
    assert_eq!(planted.len(), 2, "{body}");
    for change in planted {
        assert_eq!(
            change["data"],
            serde_json::Value::Null,
            "B's row must never be loaded for A: {change}"
        );
    }
}
