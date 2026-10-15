//! A43's abuse controls: the signup limiter, the spend cap, and the verified-email gate.
//!
//! Three controls, and each one is asserted through the thing it actually protects:
//!
//!  * The **signup limiter** is database-backed (A154), so the tests drive real
//!    signups and then read `signup_attempts` back. An in-process bucket would pass
//!    a test that only counted requests while failing the property the control
//!    exists for (a restart is free for a script).
//!  * The **spend cap** bounds the provider-cost ceiling's overshoot (A155), so the
//!    test asserts both halves: the allowance is not spent early, and a read is
//!    never capped at all (A149).
//!  * The **verified-email gate** is A43's first control, and A359 widened it from the four
//!    spending routes to the whole workspace: it is a property of the routes' extractors
//!    rather than of any handler, and `route_sweep.rs` drives every route on that axis in both
//!    directions (the ones that must refuse, and the account's own surface, which must not).
//!
//! R5 is why the per-IP test also asserts the *log line*: the false-positive rate on
//! a campus NAT has to be measurable before the number is tuned, and a 429 with no
//! trace of who hit it is not measurable.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

use common::{json, multipart, post_json, signed_up};
use stvdy_server::testing::TestState;

/// The same signup body every test uses, so a change to the shape is one edit.
fn signup_body(email: &str) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": "correct horse battery",
        "device": { "name": "Laptop", "platform": "macos" }
    })
}

/// A request carrying an explicit peer address.
///
/// The limiter counts `x-real-ip` when it is present (what nginx sets from the
/// address its `real_ip` module resolved) and the connection's `ConnectInfo`
/// otherwise. A test that needs a second IP supplies `ConnectInfo` the way a
/// listener would. This is also the assertion that `X-Forwarded-For` is ignored:
/// the header is set to a different address and the limiter must not believe it.
async fn signup_from(app: &axum::Router, ip: &str, email: &str) -> (StatusCode, serde_json::Value) {
    let body = serde_json::to_vec(&signup_body(email)).expect("serialize");
    let mut request = Request::builder()
        .method("POST")
        .uri("/auth/signup")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "198.51.100.200")
        .body(Body::from(body))
        .expect("request");
    // The port is irrelevant: the limiter keys on the address. `ConnectInfo` is what
    // a real listener inserts, so this is the same value the guard would see.
    let address = ip.parse::<std::net::IpAddr>().expect("a peer address");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(address, 4242)));

    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

/// A subscriber that keeps what it formats, so a test can assert on the trace.
///
/// `tracing_subscriber` is already a dependency and `#[sqlx::test]` runs its body on
/// a current-thread runtime, so a thread-local subscriber sees the handler's events.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

async fn router(harness: &TestState) -> axum::Router {
    harness.seed_plans().await.expect("the plan seed");
    stvdy_server::routes::router(harness.state.clone()).expect("the router")
}

