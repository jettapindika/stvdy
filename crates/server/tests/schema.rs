#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Task 1).
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
    "gateway_events",
    "discount_codes",
    // Phase 13. The once-per-user redemption row (A298): a billing internal like
    // `discount_codes` itself, and no client entity reaches it.
    "discount_redemptions",
    "user_api_keys",
    "report_revisions",
    "calendar_connections",
    "devices",
    "auth_sessions",
    "push_tokens",
    "plans",
    "users",
    // A40: a provider result set is a server-side cost control with no client
    // entity, so it must never reach a device through the change log.
    "search_cache",
    // Phase 8. Compaction's per-user watermark is a server-side cursor fact with no
    // client entity: a device that learned it would learn nothing it could act on
    // (A165/A184).
    "change_log_watermark",
    // Phase 11. A syllabus draft is server-owned: the model produced it and the student
    // confirms it, and only the confirmed items reach `events`, `tasks` and `goals`. No
    // `Entity` variant exists for it, so it must carry no trigger (A251).
    "syllabus_drafts",
    // Phase 11. The planner's proposal is server-owned for the same reason: it is a
    // suggestion the student previews, and only the accepted blocks become `events`. No
    // `Entity` variant exists for it, so it must carry no trigger (A252).
    "plan_proposals",
    // Phase 12. Reminders are server-owned scheduling state (A283): the client's local
    // notifications are computed from *synced* events, tasks and settings, and this table
    // exists so a reminder still fires when no client is running. No `Entity` variant
    // exists for either, and `0017_sync_triggers.sql` must stay the last trigger-attaching
    // migration — a trigger here would move that boundary for a table no client pushes.
    "reminders",
    "reminder_deliveries",
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
async fn the_protocol_entity_list_matches_the_schema_trigger_set(pool: PgPool) {
    // `Entity::ALL` (the wire contract) and the trigger array in
    // 0017_sync_triggers.sql are two hand-maintained lists. If they drift, a
    // synced table exists that no client can name, or the client names a table
    // that is never logged. Either way sync silently loses data.
    let triggered: BTreeSet<String> = sqlx::query_scalar(
        "select distinct c.relname from pg_trigger t
         join pg_class c on c.oid = t.tgrelid
         where not t.tgisinternal and c.relnamespace = 'public'::regnamespace",
    )
    .fetch_all(&pool)
    .await
    .expect("list trigger tables")
    .into_iter()
    .collect();

    let protocol: BTreeSet<String> = stvdy_sync::proto::Entity::ALL
        .iter()
        .map(|e| e.table_name().to_string())
        .collect();

    assert_eq!(
        triggered, protocol,
        "the wire protocol's entity list must equal the set of synced tables"
    );
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

/// The resolution columns the confirm path writes, asserted by name (A273).
///
/// A missing `expires_at` is a lazy-expiry query that never matches, which is silent: the
/// row simply reads `pending` forever and the student's card never goes away. A *nullable*
/// one is the same failure one step later — `expires_at <= now()` against null is null, not
/// true — which is why the nullability is asserted and not just the column's existence.
#[sqlx::test(migrations = "../../migrations")]
async fn pending_actions_has_the_resolution_columns(pool: PgPool) {
    let columns: Vec<(String, String)> = sqlx::query_as(
        "select column_name, is_nullable from information_schema.columns
         where table_schema = 'public' and table_name = 'pending_actions'",
    )
    .fetch_all(&pool)
    .await
    .expect("query columns");

    for required in [
        "expires_at",
        "resolved_at",
        "resolved_by_device_id",
        "result",
        "error",
        "message_id",
    ] {
        assert!(
            columns.iter().any(|(name, _)| name == required),
            "pending_actions is missing {required} (has {:?})",
            columns.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
    }

    let nullable = columns
        .iter()
        .find(|(name, _)| name == "expires_at")
        .map(|(_, nullable)| nullable.as_str())
        .expect("expires_at is present");
    assert_eq!(
        nullable, "NO",
        "a nullable expires_at makes the lazy expiry a three-valued comparison (A274)"
    );

    // The two columns whose value outlives the row they name: a device that is revoked and
    // a message that is deleted must leave the resolution readable rather than take it with
    // them, which is what `on delete set null` buys and `on delete cascade` would not.
    let cascades: Vec<(String, String, String)> = sqlx::query_as(
        "select a.attname, rt.relname, c.confdeltype::text
           from pg_constraint c
           join pg_class t on t.oid = c.conrelid
           join pg_class rt on rt.oid = c.confrelid
           join unnest(c.conkey) as key(attnum) on true
           join pg_attribute a on a.attrelid = t.oid and a.attnum = key.attnum
          where t.relname = 'pending_actions' and c.contype = 'f'",
    )
    .fetch_all(&pool)
    .await
    .expect("query foreign keys");

    // `n` is `ON DELETE SET NULL` in `pg_constraint.confdeltype`; `c` would be cascade. The
    // column is read as **text**, not cast to an integer: `confdeltype` is Postgres' internal
    // one-byte `"char"` type and has no direct cast to `int8`, so `::int8` is a
    // `42846 cannot cast type "char" to bigint` on every version — the test would fail before
    // it reached the assertion it exists for.
    for (column, table) in [
        ("resolved_by_device_id", "devices"),
        ("message_id", "messages"),
    ] {
        let found = cascades
            .iter()
            .find(|(name, _, _)| name == column)
            .unwrap_or_else(|| panic!("{column} has no foreign key"));
        assert_eq!(found.1, table, "{column} must reference {table}");
        assert_eq!(
            found.2, "n",
            "{column} must be `on delete set null`, not cascade: the resolution has to \
             outlive the device or the message it names"
        );
    }

    // The index the lazy expiry and the pending list both read. Asserted by definition
    // rather than by name, because a rename is not the failure this catches — a missing or
    // wrongly-ordered index is.
    let indexes: Vec<String> = sqlx::query_scalar(
        "select indexdef from pg_indexes
          where schemaname = 'public' and tablename = 'pending_actions'",
    )
    .fetch_all(&pool)
    .await
    .expect("query indexes");
    assert!(
        indexes
            .iter()
            .any(|def| def.contains("(user_id, status, expires_at)")),
        "no (user_id, status, expires_at) index: {indexes:?}"
    );
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

/// A table with no client entity must never be able to reach one (A165/A184): no
/// triggers, and a primary key that is the user id rather than a row id.
#[sqlx::test(migrations = "../../migrations")]
async fn the_watermark_is_server_side_only(pool: PgPool) {
    let triggers: i64 = sqlx::query_scalar(
        "select count(*) from pg_trigger t join pg_class c on c.oid = t.tgrelid
         where not t.tgisinternal and c.relname = 'change_log_watermark'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(triggers, 0);

    let cols: Vec<String> = sqlx::query_scalar(
        "select column_name from information_schema.columns
         where table_name = 'change_log_watermark'",
    )
    .fetch_all(&pool)
    .await
    .expect("columns");
    assert_eq!(
        cols.iter().cloned().collect::<BTreeSet<_>>(),
        ["user_id", "min_retained_seq", "compacted_at"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
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
    //
    // Phase 1 added two, and neither is a `+sync` table on purpose:
    // `email_verification_tokens` holds a single-use credential, and
    // `login_tickets` holds the one the Google callback hands the app, so a
    // client must never see or replay either through the sync log.
    let documented = [
        "report_revisions",
        "cheatsheets",
        "email_verification_tokens",
        "login_tickets",
        // Phase 3. A server-side cost control with no client-visible entity, so
        // it is deliberately not a `+sync` table (A40).
        "search_cache",
        // Phase 7. A43's signup limiter is database-backed on purpose: an in-process
        // bucket resets on restart, which is exactly what a signup farm waits for. It
        // holds IPs and email domains, so it is not a `+sync` table (A154).
        "signup_attempts",
        // Phase 8. Compaction's per-user watermark: the seq below which this account's
        // history is gone, which is what a cursor is compared against for 410 (A165).
        // Not a `+sync` table: it is a server-side cursor fact with no client entity, and
        // a device that learned it would learn nothing it could act on.
        "change_log_watermark",
        // Phase 11. The syllabus extraction draft: the model produced it, the student
        // confirms it, and only the confirmed items become events, tasks and goals. Not a
        // `+sync` table (A251) — there is no `Entity` variant for a draft, and the unique
        // index on `source_id` is what makes a retried extraction overwrite rather than
        // append.
        "syllabus_drafts",
        // Phase 11. The planner's proposal: the roadmap's conflict table names "planner
        // output" among the server-owned entities that are "read-only for clients except via
        // their dedicated endpoints", which is what `GET /plans/{id}` is. Not a `+sync`
        // table (A252) — the accepted blocks reach every device as `events`, and the
        // proposal itself never syncs.
        "plan_proposals",
        // Phase 12. The reminder engine's output: one row per (subject, local date) so the
        // same reminder cannot be registered by three devices (A284/A285), and one row per
        // delivery attempt. Not `+sync` tables (A283): a reminder is server-owned scheduling
        // state, the client's own notifications are computed from the synced inputs, and
        // `0017_sync_triggers.sql` stays the last trigger-attaching migration.
        "reminders",
        "reminder_deliveries",
        // Phase 13. A discount code is once per user, and this row is what makes it so: the
        // unique key is `(code, user_id)`, so a second attempt by one account is refused by
        // the database rather than by a read-then-write in the route. It is also the answer
        // to "why was this code counted" — the row names the payment that redeemed it, and
        // `on delete cascade` on that payment is right here (unlike `gateway_events`, which
        // is evidence and takes `set null`). Not a `+sync` table (A298): it is a billing
        // internal like `discount_codes` itself, and `0017_sync_triggers.sql` stays the last
        // trigger-attaching migration.
        "discount_redemptions",
    ];

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
                "gateway_events",
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

/// Inserts one reminder and returns its id. The columns are the migration's own, so a
/// rename here is a compile-or-fail rather than a silent drift.
async fn insert_reminder(pool: &PgPool, user: Uuid, dedupe_key: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into reminders (id, user_id, kind, subject_key, local_date, fire_at, dedupe_key)
         values ($1, $2, 'task_due', 'subject', '2026-10-11', now(), $3)",
    )
    .bind(id)
    .bind(user)
    .bind(dedupe_key)
    .execute(pool)
    .await
    .expect("insert reminder");
    id
}

/// The two uniqueness rules the delivery dedupe rests on (A285), asserted as constraints
/// rather than as prose: one reminder per key, and one delivery per (reminder, channel,
/// device) — with the two-index split that makes an email delivery unique too.
#[sqlx::test(migrations = "../../migrations")]
async fn the_reminder_dedupe_constraints_exist(pool: PgPool) {
    let user = insert_user(&pool).await;
    let device = insert_device(&pool, user).await;
    let reminder = insert_reminder(&pool, user, "task_due:subject:2026-10-11").await;

    // The second device with the same key loses, and it loses *quietly*: `POST /reminders`
    // runs exactly this statement, so a conflict that raised would be a 500 for the second
    // phone rather than a `{applied: false}`.
    let duplicate: Option<Uuid> = sqlx::query_scalar(
        "insert into reminders (user_id, kind, subject_key, local_date, fire_at, dedupe_key)
         values ($1, 'task_due', 'subject', '2026-10-11', now(), $2)
         on conflict (dedupe_key) do nothing
         returning id",
    )
    .bind(user)
    .bind("task_due:subject:2026-10-11")
    .fetch_optional(&pool)
    .await
    .expect("a duplicate key must be a no-op rather than an error");
    assert!(duplicate.is_none(), "the second writer must lose");

    // A second key is a second reminder: the index is on the key, not on the user.
    let other = insert_reminder(&pool, user, "task_due:subject:2026-10-12").await;
    assert_ne!(reminder, other);

    let delivery = |channel: &'static str, device: Option<Uuid>| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "insert into reminder_deliveries (reminder_id, channel, device_id, status)
                 values ($1, $2, $3, 'sent')",
            )
            .bind(reminder)
            .bind(channel)
            .bind(device)
            .execute(&pool)
            .await
        }
    };

    // One delivery per channel per device: the second attempt for the same push is refused,
    // which is what makes a retried job record one attempt rather than two.
    delivery("push", Some(device)).await.expect("first push");
    assert!(
        delivery("push", Some(device)).await.is_err(),
        "a second push to the same device must be refused"
    );

    // The same channel with no device — email, in-app — is unique through the second partial
    // index rather than through a coalesced sentinel uuid.
    delivery("email", None).await.expect("first email");
    assert!(
        delivery("email", None).await.is_err(),
        "a second email delivery must be refused"
    );

    // A different channel on the same device is a different delivery, and so is the same
    // channel on another device.
    delivery("web", Some(device)).await.expect("web");
    let second_device = insert_device(&pool, user).await;
    delivery("push", Some(second_device))
        .await
        .expect("another device is another delivery");
}

/// The paired check makes a half-specified reminder impossible rather than relying on every
/// reader to notice, and `kind` is deliberately free text so a producer that is not the core
/// engine (A300's renewal reminders) does not need a migration.
#[sqlx::test(migrations = "../../migrations")]
async fn a_skipped_reminder_must_carry_its_reason(pool: PgPool) {
    let user = insert_user(&pool).await;

    let insert = |status: &'static str, reason: Option<&'static str>, kind: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "insert into reminders (user_id, kind, subject_key, local_date, fire_at,
                                        dedupe_key, status, skip_reason)
                 values ($1, $2, 'subject', '2026-10-11', now(), $3, $4, $5)",
            )
            .bind(user)
            .bind(kind)
            .bind(format!("{kind}:{status}:{reason:?}"))
            .bind(status)
            .bind(reason)
            .execute(&pool)
            .await
        }
    };

    // A skipped reminder must say why: a reminder that silently never arrives is what the
    // student reports as "reminders are broken", and the reason is the only thing that
    // distinguishes "quiet hours moved it" from "the token is dead".
    assert!(
        insert("skipped", None, "task_due").await.is_err(),
        "skipped without a reason must be refused"
    );
    // And a reason without a skip is a row claiming a skip that did not happen.
    assert!(
        insert("pending", Some("quiet_hours"), "task_due")
            .await
            .is_err(),
        "a pending reminder must not carry a skip reason"
    );
    insert("skipped", Some("quiet_hours"), "task_due")
        .await
        .expect("a skipped reminder with its reason");
    insert("pending", None, "task_due").await.expect("pending");
    insert("sent", None, "task_due").await.expect("sent");

    // The vocabulary is closed for the two columns the engine owns.
    assert!(
        insert("delivered", None, "task_due").await.is_err(),
        "an unknown status must be refused"
    );
    let unknown_origin = sqlx::query(
        "insert into reminders (user_id, kind, subject_key, local_date, fire_at, dedupe_key, origin)
         values ($1, 'task_due', 'subject', '2026-10-11', now(), 'origin-check', 'robot')",
    )
    .bind(user)
    .execute(&pool)
    .await;
    assert!(unknown_origin.is_err(), "an unknown origin must be refused");

    // `kind` is not closed: A300 routes Phase 13's renewal reminders through this table, and
    // a check constraint here would make a new producer a migration.
    insert("pending", None, "renewal")
        .await
        .expect("a producer's own kind");
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

