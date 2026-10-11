//! Shared test helpers for the Phase 2 suites.
//!
//! A module rather than a copy in each file: `signed_up` in particular encodes a
//! rule (an account must verify its email before it can spend), and a second copy
//! that forgot to verify would silently test a different thing.
//!
//! `#![allow]` on the dead-code lint because each integration test binary
//! compiles this module separately and uses a different subset of it.

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use stvdy_server::testing::TestState;
use tower::ServiceExt;

/// A JSON or multipart request, returning the status and the parsed body.
pub async fn post_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }

    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body)).expect("request"))
        .await
        .expect("response");

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    let value: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

/// A JSON request with no body (a GET, or a DELETE).
pub async fn json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("serialize")
    };
    post_json(
        app,
        method,
        uri,
        token,
        if body.is_null() {
            None
        } else {
            Some("application/json")
        },
        payload,
    )
    .await
}

/// A multipart body with one `kind` field and one payload field.
pub fn multipart(fields: &[(&str, &str, Option<&[u8]>)]) -> (String, Vec<u8>) {
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

/// Signs up, verifies the email, and returns the access token.
///
/// Verification is not optional here: A43 keeps chat and ingestion locked until
/// the address is confirmed, so a helper that skipped it would make every suite
/// test the refusal path instead of the feature.
pub async fn signed_up(app: &axum::Router, harness: &TestState, email: &str) -> String {
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
        .expect("an access token")
        .to_string()
}

/// One parsed SSE event.
#[derive(Debug, Clone)]
pub struct SseEvent {
    name: String,
    data: serde_json::Value,
}

impl SseEvent {
    pub fn kind(&self) -> &str {
        &self.name
    }

    pub fn json(&self) -> &serde_json::Value {
        &self.data
    }
}

/// Drives the chat route and parses the event stream.
///
/// `oneshot` buffers the whole response, which is exactly what a test wants: the
/// stream is consumed to completion before the assertions run, so there is no
/// race between the assertions and the server task.
pub async fn sse_events(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, Vec<SseEvent>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(Body::from(serde_json::to_vec(&body).expect("serialize")))
                .expect("request"),
        )
        .await
        .expect("response");

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");

    (status, parse_sse(&String::from_utf8_lossy(&bytes)))
}

/// Parses an SSE body into its events.
///
/// Written by hand rather than with a crate because the point is to check the
/// wire format the client will see: a parser that shared code with the writer
/// could agree with it about a mistake.
pub fn parse_sse(body: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    let mut name = String::new();
    let mut data = String::new();

    for line in body.lines() {
        if line.is_empty() {
            if !name.is_empty() {
                events.push(SseEvent {
                    name: std::mem::take(&mut name),
                    data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
                });
            }
            data.clear();
            continue;
        }
        // A comment line (a heartbeat) carries no event name and is skipped.
        if line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.trim_start());
        }
    }

    // A body that does not end in a blank line still has a final event.
    if !name.is_empty() {
        events.push(SseEvent {
            name,
            data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
        });
    }

    events
}

/// Runs the job worker until the queue is empty, bounded so a stuck job fails the
/// test loudly rather than hanging it.
pub async fn drain_jobs(harness: &TestState) {
    let worker = stvdy_server::jobs::worker::spawn(harness.state.clone());
    for _ in 0..200 {
        let queued: i64 =
            sqlx::query_scalar("select count(*) from jobs where status in ('queued', 'running')")
                .fetch_one(&harness.state.pool)
                .await
                .expect("counting jobs");
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
            .expect("counting jobs");
    assert_eq!(remaining, 0, "the worker did not drain the queue");
}

/// Creates a workspace and ingests one text source into it, returning
/// `(workspace_id, source_id)`.
pub async fn workspace_with_text(
    app: &axum::Router,
    harness: &TestState,
    token: &str,
    name: &str,
    title: &str,
    text: &str,
) -> (String, String) {
    let (status, workspace) = json(
        app,
        "POST",
        "/workspaces",
        token,
        serde_json::json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{workspace}");
    let workspace_id = workspace["workspace"]["id"].as_str().unwrap().to_string();

    let (content_type, body) = multipart(&[
        ("kind", "text", None),
        ("title", title, None),
        ("text", text, None),
    ]);
    let (status, created) = post_json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/sources"),
        token,
        Some(&content_type),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let source_id = created["source"]["id"].as_str().unwrap().to_string();

    drain_jobs(harness).await;
    (workspace_id, source_id)
}
