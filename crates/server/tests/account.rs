//! Account deletion and the data export (A355).
//!
//! Four properties this file exists to prove, each of which a plausible implementation gets
//! wrong in a way nothing else catches:
//!
//!  * **Deletion is effective at the request, not at the purge.** A signed-in device that keeps
//!    working for the length of the grace window would make the confirmation a lie, so the
//!    session is asserted refused *before* the purge has run, from the token the request was
//!    made with.
//!  * **The purge is per table group, not a row count.** The financial rows survive and the
//!    content rows do not; asserting "1,204 rows deleted" would pass for a purge that deleted
//!    the payment ledger and kept the messages.
//!  * **The export is scoped to the caller.** A missing `user_id` predicate in an export is the
//!    worst possible version of that bug, so the cross-tenant case is not optional.
//!  * **A second pass is a no-op.** The job is enqueued by the request and by the scheduler's
//!    backstop, so a double run is a normal occurrence rather than an edge case.
//!  * **The identity is released at the request, not at the purge.** A marked account cannot be
//!    found by anything a student can type — its address, password and Google subject are gone
//!    with the request — so deleting and registering again with the same address works
//!    immediately, and the tombstone the purge eventually rewrites is already anonymous.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Test code may unwrap/expect/panic; `#[sqlx::test]` is a macro harness, so the
// relaxations in clippy.toml are not guaranteed to apply (see Phase 0 Task 1).
use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::storage::Storage;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{document, json, signed_up};

/// Signs up and returns `(access_token, refresh_token)`.
///
/// The refresh token comes from a login rather than from signup because `signed_up` returns
/// only the access token, and the refresh is what proves a session family is dead: an access
/// token expires on its own, a refresh token does not.
async fn signed_up_with_refresh(
    app: &axum::Router,
    harness: &TestState,
    email: &str,
) -> (String, String) {
    let access = signed_up(app, harness, email).await;
    let (status, body) = json(
        app,
        "POST",
        "/auth/login",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        access,
        body["refresh_token"]
            .as_str()
            .expect("a refresh token")
            .to_string(),
    )
}

