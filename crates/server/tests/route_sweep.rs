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
    /// A route whose work is per mutation rather than per request: the batch itself is
    /// always admitted, and the refusal is a per-mutation result. `/sync/push` is the only
    /// one, because all six pushable entities are free (R1) and a `Message` or a `Source`
    /// in the batch is refused individually rather than failing the request.
    SweptPerMutation,
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
        ro(
            "GET",
            "/sync/pull",
            "reads the caller's own change log; a pull that needed a plan would break account sync on free",
        ),
        ro(
            "GET",
            "/sync/stream",
            "an SSE hint; it reads the caller's own clock and spends nothing",
        ),
        ro("GET", "/tasks", "reads the caller's own tasks"),
        ro("POST", "/tasks", "creating a task spends nothing"),
        // Phase 11. Goals are not plan-gated: the free plan has goals and a **manual**
        // planner, so only the two generating routes carry a refusal. `GET /plans/{id}` and
        // `POST /plans/accept` are reads of the caller's own proposal — the two credits were
        // charged at plan time (A264).
        ro("GET", "/goals", "reads the caller's own goals"),
        ro(
            "POST",
            "/goals",
            "creating a goal spends nothing; the planner is the gated part",
        ),
        ro("GET", "/goals/{id}", "reads the caller's own goal"),
        ro("PATCH", "/goals/{id}", "editing a goal spends nothing"),
        ro(
            "DELETE",
            "/goals/{id}",
            "deleting frees the anchor, not a slot",
        ),
        ro(
            "GET",
            "/goals/{id}/progress",
            "computed from the caller's own attempts",
        ),
        ro("GET", "/plans/{id}", "reads the caller's own proposal"),
        ro(
            "POST",
            "/plans/accept",
            "the 2 credits were charged at plan time",
        ),
        ro("PATCH", "/tasks/{id}", "editing a task spends nothing"),
        ro("DELETE", "/tasks/{id}", "deleting a task spends nothing"),
        ro("GET", "/events", "reads the caller's own events"),
        ro("POST", "/events", "creating an event spends nothing"),
        ro("PATCH", "/events/{id}", "editing an event spends nothing"),
        ro("DELETE", "/events/{id}", "deleting an event spends nothing"),
        ro(
            "GET",
            "/calendar/ics-token",
            "reads the caller's own feed token to render the settings screen's link; a read, so an over-limit account keeps it (A149/A242)",
        ),
        ro(
            "POST",
            "/calendar/ics-token/regenerate",
            "rotating the feed token grants nothing and spends nothing",
        ),
        ro(
            "GET",
            "/syllabus/{id}/draft",
            "reads the caller's own draft; another account's is a 404",
        ),
        ro(
            "POST",
            "/syllabus/{id}/confirm",
            "the import was already counted and charged at upload, so confirming writes rows \
             and spends nothing",
        ),
        ro(
            "POST",
            "/calendar/ics",
            "an ICS connection is available on free (the matrix's `ICS feed + import` row), \
             so refusing this would refuse the feature the plan says every plan has",
        ),
        ro(
            "GET",
            "/calendar/connections",
            "reads the caller's own connections (A249); a read, so an over-limit account keeps it (A149)",
        ),
        ro(
            "DELETE",
            "/calendar/connections/{id}",
            "deleting a connection is the remedy a connection refusal names, and it must work \
             for an account whose plan lapsed",
        ),
        ro("GET", "/sessions", "reads the caller's own sessions"),
        ro("POST", "/sessions", "a session is free (R1), like a review"),
        ro("GET", "/activity", "reads the caller's own activity"),
        // The dashboard's ungated half (A289). Every figure is a count or a sum over the
        // caller's own rows, so there is nothing a plan could refuse. The *advanced* half is
        // `Swept` below, which is why the dashboard is two routes rather than one: the screen
        // renders the free part and a locked card for the rest.
        ro(
            "GET",
            "/dashboard",
            "reads the caller's own sessions, reviews and due count",
        ),
        ro("GET", "/settings", "reads the caller's own settings"),
        // Phase 12's action surface. Both of these are reads in the sweep's sense: the list
        // reads the caller's own proposals, and a rejection spends nothing and frees the
        // student from a card. Reject is deliberately **not** plan-gated — a downgraded
        // account that can no longer confirm a proposal must still be able to dismiss it, or
        // the card is stuck on screen forever. The confirm is `Swept` below, because that is
        // the one that acts.
        ro(
            "GET",
            "/actions/pending",
            "reads the caller's own proposals, and expires its own stale ones",
        ),
        ro(
            "POST",
            "/actions/{id}/reject",
            "rejecting spends nothing and is the remedy a stuck card needs",
        ),
        ro(
            "PUT",
            "/settings",
            "a channel the plan does not grant is refused per channel by the engine, which \
             crates/server/tests/settings.rs asserts with a body (A209)",
        ),
        // Phase 12's reminder surface. All three are `ReadOnly` in the sweep's sense, and the
        // reason they can be is A201: the *channel* grant is checked once, on `PUT /settings`,
        // and every plan grants at least one channel — so there is no request here that an
        // exhausted account could be refused on. A `Swept` row would need a refusal to assert,
        // and inventing one would be inventing a policy the matrix does not have.
        ro(
            "GET",
            "/reminders",
            "reads the caller's own due reminders, filtered by this device's acks",
        ),
        ro(
            "POST",
            "/reminders",
            "registers a reminder for the caller's own subject; the key makes a retry a no-op \
             and no credit is spent",
        ),
        ro(
            "POST",
            "/reminders/{id}/ack",
            "one delivery row for the calling device, `on conflict do nothing`",
        ),
        // BYOK (A302). `GET` and `DELETE` are read-only in the sweep's sense: removing a key
        // is the remedy a refusal names, and a read of one's own key description must work for
        // an account whose plan no longer grants BYOK — that is exactly when a student needs
        // to see what is stored. `PUT` is the one that is gated, by the plan *and* the
        // platform, and both are asserted in tests/byok.rs.
        ro(
            "GET",
            "/settings/api-key",
            "reads whether the caller has a key, never the key",
        ),
        ro(
            "DELETE",
            "/settings/api-key",
            "removing one's own key is a remedy, not a spend",
        ),
        ro(
            "PUT",
            "/settings/api-key",
            "the plan gate (Feature::Byok) and the platform refusal are asserted in \
             tests/byok.rs, the same way tests/settings.rs asserts the channel refusal (A302)",
        ),
        // Phase 13's billing routes. All `ReadOnly`, and the reason is the same for all of
        // them: **buying is the remedy an exhausted account needs**, so gating checkout on
        // a plan or a balance would lock the door the refusal points at. The platform rule
        // is a different check (`checkout_allowed`), and `tests/billing.rs` drives it.
        ro(
            "GET",
            "/plans",
            "reads the matrix the caller's own screen shows; a locked plan must still be \
             visible to be offered",
        ),
        ro(
            "POST",
            "/billing/checkout",
            "buying is the remedy a refusal names; the platform refusal (A151) and the \
             verified-email rule (A43) are asserted in tests/billing.rs",
        ),
        ro(
            "GET",
            "/billing/payments/{id}",
            "reads the caller's own payment row, scoped by user_id in the statement",
        ),
        ro(
            "GET",
            "/billing/invoices",
            "reads the caller's own payment history; `payments` is not synced, so there is \
             no client-side substitute for it (docs/SPEC.md line 137)",
        ),
        ro(
            "POST",
            "/billing/upgrade",
            "an upgrade is money, not a plan feature: the target plan is priced by \
             upgrade_delta and the platform rule is checked inside periods::upgrade",
        ),
        ro(
            "POST",
            "/billing/downgrade",
            "a downgrade records an intent and changes nothing until the period end (A296); \
             refusing it would trap a student on a plan they cannot leave",
        ),
        ro(
            "POST",
            "/billing/student/verify",
            "sends a verification link; the suffix list bounds the content and the rate \
             limiter bounds the volume (A299)",
        ),
        ro(
            "POST",
            "/billing/student/confirm",
            "spends a single-use token the caller holds; the token is the authorisation",
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
        // Phase 12. The confirm is the only thing in the product that turns a proposal into a
        // row (A271), so it is the action the sweep must cover. The plan check runs **before**
        // the row is looked up (A272), which is why the driver arm needs no per-row body: a
        // free account meets the matrix's `plan_required` whatever the id names.
        swept(
            "POST",
            "/actions/{id}/confirm",
            "AgentAction",
            403,
            "plan_required",
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
        // Both are refused before the model is reached, and both are refused by the matrix
        // rather than by a literal here: free has `quick_capture` and `task_ai_breakdown`
        // false, so the engine's `plan_required` is what an exhausted free account meets.
        swept(
            "POST",
            "/tasks/quick-capture",
            "QuickCapture",
            403,
            "plan_required",
        ),
        swept(
            "POST",
            "/tasks/{id}/breakdown",
            "TaskBreakdown",
            403,
            "plan_required",
        ),
        // Google is the only calendar provider a plan gates: free has `google_calendar =
        // false` and the ICS feed and import are available on every plan (the matrix's own
        // row), which is why `POST /calendar/ics` and the connection delete are `ReadOnly`
        // below and this one is not.
        swept(
            "POST",
            "/calendar/sync",
            "GoogleCalendar",
            403,
            "plan_required",
        ),
        // Task 6's consent flow. Free has `google_calendar = false` **and**
        // `google_accounts_max = 0`, and the route checks the boolean first (A245), so an
        // exhausted free account meets `plan_required` rather than "0 of 0 accounts". The
        // route answers 404 when the OAuth client is unset, which is why the test config
        // carries one — `TestState::without_google_calendar` is the state that asserts the
        // 404, and it is the only test that should see it.
        swept(
            "GET",
            "/calendar/google/connect",
            "GoogleCalendar, then GoogleAccount (A245)",
            403,
            "plan_required",
        ),
        // The import is counted and priced: free allows one a month
        // (`syllabus_imports_total`), so an exhausted account meets the count's refusal
        // before the eight credits are considered.
        swept(
            "POST",
            "/workspaces/{id}/syllabus",
            "SyllabusImport",
            422,
            "limit_reached",
        ),
        // Phase 11. The plan generation is priced and gated: free's `planner = "manual"`
        // makes `Feature::PlanGeneration` false, so an exhausted free account meets
        // `plan_required` before the two credits are considered (A257).
        swept(
            "POST",
            "/goals/{id}/plan",
            "PlanGeneration",
            403,
            "plan_required",
        ),
        swept(
            "POST",
            "/plan/replan",
            "PlanGeneration",
            403,
            "plan_required",
        ),
        // The dashboard's advanced half (A289). `AdvancedAnalytics` is Power-only, so a free
        // account meets `plan_required` — which is the refusal the screen renders as a locked
        // card rather than hiding the section, and it is why the dashboard is two routes: the
        // ungated one above keeps working while this one is refused.
        swept(
            "GET",
            "/dashboard/topics",
            "AdvancedAnalytics",
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
            "GET",
            "/assets/{*path}",
            "no user: the fetch is headless Chromium printing a document, and every file is a \
             vendored upstream release (KaTeX, two typefaces) with no user data in it (A339). \
             There is no entitlement decision to sweep and no account to exhaust",
        ),
        exempt(
            "GET",
            "/cal/{token}",
            "token-authenticated: the path token is the credential and there is no bearer user, \
             like a signed object URL (A242). Registered as `{token}` because the router cannot \
             express the documented `.ics` suffix after a parameter; the handler requires it",
        ),
        exempt(
            "GET",
            "/calendar/google/callback",
            "no user: a browser redirect from Google carries no bearer token, exactly like \
             `/auth/google/callback` above. It acts only on an encrypted `state` this server \
             minted, and its refusal is a redirect to the app rather than an error status, so \
             there is no entitlement decision to sweep (Task 6/A235)",
        ),
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
        exempt(
            "GET",
            "/admin/payments",
            "AdminUser only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep",
        ),
        exempt(
            "POST",
            "/admin/payments/{id}/mark-paid",
            "AdminUser only, and it runs the webhook's own `apply_paid` rather than a second \
             activation path (A308); tests/admin_margin.rs asserts both the activation and the \
             404 a non-admin sees",
        ),
        exempt(
            "POST",
            "/admin/payments/{id}/refund",
            "AdminUser only: the extractor answers 404 for a non-admin (A7). A refund records \
             the row and leaves the subscription alone (A308)",
        ),
        exempt(
            "PATCH",
            "/admin/subscriptions/{user_id}",
            "AdminUser only: the extractor answers 404 for a non-admin (A7). A manual \
             extension recomputes the read-only flags through the same path the rollover uses",
        ),
        exempt(
            "POST",
            "/webhooks/qris",
            "the gateway's callback: no session and no bearer token, and the signature over \
             the raw body is the authorisation (A292/A309). Its refusals — 401 on a bad \
             signature, 404 on an unknown order, 502 with no gateway — are asserted in \
             tests/webhooks.rs",
        ),
    ]
}

