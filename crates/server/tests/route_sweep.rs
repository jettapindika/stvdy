//! The route sweep: every registered route is classified, and every swept one refuses.
//!
//! The roadmap asks for a "grep test that shows no bypass". A grep that only *lists* routes
//! would pass while a route called nothing, so this suite is split in two halves and each
//! half catches a different failure:
//!
//!  * [`every_registered_route_is_classified`] fails when a route is added without a row in
//!    [`ROUTES`] or [`EXEMPT`]. It reads whole file bodies, not lines: three sites write
//!    `.route(` and its arguments across two source lines, so a line-oriented extractor
//!    would silently classify fewer paths and still pass.
//!  * [`every_swept_route_refuses_an_exhausted_account`] drives each `Swept` route with an
//!    account whose every limit is used and asserts the exact status and code. A route that
//!    is listed but calls nothing returns 200 or 500 here.
//!
//! The third test is the other half of A149: a plan limit read anywhere but the entitlements
//! layer is a second source of truth, and the way that happens is a handler reaching for
//! `plans.get(plan)` "just this once".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::config::PlanId;
use stvdy_server::testing::TestState;
use uuid::Uuid;

mod common;
use common::{json, multipart, post_json, signed_up};

/// How a route relates to the entitlement decision (A149).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Calls the engine for a feature. The `refusal` column is what an exhausted account
    /// must get from it.
    Swept,
    /// A read of the user's own rows, or an action that spends nothing. Never gated: the
    /// spec requires over-limit data to stay viewable and exportable.
    ReadOnly,
    /// No authenticated user at all: a health check, a credentials route, an OAuth
    /// redirect, or an HMAC-signed object download. The admin routes are here too: they
    /// take `AdminUser`, whose refusal is a 404 (A7), so there is no entitlement decision
    /// to sweep.
    Exempt,
}

struct Row {
    method: &'static str,
    path: &'static str,
    class: Class,
    /// Why a `ReadOnly` or `Exempt` route is not swept. One line, so the table explains
    /// itself rather than being a list of names.
    reason: &'static str,
    /// The feature the route's decision uses. Empty for a non-`Swept` row.
    feature: &'static str,
    /// The `(status, code)` an account whose every limit is used must get.
    refusal: Option<(u16, &'static str)>,
}

/// Every authenticated route that is not swept, and why.
fn read_only_rows() -> Vec<Row> {
    let ro = |method: &'static str, path: &'static str, reason: &'static str| Row {
        method,
        path,
        class: Class::ReadOnly,
        reason,
        feature: "",
        refusal: None,
    };

