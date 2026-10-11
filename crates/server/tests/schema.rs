// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Task 1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Guards the sync contract: every +sync table carries the sync columns and both
//! triggers, the trigger set matches the spec's table list, and a write produces
//! exactly one `change_log` row with the right op.
//!
//! Requires DATABASE_URL pointing at a Postgres server with pgvector.
//! `sqlx::test` creates a scratch database per test and runs ../../migrations.

use std::collections::BTreeSet;

use sqlx::{PgPool, Row};
use uuid::Uuid;

/// The synced tables, i.e. the contract in migrations/0017_sync_triggers.sql.
const SYNC_TABLES: &[&str] = &[
    "workspaces",
    "sources",
    "messages",
    "decks",
    "flashcards",
    "flashcard_reviews",
    "quizzes",
    "quiz_questions",
    "quiz_attempts",
    "reports",
    "cheatsheets",
    "tasks",
    "goals",
    "events",
    "study_sessions",
    "pending_actions",
];

/// Tables that must NOT be synced: server-side internals and billing data.
const NON_SYNC_TABLES: &[&str] = &[
    "chunks",
    "change_log",
    "mutation_log",
    "jobs",
    "usage_events",
    "credit_ledger",
    "payments",
    "subscriptions",
    "webhook_events",
    "discount_codes",
    "user_api_keys",
    "report_revisions",
    "calendar_connections",
    "devices",
    "auth_sessions",
    "push_tokens",
    "plans",
    "users",
];

async fn insert_user(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into users (id, email, password_hash) values ($1, $2, 'x')")
        .bind(id)
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await
        .expect("insert user");
    id
}