#[sqlx::test(migrations = "../../migrations")]
async fn users_supports_oidc_only_accounts(pool: PgPool) {
    // A48: a Google sign-in creates an account with no password. If
    // `password_hash` were still `not null`, OIDC signup would fail at the
    // insert, and `google_sub` is the stable link key (an email can be
    // reassigned, a sub cannot).
    let columns: Vec<(String, String)> = sqlx::query_as(
        "select column_name, is_nullable from information_schema.columns
         where table_schema = 'public' and table_name = 'users'",
    )
    .fetch_all(&pool)
    .await
    .expect("query columns");

    let nullable = |name: &str| {
        columns
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, n)| n == "YES")
    };

    assert_eq!(
        nullable("password_hash"),
        Some(true),
        "OIDC users have no password"
    );
    assert_eq!(nullable("google_sub"), Some(true));
    assert_eq!(nullable("email_verified_at"), Some(true));
    // The entitlement boolean stays non-null: A43 gates AI actions on it.
    assert_eq!(nullable("email_verified"), Some(false));

    // A passwordless account is insertable, and a duplicate google_sub is not.
    sqlx::query("insert into users (email, google_sub) values ('oidc@example.test', 'sub-1')")
        .execute(&pool)
        .await
        .expect("an OIDC-only user must be insertable");

    let duplicate =
        sqlx::query("insert into users (email, google_sub) values ('other@example.test', 'sub-1')")
            .execute(&pool)
            .await;
    assert!(duplicate.is_err(), "google_sub must be unique");
}