fn table() -> Vec<Row> {
    let mut rows = read_only_rows();
    rows.extend(swept_rows());
    rows.extend(per_mutation_rows());
    rows.extend(exempt_rows());
    rows
}

/// Routes whose decision is per mutation, not per request.
fn per_mutation_rows() -> Vec<Row> {
    vec![Row {
        method: "POST",
        path: "/sync/push",
        class: Class::SweptPerMutation,
        reason: "a batch is admitted for any plan; the refusal is per mutation, and all six pushable entities are free (R1)",
        feature: "",
        refusal: None,
    }]
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

    // A per-mutation route is admitted for the exhausted account, and the refusal is a
    // result inside the batch rather than a status on the request: the task is free to
    // push (R1) and the message is server-owned, so one applied and one refused.
    for row in table()
        .iter()
        .filter(|row| row.class == Class::SweptPerMutation)
    {
        let (status, body) = fixture.call(&app, &token, row.method, row.path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} {} must admit the batch for any plan: {body}",
            row.method,
            row.path
        );
        let results = body["results"].as_array().expect("a results array");
        assert_eq!(results.len(), 2, "{body}");
        assert_eq!(
            results[0]["status"], "applied",
            "a free entity must still be applied: {body}"
        );
        assert_eq!(results[1]["status"], "rejected", "{body}");
        assert_eq!(
            results[1]["code"], "read_only_entity",
            "a server-owned entity is refused per mutation: {body}"
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
    task_id: Uuid,
    goal_id: Uuid,
    action_id: Uuid,
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
        let task_id = Uuid::now_v7();

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

        // The syllabus row `POST /workspaces/{id}/syllabus` asserts a refusal for.
        //
        // It has to be here, and it has to be `kind = 'syllabus'`, because that is what
        // `entitlements::snapshot` counts for `Feature::SyllabusImport` (`sources … and
        // kind = 'syllabus'`). Without it the count reads 0 against free's
        // `syllabus_imports_total = 1`, `counted` passes, and the refusal comes from
        // `affordable` instead — a 402 the sweep row does not assert, so the row fails. The
        // row's own comment claims the *count* refuses, and `counted` running before
        // `affordable` is what makes that true.
        //
        // `sources_total` is deliberately not the axis: free caps it at 20 and the loop above
        // already fills it, so `POST /workspaces/{id}/sources` keeps refusing on the count it
        // always did and this row does not change its reason.
        sqlx::query(
            "insert into sources
                (id, user_id, workspace_id, kind, title, file_key, file_size, mime, status)
             values ($1, $2, $3, 'syllabus', 'Silabus', $4, 16, 'text/plain', 'ready')",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("users/{user_id}/sources/syllabus.txt"))
        .execute(&harness.state.pool)
        .await
        .unwrap();

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

        // The task the break-down row addresses. Its own row is enough: the route refuses
        // on the plan before it reads the task, which is the order a counted refusal must
        // take.
        sqlx::query("insert into tasks (id, user_id, title) values ($1, $2, 'Tugas')")
            .bind(task_id)
            .bind(user_id)
            .execute(&harness.state.pool)
            .await
            .unwrap();

        // The goal the two plan routes address. Its own row is enough: the route refuses on
        // the plan before it reads the goal, which is the order a counted refusal must take.
        let goal_id = Uuid::now_v7();
        sqlx::query(
            "insert into goals (id, user_id, workspace_id, title, target_date)
             values ($1, $2, $3, 'UAS', now() + interval '21 days')",
        )
        .bind(goal_id)
        .bind(user_id)
        .bind(workspace_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();

        // The proposal the confirm row addresses. Its own row is enough for the same reason
        // the task's is: `Feature::AgentAction` is decided before the id is looked up (A272),
        // so an exhausted free account meets the refusal whatever this id names.
        let action_id = Uuid::now_v7();
        sqlx::query(
            "insert into pending_actions (id, user_id, tool, args, status, expires_at)
             values ($1, $2, 'create_task', '{\"title\": \"Tugas\"}'::jsonb, 'pending',
                     now() + interval '30 minutes')",
        )
        .bind(action_id)
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
            task_id,
            goal_id,
            action_id,
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
        let task = self.task_id.to_string();
        let action = self.action_id.to_string();

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
            ("POST", "/tasks/quick-capture") => {
                json(
                    app,
                    "POST",
                    "/tasks/quick-capture",
                    token,
                    serde_json::json!({ "text": "selesaikan tugas kripto hari Jumat" }),
                )
                .await
            }
            ("POST", "/tasks/{id}/breakdown") => {
                json(
                    app,
                    "POST",
                    &format!("/tasks/{task}/breakdown"),
                    token,
                    serde_json::json!({}),
                )
                .await
            }
            // Phase 12. Refused before the proposal is read, which is the order A272 pins:
            // `Feature::AgentAction` is free's `agent_actions = false`, so an exhausted free
            // account meets `plan_required` whatever this id names. The id is a real row the
            // fixture wrote, so a route that read it before deciding would still get past that
            // read rather than 404 first.
            ("POST", "/actions/{id}/confirm") => {
                json(
                    app,
                    "POST",
                    &format!("/actions/{action}/confirm"),
                    token,
                    serde_json::json!({}),
                )
                .await
            }
            // Phase 11. Both are refused before the goal is read: the refusal is the plan
            // matrix's, which is what an exhausted free account meets. The goal id is a
            // real one the fixture wrote, so a route that read it before deciding would
            // still get past that read rather than 404 first.
            ("POST", "/goals/{id}/plan") => {
                let goal = self.goal_id.to_string();
                json(
                    app,
                    "POST",
                    &format!("/goals/{goal}/plan"),
                    token,
                    serde_json::Value::Null,
                )
                .await
            }
            ("POST", "/plan/replan") => {
                let goal = self.goal_id.to_string();
                json(
                    app,
                    "POST",
                    "/plan/replan",
                    token,
                    serde_json::json!({ "goal_id": goal }),
                )
                .await
            }
            // The dashboard's advanced half. A plain authenticated GET: the refusal is the
            // plan matrix's, and it fires before any row is read, so there is no body and no
            // fixture id to thread through (A289).
            ("GET", "/dashboard/topics") => {
                json(
                    app,
                    "GET",
                    "/dashboard/topics",
                    token,
                    serde_json::Value::Null,
                )
                .await
            }
            ("POST", "/workspaces/{id}/syllabus") => {
                // The refusal has to come from the count, not from the multipart read, so the
                // body is a valid one: an exhausted account must meet `limit_reached` rather
                // than a 400 about a missing field.
                let (content_type, body) = multipart(&[("text", "Kuliah Senin 08:00", None)]);
                post_json(
                    app,
                    "POST",
                    &format!("/workspaces/{workspace}/syllabus"),
                    token,
                    Some(&content_type),
                    body,
                )
                .await
            }
            ("POST", "/calendar/sync") => {
                // The refusal is on the plan, before any connection is read, so the body is
                // irrelevant to the answer — which is the order a counted refusal must take.
                json(&app, "POST", "/calendar/sync", token, serde_json::json!({})).await
            }
            // The consent flow's first leg. A plain authenticated GET: the refusal is the
            // plan matrix's and it fires before anything is minted, so there is no body and
            // no fixture id to thread through.
            ("GET", "/calendar/google/connect") => {
                json(
                    app,
                    "GET",
                    "/calendar/google/connect",
                    token,
                    serde_json::Value::Null,
                )
                .await
            }
            ("POST", "/sync/push") => {
                let device_id: Uuid = sqlx::query_scalar(
                    "select id from devices where user_id = $1 and revoked_at is null",
                )
                .bind(self.user_id)
                .fetch_one(&self.pool)
                .await
                .unwrap();
                let client_ts = chrono::Utc::now().to_rfc3339();
                json(
                    app,
                    "POST",
                    "/sync/push",
                    token,
                    serde_json::json!({
                        "device_id": device_id,
                        "mutations": [
                            {
                                "mutation_id": Uuid::now_v7(),
                                "entity": "task",
                                "op": "upsert",
                                "id": Uuid::now_v7(),
                                "payload": {
                                    "patch": { "title": "Tugas", "status": "todo", "priority": 2 },
                                    "base": null
                                },
                                "client_ts": client_ts,
                            },
                            {
                                "mutation_id": Uuid::now_v7(),
                                "entity": "message",
                                "op": "upsert",
                                "id": Uuid::now_v7(),
                                "payload": { "content": "hi" },
                                "client_ts": client_ts,
                            }
                        ]
                    }),
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
    }

    // The provider cost ceiling has **exactly one reader** (A148), and it is `credits.rs`.
    //
    // This check is deliberately independent of `ALLOWED`: it scans every source file, so
    // the property is "one reader", not "no reader outside the entitlement layer". A
    // second reader of the config *field* would be a second answer to "how much have they
    // spent", which is the thing the ceiling's single enforcement point exists to prevent.
    //
    // A read for display is not a second reader: `GET /admin/margin` reports the bound
    // beside the spend so an admin can read a percentage rather than a bare dollar figure,
    // and it does so through `credits::ceiling_usd`, the accessor that owns the field. The
    // *comparison* still happens once, in `credits::spend_in` under the spend's lock.
    // A *read* is a field access — `.provider_cost_ceiling_usd` — not the name appearing in
    // prose, so a doc comment that names the field (which `admin.rs` and `credits.rs` both
    // do, to say who owns it) is not a second reader.
    const CEILING_FIELD: &str = ".provider_cost_ceiling_usd";
    let mut ceiling_readers = Vec::new();
    for file in source_files() {
        let source = std::fs::read_to_string(&file).expect("a readable source file");
        if source.contains(CEILING_FIELD) {
            ceiling_readers.push(file.display().to_string());
        }
    }
    let is_credits = |path: &str| path.ends_with("/credits.rs");
    // The positive half first, because it is what keeps the scan below non-vacuous: if the
    // config field were ever renamed, the loop would find no reader at all and pass while
    // checking nothing.
    assert!(
        ceiling_readers.iter().any(|path| is_credits(path)),
        "no file reads provider_cost_ceiling_usd, so the ceiling is configured but \
         unenforced (or the field was renamed and this scan now checks nothing): \
         {ceiling_readers:?}"
    );
    for path in &ceiling_readers {
        assert!(
            is_credits(path),
            "{path} reads provider_cost_ceiling_usd; the one reader is credits.rs, and \
             every other caller goes through credits::ceiling_usd"
        );
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