    vec![
        ro("GET", "/auth/me", "reads the caller's own account row"),
        ro(
            "POST",
            "/auth/logout-all",
            "revoking sessions frees a device slot",
        ),
        ro(
            "POST",
            "/auth/resend-verification",
            "must work while locked out by A43",
        ),
        ro(
            "POST",
            "/auth/google/set-password",
            "an account action, not a plan feature",
        ),
        ro("GET", "/devices", "reads the caller's own devices"),
        ro("PATCH", "/devices/{id}", "renaming a device spends nothing"),
        ro(
            "DELETE",
            "/devices/{id}",
            "revoking a device is the remedy a device refusal names",
        ),
        ro(
            "PUT",
            "/devices/{id}/push-token",
            "registering a token is not a plan feature",
        ),
        ro("GET", "/workspaces", "reads the caller's own workspaces"),
        ro(
            "GET",
            "/workspaces/{id}",
            "reads the caller's own workspace",
        ),
        ro("PATCH", "/workspaces/{id}", "renaming spends nothing"),
        ro(
            "DELETE",
            "/workspaces/{id}",
            "deleting is the remedy a workspace refusal names",
        ),
        ro(
            "GET",
            "/chunks/{id}",
            "reads a chunk of the caller's source",
        ),
        ro(
            "GET",
            "/workspaces/{id}/messages",
            "reads the caller's own thread",
        ),
        ro(
            "GET",
            "/workspaces/{id}/decks",
            "reads the caller's own decks",
        ),
        ro("GET", "/decks/{id}", "reads the caller's own deck"),
        ro("GET", "/decks/{id}/due", "reads the caller's own due cards"),
        ro(
            "POST",
            "/flashcards/{id}/review",
            "a review is free (R1): studying never costs credits",
        ),
        ro(
            "GET",
            "/workspaces/{id}/quizzes",
            "reads the caller's own quizzes",
        ),
        ro("GET", "/quizzes/{id}", "reads the caller's own quiz"),
        ro(
            "POST",
            "/quizzes/{id}/attempts",
            "an attempt is free (R1), like a review",
        ),
        ro(
            "GET",
            "/quizzes/{id}/results",
            "reads the caller's own results",
        ),
        ro(
            "GET",
            "/workspaces/{id}/sources",
            "reads the caller's own sources",
        ),
        ro(
            "GET",
            "/sources/{id}/status",
            "polls ingestion, which is already paid for",
        ),
        ro("GET", "/sources/{id}/file", "reads the caller's own file"),
        ro(
            "GET",
            "/sources/{id}/preview",
            "reads the caller's own PDF rendition; a read, so an over-limit account keeps it (A149)",
        ),
        ro(
            "DELETE",
            "/sources/{id}",
            "deleting frees a slot, per the refusal's message",
        ),
        ro(
            "GET",
            "/workspaces/{id}/reports",
            "reads the caller's own reports",
        ),
        ro("GET", "/reports/{id}", "reads the caller's own report"),
        ro(
            "PATCH",
            "/reports/{id}",
            "editing an outline spends nothing",
        ),
        ro(
            "GET",
            "/cheatsheets/{id}",
            "reads the caller's own cheatsheet",
        ),
        ro(
            "GET",
            "/workspaces/{id}/cheatsheets",
            "reads the caller's own cheatsheets",
        ),
        ro(
            "GET",
            "/cheatsheets/{id}/html",
            "the HTML is the document the credits already bought; only the PDF is gated",
        ),
        ro(
            "POST",
            "/cheatsheets/{id}/re-render",
            "re-rendering is a pure function of stored blocks and charges nothing",
        ),
        ro(
            "GET",
            "/usage",
            "reads the caller's own plan, counters and cost table; no decision and nothing spent (A149/A159)",
        ),
    ]
}

/// The swept routes: every row here calls the engine, and the `refusal` column is asserted
/// against a live request in the second test.
fn swept_rows() -> Vec<Row> {
    let swept = |method: &'static str,
                 path: &'static str,
                 feature: &'static str,
                 status: u16,
                 code: &'static str| Row {
        method,
        path,
        class: Class::Swept,
        reason: "",
        feature,
        refusal: Some((status, code)),
    };

    vec![
        swept(
            "POST",
            "/workspaces",
            "CreateWorkspace",
            422,
            "limit_reached",
        ),
        swept(
            "POST",
            "/workspaces/{id}/chat",
            "ChatMessage; WebSearch too when the turn ran a chargeable search",
            422,
            "limit_reached",
        ),
        swept(
            "POST",
            "/workspaces/{id}/decks",
            "FlashcardBatch{n}",
            422,
            "limit_reached",
        ),
        swept(
            "POST",
            "/workspaces/{id}/quizzes",
            "CreateQuiz{n}",
            422,
            "limit_reached",
        ),
        swept(
            "POST",
            "/workspaces/{id}/sources",
            "UploadSource{size_bytes}",
            422,
            "limit_reached",
        ),
        swept(
            "POST",
            "/workspaces/{id}/reports/outline",
            "Report",
            422,
            "limit_reached",
        ),
        swept(
            "POST",
            "/reports/{id}/generate",
            "credits only (report_section)",
            402,
            "insufficient_credits",
        ),
        swept(
            "GET",
            "/reports/{id}/export",
            "ExportFormat{format}",
            403,
            "plan_required",
        ),
        swept(
            "POST",
            "/workspaces/{id}/cheatsheets",
            "credits only (report_section)",
            402,
            "insufficient_credits",
        ),
        swept(
            "GET",
            "/cheatsheets/{id}/pdf",
            "ExportFormat{Pdf}",
            403,
            "plan_required",
        ),
    ]
}

