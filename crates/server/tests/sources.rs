//! Phase 1 sources and ingestion: the phase's acceptance test.
//!
//! The end-to-end assertion the roadmap asks for is "PDF upload -> chunks +
//! embeddings -> ready". PDF extraction needs a pdfium shared library, which is
//! a deployment artefact rather than a repository file, so the suite covers both
//! halves honestly: a `text` source proves the whole pipeline (extract, chunk,
//! embed, dedupe, status) end to end, and the PDF arm asserts the *failure path*
//! when pdfium is absent, which is the message a user actually sees.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use stvdy_server::testing::TestState;
use tower::ServiceExt;

/// A multipart body with one `kind` field and one payload field.
fn multipart(fields: &[(&str, &str, Option<&[u8]>)]) -> (String, Vec<u8>) {
    let boundary = "----stvdy-test-boundary";
    let mut body = Vec::new();
    for (name, value, file) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(bytes) => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{value}\"\r\n\
                         Content-Type: application/pdf\r\n\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(bytes);
            }
            None => {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                body.extend_from_slice(value.as_bytes());
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body)).expect("request"))
        .await
        .expect("response");

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    let value: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

async fn json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    send(
        app,
        method,
        uri,
        token,
        Some("application/json"),
        serde_json::to_vec(&body).expect("serialize"),
    )
    .await
}