async fn insert_device(pool: &PgPool, user: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into devices (id, user_id, name, platform) values ($1, $2, 'test', 'macos')",
    )
    .bind(id)
    .bind(user)
    .execute(pool)
    .await
    .expect("insert device");
    id
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_sync_table_has_the_sync_columns(pool: PgPool) {
    for table in SYNC_TABLES {
        let rows = sqlx::query(
            "select column_name from information_schema.columns
             where table_schema = 'public' and table_name = $1",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .expect("query columns");
        let columns: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();

        for required in ["user_id", "updated_at", "deleted_at"] {
            assert!(
                columns.iter().any(|c| c == required),
                "{table} is missing the sync column {required} (has {columns:?})"
            );
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_settings_has_updated_at_but_no_tombstone(pool: PgPool) {
    let columns: Vec<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
         where table_schema = 'public' and table_name = 'user_settings'",
    )
    .fetch_all(&pool)
    .await
    .expect("query columns");

    assert!(columns.iter().any(|c| c == "user_id"));
    assert!(columns.iter().any(|c| c == "updated_at"));
    assert!(
        !columns.iter().any(|c| c == "deleted_at"),
        "user_settings has no tombstones"
    );
    assert!(
        !columns.iter().any(|c| c == "id"),
        "user_settings is keyed by user_id, which is why it needs log_change_by_user"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_sync_table_has_both_triggers(pool: PgPool) {
    for table in SYNC_TABLES {
        let names: Vec<String> = sqlx::query_scalar(
            "select t.tgname from pg_trigger t
             join pg_class c on c.oid = t.tgrelid
             where not t.tgisinternal and c.relname = $1",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .expect("query triggers");

        assert!(
            names
                .iter()
                .any(|n| n == &format!("{table}_touch_updated_at")),
            "{table} is missing touch_updated_at (has {names:?})"
        );
        assert!(
            names.iter().any(|n| n == &format!("{table}_log_change")),
            "{table} is missing log_change (has {names:?})"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_sync_tables_have_no_triggers(pool: PgPool) {
    for table in NON_SYNC_TABLES {
        let count: i64 = sqlx::query_scalar(
            "select count(*) from pg_trigger t
             join pg_class c on c.oid = t.tgrelid
             where not t.tgisinternal and c.relname = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("count triggers");
        assert_eq!(count, 0, "{table} must not be synced");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_trigger_set_is_exactly_the_sync_tables(pool: PgPool) {
    // Catches both directions of drift: a synced table missing its triggers, and
    // an internal table that accidentally got them.
    let with_triggers: BTreeSet<String> = sqlx::query_scalar(
        "select distinct c.relname from pg_trigger t
         join pg_class c on c.oid = t.tgrelid
         where not t.tgisinternal and c.relnamespace = 'public'::regnamespace",
    )
    .fetch_all(&pool)
    .await
    .expect("list trigger tables")
    .into_iter()
    .collect();

    let expected: BTreeSet<String> = SYNC_TABLES
        .iter()
        .map(|t| (*t).to_string())
        .chain(std::iter::once("user_settings".to_string()))
        .collect();

    assert_eq!(
        with_triggers, expected,
        "the trigger set must be exactly the synced tables"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_table_in_the_spec_ddl_exists(pool: PgPool) {
    // The spec's schema block is exhaustive, so it is the oracle: a table dropped
    // from a migration must fail here rather than pass silently because a
    // hand-written list was updated to match the mistake.
    let spec = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/SPEC.md"),
    )
    .expect("docs/SPEC.md must be readable");

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for line in spec.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("create table ") else {
            continue;
        };
        let name = rest
            .trim()
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or_default();
        if !name.is_empty() {
            expected.insert(name.to_string());
        }
    }
    assert!(
        expected.len() > 25,
        "expected to extract the spec's table list, got {expected:?}"
    );

    let actual: BTreeSet<String> = sqlx::query_scalar(
        "select table_name from information_schema.tables
         where table_schema = 'public' and table_type = 'BASE TABLE'",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables")
    .into_iter()
    .collect();

    let missing: Vec<&String> = expected.difference(&actual).collect();
    assert!(
        missing.is_empty(),
        "spec tables missing from the schema: {missing:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn tables_added_beyond_the_spec_are_deliberate(pool: PgPool) {
    // Anything not in the spec's DDL must be a documented deviation, so this
    // fails when someone invents a table without recording it.
    let documented = ["report_revisions", "cheatsheets"];

    let mut known: BTreeSet<String> = SYNC_TABLES
        .iter()
        .map(|t| (*t).to_string())
        .chain(
            [
                "plans",
                "users",
                "devices",
                "auth_sessions",
                "push_tokens",
                "change_log",
                "mutation_log",
                "chunks",
                "calendar_connections",
                "subscriptions",
                "payments",
                "webhook_events",
                "credit_ledger",
                "usage_events",
                "discount_codes",
                "user_api_keys",
                "jobs",
                "user_settings",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .chain(documented.iter().map(|t| (*t).to_string()))
        .collect();
    known.insert("_sqlx_migrations".to_string());

    let actual: BTreeSet<String> = sqlx::query_scalar(
        "select table_name from information_schema.tables
         where table_schema = 'public' and table_type = 'BASE TABLE'",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables")
    .into_iter()
    .collect();

    let undocumented: Vec<&String> = actual.difference(&known).collect();
    assert!(
        undocumented.is_empty(),
        "tables exist that are neither in the spec nor a documented deviation: {undocumented:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn insert_appends_one_change_log_row_with_the_device(pool: PgPool) {
    let user = insert_user(&pool).await;
    let device = insert_device(&pool, user).await;

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("select set_config('app.device_id', $1, true)")
        .bind(device.to_string())
        .execute(&mut *tx)
        .await
        .expect("set device");
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'Algorithms')")
        .bind(ws)
        .bind(user)
        .execute(&mut *tx)
        .await
        .expect("insert workspace");
    tx.commit().await.expect("commit");

    let row = sqlx::query(
        "select entity, entity_id, op, origin_device_id from change_log
         where user_id = $1 order by seq desc limit 1",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .expect("change_log row");

    assert_eq!(row.get::<String, _>("entity"), "workspaces");
    assert_eq!(row.get::<Uuid, _>("entity_id"), ws);
    assert_eq!(row.get::<String, _>("op"), "upsert");
    assert_eq!(row.get::<Option<Uuid>, _>("origin_device_id"), Some(device));
}

#[sqlx::test(migrations = "../../migrations")]
async fn soft_delete_logs_a_tombstone_and_touch_advances_updated_at(pool: PgPool) {
    let user = insert_user(&pool).await;
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(ws)
        .bind(user)
        .execute(&pool)
        .await
        .expect("insert workspace");

    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select updated_at from workspaces where id = $1")
            .bind(ws)
            .fetch_one(&pool)
            .await
            .expect("updated_at");

    sqlx::query("update workspaces set deleted_at = now() where id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .expect("soft delete");

    let (op, after): (String, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "select cl.op, w.updated_at from change_log cl
         join workspaces w on w.id = cl.entity_id
         where cl.entity_id = $1 order by cl.seq desc limit 1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .expect("latest change");

    assert_eq!(op, "delete");
    assert!(after > before, "touch_updated_at must advance updated_at");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_update_of_a_soft_deleted_row_stays_a_tombstone(pool: PgPool) {
    let user = insert_user(&pool).await;
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(ws)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update workspaces set deleted_at = now() where id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update workspaces set name = 'renamed' where id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();

    let op: String = sqlx::query_scalar(
        "select op from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(op, "delete", "a tombstone must not flip back to upsert");
}

#[sqlx::test(migrations = "../../migrations")]
async fn restoring_a_tombstoned_row_logs_an_upsert(pool: PgPool) {
    let user = insert_user(&pool).await;
    let ws = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(ws)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update workspaces set deleted_at = now() where id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    // An explicit restore is the one way a tombstone becomes an upsert again.
    sqlx::query("update workspaces set deleted_at = null where id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();

    let op: String = sqlx::query_scalar(
        "select op from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(op, "upsert");
}

#[sqlx::test(migrations = "../../migrations")]
async fn user_settings_update_logs_an_upsert_keyed_by_user(pool: PgPool) {
    let user = insert_user(&pool).await;
    sqlx::query("insert into user_settings (user_id) values ($1)")
        .bind(user)
        .execute(&pool)
        .await
        .expect("insert settings");
    sqlx::query("update user_settings set study_start_hour = 9 where user_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("update settings");

    let (entity, entity_id, op): (String, Uuid, String) = sqlx::query_as(
        "select entity, entity_id, op from change_log
         where user_id = $1 and entity = 'user_settings' order by seq desc limit 1",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .expect("settings change");

    assert_eq!(entity, "user_settings");
    assert_eq!(entity_id, user);
    assert_eq!(op, "upsert");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_concurrent_writer_blocks_so_seq_order_matches_commit_order(pool: PgPool) {
    // The pull cursor depends on this: seq is assigned by nextval() at insert
    // time but rows become visible at commit time, so a second transaction for
    // the same user MUST be held back until the first commits. If this lock ever
    // disappears, a pull can observe seq 11 while seq 10 is still uncommitted,
    // advance its cursor past 10, and lose that change forever.
    let user = insert_user(&pool).await;

    let mut tx_a = pool.begin().await.unwrap();
    let ws_a = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'a')")
        .bind(ws_a)
        .bind(user)
        .execute(&mut *tx_a)
        .await
        .unwrap();
    let seq_a: i64 = sqlx::query_scalar(
        "select seq from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws_a)
    .fetch_one(&mut *tx_a)
    .await
    .unwrap();

    // A second connection writing for the same user must block on the advisory
    // lock. Bound the wait so the test cannot hang.
    let mut tx_b = pool.begin().await.unwrap();
    sqlx::query("set local lock_timeout = '250ms'")
        .execute(&mut *tx_b)
        .await
        .unwrap();
    let blocked = sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'b')")
        .bind(Uuid::now_v7())
        .bind(user)
        .execute(&mut *tx_b)
        .await;

    assert!(
        blocked.is_err(),
        "a concurrent write for the same user must block on the per-user advisory lock"
    );
    tx_b.rollback().await.unwrap();

    tx_a.commit().await.unwrap();

    // With A committed, B proceeds and is ordered strictly after A.
    let mut tx_b = pool.begin().await.unwrap();
    let ws_b = Uuid::now_v7();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'b')")
        .bind(ws_b)
        .bind(user)
        .execute(&mut *tx_b)
        .await
        .unwrap();
    let seq_b: i64 = sqlx::query_scalar(
        "select seq from change_log where entity_id = $1 order by seq desc limit 1",
    )
    .bind(ws_b)
    .fetch_one(&mut *tx_b)
    .await
    .unwrap();
    tx_b.commit().await.unwrap();

    assert!(seq_b > seq_a, "seq {seq_b} must be ordered after {seq_a}");

    // A reader that has seen seq_b can never be missing seq_a.
    let seen: Vec<i64> = sqlx::query_scalar(
        "select seq from change_log where user_id = $1 and seq <= $2 order by seq",
    )
    .bind(user)
    .bind(seq_b)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        seen.contains(&seq_a),
        "seq_a must be visible once seq_b is: {seen:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn change_log_is_scoped_to_one_user(pool: PgPool) {
    let (a, b) = (insert_user(&pool).await, insert_user(&pool).await);
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'a')")
        .bind(Uuid::now_v7())
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'b')")
        .bind(Uuid::now_v7())
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();

    let a_seqs: Vec<i64> =
        sqlx::query_scalar("select seq from change_log where user_id = $1 order by seq")
            .bind(a)
            .fetch_all(&pool)
            .await
            .unwrap();
    let b_seqs: Vec<i64> =
        sqlx::query_scalar("select seq from change_log where user_id = $1 order by seq")
            .bind(b)
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(a_seqs.len(), 1, "user A sees only its own change");
    assert_eq!(b_seqs.len(), 1, "user B sees only its own change");
    assert!(a_seqs[0] < b_seqs[0], "seq is globally monotonic");
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_user_cascades_its_change_log(pool: PgPool) {
    let user = insert_user(&pool).await;
    sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
        .bind(Uuid::now_v7())
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("delete from users where id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar("select count(*) from change_log where user_id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_one_active_subscription_per_user_is_enforced(pool: PgPool) {
    // The spec states this as a rule; the schema must make it true.
    let user = insert_user(&pool).await;
    sqlx::query(
        "insert into plans (id,name,price_idr_monthly,price_idr_yearly,entitlements)
                 values ('pro','Pro',10000,100000,'{}') on conflict do nothing",
    )
    .execute(&pool)
    .await
    .unwrap();

    let insert = |status: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "insert into subscriptions
                   (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
                 values ($1, 'pro', $2, 'monthly', now(), now() + interval '30 days')",
            )
            .bind(user)
            .bind(status)
            .execute(&pool)
            .await
        }
    };

    insert("active").await.expect("first active subscription");
    assert!(
        insert("active").await.is_err(),
        "a second active subscription must be rejected"
    );
    // An expired one is fine alongside an active one.
    insert("expired").await.expect("an expired row may coexist");
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_job_dedupe_keys_are_rejected(pool: PgPool) {
    let insert = || async {
        sqlx::query("insert into jobs (kind, payload, dedupe_key) values ('monthly_grant','{}',$1)")
            .bind("monthly_grant:2026-10")
            .execute(&pool)
            .await
    };
    insert().await.expect("first enqueue");
    assert!(
        insert().await.is_err(),
        "a duplicate singleton job must be rejected"
    );
    // Nullable dedupe_key still allows many non-singleton jobs.
    for _ in 0..3 {
        sqlx::query("insert into jobs (kind, payload) values ('ingest_source','{}')")
            .execute(&pool)
            .await
            .expect("non-singleton jobs are not deduped");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn chunks_dedupe_index_is_not_unique_across_tenants(pool: PgPool) {
    // A unique index here would be a cross-tenant denial of service: the first
    // user to upload a shared textbook would break ingestion for everyone else,
    // and the constraint error would leak content existence.
    let user_a = insert_user(&pool).await;
    let user_b = insert_user(&pool).await;

    let source = |user: Uuid| {
        let pool = pool.clone();
        async move {
            let ws = Uuid::now_v7();
            sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, 'w')")
                .bind(ws)
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
            let src = Uuid::now_v7();
            sqlx::query("insert into sources (id, user_id, workspace_id, kind, title) values ($1,$2,$3,'pdf','t')")
                .bind(src)
                .bind(user)
                .bind(ws)
                .execute(&pool)
                .await
                .unwrap();
            src
        }
    };

    let src_a = source(user_a).await;
    let src_b = source(user_b).await;

    // The same content hash and embedder in two different users' sources.
    for src in [src_a, src_b] {
        sqlx::query(
            "insert into chunks (source_id, idx, content, content_hash, embedder)
             values ($1, 0, 'identical text', 'deadbeef', 'e5-base')",
        )
        .bind(src)
        .execute(&pool)
        .await
        .expect("the same content in another user's source must be insertable");
    }

    let count: i64 =
        sqlx::query_scalar("select count(*) from chunks where content_hash = 'deadbeef'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2, "both tenants' chunks must exist");
}