/// Routes with no authenticated user at all.
fn exempt_rows() -> Vec<Row> {
    let exempt = |method: &'static str, path: &'static str, reason: &'static str| Row {
        method,
        path,
        class: Class::Exempt,
        reason,
        feature: "",
        refusal: None,
    };

    vec![
        exempt("GET", "/health", "no user"),
        exempt(
            "POST",
            "/auth/signup",
            "credentials; a limit check here would refuse the request that could fix the account",
        ),
        exempt(
            "POST",
            "/auth/login",
            "credentials; rate-limited by ratelimit::auth_layer",
        ),
        exempt(
            "POST",
            "/auth/refresh",
            "credentials; holds no bearer token yet",
        ),
        exempt(
            "POST",
            "/auth/logout",
            "credentials; holds a refresh token, not a session",
        ),
        exempt(
            "POST",
            "/auth/verify-email",
            "credentials; the token is the authorisation",
        ),
        exempt(
            "POST",
            "/auth/google/exchange",
            "a single-use ticket, not a bearer token",
        ),
        exempt("GET", "/auth/google/start", "a browser redirect to Google"),
        exempt(
            "GET",
            "/auth/google/callback",
            "Google's redirect back, not the user's",
        ),
        exempt(
            "GET",
            "/objects/{id}",
            "an HMAC-signed URL whose whole point is that it works without a session",
        ),
        exempt(
            "PATCH",
            "/admin/users/{id}/plan",
            "AdminUser only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep",
        ),
        exempt(
            "POST",
            "/admin/users/{id}/credits",
            "AdminUser only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep",
        ),
        exempt(
            "GET",
            "/admin/margin",
            "AdminUser only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep",
        ),
    ]
}

fn table() -> Vec<Row> {
    let mut rows = read_only_rows();
    rows.extend(swept_rows());
    rows.extend(exempt_rows());
    rows
}

/// Every `(METHOD, path)` pair registered under `crates/server/src`.
///
/// The extractor reads whole file bodies: three `.route(` calls in this tree write the path
/// and the handlers across two source lines, and a line-oriented regex would classify fewer
/// paths than exist while still passing.
fn registered_routes() -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for file in source_files() {
        let source = std::fs::read_to_string(&file).expect("a readable source file");
        for arguments in route_arguments(&source) {
            let (path, methods) = split_route_arguments(&arguments);
            let Some(path) = path else {
                panic!(
                    "{}: a .route() call with no path literal: {arguments}",
                    file.display()
                );
            };
            for method in methods {
                found.insert((method, path.clone()));
            }
        }
    }
    found
}

fn source_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    files
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// The argument text of every `.route(...)` call in `source`.
///
/// Hand-written rather than a regex because the argument list ends at the matching paren,
/// not at the first one: `.route("/x", post(a).get(b))` has two handlers, and a `[^)]*`
/// capture would see only `post`.
fn route_arguments(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index..].starts_with(b".route(") {
            let start = index + ".route(".len();
            let end = matching_paren(bytes, start);
            found.push(source[start..end].to_string());
            index = end + 1;
        } else {
            index = skip_token(bytes, index);
        }
    }

    found
}

/// The index of the `)` that closes the paren opened before `start`.
fn matching_paren(bytes: &[u8], start: usize) -> usize {
    let mut depth = 1usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
                index += 1;
            }
            _ => index = skip_token(bytes, index),
        }
    }
    panic!("a .route() call with no closing paren");
}