/// A43(1)/A359: an unverified account cannot reach the workspace at all.
///
/// This used to be the narrower rule — "cannot chat, generate or ingest" — and it created a
/// workspace first to reach those routes, with a comment saying so. That is the behaviour A359
/// removed: the workspace is where the gate starts, so the four refusals are now one of a
/// larger set and `route_sweep.rs` drives every route on this axis in both directions. What is
/// asserted here is the property a client sees for the four that spend money: 403
/// `email_not_verified`, and **no** work started — no chunks, no queued job, no credit spent.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unverified_account_cannot_reach_its_workspace(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;

    let (status, body) = json(
        &app,
        "POST",
        "/auth/signup",
        "",
        signup_body("unverified@ugm.ac.id"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["access_token"].as_str().expect("a token").to_string();

    // Creating a workspace is refused too (A359). It spends nothing, which was the reason it
    // used to be allowed — but an account that can hold a workspace can be filled from a
    // device's local cache and read back, so "costs nothing" is not the test any more.
    let (status, refused) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Cell biology" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "email_not_verified");

    // Every route behind the workspace answers the same way, and the id is arbitrary: the gate
    // runs in the extractor, before the handler looks anything up.
    let workspace_id = uuid::Uuid::now_v7();
    let (status, refused) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "what is a mitochondrion?" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "email_not_verified");

    let (status, refused) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/decks"),
        &token,
        serde_json::json!({ "title": "Deck", "source_ids": [], "count": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "email_not_verified");

    let (status, refused) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/quizzes"),
        &token,
        serde_json::json!({
            "title": "Quiz",
            "source_ids": [],
            "count": 10,
            "difficulty": "easy",
            "types": "mcq"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "email_not_verified");

    let (content_type, upload) = multipart(&[
        ("kind", "text", None),
        ("text", "Mitochondria are the powerhouse of the cell.", None),
    ]);
    let (status, refused) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        &token,
        Some(&content_type),
        upload,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "email_not_verified");

    // The refusal is real, not cosmetic: nothing was written, no job was queued and
    // no credit left the ledger. The signup grant is a positive row and is expected;
    // a spend is the negative one.
    let (sources, chunks, jobs, spent): (i64, i64, i64, i64) = sqlx::query_as(
        "select
           (select count(*) from sources),
           (select count(*) from chunks),
           (select count(*) from jobs),
           (select count(*) from credit_ledger where delta < 0)",
    )
    .fetch_one(&harness.state.pool)
    .await
    .expect("counting");
    assert_eq!((sources, chunks, jobs, spent), (0, 0, 0, 0));
}

/// The per-IP signup limit trips, and it **logs**.
///
/// R5's requirement is that the false-positive rate on a campus NAT is measurable
/// before the limit is tuned, so this asserts the refusal *and* the tracing line.
#[sqlx::test(migrations = "../../migrations")]
async fn the_per_ip_signup_limit_trips_and_logs(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;

    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let ip = "203.0.113.7";
    let limit = stvdy_server::ratelimit::SIGNUP_PER_IP_PER_DAY;
    for index in 0..limit {
        let (status, body) = signup_from(&app, ip, &format!("student{index}@ugm.ac.id")).await;
        assert_eq!(status, StatusCode::CREATED, "attempt {index}: {body}");
    }

    let (status, refused) = signup_from(&app, ip, "one-too-many@ugm.ac.id").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["code"], "rate_limited");

    // The log is the point: an unlogged block cannot be tuned, because nobody can
    // tell a farm from a lecture hall behind one address.
    let text = logs.text();
    assert!(text.contains(ip), "the block must name the IP: {text}");
    assert!(
        text.contains("ugm.ac.id"),
        "the block must name the domain: {text}"
    );
    assert!(
        text.contains("per_ip"),
        "the block must name the rule that tripped: {text}"
    );
    assert!(
        text.contains(&limit.to_string()),
        "the block must carry the count and the limit: {text}"
    );

    // The count lives in the database, which is the whole reason the control
    // survives a restart (A154): a farm's cheapest move is to wait for one.
    let recorded: i64 = sqlx::query_scalar("select count(*) from signup_attempts where ip = $1")
        .bind(ip)
        .fetch_one(&harness.state.pool)
        .await
        .expect("counting attempts");
    assert_eq!(recorded, i64::from(limit));
}

/// A public mailbox domain is exempt from the domain rule (A154): a limit on
/// `gmail.com` would block the product's growth and stop no farm, because a farm
/// uses gmail too.
#[sqlx::test(migrations = "../../migrations")]
async fn a_public_email_domain_is_not_rate_limited(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;

    let limit = stvdy_server::ratelimit::SIGNUP_PER_DOMAIN_PER_DAY;

    // Both domains are put at their cap from distinct addresses, so only the
    // *domain* rule can trip below: the request addresses below have no history.
    for domain in ["gmail.com", "ugm.ac.id"] {
        sqlx::query(
            "insert into signup_attempts (ip, email_domain)
             select '198.51.100.' || n::text, $1
               from generate_series(1, $2) as n",
        )
        .bind(domain)
        .bind(i64::from(limit))
        .execute(&harness.state.pool)
        .await
        .expect("seeding attempts");
    }

    let (status, body) = signup_from(&app, "203.0.113.20", "a.student@gmail.com").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, refused) = signup_from(&app, "203.0.113.21", "b.student@ugm.ac.id").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["code"], "rate_limited");
}

/// A failed signup still counts: a farm's failures are the signal.
///
/// A taken address is the cheapest thing a script can try, and it is exactly the
/// attempt a limiter that only counted successes would miss.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_signup_still_records_an_attempt(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;

    let ip = "203.0.113.30";
    let (status, body) = signup_from(&app, ip, "taken@ugm.ac.id").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, refused) = signup_from(&app, ip, "taken@ugm.ac.id").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    let recorded: i64 = sqlx::query_scalar("select count(*) from signup_attempts where ip = $1")
        .bind(ip)
        .fetch_one(&harness.state.pool)
        .await
        .expect("counting attempts");
    assert_eq!(recorded, 2, "the failed attempt must count too");
}