/// The account's `deleted_at` and `purged_at`, as the database holds them.
async fn deletion_state(pool: &PgPool, user_id: Uuid) -> (bool, bool) {
    let (deleted, purged): (bool, bool) = sqlx::query_as(
        "select deleted_at is not null, purged_at is not null from users where id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("the account row");
    (deleted, purged)
}

async fn user_id_for(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .expect("the account")
}

/// One workspace owned by `token`, returning its id.
async fn workspace(app: &axum::Router, token: &str, name: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["workspace"]["id"].as_str().expect("an id").to_string()
}

/// A deletion request marks the account, kills every credential, cancels the jobs, releases the
/// identity, and leaves the address free for a new registration.
///
/// The session assertion is the one that matters and it is deliberately made **before** the
/// purge: `GET /auth/me` with the token the deletion was made with must already be refused.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deletion_request_marks_revokes_cancels_and_frees_the_address(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let email = "delete-me@example.com";
    let (access, refresh) = signed_up_with_refresh(&app, &harness, email).await;
    let user_id = user_id_for(&harness.state.pool, email).await;

    // Something to lose, and a job that must be cancelled. The job is written directly because
    // the point is the *cancellation*, not the producer: a `generate_flashcards` for this
    // account queued a moment before the deletion is exactly the row the request must kill.
    let _ = workspace(&app, &access, "Biologi").await;
    sqlx::query("insert into jobs (kind, payload) values ('generate_flashcards', $1)")
        .bind(serde_json::json!({ "user_id": user_id }))
        .execute(&harness.state.pool)
        .await
        .expect("a queued job");

    // A live feed token: it is a capability URL with no session behind it, so nothing else in
    // this request would revoke it (A242).
    sqlx::query("update users set ics_feed_token = 'live-token' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .expect("a feed token");

    let (status, body) = json(&app, "DELETE", "/account", &access, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["already_deleted"], serde_json::json!(false));
    assert!(
        body["purges_at"]
            .as_str()
            .is_some_and(|at| at > body["deleted_at"].as_str().unwrap()),
        "the purge must be scheduled after the request: {body}"
    );
    assert_eq!(
        body["sessions_revoked"].as_i64(),
        // Two families: the signup's and the login's.
        Some(2),
        "{body}"
    );
    assert_eq!(body["jobs_cancelled"].as_i64(), Some(1), "{body}");

    let (deleted, purged) = deletion_state(&harness.state.pool, user_id).await;
    assert!(deleted, "the account is marked");
    assert!(!purged, "and not yet purged: the window has not closed");

    // **The session is refused immediately, before the purge.** This is the assertion the
    // window would otherwise make false.
    let (status, _) = json(&app, "GET", "/auth/me", &access, serde_json::Value::Null).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the access token must be dead"
    );

    let (status, _) = json(
        &app,
        "POST",
        "/auth/refresh",
        "",
        serde_json::json!({ "refresh_token": refresh }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the refresh token must be dead"
    );

    // The capability URL, which no session revocation reaches.
    let feed_token: Option<String> =
        sqlx::query_scalar("select ics_feed_token from users where id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the feed token");
    assert_eq!(feed_token, None, "the feed token must be revoked");

    // The job is cancelled rather than left to run against an account that is going away.
    let job_status: String =
        sqlx::query_scalar("select status from jobs where payload->>'user_id' = $1::text")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the job row");
    assert_eq!(job_status, "failed");

    // **And the identity is gone with the request, not with the purge (A362).** The address the
    // account was registered with is no longer on the row, so the next sign-in finds nothing:
    // the same 401 an address that never had an account gets. This is the assertion that makes
    // "login is refused" true in the strongest sense — there is no account to refuse.
    let (status, body) = json(
        &app,
        "POST",
        "/auth/login",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the address must no longer resolve to the marked account: {body}"
    );

    // The row is still there and still marked — it is the purge that removes the data, and the
    // student's records are what the window is for. Only the identity moved.
    let (deleted, purged) = deletion_state(&harness.state.pool, user_id).await;
    assert!(deleted, "the row survives its identity");
    assert!(!purged, "and the purge has not run");

    // Registering again with the same address works, immediately. This is the half the
    // `account_deleted` refusal used to stand in the way of: a student who deleted an account
    // and changed their mind had to wait out the window to use their own address, for a row
    // they could not see and had already asked us to erase.
    let (status, body) = json(
        &app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": email,
            "password": "a different passphrase",
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let registered_again = body["user"]["id"].as_str().expect("an id").to_string();
    assert_ne!(
        registered_again,
        user_id.to_string(),
        "the new account is a new row, not the tombstone"
    );
    assert_eq!(
        body["user"]["email"], email,
        "and it holds the address the student typed"
    );

    // The tombstone keeps the address the purge's anonymization derives from the row's own id,
    // which is what makes the release collision-free: a second account cannot be handed a
    // `deleted-…@deleted.invalid` address by anything but its own id.
    let tombstone: String = sqlx::query_scalar("select email from users where id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("the tombstone");
    assert_eq!(tombstone, format!("deleted-{user_id}@deleted.invalid"));

    // The two rows are distinct, and the new one is the live account for this address.
    let live: i64 = sqlx::query_scalar("select count(*) from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .expect("counting the accounts at this address");
    assert_eq!(live, 1, "exactly one row holds the address, and it is live");
}

/// A deletion request schedules exactly one purge, at the end of the window, and the scheduler's
/// backstop does not add a second or run one early.
#[sqlx::test(migrations = "../../migrations")]
async fn the_purge_waits_for_the_window(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let access = signed_up(&app, &harness, "window@example.com").await;
    let user_id = user_id_for(&harness.state.pool, "window@example.com").await;

    let (status, _) = json(&app, "DELETE", "/account", &access, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);

    // One job, keyed by the account, scheduled in the future.
    let (kind, key, future): (String, String, bool) =
        sqlx::query_as("select kind, dedupe_key, run_at > now() from jobs where dedupe_key = $1")
            .bind(stvdy_server::account::purge_dedupe_key(user_id))
            .fetch_one(&harness.state.pool)
            .await
            .expect("the scheduled purge");
    assert_eq!(kind, "purge_deleted_accounts");
    // The key is the account's, which is what makes the backstop's enqueue a no-op rather than
    // a second job.
    assert_eq!(key, stvdy_server::account::purge_dedupe_key(user_id));
    assert!(future, "the purge must not be due before the window closes");

    // The backstop pass leaves it alone: the account is inside its window, so it is not even
    // examined, and the job is already queued.
    let enqueued = stvdy_server::jobs::schedule::purge_pass(&harness.state)
        .await
        .expect("a purge pass");
    assert_eq!(
        enqueued, 0,
        "a marked account inside its window is not re-enqueued"
    );

    // And it is still not purged.
    let (_, purged) = deletion_state(&harness.state.pool, user_id).await;
    assert!(!purged);
}

/// The purge deletes the student's content and anonymizes the account, asserted per table group.
///
/// The groups are the point: a row count would pass for a purge that deleted the payment ledger
/// and kept the messages, and both halves of that are exactly wrong.
#[sqlx::test(migrations = "../../migrations")]
async fn the_purge_deletes_content_and_anonymizes_the_account(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let email = "purge-me@example.com";
    let access = signed_up(&app, &harness, email).await;
    let user_id = user_id_for(&harness.state.pool, email).await;

    // Content: a workspace, a source, a message, a task and a goal.
    let workspace_id = workspace(&app, &access, "Fisika").await;
    let (content_type, body) = common::multipart(&[
        ("kind", "text", None),
        ("title", "Catatan", None),
        ("text", "Hukum Newton menjelaskan gerak benda.", None),
    ]);
    let (status, created) = common::post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        &access,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    common::drain_jobs(&harness).await;

    let (status, _) = json(
        &app,
        "POST",
        "/tasks",
        &access,
        serde_json::json!({ "title": "Latihan soal", "workspace_id": workspace_id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // A chunk is the extracted text: it has no `user_id` and must go with its source.
    let chunks: i64 = sqlx::query_scalar(
        "select count(*) from chunks c join sources s on s.id = c.source_id where s.user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .expect("chunks");
    assert!(
        chunks > 0,
        "the source must have been ingested for this test to mean anything"
    );

    // Financial rows: a paid payment and its ledger entries. Written directly, because the
    // point is that the purge keeps them.
    let payment_id = Uuid::now_v7();
    sqlx::query(
        "insert into payments (id, user_id, kind, order_id, amount_idr, expires_at, status, paid_at)
         values ($1, $2, 'subscription', 'order-1', 49000, now() + interval '1 day', 'paid', now())",
    )
    .bind(payment_id)
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .expect("a payment");
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason) values ($1, 100, 'monthly', 'monthly_grant')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .expect("a ledger row");
    sqlx::query(
        "insert into subscriptions (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .expect("a subscription");

    // A rendered document, whose bytes live in object storage. Written directly because
    // generating one needs a model; what this test is about is the *purge*, not the producer.
    let cheatsheet_key = format!("cheatsheets/{user_id}/document.html");
    harness
        .storage
        .put(
            &cheatsheet_key,
            axum::body::Bytes::from_static(b"<html>rendered</html>"),
            "text/html",
        )
        .await
        .expect("a stored object");
    sqlx::query(
        "insert into cheatsheets (user_id, workspace_id, title, scope, depth, status, html_key)
         values ($1, $2, 'Ringkasan', '{}'::jsonb, 'standard', 'ready', $3)",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(&workspace_id).expect("the workspace id"))
    .bind(&cheatsheet_key)
    .execute(&harness.state.pool)
    .await
    .expect("a cheatsheet");

    // The two keys the purge must reach in storage. Read before the deletes, because after them
    // there is nothing left to say which bytes belonged to this account.
    let source_file_key: String =
        sqlx::query_scalar("select file_key from sources where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the source's object key");

    // Mark and purge in one step: the window is the job's `run_at`, and this test is about the
    // handler, so it drives `purge` directly rather than waiting seven days.
    let ledger_before: i64 =
        sqlx::query_scalar("select count(*) from credit_ledger where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the ledger before the purge");

    stvdy_server::account::mark_deleted(&harness.state, user_id)
        .await
        .expect("a deletion");
    let purge = stvdy_server::jobs::handlers::purge_accounts::purge(&harness.state, user_id)
        .await
        .expect("a purge");
    assert!(
        purge.objects_deleted >= 2,
        "the purge must have reached both stored objects, got {}",
        purge.objects_deleted
    );

    let count = |table: &'static str| {
        let pool = harness.state.pool.clone();
        async move {
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "select count(*) from {table} where user_id = $1"
            )))
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("a count")
        }
    };

    // Gone: the student's content and account state.
    for table in [
        "workspaces",
        "sources",
        "messages",
        "tasks",
        "goals",
        "decks",
        "flashcards",
        "quizzes",
        "reports",
        "cheatsheets",
        "events",
        "study_sessions",
        "reminders",
        "devices",
        "auth_sessions",
        "user_settings",
        "calendar_connections",
        "change_log_watermark",
    ] {
        let remaining: i64 = count(table).await;
        assert_eq!(remaining, 0, "{table} must be empty after the purge");
    }

    // Gone by cascade: the two tables that hang off a deleted parent and have no `user_id`.
    let chunks_after: i64 = sqlx::query_scalar(
        "select count(*) from chunks where source_id not in (select id from sources)",
    )
    .fetch_one(&harness.state.pool)
    .await
    .expect("orphan chunks");
    assert_eq!(chunks_after, 0, "chunks cascade from the deleted sources");

    // Gone from **object storage** as well as from the database. The rows name bytes in the
    // bucket, and a purge that deleted only the rows would leave every uploaded PDF and every
    // rendered document behind — which `docs/OPERATIONS.md` § Retention already promises account
    // deletion removes. `TestState` uses `InMemoryStorage`, so this is the real backend's
    // contract rather than a mock echo.
    assert!(
        harness.storage.get(&source_file_key).await.is_err(),
        "the uploaded original must be removed from storage"
    );
    assert!(
        harness.storage.get(&cheatsheet_key).await.is_err(),
        "the rendered document must be removed from storage"
    );

    // Kept: the financial records, still attached to the account row. The ledger's exact count
    // is the account's own history — the signup grant is a row — so it is read before the purge
    // and compared, rather than asserted against a number that would change with the grant
    // policy.
    let payments: i64 = count("payments").await;
    assert_eq!(
        payments, 1,
        "a paid payment is a financial record and must survive"
    );
    let ledger: i64 = count("credit_ledger").await;
    assert_eq!(
        ledger, ledger_before,
        "every ledger row is the balance's history and must survive the purge"
    );
    assert!(ledger > 0, "the account had a ledger to keep");

    // Kept: the subscription, cancelled rather than deleted, so no live period is read as
    // current revenue for an account that no longer exists.
    let subscription_status: String =
        sqlx::query_scalar("select status from subscriptions where user_id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the subscription");
    assert_eq!(subscription_status, "canceled");

    // Anonymized: the account row is a tombstone. It exists (the ledger needs it), and it no
    // longer identifies anyone.
    let (account_email, password_hash, google_sub, feed_token, plan, admin, purged): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        bool,
        bool,
    ) = sqlx::query_as(
        "select email, password_hash, google_sub, ics_feed_token, plan_id, is_admin,
                purged_at is not null
           from users where id = $1",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .expect("the tombstone");
    assert!(
        account_email.ends_with("@deleted.invalid"),
        "the address must be anonymized, got {account_email}"
    );
    assert_eq!(password_hash, None);
    assert_eq!(google_sub, None);
    assert_eq!(feed_token, None);
    assert_eq!(plan, "free");
    assert!(!admin);
    assert!(purged, "purged_at records that the data is gone");
}

/// A second purge pass over an already-purged account is a no-op.
///
/// Not a hypothetical: the request schedules the job and the scheduler's backstop enqueues for
/// any marked account whose job is missing, so two passes over one account is a normal race.
#[sqlx::test(migrations = "../../migrations")]
async fn a_second_purge_pass_is_a_no_op(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let access = signed_up(&app, &harness, "twice@example.com").await;
    let user_id = user_id_for(&harness.state.pool, "twice@example.com").await;
    let _ = workspace(&app, &access, "Kimia").await;

    stvdy_server::account::mark_deleted(&harness.state, user_id)
        .await
        .expect("a deletion");
    let first = stvdy_server::jobs::handlers::purge_accounts::purge(&harness.state, user_id)
        .await
        .expect("the first purge");
    assert!(
        first.rows_deleted > 0,
        "the first pass must have deleted something"
    );
    assert!(!first.already_purged);

    let purged_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select purged_at from users where id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the purge date");

    let second = stvdy_server::jobs::handlers::purge_accounts::purge(&harness.state, user_id)
        .await
        .expect("the second purge");
    assert_eq!(second.rows_deleted, 0, "nothing is left to delete");
    assert!(
        second.already_purged,
        "the pass must report that it had already run"
    );

    let purged_again: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select purged_at from users where id = $1")
            .bind(user_id)
            .fetch_one(&harness.state.pool)
            .await
            .expect("the purge date");
    assert_eq!(
        purged_at, purged_again,
        "a second pass must not move the date it recorded"
    );

    // And the backstop does not enqueue a third: the account is purged, so it is not selected.
    let enqueued = stvdy_server::jobs::schedule::purge_pass(&harness.state)
        .await
        .expect("a purge pass");
    assert_eq!(
        enqueued, 0,
        "a purged account must not be re-enqueued forever"
    );
}

/// The scheduler's backstop purges a marked account whose own job was lost.
#[sqlx::test(migrations = "../../migrations")]
async fn the_backstop_pass_purges_a_marked_account(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let access = signed_up(&app, &harness, "backstop@example.com").await;
    let user_id = user_id_for(&harness.state.pool, "backstop@example.com").await;
    let _ = workspace(&app, &access, "Sejarah").await;

    let (status, _) = json(&app, "DELETE", "/account", &access, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);

    // The job the request scheduled is gone — the case the backstop exists for. The account's
    // window is moved into the past rather than waiting for it, because the window is the
    // job's `run_at` and the pass's cutoff is derived from the same two facts.
    sqlx::query("delete from jobs where dedupe_key = $1")
        .bind(stvdy_server::account::purge_dedupe_key(user_id))
        .execute(&harness.state.pool)
        .await
        .expect("dropping the job");
    sqlx::query("update users set deleted_at = now() - interval '30 days' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .expect("aging the window");

    let enqueued = stvdy_server::jobs::schedule::purge_pass(&harness.state)
        .await
        .expect("a purge pass");
    assert_eq!(enqueued, 1, "the backstop must enqueue the missing purge");

    common::drain_jobs(&harness).await;

    let (deleted, purged) = deletion_state(&harness.state.pool, user_id).await;
    assert!(deleted);
    assert!(purged, "the backstop's job must have run");

    let workspaces: i64 = sqlx::query_scalar("select count(*) from workspaces where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("workspaces");
    assert_eq!(workspaces, 0);
}

/// The export carries the caller's own data and **not another account's**.
///
/// The mandatory case: a missing `user_id` predicate in an export is the worst possible version
/// of that bug, because it hands one student another student's notes in a file they keep.
#[sqlx::test(migrations = "../../migrations")]
async fn the_export_is_scoped_to_the_caller(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let mine = signed_up(&app, &harness, "mine@example.com").await;
    let theirs = signed_up(&app, &harness, "theirs@example.com").await;

    let my_workspace = workspace(&app, &mine, "Matematika Saya").await;
    let their_workspace = workspace(&app, &theirs, "Fisika Orang Lain").await;
    let my_user_id = user_id_for(&harness.state.pool, "mine@example.com").await;
    let their_user_id = user_id_for(&harness.state.pool, "theirs@example.com").await;

    let (status, content_type, bytes) = document(&app, "/account/export", &mine).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("application/json"),
        "{content_type}"
    );
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON document");

    assert_eq!(body["account"]["id"], serde_json::json!(my_user_id));
    let workspaces = body["workspaces"].as_array().expect("a workspaces array");
    let names: Vec<&str> = workspaces
        .iter()
        .filter_map(|row| row["name"].as_str())
        .collect();
    assert!(names.contains(&"Matematika Saya"), "{names:?}");
    assert!(
        !names.contains(&"Fisika Orang Lain"),
        "the export must not carry another account's workspace: {names:?}"
    );

    // Asserted on the raw bytes as well as the parsed document: a predicate that leaked a row
    // would leak its id, and the id is a string the parse cannot normalise away.
    let raw = String::from_utf8_lossy(&bytes);
    assert!(
        !raw.contains(&their_workspace),
        "another account's workspace id leaked"
    );
    assert!(
        !raw.contains(&their_user_id.to_string()),
        "another account's id leaked"
    );
    assert!(raw.contains(&my_workspace));

    // And the credentials of the caller's *own* account are not in it: a data export that hands
    // back a password hash is a leak with a download button.
    assert!(
        !raw.contains("password_hash"),
        "the export must omit credentials"
    );
    assert!(!raw.contains("google_sub"));
    assert!(!raw.contains("ics_feed_token"));
}

/// The three formats the spec names, each with the content type a client needs to keep it.
#[sqlx::test(migrations = "../../migrations")]
async fn the_export_offers_json_markdown_and_ics(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let token = signed_up(&app, &harness, "formats@example.com").await;
    let _ = workspace(&app, &token, "Biologi").await;

    // A task with a due date, so the ICS document has a `VEVENT` to carry.
    let (status, _) = json(
        &app,
        "POST",
        "/tasks",
        &token,
        serde_json::json!({
            "title": "Kumpulkan laporan",
            "due_at": "2026-11-01T09:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, content_type, bytes) = document(&app, "/account/export", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("application/json"),
        "{content_type}"
    );
    let json_body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert!(json_body["row_limit"]["per_collection"].is_i64());

    let (status, content_type, bytes) = document(&app, "/account/export?format=md", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/markdown"), "{content_type}");
    let markdown = String::from_utf8_lossy(&bytes);
    assert!(markdown.starts_with("# Your Stvdy data"), "{markdown}");
    assert!(markdown.contains("## Workspaces"), "{markdown}");
    assert!(markdown.contains("Biologi"), "{markdown}");
    // A section with nothing in it is named, so the document cannot read as "we hold nothing"
    // when it means "we did not look".
    assert!(markdown.contains("## Cheatsheets"), "{markdown}");

    let (status, content_type, bytes) = document(&app, "/account/export?format=ics", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/calendar"), "{content_type}");
    let ics = String::from_utf8_lossy(&bytes);
    assert!(ics.contains("BEGIN:VCALENDAR"), "{ics}");
    assert!(ics.contains("BEGIN:VEVENT"), "{ics}");
    assert!(ics.contains("Kumpulkan laporan"), "{ics}");

    // An unknown format is refused rather than silently answered with JSON.
    let (status, _, _) = document(&app, "/account/export?format=xlsx", &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The purge refuses a live account rather than deleting data nobody asked to have deleted.
#[sqlx::test(migrations = "../../migrations")]
async fn the_purge_refuses_an_account_that_was_never_marked(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.expect("the plans");
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let token = signed_up(&app, &harness, "live@example.com").await;
    let user_id = user_id_for(&harness.state.pool, "live@example.com").await;
    let _ = workspace(&app, &token, "Masih Ada").await;

    let failure = stvdy_server::jobs::handlers::purge_accounts::purge(&harness.state, user_id)
        .await
        .expect_err("a live account must not be purged");
    assert!(
        !failure.retryable,
        "no retry will mark an account the student did not delete"
    );

    let workspaces: i64 = sqlx::query_scalar("select count(*) from workspaces where user_id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .expect("workspaces");
    assert_eq!(workspaces, 1, "the refusal must leave the account alone");
}