/// The next position after whatever begins at `from`, skipping the things whose contents
/// are not code: comments, strings, raw strings and char literals.
///
/// A char literal has to be recognised, not just skipped: `'"'` and `'\''` are real in this
/// tree, and treating the quote inside one as the start of a string swallows the rest of the
/// file and reports an unterminated literal.
fn skip_token(bytes: &[u8], from: usize) -> usize {
    match bytes[from] {
        b'/' if bytes.get(from + 1) == Some(&b'/') => {
            let mut index = from;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            index
        }
        b'/' if bytes.get(from + 1) == Some(&b'*') => {
            let mut index = from + 2;
            let mut depth = 1usize;
            while index < bytes.len() && depth > 0 {
                if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
                    depth += 1;
                    index += 2;
                } else if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            index
        }
        b'r' if raw_string_hashes(bytes, from).is_some() => {
            let hashes = raw_string_hashes(bytes, from).expect("checked above");
            let mut index = from + 1 + hashes + 1;
            while index < bytes.len() {
                if bytes[index] == b'"'
                    && bytes.len() >= index + 1 + hashes
                    && bytes[index + 1..index + 1 + hashes]
                        .iter()
                        .all(|b| *b == b'#')
                {
                    return index + 1 + hashes;
                }
                index += 1;
            }
            panic!("an unterminated raw string");
        }
        b'"' => {
            let mut index = from + 1;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index += 2,
                    b'"' => return index + 1,
                    _ => index += 1,
                }
            }
            panic!("an unterminated string literal");
        }
        b'\'' => {
            if bytes.get(from + 1) == Some(&b'\\') {
                // An escaped char: the closing quote is past the backslash and the escape.
                let mut index = from + 3;
                while index < bytes.len() && bytes[index] != b'\'' {
                    index += 1;
                }
                return index + 1;
            }
            if bytes.get(from + 2) == Some(&b'\'') {
                return from + 3;
            }
            // A lifetime, not a literal.
            from + 1
        }
        _ => from + 1,
    }
}

/// `Some(n)` when a raw string with `n` hashes begins at `from`.
fn raw_string_hashes(bytes: &[u8], from: usize) -> Option<usize> {
    if bytes.get(from) != Some(&b'r') {
        return None;
    }
    let mut index = from + 1;
    while bytes.get(index) == Some(&b'#') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'"')).then_some(index - from - 1)
}

/// Splits a `.route(...)` argument list into its path literal and its HTTP methods.
///
/// The handlers are read from the argument list at nesting depth zero, so `post(a).get(b)`
/// yields both and `get(handler)` yields one.
fn split_route_arguments(arguments: &str) -> (Option<String>, Vec<String>) {
    let bytes = arguments.as_bytes();
    let mut index = 0;
    let mut path = None;

    while index < bytes.len() {
        if bytes[index] == b'"' {
            let end = skip_token(bytes, index);
            path = Some(arguments[index + 1..end - 1].to_string());
            index = end;
            break;
        }
        index += 1;
    }

    let mut methods = Vec::new();
    let mut depth = 0i32;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth -= 1;
                index += 1;
            }
            byte if depth == 0 && (byte.is_ascii_alphabetic() || byte == b'_') => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let word = &arguments[start..index];
                // Only a call counts: `.get(` is a handler, and `get` on its own is not.
                let mut lookahead = index;
                while lookahead < bytes.len() && bytes[lookahead].is_ascii_whitespace() {
                    lookahead += 1;
                }
                if bytes.get(lookahead) == Some(&b'(') {
                    let method = match word {
                        "get" => Some("GET"),
                        "post" => Some("POST"),
                        "put" => Some("PUT"),
                        "patch" => Some("PATCH"),
                        "delete" => Some("DELETE"),
                        _ => None,
                    };
                    if let Some(method) = method {
                        methods.push(method.to_string());
                    }
                }
            }
            _ => index += 1,
        }
    }

    (path, methods)
}