/// The columns the calendar poll, the conditional GET and the two-way sync need, asserted
/// by name so a migration that silently omits one fails here rather than at first use.
///
/// The two defaults are asserted because they are load-bearing rather than cosmetic:
/// `status = 'connected'` is what makes a connection the screen can render before its first
/// poll, and `last_sync_was_full = true` is what makes an incremental sync that has never run
/// read as "no cursor yet" instead of as an incremental pass that found nothing (A236).
#[sqlx::test(migrations = "../../migrations")]
async fn calendar_connections_carries_the_poll_and_sync_state(pool: PgPool) {
    let columns: Vec<(String, Option<String>)> = sqlx::query_as(
        "select column_name, column_default from information_schema.columns
         where table_schema = 'public' and table_name = 'calendar_connections'",
    )
    .fetch_all(&pool)
    .await
    .expect("query columns");

    let default_of = |name: &str| {
        columns
            .iter()
            .find(|(c, _)| c == name)
            .and_then(|(_, d)| d.clone())
    };

    for column in [
        "created_at",
        "updated_at",
        "status",
        "error",
        "ics_etag",
        "ics_last_modified",
        "study_calendar_id",
        "last_sync_was_full",
        "watch_channel_id",
        "watch_expires_at",
    ] {
        assert!(
            columns.iter().any(|(c, _)| c == column),
            "calendar_connections is missing {column} (has {:?})",
            columns.iter().map(|(c, _)| c).collect::<Vec<_>>()
        );
    }

    assert!(
        default_of("status").is_some_and(|d| d.contains("connected")),
        "status must default to 'connected': {:?}",
        default_of("status")
    );
    assert!(
        default_of("last_sync_was_full").is_some_and(|d| d.contains("true")),
        "last_sync_was_full must default to true: {:?}",
        default_of("last_sync_was_full")
    );
}