/// A43's per-account spend cap (A155): the eleventh spend action in a minute is
/// refused with the roadmap's `rate_limited` (429), and a **read** never is.
#[sqlx::test(migrations = "../../migrations")]
async fn the_spend_cap_trips_at_ten_a_minute_and_never_caps_a_read(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;
    let token = signed_up(&app, &harness, "spender@ugm.ac.id").await;

    let (status, workspace) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Cell biology" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workspace}");
    let workspace_id = workspace["workspace"]["id"]
        .as_str()
        .expect("a workspace id")
        .to_string();

    let limit = stvdy_server::ratelimit::SPEND_PER_MINUTE;
    for index in 0..limit {
        let (status, body) = json(
            &app,
            "POST",
            &format!("/workspaces/{workspace_id}/chat"),
            &token,
            serde_json::json!({ "message": format!("question {index}") }),
        )
        .await;
        // What the chat route answers with no model configured is its own business
        // (it is a provider error); what this asserts is that the cap did not fire
        // before the allowance was spent.
        assert_ne!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "action {index} is inside the allowance: {body}"
        );
    }

    let (status, refused) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": "one too many" }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["code"], "rate_limited");

    // A read is not a spend, and the SPA polls `/sources/{id}/status` every 1.5s.
    // Capping reads would break the screen the cap exists to keep honest.
    for _ in 0..(limit * 2) {
        let (status, body) = json(
            &app,
            "GET",
            &format!("/workspaces/{workspace_id}/sources"),
            &token,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

/// The cap is per account, not global: one account's burst must not refuse
/// another's first action. A shared bucket is the bug this catches.
#[sqlx::test(migrations = "../../migrations")]
async fn the_spend_cap_is_per_account(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;

    let noisy = signed_up(&app, &harness, "noisy@ugm.ac.id").await;
    let quiet = signed_up(&app, &harness, "quiet@ugm.ac.id").await;

    let (status, workspace) = json(
        &app,
        "POST",
        "/workspaces",
        &noisy,
        serde_json::json!({ "name": "Noisy" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workspace}");
    let workspace_id = workspace["workspace"]["id"]
        .as_str()
        .expect("a workspace id")
        .to_string();

    for _ in 0..=stvdy_server::ratelimit::SPEND_PER_MINUTE {
        let _ = json(
            &app,
            "POST",
            &format!("/workspaces/{workspace_id}/chat"),
            &noisy,
            serde_json::json!({ "message": "spend" }),
        )
        .await;
    }

    // The other account has its own bucket, and this is the assertion that a
    // single global counter would fail.
    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &quiet,
        serde_json::json!({ "message": "hello" }),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "another account's bucket must not be spent: {body}"
    );

    // And the noisy account is still refused, so the loop above really did trip it.
    let (status, refused) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/chat"),
        &noisy,
        serde_json::json!({ "message": "spend" }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
}

/// `X-Forwarded-For` is not trusted.
///
/// With no proxy in front the header is attacker-controlled, so honouring it would
/// let one caller spend every address's allowance by inventing a header. The helper
/// above sets it to a constant; this asserts the limiter counted the *connection*
/// address by exhausting one IP while the header stays the same.
#[sqlx::test(migrations = "../../migrations")]
async fn a_forwarded_for_header_cannot_buy_a_second_allowance(pool: PgPool) {
    let harness = TestState::new(pool);
    let app = router(&harness).await;

    let ip = "203.0.113.40";
    for index in 0..stvdy_server::ratelimit::SIGNUP_PER_IP_PER_DAY {
        let (status, body) = signup_from(&app, ip, &format!("fwd{index}@ugm.ac.id")).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    // A different claimed `X-Forwarded-For` value would be a second allowance if the
    // header were believed. It is not: the peer address is still at its cap.
    let (status, refused) = signup_from(&app, ip, "another@ugm.ac.id").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["code"], "rate_limited");

    let forwarded: i64 = sqlx::query_scalar("select count(*) from signup_attempts where ip = $1")
        .bind("198.51.100.200")
        .fetch_one(&harness.state.pool)
        .await
        .expect("counting attempts");
    assert_eq!(
        forwarded, 0,
        "the header value must never be recorded as the IP"
    );
}
