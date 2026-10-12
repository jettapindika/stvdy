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

/// A request whose body is a document rather than JSON: its status, its content type and its
/// raw bytes.
///
/// The export routes and the cheatsheet's HTML and PDF answer bytes on purpose, so a helper
/// that parsed them as JSON would report `null` for a response that is perfectly correct —
/// which is the shape of a test that passes without looking at the document.
pub async fn document(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, String, Vec<u8>) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");

    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();

    (status, content_type, bytes)
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
    signed_up_on(app, harness, email, "macos").await
}

/// [`signed_up`] with the device platform named.
///
/// The platform is a fact about the **device row**, and the billing policy reads it (A144),
/// so a suite that could only sign up as `macos` could not test the mobile refusal at all.
pub async fn signed_up_on(
    app: &axum::Router,
    harness: &TestState,
    email: &str,
    platform: &str,
) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/auth/signup",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": "Laptop", "platform": platform }
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

/// One request a [`StubHttp`] received.
///
/// Recorded because the interesting assertions about a client are about what it
/// *sent*: the path, the auth header and the JSON body are the contract, and a stub
/// that only returned a body would let a client that posted to the wrong path pass.
#[derive(Debug, Clone)]
pub struct StubRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl StubRequest {
    /// A header's value, lowercased name, case-insensitive.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A one-endpoint HTTP server that answers every request with `status` and `body`,
/// and records what it was asked.
///
/// The single copy of the listener `tests/search.rs` used to hand-roll: two copies
/// would be two places to get the response framing wrong, and the qris suite needs the
/// same thing. It is enough of the protocol for a `reqwest` client to read one JSON
/// response, and no more — the point is to exercise the *client's* mapping, not to be
/// an HTTP server.
pub struct StubHttp {
    base_url: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<StubRequest>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl StubHttp {
    /// The base URL to point a client at, with no trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<StubRequest> {
        self.requests
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl Drop for StubHttp {
    fn drop(&mut self) {
        // Aborted rather than leaked: a suite runs many cases, and a listener per case
        // that outlives the test is a file descriptor per case.
        self.handle.abort();
    }
}

/// Starts a stub answering `status` and `body`.
pub async fn stub_http_json(status: u16, body: impl Into<String>) -> StubHttp {
    use tokio::io::AsyncWriteExt;

    let body = body.into();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a stub listener");
    let address = listener.local_addr().expect("a stub address");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = requests.clone();

    let handle = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let request = read_stub_request(&mut socket).await;
            if let Some(request) = request
                && let Ok(mut log) = recorded.lock()
            {
                log.push(request);
            }
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                404 => "Not Found",
                500 => "Internal Server Error",
                502 => "Bad Gateway",
                _ => "Status",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    StubHttp {
        base_url: format!("http://{address}"),
        requests,
        handle,
    }
}

/// Reads one HTTP request: headers to the blank line, then `content-length` bytes.
///
/// A request without a `content-length` (a GET) is complete at the blank line. A
/// malformed one returns `None`, which the caller records as no request rather than
/// answering something the client did not ask for.
async fn read_stub_request(socket: &mut tokio::net::TcpStream) -> Option<StubRequest> {
    use tokio::io::AsyncReadExt;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(end) = find_header_end(&buffer) {
            break end;
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }

    Some(StubRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&buffer[body_start..]).to_string(),
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
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