/// A route added without a row here fails, which is the whole point: the sweep is a test,
/// not a review.
#[test]
fn every_registered_route_is_classified() {
    let registered = registered_routes();
    let classified: BTreeSet<(String, String)> = table()
        .iter()
        .map(|row| (row.method.to_string(), row.path.to_string()))
        .collect();

    let unclassified: Vec<_> = registered.difference(&classified).collect();
    assert_eq!(
        unclassified,
        Vec::<&(String, String)>::new(),
        "classify these routes in ROUTES (Swept / ReadOnly) or EXEMPT"
    );

    // And the other direction: a row for a route that no longer exists is a lie about what
    // the sweep covers, and it would let a real route go unclassified by accident.
    let stale: Vec<_> = classified.difference(&registered).collect();
    assert_eq!(
        stale,
        Vec::<&(String, String)>::new(),
        "these rows name routes that are not registered"
    );

    // The table is a set, so a duplicate row would silently shrink the classification.
    assert_eq!(
        classified.len(),
        table().len(),
        "the table has a duplicate (method, path) row"
    );

    // A `Swept` row without a refusal cannot be driven, and the second test would skip it.
    for row in table().iter().filter(|row| row.class == Class::Swept) {
        assert!(
            row.refusal.is_some(),
            "{} {} is swept but names no refusal",
            row.method,
            row.path
        );
        assert!(
            !row.feature.is_empty(),
            "{} {} is swept but names no feature",
            row.method,
            row.path
        );
    }
    // A `ReadOnly` or `Exempt` row must say why, or the table stops explaining itself.
    for row in table().iter().filter(|row| row.class != Class::Swept) {
        assert!(
            !row.reason.is_empty(),
            "{} {} is not swept and gives no reason",
            row.method,
            row.path
        );
    }
}

/// Every route the spend cap covers is also a `Swept` row.
///
/// Two lists of "the routes that spend" would drift, and the drift has one shape: a
/// route the sweep classifies but the cap forgets is a route whose burst can overshoot
/// the provider-cost ceiling, because the ceiling is checked before the provider call
/// and the cost is recorded after it (A155). The cap's list is therefore a *subset* of
/// the swept rows, not a second table: the entitlement decision stays the one place
/// that decides whether an action is allowed, and the cap only decides how fast.
#[test]
fn the_spend_cap_covers_only_swept_routes() {
    let swept: BTreeSet<(&str, &str)> = table()
        .iter()
        .filter(|row| row.class == Class::Swept)
        .map(|row| (row.method, row.path))
        .collect();

    for (method, path) in stvdy_server::ratelimit::spending_routes() {
        assert!(
            swept.contains(&(*method, *path)),
            "{method} {path} is capped as a spending route but the sweep does not classify it as Swept"
        );
    }

    // And the cap must not be empty: a list that named nothing would pass the loop
    // above while capping nothing at all.
    assert!(
        !stvdy_server::ratelimit::spending_routes().is_empty(),
        "the spend cap covers no route"
    );
}

/// Every swept route refuses an exhausted account with the exact code the table names.
/// The account is exhausted by direct writes rather than through the API, because the API
/// would refuse the very requests that fill it. Every limit is at its cap and the balance is
/// negative, so the *first* refusal the engine produces is the one asserted here: a counted
/// feature refuses with `limit_reached` before it prices anything, which is why the two
/// credit-only routes are the only `insufficient_credits` rows.
#[sqlx::test(migrations = "../../migrations")]
async fn every_swept_route_refuses_an_exhausted_account(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "exhausted@example.test").await;

    let fixture = Fixture::build(&harness, "exhausted@example.test").await;

    for row in table().iter().filter(|row| row.class == Class::Swept) {
        let (status, code) = row.refusal.expect("a swept row names its refusal");
        let response = fixture.call(&app, &token, row.method, row.path).await;

        assert_eq!(
            response.0,
            StatusCode::from_u16(status).unwrap(),
            "{} {} ({}) should answer {status} {code}, got {} {}",
            row.method,
            row.path,
            row.feature,
            response.0,
            response.1
        );
        assert_eq!(
            response.1["code"], code,
            "{} {} ({}) should answer {code}: {}",
            row.method, row.path, row.feature, response.1
        );
        assert!(
            response.1["message"].is_string(),
            "{} {} ({}) must explain itself: {}",
            row.method,
            row.path,
            row.feature,
            response.1
        );
    }

    // The balance case the exhausted account cannot reach: its caps are full, so a counted
    // feature refuses before the price is considered. This is the same route with the caps
    // free and the balance spent, which is the other half of the chat row's table entry.
    fixture.free_the_caps().await;
    let (status, body) = fixture
        .call(&app, &token, "POST", "/workspaces/{id}/chat")
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "insufficient_credits", "{body}");
}