/// The poll's query is "connections whose watermark is older than the interval", so the index
/// has to carry `last_synced_at`. A missing index is not a wrong answer, it is a full table
/// scan every fifteen minutes on a table that grows with every connected account.
#[sqlx::test(migrations = "../../migrations")]
async fn the_calendar_poll_index_carries_the_watermark(pool: PgPool) {
    let definitions: Vec<String> = sqlx::query_scalar(
        "select indexdef from pg_indexes
         where schemaname = 'public' and tablename = 'calendar_connections'",
    )
    .fetch_all(&pool)
    .await
    .expect("query indexes");

    assert!(
        definitions.iter().any(|d| {
            let d = d.to_ascii_lowercase();
            d.contains("(provider, last_synced_at)")
        }),
        "no index on (provider, last_synced_at): {definitions:?}"
    );
}

/// `0017_sync_triggers.sql` promises, in its own header, to be the last migration that
/// attaches a trigger. Phase 10 adds no `+sync` table, so the promise has to still hold: a
/// later migration that created a trigger would mean the trigger set is no longer readable
/// in one file, which is what `every_sync_table_has_both_triggers` audits against.
///
/// Reads the migration files rather than the database: the property is about the *files*, and
/// a scratch database cannot tell which migration wrote a trigger.
#[test]
fn no_migration_after_0017_attaches_a_trigger() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let mut attaching: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&dir).expect("a readable migrations directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|extension| extension != "sql") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a readable migration");
        let lowered = source.to_ascii_lowercase();
        if lowered.contains("create trigger") || lowered.contains("create constraint trigger") {
            attaching.push(
                path.file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }

    attaching.sort();
    assert_eq!(
        attaching.last().map(String::as_str),
        Some("0017_sync_triggers.sql"),
        "0017_sync_triggers.sql must stay the last migration that attaches a trigger, \
         got {attaching:?}"
    );
}