/// Signs up, verifies the email, and returns the access token. Ingestion needs a
/// verified account once Phase 7 wires A43 through, so the helper does it now
/// rather than letting every test learn the rule later.
async fn signed_up(app: &axum::Router, harness: &TestState, email: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let link = harness
        .last_verification_link()
        .expect("a verification link");
    let token = link.split("token=").nth(1).expect("token").to_string();
    let (status, _) = json(
        app,
        "POST",
        "/auth/verify-email",
        "",
        serde_json::json!({ "token": token }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    body["access_token"]
        .as_str()
        .expect("access token")
        .to_string()
}

/// Runs the worker until it has nothing left to do, or the budget runs out.
///
/// The worker is a real task rather than a direct handler call, so this exercises
/// the claim/backoff path too. A bounded loop rather than a sleep: the test must
/// fail loudly if the job never completes, not hang.
async fn drain_jobs(harness: &TestState) {
    let worker = stvdy_server::jobs::worker::spawn(harness.state.clone());
    for _ in 0..100 {
        let queued: i64 =
            sqlx::query_scalar("select count(*) from jobs where status in ('queued', 'running')")
                .fetch_one(&harness.state.pool)
                .await
                .unwrap();
        if queued == 0 {
            break;
        }
        harness.state.jobs.wake();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    worker.abort();

    let remaining: i64 =
        sqlx::query_scalar("select count(*) from jobs where status in ('queued', 'running')")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "the worker did not drain the queue");
}

async fn create_workspace(app: &axum::Router, token: &str, name: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["workspace"]["id"]
        .as_str()
        .expect("workspace id")
        .to_string()
}

/// The acceptance test: a pasted-text source becomes chunks with embeddings and
/// reports `ready`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_text_source_ingests_to_ready_with_chunks_and_embeddings(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "ingest@example.test").await;
    let workspace = create_workspace(&app, &token, "Biology").await;

    // Long enough to produce several chunks at the 480-token target.
    let text = (0..80)
        .map(|i| {
            format!(
                "Mitochondria are the powerhouse of the cell, and paragraph {i} \
                 explains oxidative phosphorylation in some detail."
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", "Cell biology", None),
        ("text", &text, None),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"]
        .as_str()
        .expect("source id")
        .to_string();
    assert_eq!(created["source"]["status"], "pending");
    assert_eq!(created["source"]["title"], "Cell biology");

    drain_jobs(&harness).await;

    let (status, status_body) = json(
        &app,
        "GET",
        &format!("/sources/{source_id}/status"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{status_body}");
    assert_eq!(
        status_body["status"], "ready",
        "the source must reach ready: {status_body}"
    );
    let chunk_count = status_body["chunk_count"].as_i64().unwrap();
    assert!(
        chunk_count >= 2,
        "expected several chunks, got {chunk_count}"
    );
    assert!(status_body["error"].is_null());

    // Every chunk has a 768-dimension vector from the configured embedder, and
    // the stored text does not carry the E5 `passage: ` prefix (A2: only the
    // vector input is prefixed).
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select content, embedder, vector_dims(embedding)::text
           from chunks where source_id = $1",
    )
    .bind(uuid::Uuid::parse_str(&source_id).unwrap())
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(rows.len() as i64, chunk_count);
    for (content, embedder, dims) in &rows {
        assert_eq!(embedder, "hash-embedder-768");
        assert_eq!(dims.as_deref(), Some("768"));
        assert!(
            !content.starts_with("passage: ") && !content.starts_with("query: "),
            "the stored text must stay unprefixed"
        );
    }

    // The chunk is readable through the documented endpoint, with its page.
    let chunk_id: uuid::Uuid =
        sqlx::query_scalar("select id from chunks where source_id = $1 order by idx limit 1")
            .bind(uuid::Uuid::parse_str(&source_id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let (status, chunk) = json(
        &app,
        "GET",
        &format!("/chunks/{chunk_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chunk}");
    assert_eq!(chunk["source_id"], source_id);
    assert!(chunk["content"].as_str().unwrap().contains("Mitochondria"));
}

/// The dedupe rule from the spec: a chunk whose content already exists is not
/// re-embedded. Asserted by comparing the stored vectors, because that is what
/// "not re-embedded" means.
#[sqlx::test(migrations = "../../migrations")]
async fn identical_content_reuses_the_stored_embedding(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "dedupe@example.test").await;
    let workspace = create_workspace(&app, &token, "Notes").await;

    let text = "Glycolysis converts glucose into pyruvate and yields two ATP.";

    for title in ["First", "Second"] {
        let (content_type, body) = multipart(&[
            ("kind", "text", None),
            ("title", title, None),
            ("text", text, None),
        ]);
        let (status, created) = send(
            &app,
            "POST",
            &format!("/workspaces/{workspace}/sources"),
            &token,
            Some(&content_type),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        drain_jobs(&harness).await;
    }

    // `chunks` has no `created_at` (it is server-only and never synced), so the
    // order comes from the source, which does.
    let vectors: Vec<String> = sqlx::query_scalar(
        "select c.embedding::text from chunks c
           join sources s on s.id = c.source_id
          where c.content = $1
          order by s.created_at",
    )
    .bind(text)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();

    assert_eq!(vectors.len(), 2, "both sources produced the chunk");
    assert_eq!(
        vectors[0], vectors[1],
        "the second copy must reuse the first copy's vector"
    );
}

/// The quota checks happen before the bytes are stored, and the error names the
/// plan. A file over `max_file_mb` is a validation-shaped failure of the plan.
#[sqlx::test(migrations = "../../migrations")]
async fn uploads_are_checked_against_the_plan_limits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "limits@example.test").await;
    let workspace = create_workspace(&app, &token, "Limits").await;

    let free = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .clone();

    // Over `max_file_mb` for the free plan.
    let oversized = "x".repeat((free.max_file_mb as usize + 1) * 1024 * 1024);
    let (content_type, body) = multipart(&[("kind", "text", None), ("text", &oversized, None)]);
    let (status, rejected) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{rejected}");
    assert_eq!(rejected["code"], "limit_reached");
    assert_eq!(rejected["min_plan"], "pro");

    // Nothing was stored: the check runs before `put`.
    let objects: i64 = sqlx::query_scalar("select count(*) from sources")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(objects, 0);

    // An unsupported kind is a validation error, not a silent success.
    let (content_type, body) = multipart(&[("kind", "docx", None), ("text", "hi", None)]);
    let (status, rejected) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["code"], "validation");
}

/// A PDF whose extraction is unavailable must land `failed` with a message the
/// user can act on, and must not be retried forever.
#[sqlx::test(migrations = "../../migrations")]
async fn a_pdf_without_a_pdfium_library_fails_readably(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "pdf@example.test").await;
    let workspace = create_workspace(&app, &token, "Papers").await;

    // A minimal, syntactically valid PDF. Extraction is never reached when the
    // library is missing, which is what this asserts: the *failure* path, not a
    // fabricated success.
    let minimal_pdf = b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF\n";
    let (content_type, body) = multipart(&[
        ("kind", "pdf", None),
        ("file", "paper.pdf", Some(minimal_pdf)),
    ]);

    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"].as_str().unwrap().to_string();

    drain_jobs(&harness).await;

    let (status, status_body) = json(
        &app,
        "GET",
        &format!("/sources/{source_id}/status"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // With a configured-but-absent library the ingest must fail with the path in
    // the message; with no path configured it must say so. Either way: readable,
    // and never a silent `ready`.
    assert_eq!(
        status_body["status"], "failed",
        "an unextractable PDF must not be reported as ready: {status_body}"
    );
    let error = status_body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("pdfium") || error.contains("PDFIUM_LIB_PATH"),
        "the error must name the missing extractor, got {error:?}"
    );

    // Retries are bounded: the job must not be left queued for another attempt.
    let failed_jobs: i64 = sqlx::query_scalar("select count(*) from jobs where status = 'failed'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(failed_jobs, 1);
}

/// Every read is scoped to the owner. A row that exists for someone else must be
/// a 404, never a 403: existence must not leak.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_source_is_not_visible(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let owner = signed_up(&app, &harness, "owner@example.test").await;
    let intruder = signed_up(&app, &harness, "intruder@example.test").await;
    let workspace = create_workspace(&app, &owner, "Private").await;

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", "Secret", None),
        ("text", "The answer is 42.", None),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &owner,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let source_id = created["source"]["id"].as_str().unwrap().to_string();

    drain_jobs(&harness).await;

    let chunk_id: uuid::Uuid =
        sqlx::query_scalar("select id from chunks where source_id = $1 limit 1")
            .bind(uuid::Uuid::parse_str(&source_id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

    for uri in [
        format!("/sources/{source_id}/status"),
        format!("/sources/{source_id}/file"),
        format!("/chunks/{chunk_id}"),
        format!("/workspaces/{workspace}/sources"),
    ] {
        let (status, _) = json(&app, "GET", &uri, &intruder, serde_json::Value::Null).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{uri} leaked to another account"
        );
    }
}

/// Deleting a source tombstones it, drops its chunks, and removes the object.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_source_removes_its_chunks_and_object(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "delete@example.test").await;
    let workspace = create_workspace(&app, &token, "Temp").await;

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("text", "Transient content that will be deleted.", None),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let source_id = created["source"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let key: String = sqlx::query_scalar("select file_key from sources where id = $1")
        .bind(uuid::Uuid::parse_str(&source_id).unwrap())
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/sources/{source_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let chunks: i64 = sqlx::query_scalar("select count(*) from chunks where source_id = $1")
        .bind(uuid::Uuid::parse_str(&source_id).unwrap())
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(chunks, 0, "chunks must not outlive their source");

    let (status, _) = json(
        &app,
        "GET",
        &format!("/sources/{source_id}/status"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(list["sources"].as_array().unwrap().len(), 0);

    // The object is gone from the double's store, which is what a bucket would
    // show.
    assert!(
        stvdy_core::traits::storage::Storage::get(harness.storage.as_ref(), &key)
            .await
            .is_err(),
        "the stored object must be deleted"
    );
}

/// The signed download URL is the local backend's equivalent of an S3 presigned
/// URL: no bearer token, and the signature is bound to the object and the expiry.
#[sqlx::test(migrations = "../../migrations")]
async fn the_signed_file_url_works_without_a_bearer_token(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "file@example.test").await;
    let workspace = create_workspace(&app, &token, "Files").await;

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("text", "Downloadable content.", None),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let source_id = created["source"]["id"].as_str().unwrap().to_string();

    let (status, signed) = json(
        &app,
        "GET",
        &format!("/sources/{source_id}/file"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    let url = signed["url"].as_str().expect("a URL").to_string();
    assert!(url.contains("/objects/"), "{url}");
    assert!(url.contains("sig="), "{url}");
    assert!(signed["expires_at"].is_string());

    // Fetch it with no Authorization header at all.
    let path = url.split("http://localhost:8080").nth(1).expect("a path");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"Downloadable content.");

    // A tampered signature is refused, and so is a signature for another object.
    let bad = path.replace("sig=", "sig=0");
    let response = app
        .clone()
        .oneshot(Request::builder().uri(bad).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // A workspace deleted first leaves its sources unreachable, which is the
    // same guarantee as the source list.
    let other_workspace = create_workspace(&app, &token, "Other").await;
    let (_, other_source) = {
        let (content_type, body) =
            multipart(&[("kind", "text", None), ("text", "Another body.", None)]);
        send(
            &app,
            "POST",
            &format!("/workspaces/{other_workspace}/sources"),
            &token,
            Some(&content_type),
            body,
        )
        .await
    };
    let other_id = other_source["source"]["id"].as_str().unwrap().to_string();

    // The first source's signature must not open the second object.
    let cross = path.replace(&source_id, &other_id);
    let response = app
        .oneshot(Request::builder().uri(cross).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// A43: ingestion is one of the four things an unverified account cannot do, and
/// it is the first route in the project that spends provider money on embedding
/// calls. The check must happen before anything is stored, or a scripted account
/// can fill the bucket without ever confirming its address.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unverified_account_cannot_ingest(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    // Signup WITHOUT consuming the verification link, unlike `signed_up`.
    let (status, body) = json(
        &app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": "unverified@example.test",
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": "macos" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["access_token"]
        .as_str()
        .expect("access token")
        .to_string();

    // Creating a workspace is allowed: only spending is gated.
    let workspace = create_workspace(&app, &token, "Unverified").await;

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("text", "This must never be embedded.", None),
    ]);
    let (status, rejected) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{rejected}");
    assert_eq!(rejected["code"], "email_not_verified");

    // Nothing was written: no source row, no chunks, no job, and the object
    // store is untouched.
    let sources: i64 = sqlx::query_scalar("select count(*) from sources")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let chunks: i64 = sqlx::query_scalar("select count(*) from chunks")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let jobs: i64 = sqlx::query_scalar("select count(*) from jobs")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!((sources, chunks, jobs), (0, 0, 0));
}

/// `sources_total` is a separate axis from `storage_mb`, and a text source is a
/// few KB, so without this check one account could create unbounded sources while
/// staying far under the storage cap.
#[sqlx::test(migrations = "../../migrations")]
async fn the_source_count_limit_is_enforced(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "many@example.test").await;
    let workspace = create_workspace(&app, &token, "Many").await;

    // Read the cap from the config, never a literal: retuning plans.toml must not
    // leave a stale expectation behind.
    let max = harness
        .state
        .config
        .plans
        .get(stvdy_core::config::PlanId::Free)
        .sources_total
        .max()
        .expect("the free plan caps sources");

    for index in 0..max {
        let (content_type, body) = multipart(&[
            ("kind", "text", None),
            ("text", &format!("Note number {index}."), None),
        ]);
        let (status, created) = send(
            &app,
            "POST",
            &format!("/workspaces/{workspace}/sources"),
            &token,
            Some(&content_type),
            body,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "source {index} of {max}: {created}"
        );
    }

    let (content_type, body) =
        multipart(&[("kind", "text", None), ("text", "One too many.", None)]);
    let (status, rejected) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{rejected}");
    assert_eq!(rejected["code"], "limit_reached");
    assert_eq!(rejected["min_plan"], "pro");
    assert_eq!(rejected["limit"], max as i64);
    assert_eq!(rejected["used"], max as i64);

    // Deleting one frees a slot, which is what the message implies.
    let source_id: uuid::Uuid = sqlx::query_scalar(
        "select id from sources where deleted_at is null order by created_at limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/sources/{source_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (content_type, body) = multipart(&[("kind", "text", None), ("text", "Fits now.", None)]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

/// Deleting a workspace must reclaim exactly what deleting its sources would:
/// the chunks and the stored objects. Otherwise it frees the user's storage
/// *quota* (`enforce_storage` sums `file_size` over live sources) while the
/// embeddings and the objects stay forever, and a create-delete loop keeps quota
/// at zero with unbounded storage behind it.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_workspace_reclaims_its_chunks_and_objects(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "churn@example.test").await;
    let workspace = create_workspace(&app, &token, "Churn").await;

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("text", "Content that must not survive its workspace.", None),
    ]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let source_id = uuid::Uuid::parse_str(created["source"]["id"].as_str().unwrap()).unwrap();
    drain_jobs(&harness).await;

    let key: String = sqlx::query_scalar("select file_key from sources where id = $1")
        .bind(source_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    let before: i64 = sqlx::query_scalar("select count(*) from chunks where source_id = $1")
        .bind(source_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert!(
        before > 0,
        "the source ingested, so there is something to reclaim"
    );

    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/workspaces/{workspace}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let after: i64 = sqlx::query_scalar("select count(*) from chunks where source_id = $1")
        .bind(source_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(after, 0, "a workspace delete must drop its chunks");

    assert!(
        stvdy_core::traits::storage::Storage::get(harness.storage.as_ref(), &key)
            .await
            .is_err(),
        "a workspace delete must delete its stored objects"
    );

    // And the quota is genuinely free again: a new source fits.
    let new_workspace = create_workspace(&app, &token, "After").await;
    let (content_type, body) =
        multipart(&[("kind", "text", None), ("text", "Fresh content.", None)]);
    let (status, created) = send(
        &app,
        "POST",
        &format!("/workspaces/{new_workspace}/sources"),
        &token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}