/// The ids and counters the exhaustion test drives, all written directly so the account can
/// be put in a state the API would refuse to reach.
struct Fixture {
    pool: PgPool,
    workspace_id: Uuid,
    source_id: Uuid,
    report_id: Uuid,
    cheatsheet_id: Uuid,
    user_id: Uuid,
}

impl Fixture {
    async fn build(harness: &TestState, email: &str) -> Self {
        let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
            .bind(email)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();

        let plans = &harness.state.config.plans;
        let free = plans.get(PlanId::Free);
        let workspaces = free.workspaces.max().expect("free caps workspaces") as i64;
        let sources = free.sources_total.max().expect("free caps sources") as i64;
        let quizzes = free.quizzes_per_month.max().expect("free caps quizzes") as i64;
        let reports = free.reports_per_month.max().expect("free caps reports") as i64;
        let flashcards = free
            .flashcards_per_month
            .max()
            .expect("free caps flashcards") as i64;
        let chat = free.chat_msgs_per_day.max().expect("free caps messages") as i64;
        let searches = i64::from(free.web_searches_per_day);

        let workspace_id = Uuid::now_v7();
        let source_id = Uuid::now_v7();
        let report_id = Uuid::now_v7();
        let cheatsheet_id = Uuid::now_v7();

        // The workspace the routes address, plus enough filler to reach the cap.
        for index in 0..workspaces {
            let id = if index == 0 {
                workspace_id
            } else {
                Uuid::now_v7()
            };
            sqlx::query("insert into workspaces (id, user_id, name) values ($1, $2, $3)")
                .bind(id)
                .bind(user_id)
                .bind(format!("Workspace {index}"))
                .execute(&harness.state.pool)
                .await
                .unwrap();
        }

        // One ready source the generation routes can draw on, with a chunk so the outline's
        // chunk load is not empty, plus filler to reach `sources_total`.
        for index in 0..sources {
            let id = if index == 0 {
                source_id
            } else {
                Uuid::now_v7()
            };
            sqlx::query(
                "insert into sources
                    (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
                 values ($1, $2, $3, 'text', $4, $5, 16, 'text/plain', 'ready')",
            )
            .bind(id)
            .bind(user_id)
            .bind(workspace_id)
            .bind(format!("Source {index}"))
            .bind(format!("users/{user_id}/sources/{id}.txt"))
            .execute(&harness.state.pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "insert into chunks (source_id, idx, content, content_hash, embedder)
             values ($1, 0, 'the material the generator reads', 'hash', 'test')",
        )
        .bind(source_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

        // The monthly counters, in the rows their own queries read.
        sqlx::query(
            "insert into decks (user_id, workspace_id, title, source_ids, status, requested_count)
             values ($1, $2, 'Reserved', array[$3], 'generating', $4)",
        )
        .bind(user_id)
        .bind(workspace_id)
        .bind(source_id)
        .bind(flashcards as i32)
        .execute(&harness.state.pool)
        .await
        .unwrap();

        for index in 0..quizzes {
            sqlx::query(
                "insert into quizzes
                    (user_id, workspace_id, title, difficulty, status, source_ids,
                     requested_count, question_types)
                 values ($1, $2, $3, 'medium', 'ready', array[$4], 5, 'mcq')",
            )
            .bind(user_id)
            .bind(workspace_id)
            .bind(format!("Quiz {index}"))
            .bind(source_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
        }

        // The report the section route addresses: a draft with an outline, which is what
        // makes index 0 a valid section to ask for.
        for index in 0..reports {
            let id = if index == 0 {
                report_id
            } else {
                Uuid::now_v7()
            };
            sqlx::query(
                "insert into reports
                    (id, user_id, workspace_id, title, status, source_ids, style, length, outline)
                 values ($1, $2, $3, $4, 'draft', array[$5], 'academic', 'medium',
                         '{\"sections\": [{\"heading\": \"Pendahuluan\"}]}'::jsonb)",
            )
            .bind(id)
            .bind(user_id)
            .bind(workspace_id)
            .bind(format!("Report {index}"))
            .bind(source_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
        }

        let cheatsheet_id = {
            sqlx::query(
                "insert into cheatsheets
                    (id, user_id, workspace_id, title, scope, depth, source_ids, status)
                 values ($1, $2, $3, 'Sheet', 'Scope', 'concise', array[$4], 'ready')",
            )
            .bind(cheatsheet_id)
            .bind(user_id)
            .bind(workspace_id)
            .bind(source_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
            cheatsheet_id
        };

        // The daily counters, at their caps, and a balance that cannot pay for anything.
        for _ in 0..chat {
            sqlx::query(
                "insert into usage_events (user_id, action, credits) values ($1, 'chat_message', 1)",
            )
            .bind(user_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
        }
        for _ in 0..searches {
            sqlx::query(
                "insert into usage_events (user_id, action, credits, cache_hit)
                 values ($1, 'web_search', 2, false)",
            )
            .bind(user_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "insert into credit_ledger (user_id, delta, bucket, reason)
             values ($1, -1000, 'monthly', 'test')",
        )
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

        Self {
            pool: harness.state.pool.clone(),
            workspace_id,
            source_id,
            report_id,
            cheatsheet_id,
            user_id,
        }
    }

    /// Clears the daily counters so the chat route reaches its price instead of its cap.
    async fn free_the_caps(&self) {
        sqlx::query("delete from usage_events where user_id = $1")
            .bind(self.user_id)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn call(
        &self,
        app: &axum::Router,
        token: &str,
        method: &str,
        path: &str,
    ) -> (StatusCode, serde_json::Value) {
        let workspace = self.workspace_id.to_string();
        let report = self.report_id.to_string();
        let cheatsheet = self.cheatsheet_id.to_string();
        let source = self.source_id.to_string();

        match (method, path) {
            ("POST", "/workspaces") => {
                json(
                    app,
                    "POST",
                    "/workspaces",
                    token,
                    serde_json::json!({ "name": "Nope" }),
                )
                .await
            }
            ("POST", "/workspaces/{id}/chat") => {
                json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/chat"),
                    token,
                    serde_json::json!({ "message": "hello" }),
                )
                .await
            }
            ("POST", "/workspaces/{id}/decks") => {
                json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/decks"),
                    token,
                    serde_json::json!({
                        "title": "Deck",
                        "source_ids": [source],
                        "count": 10
                    }),
                )
                .await
            }
            ("POST", "/workspaces/{id}/quizzes") => {
                json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/quizzes"),
                    token,
                    serde_json::json!({
                        "title": "Quiz",
                        "source_ids": [source],
                        "count": 10,
                        "difficulty": "medium",
                        "types": "mcq"
                    }),
                )
                .await
            }
            ("POST", "/workspaces/{id}/sources") => {
                let (content_type, body) =
                    multipart(&[("kind", "text", None), ("text", "a note", None)]);
                post_json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/sources"),
                    token,
                    Some(&content_type),
                    body,
                )
                .await
            }
            ("POST", "/workspaces/{id}/reports/outline") => {
                json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/reports/outline"),
                    token,
                    serde_json::json!({
                        "topic": "Jaringan",
                        "style": "academic",
                        "length": "medium",
                        "source_ids": [source]
                    }),
                )
                .await
            }
            ("POST", "/reports/{id}/generate") => {
                json(
                    app,
                    "POST",
                    &format!("/reports/{report}/generate"),
                    token,
                    serde_json::json!({ "index": 0 }),
                )
                .await
            }
            ("GET", "/reports/{id}/export") => {
                json(
                    app,
                    "GET",
                    &format!("/reports/{report}/export?format=pdf"),
                    token,
                    serde_json::Value::Null,
                )
                .await
            }
            ("POST", "/workspaces/{id}/cheatsheets") => {
                json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/cheatsheets"),
                    token,
                    serde_json::json!({
                        "scope": "UTS",
                        "depth": "normal",
                        "source_ids": [source]
                    }),
                )
                .await
            }
            ("GET", "/cheatsheets/{id}/pdf") => {
                json(
                    app,
                    "GET",
                    &format!("/cheatsheets/{cheatsheet}/pdf"),
                    token,
                    serde_json::Value::Null,
                )
                .await
            }
            other => panic!("the sweep test has no request for {other:?}"),
        }
    }
}

/// A plan limit read anywhere but the entitlements layer is a second source of truth, and
/// the way it appears is a handler reaching for the matrix "just this once".
///
/// The allowed readers each have one job: `entitlements.rs` builds the snapshot and the
/// view, `limits.rs` is the credit spend's in-transaction check, `credits.rs` reads the cost
/// table and the daily caps, and `sources/routes.rs` sizes the HTTP body limit (which is not
/// an entitlement decision: it stops a body no plan could accept from being read at all).
/// Everything else must go through `entitlements::require`.
#[test]
fn no_file_outside_the_entitlements_layer_reads_a_plan_limit() {
    /// The readers that may name a plan limit, with the reason. Matched by path suffix,
    /// because two files are called `routes.rs`.
    const ALLOWED: &[(&str, &str)] = &[
        ("/entitlements.rs", "the snapshot and the limits view"),
        ("/limits.rs", "the credit spend's in-transaction cap check"),
        ("/credits.rs", "the cost table and the daily caps"),
        (
            "/usage.rs",
            "the plan/usage view: the limits the screen renders (A159)",
        ),
        (
            "/sources/routes.rs",
            "the HTTP body limit, not an entitlement",
        ),
    ];

    /// Every field of `PlanConfig` that is a limit or a feature rather than a label.
    const LIMITS: &[&str] = &[
        "workspaces",
        "sources_total",
        "max_file_mb",
        "storage_mb",
        "devices_max",
        "credits_per_month",
        "chat_msgs_per_day",
        "web_searches_per_day",
        "flashcards_per_month",
        "quizzes_per_month",
        "reports_per_month",
        "syllabus_imports_total",
        "google_accounts_max",
        "export_formats",
        "task_ai_breakdown",
        "quick_capture",
        "google_calendar",
        "agent_actions",
        "reminder_channels",
        "model_tier",
        "byok",
        "advanced_analytics",
    ];

    let mut violations = Vec::new();
    for file in source_files() {
        let path = file.to_string_lossy();
        if ALLOWED.iter().any(|(allowed, _)| path.ends_with(allowed)) {
            continue;
        }
        let source = std::fs::read_to_string(&file).expect("a readable source file");
        for field in plan_limit_reads(&source) {
            if LIMITS.contains(&field.as_str()) {
                violations.push(format!("{}: plans.get(..).{field}", file.display()));
            }
        }

        // The provider cost ceilings have exactly one reader in this phase (A148), and it
        // is the credits module. A second one would be a second answer to "how much have
        // they spent".
        if source.contains("provider_cost_ceiling_usd") {
            violations.push(format!(
                "{}: reads provider_cost_ceiling_usd outside credits.rs",
                file.display()
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "these read a plan limit directly; go through entitlements::require:\n{}",
        violations.join("\n")
    );
}

/// The field names read off the matrix in `source`: every `plans.get(<x>).<field>`.
///
/// Hand-written for the same reason as the route extractor: the argument is a parenthesised
/// expression, and a regex would have to be right about it.
fn plan_limit_reads(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let needle = b"plans.get(";
    let mut found = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index..].starts_with(needle) {
            let start = index + needle.len();
            let end = matching_paren(bytes, start);
            let mut after = end + 1;
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if bytes.get(after) == Some(&b'.') {
                let field_start = after + 1;
                let mut field_end = field_start;
                while field_end < bytes.len()
                    && (bytes[field_end].is_ascii_alphanumeric() || bytes[field_end] == b'_')
                {
                    field_end += 1;
                }
                found.push(source[field_start..field_end].to_string());
            }
            index = end + 1;
        } else {
            index = skip_token(bytes, index);
        }
    }

    found
}
