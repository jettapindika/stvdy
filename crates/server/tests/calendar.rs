//! The calendar's four surfaces: the feed out, the ICS import, the consent flow and
//! the two-way sync. Split by surface, not by module, because each has a failure the
//! others cannot produce: a feed a client refuses, a private host reached, a token in a
//! deep link, a sync that stopped being incremental.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use parking_lot::Mutex;
use sqlx::PgPool;
use std::collections::VecDeque;
use std::sync::Arc;
use stvdy_core::traits::calendar::{
    AccessToken, CalendarApi, CalendarError, CalendarMeta, EventPage, RemoteEvent,
};
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::calendar::google::GoogleCalendarApi;
use stvdy_server::calendar::sync::{SyncOutcome, load, sync_google};
use stvdy_server::testing::{Scripted, ScriptedIcsFetch, TestState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;
use uuid::Uuid;

mod common;
use common::{json, signed_up, stub_http_json};

/// A request whose response headers and raw bytes are both read.
///
/// `common::json` parses the body and drops the headers, and the feed's contract *is* its
/// headers (`text/calendar`, `no-store`, `noindex`): a helper that only returned the body
/// would make the interesting half of the route unassertable.
async fn raw(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");

    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// One `events` row, written directly: the feed's job is to select rows, and going through
/// `POST /events` for each one would make this suite depend on Phase 9's validator.
#[allow(clippy::too_many_arguments)]
async fn insert_event(
    harness: &TestState,
    user_id: Uuid,
    kind: &str,
    title: &str,
    starts_at: &str,
    ends_at: &str,
    source: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into events (id, user_id, kind, title, starts_at, ends_at, source)
         values ($1, $2, $3, $4, $5::timestamptz, $6::timestamptz, $7)",
    )
    .bind(id)
    .bind(user_id)
    .bind(kind)
    .bind(title)
    .bind(starts_at)
    .bind(ends_at)
    .bind(source)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}

/// The feed validates, carries the caller's own rows, and a regenerated token retires the old
/// one. The last assertion is the one that matters: a revoked token that still works is not
/// revoked.
#[sqlx::test(migrations = "../../migrations")]
async fn the_feed_validates_and_a_regenerated_token_stops_working(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "feed@example.test").await;
    let user_id = user_id_of(&harness, "feed@example.test").await;

    // One of each kind the feed carries, one kind it does not, and one row that is not the
    // student's own (A241).
    insert_event(
        &harness,
        user_id,
        "exam",
        "UAS Jaringan Komputer",
        "2026-06-10T01:00:00Z",
        "2026-06-10T03:00:00Z",
        "app",
    )
    .await;
    insert_event(
        &harness,
        user_id,
        "study_block",
        "Blok belajar",
        "2026-06-11T09:00:00Z",
        "2026-06-11T10:00:00Z",
        "app",
    )
    .await;
    insert_event(
        &harness,
        user_id,
        "external",
        "Kuliah umum",
        "2026-06-12T09:00:00Z",
        "2026-06-12T10:00:00Z",
        "app",
    )
    .await;
    // A Google row: echoing it back would show the student's own Google event twice.
    insert_event(
        &harness,
        user_id,
        "class",
        "Rapat himpunan",
        "2026-06-13T09:00:00Z",
        "2026-06-13T10:00:00Z",
        "google",
    )
    .await;
    // A task with a due date, which the feed renders as an all-day deadline (A248).
    sqlx::query(
        "insert into tasks (id, user_id, title, due_at) values ($1, $2, $3, $4::timestamptz)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind("Kumpulkan laporan")
    .bind("2026-06-14T17:00:00Z")
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // The token does not exist until it is generated, and reading it says so rather than 404ing.
    let (status, body) = json(
        &app,
        "GET",
        "/calendar/ics-token",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["ics_feed_token"].is_null(), "{body}");

    let (status, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let first_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    assert!(
        first_token.len() >= 40,
        "a token must be long enough to be unguessable"
    );

    // Reading it back is the same value: the screen shows the link, it does not rotate it.
    let (_, read_back) = json(
        &app,
        "GET",
        "/calendar/ics-token",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(read_back["ics_feed_token"], first_token);

    let (status, headers, body) = raw(&app, "GET", &format!("/cal/{first_token}.ics"), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["content-type"], "text/calendar; charset=utf-8");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-robots-tag"], "noindex");
    assert!(body.contains("BEGIN:VCALENDAR"), "{body}");
    assert!(body.contains("UAS Jaringan Komputer"), "{body}");
    assert!(body.contains("Blok belajar"), "{body}");
    assert!(body.contains("Kumpulkan laporan"), "{body}");
    // `kind = 'external'` and every `source != 'app'` row are excluded (A241).
    assert!(!body.contains("Kuliah umum"), "{body}");
    assert!(!body.contains("Rapat himpunan"), "{body}");
    // The task's due date is an all-day deadline in the user's zone, not a timed event.
    assert!(body.contains("DTSTART;VALUE=DATE:20260615"), "{body}");
    // And the exam, which is an instant, is UTC.
    assert!(body.contains("DTSTART:20260610T010000Z"), "{body}");

    // Another account's token resolves to that account's rows and never this one's.
    let other = signed_up(&app, &harness, "other-feed@example.test").await;
    let (_, other_rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &other,
        serde_json::json!({}),
    )
    .await;
    let other_token = other_rotated["ics_feed_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(other_token, first_token);
    let (status, _, other_body) = raw(&app, "GET", &format!("/cal/{other_token}.ics"), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !other_body.contains("UAS Jaringan Komputer"),
        "{other_body}"
    );
    assert!(!other_body.contains("Kumpulkan laporan"), "{other_body}");

    // Regenerate retires the old token: this is the whole remedy A242 promises.
    let (_, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    let new_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    assert_ne!(new_token, first_token);
    assert_eq!(
        raw(&app, "GET", &format!("/cal/{first_token}.ics"), "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        raw(&app, "GET", &format!("/cal/{new_token}.ics"), "")
            .await
            .0,
        StatusCode::OK
    );

    // A token that is not a token is 404, not 500: the lookup is an index hit or a miss.
    assert_eq!(
        raw(&app, "GET", "/cal/nope.ics", "").await.0,
        StatusCode::NOT_FOUND
    );
    // And the documented suffix is required: one token has exactly one URL.
    assert_eq!(
        raw(&app, "GET", &format!("/cal/{new_token}"), "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        raw(&app, "GET", "/cal/.ics", "").await.0,
        StatusCode::NOT_FOUND
    );
}

/// The feed is a cache the client owns (A230), so two renders of an unchanged row must be
/// byte-identical. A `DTSTAMP` written from a clock reading, or a `UID` regenerated per
/// request, would make every poll a new document and every client re-import it — with no
/// error on either side.
#[sqlx::test(migrations = "../../migrations")]
async fn two_renders_of_an_unchanged_feed_are_byte_identical(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "stable-feed@example.test").await;
    let user_id = user_id_of(&harness, "stable-feed@example.test").await;

    let event = insert_event(
        &harness,
        user_id,
        "class",
        "Kriptografi",
        "2026-06-10T01:00:00Z",
        "2026-06-10T03:00:00Z",
        "app",
    )
    .await;
    let (_, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    let feed_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    let uri = format!("/cal/{feed_token}.ics");

    let (_, _, first) = raw(&app, "GET", &uri, "").await;
    let (_, _, second) = raw(&app, "GET", &uri, "").await;
    assert_eq!(first, second, "an unchanged feed must be byte-identical");
    assert!(first.contains(&format!("UID:{event}@stvdy.app")), "{first}");
    // A new row has never been edited, so `SEQUENCE:0` (A230).
    assert_eq!(sequence_of(&first), 0, "{first}");

    // An edit raises it. `updated_at` is touched by the trigger, so this is the same path a
    // real edit takes.
    sqlx::query("update events set title = $2 where id = $1")
        .bind(event)
        .bind("Kriptografi (revisi)")
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let (_, _, edited) = raw(&app, "GET", &uri, "").await;
    assert_ne!(edited, first);
    assert!(edited.contains("Kriptografi (revisi)"), "{edited}");
    assert!(
        sequence_of(&edited) >= 1,
        "an edit must move SEQUENCE off zero: {edited}"
    );
}

/// The `SEQUENCE` value the feed carries, parsed rather than pattern-matched: the assertion
/// is that an edit *raised* it, and `!contains("SEQUENCE:0")` would also pass on a feed that
/// rendered a different integer for an unrelated reason.
fn sequence_of(ics: &str) -> i64 {
    ics.split("\r\n")
        .find_map(|line| line.strip_prefix("SEQUENCE:"))
        .expect("a SEQUENCE line")
        .trim()
        .parse()
        .expect("SEQUENCE is an integer")
}

/// An ICS URL is imported with its recurrence, and a second poll sends the validator so an
/// unchanged feed touches nothing.
///
/// The second half is the one that matters: an unconditional re-upsert of a synced table
/// writes one `change_log` row per event per poll, and every device pulls the whole calendar
/// every fifteen minutes (A239). The assertion is therefore on the *change log*, not only on
/// the event count — a `304` that re-upserted identical rows would keep the count right and
/// still churn every device.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ics_url_is_imported_with_its_recurrence_and_a_poll_sends_the_validator(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // The clock is pinned, and that is not decoration: the import window starts at the user's
    // local day start, so a feed whose classes are in June would import **nothing** in October
    // and this test would pass for the wrong reason or fail for one that has nothing to do
    // with the code under test. A fixture date plus a fixed clock is the only combination that
    // keeps meaning the same thing next year.
    let harness = harness.with_clock(Arc::new(FixedClock::at(at("2026-06-01T00:00:00Z"))));

    // A weekly class with COUNT=8 and a one-off exam. Both in Jakarta, because a wrong `TZID`
    // resolution is the silent failure this suite exists for: 08:00+07:00 is 01:00Z, and a
    // build that treated the value as UTC would store 08:00Z and look perfectly reasonable.
    let feed = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                PRODID:-//faculty//EN\r\n\
                BEGIN:VEVENT\r\n\
                UID:kelas-jaringan@faculty.example\r\n\
                SUMMARY:UAS Jaringan Komputer\r\n\
                DTSTART;TZID=Asia/Jakarta:20260610T080000\r\n\
                DTEND;TZID=Asia/Jakarta:20260610T100000\r\n\
                RRULE:FREQ=WEEKLY;COUNT=8\r\n\
                END:VEVENT\r\n\
                BEGIN:VEVENT\r\n\
                UID:ujian-susulan@faculty.example\r\n\
                SUMMARY:Kuis susulan\r\n\
                DTSTART;TZID=Asia/Jakarta:20260615T130000\r\n\
                DTEND;TZID=Asia/Jakarta:20260615T140000\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    // The re-poll's body: the same feed with one title changed, so a re-poll that does nothing
    // is distinguishable from one that updated the row.
    let edited = feed.replace("UAS Jaringan Komputer", "UAS Jaringan Komputer (revisi)");

    // Three answers, in the order the three passes ask for them: the route's first import, the
    // scheduler's re-poll, and the manual sync's conditional GET.
    //
    // **One fetcher for all three, and that is load-bearing.** The job worker `drain_jobs`
    // spawns reads the fetcher off the *harness's* state, so a test that installs a second
    // fetcher for the re-poll leaves `fake.calls()` blind to that fetch — which is how this
    // case failed: the re-poll ran and wrote the row, while the assertion on `fake` still saw
    // one call.
    let fake = Arc::new(ScriptedIcsFetch::new(vec![
        Scripted::Body {
            body: feed.to_string(),
            etag: Some("\"v1\"".to_string()),
            last_modified: None,
        },
        // The re-poll: a changed body under the *same* validator, so the one stored etag is
        // still current when the manual sync asks again.
        Scripted::Body {
            body: edited.clone(),
            etag: Some("\"v1\"".to_string()),
            last_modified: None,
        },
        // The manual sync: the server answers 304 because the validators match.
        Scripted::NotModified,
    ]));
    let mut harness = harness.with_ics_fetch(fake.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "ics@example.test").await;
    // The ICS import itself is available on free; `POST /calendar/sync` is not, because it is
    // the manual *Google* sync (the matrix's `google_calendar` row). This test uses the manual
    // route as a second pass, so the account is pro.
    make_pro(&harness, "ics@example.test").await;

    let (status, created) = json(
        &app,
        "POST",
        "/calendar/ics",
        &token,
        serde_json::json!({ "url": "https://faculty.example/academic.ics" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let connection_id = created["connection"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["connection"]["provider"], "ics");
    assert_eq!(created["connection"]["status"], "connected");
    // A249: the view carries no token, no cursor and no URL.
    assert!(created["connection"].get("ics_url").is_none(), "{created}");
    assert!(
        created["connection"].get("sync_token").is_none(),
        "{created}"
    );
    assert!(
        created["connection"].get("refresh_token_enc").is_none(),
        "{created}"
    );

    common::drain_jobs(&harness).await;

    // Eight occurrences plus the exam, each its own row keyed by the occurrence (A231).
    let (status, events) = json(
        &app,
        "GET",
        "/events?from=2026-06-01T00:00:00Z&to=2026-08-31T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    let rows = events["events"].as_array().unwrap();
    assert_eq!(rows.len(), 9, "eight occurrences plus the exam: {events}");

    // The wall clock, resolved through `chrono_tz` and not through a fixed offset: 08:00 in
    // Jakarta is 01:00Z.
    let exam = rows
        .iter()
        .find(|row| row["title"] == "UAS Jaringan Komputer")
        .expect("the recurring class");
    assert_eq!(exam["starts_at"], "2026-06-10T01:00:00Z", "{exam}");
    assert_eq!(exam["kind"], "external", "{exam}");
    assert_eq!(exam["source"], "ics", "{exam}");

    // Read-only to a client on **both** write paths (A198/A240), which is Task 9's assertion:
    // the rule lives in the routes and the push validator, so a client meets it through either.
    let exam_id = exam["id"].as_str().unwrap();
    assert_eq!(
        json(
            &app,
            "PATCH",
            &format!("/events/{exam_id}"),
            &token,
            serde_json::json!({ "title": "x" })
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json(
            &app,
            "DELETE",
            &format!("/events/{exam_id}"),
            &token,
            serde_json::Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, pushed) = json(
        &app,
        "POST",
        "/sync/push",
        &token,
        // A **complete** patch: the field rules run before the ownership check, so a patch
        // missing `kind` would be refused for that reason instead, and the assertion below
        // would pass while testing nothing about ownership.
        serde_json::json!({
            "device_id": device_id_of(&harness, "ics@example.test").await,
            "mutations": [{
                "mutation_id": Uuid::now_v7(),
                "entity": "event",
                "op": "upsert",
                "id": exam_id,
                "payload": { "patch": {
                    "kind": "class",
                    "title": "x",
                    "starts_at": "2026-06-10T01:00:00Z",
                    "ends_at": "2026-06-10T03:00:00Z",
                }},
                "client_ts": chrono::Utc::now().to_rfc3339(),
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pushed}");
    assert_eq!(
        pushed["results"][0]["code"], "read_only_entity",
        "the push path must refuse it with the same code the REST path refuses with: {pushed}"
    );

    // And the sync's own write is **not** affected by that rule (A240), which is the half that
    // can silently break: a later refactor that "tidies" the check into a trigger or a shared
    // validator would refuse this too, and the failure would be a connection that imports once
    // and then holds a stale timetable forever.
    //
    // Two things have to be true before the re-poll can run, and both are properties of the
    // poll rather than of this test:
    //
    //  * the watermark must be stale, or `calendar_pass` correctly declines to examine the
    //    connection. It is written against the **injected** clock, not the database's, because
    //    the pass reads both its cutoff and its dedupe window from that one clock;
    //  * the clock must be in a **later** window. `jobs::enqueue_at`'s conflict clause is
    //    `do update set payload = excluded.payload` with no status reset, so enqueuing against
    //    this connection's already-completed job with the same `dedupe_key` updates that row
    //    and leaves it `done` — no new work. That is the same property the poll test proves
    //    from the other side, and here it would silently turn the re-poll into a no-op.
    harness.state.clock = Arc::new(FixedClock::at(at("2026-06-01T02:00:00Z")));
    sqlx::query("update calendar_connections set last_synced_at = $1")
        .bind(harness.state.clock.now() - chrono::Duration::hours(2))
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let rescheduled = stvdy_server::jobs::schedule::calendar_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(
        rescheduled, 1,
        "a stale watermark must put the connection back in the poll"
    );

    // The row the pass produced, named by the *second* window's key. Without this, a future
    // change to the key could make the re-poll fetch happen for a different reason — a manual
    // sync, a second connection — and the assertions below would still pass.
    let window = harness.state.clock.now().timestamp()
        / harness.state.config.calendar_poll_interval.as_secs().max(1) as i64;
    let second: Option<String> =
        sqlx::query_scalar("select status from jobs where dedupe_key = $1")
            .bind(format!("sync_calendar:{connection_id}:{window}"))
            .fetch_optional(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        matches!(second.as_deref(), Some("queued") | Some("done")),
        "the second window must have its own job, queued or done: {second:?}"
    );

    common::drain_jobs(&harness).await;
    let (_, events) = json(
        &app,
        "GET",
        "/events?from=2026-06-01T00:00:00Z&to=2026-08-31T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    let rows = events["events"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        9,
        "a re-poll updates rather than duplicates: {events}"
    );
    let renamed = rows
        .iter()
        .find(|row| row["id"] == exam_id)
        .expect("the same row");
    assert_eq!(
        renamed["title"], "UAS Jaringan Komputer (revisi)",
        "the sync must be able to write a row the routes refuse: {renamed}"
    );
    assert_eq!(renamed["source"], "ics");

    // The re-poll above and this manual sync both carry the stored validator, and a 304
    // changes nothing. `last_synced_at` is advanced even so, because the watermark is what
    // keeps the poll from re-enqueueing.
    let changes_before: i64 = sqlx::query_scalar(
        "select count(*) from change_log where user_id = (select id from users where email = $1)",
    )
    .bind("ics@example.test")
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // A manual sync is the same row the poll uses, and its own per-minute window (A238).
    let (status, queued) = json(
        &app,
        "POST",
        "/calendar/sync",
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert_eq!(queued["queued"], 1, "{queued}");
    common::drain_jobs(&harness).await;

    let calls = fake.calls();
    assert_eq!(calls.len(), 3, "import, re-poll and manual sync: {calls:?}");
    assert_eq!(calls[0].etag, None, "the first fetch is unconditional");
    assert_eq!(
        calls[1].etag.as_deref(),
        Some("\"v1\""),
        "the re-poll must send the stored validator: {calls:?}"
    );
    assert_eq!(
        calls[2].etag.as_deref(),
        Some("\"v1\""),
        "the manual sync must send the stored validator: {calls:?}"
    );

    let changes_after: i64 = sqlx::query_scalar(
        "select count(*) from change_log where user_id = (select id from users where email = $1)",
    )
    .bind("ics@example.test")
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        changes_after, changes_before,
        "a 304 must not touch a row: every touched row is a change_log entry every device pulls"
    );

    // And the watermark moved, so the poll will not re-enqueue inside the interval.
    let (_, connections) = json(
        &app,
        "GET",
        "/calendar/connections",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(connections["connections"].as_array().unwrap().len(), 1);
    assert!(
        connections["connections"][0]["last_synced_at"].is_string(),
        "{connections}"
    );
    assert_eq!(connections["connections"][0]["id"], connection_id);

    // Deleting the connection takes its events with it, because a row from a calendar the
    // student just removed is a row they cannot edit and no sync will ever update again.
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/calendar/connections/{connection_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, events) = json(
        &app,
        "GET",
        "/events?from=2026-06-01T00:00:00Z&to=2026-08-31T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        events["events"].as_array().unwrap().len(),
        0,
        "the connection's events must go with it: {events}"
    );
}

/// A private host is refused at save time with a readable reason, and at fetch time by the
/// guard (A246). The two checks do different jobs: this one is for the user, and the guard is
/// the protection, because DNS can change between them.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ics_url_that_is_not_fetchable_is_refused_when_it_is_saved(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "bad-ics@example.test").await;

    for url in [
        "http://169.254.169.254/feed.ics",
        "http://127.0.0.1/feed.ics",
        "file:///etc/passwd",
        "https://user:pw@faculty.example/feed.ics",
        "not a url",
    ] {
        let (status, body) = json(
            &app,
            "POST",
            "/calendar/ics",
            &token,
            serde_json::json!({ "url": url }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {body}");
        assert_eq!(body["code"], "validation", "{url}: {body}");
        assert!(body["message"].is_string(), "{url}: {body}");
    }

    // Nothing was written: a refused URL must not leave a connection that never syncs.
    let (_, connections) = json(
        &app,
        "GET",
        "/calendar/connections",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(connections["connections"].as_array().unwrap().len(), 0);
}

/// A truncated expansion keeps what it read and records the reason (A247, A232).
///
/// The assertion is on **both** halves: 500 rows stored (not zero, not an error page) and
/// `recurrence_truncated` on the connection. A pass that recorded the error and stored
/// nothing, or stored everything and recorded nothing, would each pass one half of this and
/// fail the student in a different way.
#[sqlx::test(migrations = "../../migrations")]
async fn a_truncated_recurrence_imports_what_it_read_and_records_the_error(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_clock(Arc::new(FixedClock::at(at("2026-06-01T00:00:00Z"))));

    // An endless rule inside the horizon: `FREQ=MINUTELY` with no UNTIL, so the occurrence cap
    // is what stops it. The bound is the whole point — without it this test hangs rather than
    // fails, which is the roadmap's named risk.
    let feed = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:endless@faculty.example\r\n\
                SUMMARY:Tanpa akhir\r\n\
                DTSTART;TZID=Asia/Jakarta:20260610T080000\r\n\
                DTEND;TZID=Asia/Jakarta:20260610T080500\r\n\
                RRULE:FREQ=MINUTELY\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    let harness = harness.with_ics_fetch(Arc::new(ScriptedIcsFetch::always(feed, None)));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "truncated@example.test").await;
    make_pro(&harness, "truncated@example.test").await;

    let (status, created) = json(
        &app,
        "POST",
        "/calendar/ics",
        &token,
        serde_json::json!({ "url": "https://faculty.example/endless.ics" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    common::drain_jobs(&harness).await;

    let stored: i64 = sqlx::query_scalar(
        "select count(*) from events where user_id = (select id from users where email = $1)
           and deleted_at is null",
    )
    .bind("truncated@example.test")
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        stored, 500,
        "a truncated import keeps every occurrence it read"
    );

    let (_, connections) = json(
        &app,
        "GET",
        "/calendar/connections",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        connections["connections"][0]["error"],
        "recurrence_truncated"
    );
    assert_eq!(connections["connections"][0]["status"], "error");

    // A clean pass clears it, which is the other half of A247: the error is a fact about the
    // last pass, not a permanent mark on the connection.
    let clean = "BEGIN:VCALENDAR\r\n\
                 VERSION:2.0\r\n\
                 BEGIN:VEVENT\r\n\
                 UID:one-off@faculty.example\r\n\
                 SUMMARY:Sekali saja\r\n\
                 DTSTART;TZID=Asia/Jakarta:20260611T080000\r\n\
                 DTEND;TZID=Asia/Jakarta:20260611T090000\r\n\
                 END:VEVENT\r\n\
                 END:VCALENDAR\r\n";
    let harness = harness.with_ics_fetch(Arc::new(ScriptedIcsFetch::always(clean, None)));
    let (_, queued) = json(
        &app,
        "POST",
        "/calendar/sync",
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(queued["queued"], 1, "{queued}");
    common::drain_jobs(&harness).await;

    let (_, connections) = json(
        &app,
        "GET",
        "/calendar/connections",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert!(
        connections["connections"][0]["error"].is_null(),
        "{connections}"
    );
    assert_eq!(connections["connections"][0]["status"], "connected");
}

/// The poll is a row with a windowed dedupe key (A238): a restart, a second instance or a
/// double tick is a no-op rather than a double sync. And the manual sync is the same row, so
/// a user holding the button cannot open a sync per press.
///
/// The assertion is on the *job count* rather than on a sync count, because that is the
/// property: the failure this catches is two rows, which produces two passes over one
/// connection and two full re-upserts of a synced table.
#[sqlx::test(migrations = "../../migrations")]
async fn the_poll_job_is_dedupe_guarded_and_a_manual_sync_is_rate_limited(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // A clock that does not advance, so two passes are genuinely inside one window. With a
    // live clock a pass that straddled a boundary would enqueue two jobs and this test would
    // fail for a reason that has nothing to do with the dedupe key.
    let harness = harness.with_clock(Arc::new(FixedClock::at(
        "2026-06-01T00:00:00Z".parse().unwrap(),
    )));
    let mut harness = harness.with_ics_fetch(Arc::new(ScriptedIcsFetch::always(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a@f\r\nSUMMARY:Kelas\r\n\
         DTSTART;TZID=Asia/Jakarta:20260602T080000\r\nDTEND;TZID=Asia/Jakarta:20260602T090000\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n",
        None,
    )));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "poll@example.test").await;

    // A pro account, because the manual route is gated on `google_calendar` (A245's neighbour:
    // `POST /calendar/sync` is the `Swept` row) and this test is about the dedupe key rather
    // than about the refusal, which `route_sweep` drives.
    make_pro(&harness, "poll@example.test").await;

    // The connection is written directly rather than through `POST /calendar/ics`, and that is
    // the point of this test: the route's own first import already consumes the poll's dedupe
    // key for this window, so a poll after it is a no-op for a reason that has nothing to do
    // with the guard under test. `last_synced_at` is back-dated to the state a poll actually
    // meets — a connection nobody has synced for two hours — and against the **injected**
    // clock, because the pass reads both its cutoff and its dedupe window from that clock.
    sqlx::query(
        "insert into calendar_connections (user_id, provider, ics_url, last_synced_at)
         select id, 'ics', 'https://faculty.example/academic.ics', $2
           from users where email = $1",
    )
    .bind("poll@example.test")
    .bind(harness.state.clock.now() - chrono::Duration::hours(2))
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // Two passes inside one window enqueue one job.
    let first = stvdy_server::jobs::schedule::calendar_pass(&harness.state)
        .await
        .unwrap();
    let second = stvdy_server::jobs::schedule::calendar_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(first, 1, "the first pass must pick the connection up");
    assert_eq!(
        second, 0,
        "the second pass must find the job already waiting"
    );
    assert_eq!(
        queued_jobs(&harness, "sync_calendar").await,
        1,
        "two passes in one window must enqueue one job"
    );

    // The handler runs it, moves the watermark, and re-enqueues nothing itself. The watermark
    // is the injected clock's instant, so it is read back against the same clock.
    common::drain_jobs(&harness).await;
    let synced: bool = sqlx::query_scalar("select last_synced_at > $1 from calendar_connections")
        .bind(harness.state.clock.now() - chrono::Duration::minutes(1))
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert!(synced, "the handler must advance the watermark");

    // A connection synced inside the interval is not even **examined**. The clock is moved one
    // poll interval on — which is also exactly the staleness boundary, because the poll's
    // interval and its dedupe window are the same length, and the strict `<` is what keeps a
    // connection synced at the boundary unexamined. A fresh key exists at that instant, so the
    // row count is what proves the connection was not selected: a selected connection inserts
    // a new row (the `not exists` guard only matches a *queued* key, and this one is `done`),
    // while a connection that was never examined leaves the count at one.
    let interval = harness.state.config.calendar_poll_interval.as_secs().max(1) as i64;
    harness.state.clock = Arc::new(FixedClock::at(
        harness.state.clock.now() + chrono::Duration::seconds(interval),
    ));
    let third = stvdy_server::jobs::schedule::calendar_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(third, 0, "a freshly synced connection must not be examined");
    let rows: i64 = sqlx::query_scalar("select count(*) from jobs where kind = 'sync_calendar'")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "no second job row may exist");

    // The manual route enqueues the same kind and answers 202.
    let (status, manual) = json(
        &app,
        "POST",
        "/calendar/sync",
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{manual}");
    assert_eq!(manual["queued"], 1, "{manual}");
    assert_eq!(
        json(
            &app,
            "POST",
            "/calendar/sync",
            &token,
            serde_json::json!({})
        )
        .await
        .1["queued"],
        0,
        "one manual sync per minute per connection"
    );

    // The handler updates the watermark, and the row it wrote is the one the feed carries.
    common::drain_jobs(&harness).await;
    let (_, connections) = json(
        &app,
        "GET",
        "/calendar/connections",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert!(
        connections["connections"][0]["last_synced_at"].is_string(),
        "{connections}"
    );

    // An unknown kind is still a permanent failure, not a panic: the calendar job is one arm
    // of the same dispatch, and a payload it cannot parse must not take the worker down.
    sqlx::query("insert into jobs (kind, payload) values ('sync_calendar', '{}'::jsonb)")
        .execute(&harness.state.pool)
        .await
        .unwrap();
    common::drain_jobs(&harness).await;
    let failed: i64 = sqlx::query_scalar(
        "select count(*) from jobs where kind = 'sync_calendar' and status = 'failed'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        failed, 1,
        "a payload with no connection_id is a permanent failure"
    );
}

/// How many `sync_calendar` jobs are waiting.
async fn queued_jobs(harness: &TestState, kind: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from jobs where kind = $1 and status in ('queued', 'running')",
    )
    .bind(kind)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// The instant a fixture date is written against.
fn at(raw: &str) -> chrono::DateTime<chrono::Utc> {
    raw.parse().expect("an RFC 3339 instant")
}

/// Puts an account on pro with a live subscription.
///
/// Both halves are needed and they are not interchangeable: `users.plan_id` alone is honoured
/// only while a subscription grants paid limits (A146), so an account with the plan id and no
/// row is *free* — which is exactly the state this helper exists to avoid producing.
async fn make_pro(harness: &TestState, email: &str) {
    let user_id = user_id_of(harness, email).await;
    sqlx::query("update users set plan_id = 'pro' where id = $1")
        .bind(user_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into subscriptions
            (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, 'pro', 'active', 'monthly', now(), now() + interval '30 days')",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// The caller's live device, for a push body.
async fn device_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar(
        "select d.id from devices d join users u on u.id = d.user_id
          where u.email = $1 and d.revoked_at is null",
    )
    .bind(email)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------------------
// Gate 6: Indonesian end to end — the server data half
// ---------------------------------------------------------------------------------------

/// Gate 6's data half, end to end through the routes: an Indonesian event title survives the
/// ICS import and the feed's escaping, a `TZID`-bearing `DTSTART` imports to the right wall
/// clock, and a date-only deadline is the user's **local** day.
///
/// This is the composed case the plan's Step 7 names and the phase's verification table
/// recorded as not measured. The unit halves exist — `crates/core/tests/calendar.rs` proves
/// the placement rule and the folding — but neither is the composed measurement, and the two
/// silent failures gate 6 exists for live exactly in the seam:
///
///  * a `TZID` resolved through the machine's zone or through a fixed offset: a Jakarta class
///    seven hours out, which no English test notices;
///  * a date-only deadline computed in UTC: a Friday deadline landing on Thursday.
///
/// The pair that catches a UTC implementation is asserted explicitly: the event is
/// **06:00 Jakarta on the 16th**, which is `23:00Z` on the **15th**. A build that read the
/// instant as UTC would store 06:00Z and render the deadline on the 15th, and every
/// assertion below would still be self-consistent.
#[sqlx::test(migrations = "../../migrations")]
async fn an_indonesian_feed_imports_to_the_right_wall_clock_and_renders_the_local_day(
    pool: PgPool,
) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();

    // A class at 06:00 Jakarta on the 16th. In UTC that is **23:00 on the 15th** — the pair
    // that catches a UTC implementation.
    let feed = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                PRODID:-//faculty//EN\r\n\
                BEGIN:VEVENT\r\n\
                UID:uts-jaringan@faculty.example\r\n\
                SUMMARY:UTS Jaringan Komputer; bab 3,4 dan 5\r\n\
                DTSTART;TZID=Asia/Jakarta:20261016T060000\r\n\
                DTEND;TZID=Asia/Jakarta:20261016T080000\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";
    let fake = Arc::new(ScriptedIcsFetch::new(vec![Scripted::Body {
        body: feed.to_string(),
        etag: Some("\"g6\"".to_string()),
        last_modified: None,
    }]));
    // The fetcher is installed **before** the router is built: `drain_jobs` reads it off the
    // harness's state, so a fetcher swapped in after the job was enqueued would be a fetcher
    // the worker never sees.
    let harness = harness.with_ics_fetch(fake);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "gate6@example.test").await;

    // The user's own zone, which is the default the schema ships.
    let user_id = user_id_of(&harness, "gate6@example.test").await;
    let zone: String = sqlx::query_scalar("select timezone from users where id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(zone, "Asia/Jakarta", "the spec's default zone");

    let (status, created) = json(
        &app,
        "POST",
        "/calendar/ics",
        &token,
        serde_json::json!({ "url": "https://faculty.example/gate6.ics" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    common::drain_jobs(&harness).await;

    let (status, events) = json(
        &app,
        "GET",
        "/events?from=2026-10-01T00:00:00Z&to=2026-10-31T00:00:00Z",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    let rows = events["events"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "one imported class: {events}");

    // (1) The wall clock, resolved through `chrono_tz`: 06:00+07:00 is 23:00Z **the day
    // before**. Both halves are asserted, because the date is the part a fixed-offset
    // implementation gets wrong.
    assert_eq!(
        rows[0]["starts_at"], "2026-10-15T23:00:00Z",
        "06:00 in Jakarta on the 16th is 23:00Z on the 15th: {events}"
    );
    assert_eq!(rows[0]["ends_at"], "2026-10-16T01:00:00Z", "{events}");

    // (2) The Indonesian title, stored unmangled. The comma and the semicolon are ICS TEXT
    // metacharacters: an unescaped `SUMMARY` shifts every later field, and a folder that cuts
    // mid-character produces a replacement character that no English test sees.
    assert_eq!(
        rows[0]["title"], "UTS Jaringan Komputer; bab 3,4 dan 5",
        "the title must round-trip whole: {events}"
    );

    // (3) A date-only deadline on the user's own day. 2026-10-15T17:00Z is already the 16th
    // in Jakarta, so a feed that rendered the UTC date would say "due the 15th" to a student
    // whose Friday is the 16th.
    insert_event(
        &harness,
        user_id,
        "deadline",
        "Kumpulkan laporan",
        "2026-10-15T17:00:00Z",
        "2026-10-15T17:00:00Z",
        "app",
    )
    .await;

    // And an app-owned event carrying the same Indonesian metacharacters, because the feed
    // deliberately excludes `source != 'app'` (A241) — the imported class above is `ics`, so
    // it is *not* in the feed, and an assertion that looked for it there would be asserting
    // the exclusion backwards. This row is what puts the escaping on the wire.
    insert_event(
        &harness,
        user_id,
        "study_block",
        "UTS Jaringan Komputer; bab 3,4 dan 5",
        "2026-10-16T09:00:00Z",
        "2026-10-16T10:00:00Z",
        "app",
    )
    .await;

    let (_, rotated) = json(
        &app,
        "POST",
        "/calendar/ics-token/regenerate",
        &token,
        serde_json::json!({}),
    )
    .await;
    let feed_token = rotated["ics_feed_token"].as_str().unwrap().to_string();
    let (status, headers, body) = raw(&app, "GET", &format!("/cal/{feed_token}.ics"), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["content-type"], "text/calendar; charset=utf-8");

    // The deadline is date-only and on the **local** 16th, and its `DTEND` is the next local
    // date because an all-day event's end is exclusive.
    assert!(
        body.contains("DTSTART;VALUE=DATE:20261016"),
        "the deadline is the student's own day, not the UTC one: {body}"
    );
    assert!(
        body.contains("DTEND;VALUE=DATE:20261017"),
        "an all-day event's DTEND is exclusive: {body}"
    );
    // The escaping survives the whole round trip: the stored title reaches the wire with its
    // two metacharacters escaped, so a strict client parses one `SUMMARY` and not three. A
    // broken line is a shifted field, not a cosmetic difference.
    assert!(
        body.contains(r"SUMMARY:UTS Jaringan Komputer\; bab 3\,4 dan 5"),
        "the title must be ICS-TEXT escaped on the wire: {body}"
    );
    // The imported class is `source = 'ics'`, so the feed excludes it (A241): echoing another
    // calendar's event back would show the student their own timetable twice.
    assert!(
        !body.contains("UTS Jaringan Komputer; bab 3,4 dan 5\r\n"),
        "an unescaped title would mean the escaping was skipped: {body}"
    );
}

// ---------------------------------------------------------------------------------------
// Task 6: the Google consent flow
// ---------------------------------------------------------------------------------------
// **What these cases can and cannot prove.** The OAuth client does not exist yet, so no
// exchange against Google has ever run here. Everything that does not need Google is
// asserted: the consent URL's parameters (including PKCE `S256` and `access_type=offline`),
// the state round trip, the refusals, and the fact that the deep link carries a status and a
// reason and never a token. What needs the real client is named in
// `the_consent_url_asks_for_offline_access_and_the_callback_never_returns_a_token`: the
// exchange itself, the "Study Plan" calendar's discovery (A237), and the first sync.

/// The consent URL asks for offline access, uses PKCE, carries an **encrypted** state, and a
/// free account is refused before any of it (A234/A235/A245).
///
/// The scope assertion is the load-bearing one: `calendar.app.created` plus
/// `calendar.events.readonly` is what makes "Google wins for an event we did not create"
/// structural rather than a rule in a merge function. A widened scope would silently move
/// that guarantee, so it is asserted as an exact string rather than by `contains`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_consent_url_asks_for_offline_access_and_carries_an_encrypted_state(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "consent@example.test").await;

    // Free first, and the order matters: free has `google_calendar = false` **and**
    // `google_accounts_max = 0`, and the route checks the boolean first so the student is
    // told the feature is not in their plan rather than "0 of 0 accounts".
    let (status, refused) = json(
        &app,
        "GET",
        "/calendar/google/connect",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "plan_required", "{refused}");
    assert_eq!(refused["min_plan"], "pro", "{refused}");

    make_pro(&harness, "consent@example.test").await;

    let (status, body) = json(
        &app,
        "GET",
        "/calendar/google/connect",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = url::Url::parse(body["url"].as_str().expect("a url")).expect("a URL");

    // The consent's parameters. `access_type=offline` and `prompt=consent` together are what
    // make Google return a refresh token on a *re*-connect; without `prompt`, a student who
    // revoked the grant and reconnected would get no token and a dead connection.
    let q: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(q["access_type"], "offline", "{url}");
    assert_eq!(q["prompt"], "consent", "{url}");
    assert_eq!(q["code_challenge_method"], "S256", "{url}");
    assert_eq!(q["response_type"], "code", "{url}");
    assert_eq!(
        q["scope"],
        "https://www.googleapis.com/auth/calendar.app.created \
         https://www.googleapis.com/auth/calendar.events.readonly",
        "{url}"
    );
    assert_eq!(
        q["redirect_uri"], "http://127.0.0.1:8099/calendar/google/callback",
        "{url}"
    );

    // The state is **opaque**: it carries the PKCE verifier, and a state that leaked it would
    // hand the verifier to the browser, which is the party PKCE exists to distrust. So the
    // assertion is that the verifier is not in the URL at all — the payload is ciphertext.
    let state_param = q["state"].to_string();
    assert!(!state_param.is_empty(), "{url}");
    assert!(
        !state_param.contains("verifier"),
        "the state must be ciphertext, not a readable payload: {state_param}"
    );
    // And it round-trips through the shared codec as a *calendar* state, which is what makes
    // the callback able to read the account back out of it.
    let payload = stvdy_server::auth::oauth_state::decrypt_state(
        &harness.state,
        stvdy_server::auth::oauth_state::Purpose::Calendar,
        &state_param,
    )
    .expect("the state this server minted must decrypt as a calendar state");
    assert_eq!(
        payload.user_id,
        Some(user_id_of(&harness, "consent@example.test").await),
        "the state must name the account the flow was started for"
    );
    assert!(
        payload.verifier.len() >= 40,
        "the state carries the PKCE verifier: {}",
        payload.verifier.len()
    );
}

/// The callback refuses a state that is not a calendar state this server minted, and it does
/// so by **redirecting** to the app with a status and a reason — never with a token.
///
/// Three refusals, three different attacks: a state minted by the login flow (a replay across
/// flows), an unknown or tampered state (a forgery), and a missing one. All three have to be
/// a redirect rather than a JSON error: the browser is sitting on Google's redirect URI, and
/// an error page there leaves the student with no way back to the app.
#[sqlx::test(migrations = "../../migrations")]
async fn the_callback_refuses_a_state_that_is_not_its_own_and_never_returns_a_token(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "callback@example.test").await;
    make_pro(&harness, "callback@example.test").await;

    // A **login** state, minted by the other flow through the same codec. This is the replay
    // the shared cipher makes possible and the `purpose` field exists to refuse (A235).
    let login_state = stvdy_server::auth::oauth_state::encrypt_state(
        &harness.state,
        &stvdy_server::auth::oauth_state::StatePayload {
            purpose: stvdy_server::auth::oauth_state::Purpose::Login,
            verifier: "v".to_string(),
            nonce: "n".to_string(),
            exp: stvdy_server::auth::oauth_state::state_expiry(),
            user_id: None,
            device_name: "laptop".to_string(),
            device_platform: "macos".to_string(),
            app_version: None,
        },
    )
    .unwrap();

    let cross_flow = format!("code=abc&state={login_state}");
    let no_code = format!(
        "state={}",
        consent_state(&harness, "callback@example.test").await
    );
    let cases: [(&str, &str); 4] = [
        // A state from the other flow: accepted by the login callback, refused here.
        ("cross-flow", cross_flow.as_str()),
        // A state nobody minted.
        ("unknown", "code=abc&state=not-a-real-state"),
        // No state at all.
        ("missing", "code=abc"),
        // A state with no code: the consent return without its grant.
        ("no-code", no_code.as_str()),
    ];

    for (label, query) in cases {
        let (status, headers, body) = raw(
            &app,
            "GET",
            &format!("/calendar/google/callback?{query}"),
            "",
        )
        .await;
        assert_eq!(status, StatusCode::FOUND, "{label}: {body}");
        let location = headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(
            location.starts_with("stvdy://oauth/calendar?"),
            "{label}: the refusal must return the app to itself: {location}"
        );
        assert!(
            location.contains("status=error") || location.contains("status=denied"),
            "{label}: a refusal carries a status the panel can render: {location}"
        );
        // The property the whole route exists to keep: **no token of any kind** travels in
        // the deep link. A deep link lands in browser history, referrer logs and the OS URL
        // handler (A235/A48), so a refresh token here would be a credential in all three.
        for leak in ["refresh", "access_token", "token=", "code="] {
            assert!(
                !location.contains(leak),
                "{label}: the deep link must not carry {leak:?}: {location}"
            );
        }
    }

    // The student declining is `denied`, not an error: they made a choice, and the panel says
    // so in different words with a different remedy.
    let (status, headers, _) = raw(
        &app,
        "GET",
        "/calendar/google/callback?error=access_denied",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers
        .get("location")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(location.contains("status=denied"), "{location}");
    assert!(location.contains("reason=access_denied"), "{location}");
}

/// A valid calendar state for an account, without going through the route.
///
/// Built here rather than by calling `connect` so a case can isolate the callback: the
/// `no-code` refusal above is about the *callback*'s own validation, and a test that reached
/// it through the consent route would be asserting two things at once.
async fn consent_state(harness: &TestState, email: &str) -> String {
    stvdy_server::auth::oauth_state::encrypt_state(
        &harness.state,
        &stvdy_server::auth::oauth_state::StatePayload {
            purpose: stvdy_server::auth::oauth_state::Purpose::Calendar,
            verifier: "v".to_string(),
            nonce: "n".to_string(),
            exp: stvdy_server::auth::oauth_state::state_expiry(),
            user_id: Some(user_id_of(harness, email).await),
            device_name: String::new(),
            device_platform: String::new(),
            app_version: None,
        },
    )
    .unwrap()
}

/// With no OAuth client configured, both routes are a 404 — the same rule
/// `auth::google::start` follows. A half-configured client must not produce a redirect that
/// fails at Google with a message the student cannot act on, and the panel's button must not
/// point at a route that pretends to work.
#[sqlx::test(migrations = "../../migrations")]
async fn both_consent_routes_are_404_without_a_configured_client(pool: PgPool) {
    let harness = TestState::new(pool).without_google_calendar();
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "unconfigured@example.test").await;
    make_pro(&harness, "unconfigured@example.test").await;

    let (status, body) = json(
        &app,
        "GET",
        "/calendar/google/connect",
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, _, _) = raw(
        &app,
        "GET",
        "/calendar/google/callback?code=abc&state=x",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------------------
// The Google two-way sync (Task 7)
//
// There is no Google credential on this machine, so nothing below touches Google. The pass is
// driven through a `CalendarApi` fake and the *client* is driven against a local stub: the
// client's request shape and the sync's decisions are what is asserted, and neither is a
// statement about what Google does.
// ---------------------------------------------------------------------------------------

/// What one `list_events` call answers.
///
/// A queue whose last answer repeats, like [`ScriptedIcsFetch`]: a test that runs one pass more
/// than it scripted is asking the same question again, and an empty answer would look like an
/// empty calendar rather than like a test that forgot a page.
#[derive(Debug, Clone)]
enum Listing {
    /// A page: its events and the cursor the pass stores.
    Page(Vec<RemoteEvent>, Option<&'static str>),
    /// Google's `410`: the cursor is stale.
    Gone,
    /// Google's `401`: the grant is gone.
    Unauthorized,
    /// A network failure.
    Transport,
}

/// What one `refresh_access_token` call answers.
#[derive(Debug, Clone)]
enum Grant {
    Token,
    Unauthorized,
    Transport,
}

/// One call the fake received.
///
/// Recorded because the assertions that matter are about what the pass *asked*: which calendar,
/// with which cursor, and with which refresh token — the last of which is the only proof that
/// the stored grant was decrypted. `event` is the outbound half's: the row the pass pushed, so
/// the request's content is assertable and not only the fact that a call happened.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeCall {
    method: &'static str,
    calendar_id: Option<String>,
    sync_token: Option<String>,
    refresh_token: Option<String>,
    event: Option<RemoteEvent>,
}

/// A [`CalendarApi`] with no network.
///
/// The real client cannot stand in for it: every one of its calls is a request to Google, and no
/// credential for one exists on a test machine.
struct FakeCalendar {
    listings: Mutex<VecDeque<Listing>>,
    grants: Mutex<VecDeque<Grant>>,
    calendars: Mutex<Vec<CalendarMeta>>,
    calendars_error: Mutex<Option<CalendarError>>,
    create_error: Mutex<Option<CalendarError>>,
    push_error: Mutex<Option<CalendarError>>,
    created: Mutex<Vec<String>>,
    calls: Mutex<Vec<FakeCall>>,
}

impl FakeCalendar {
    fn new() -> Self {
        Self {
            listings: Mutex::new(VecDeque::new()),
            grants: Mutex::new(VecDeque::from(vec![Grant::Token])),
            calendars: Mutex::new(Vec::new()),
            calendars_error: Mutex::new(None),
            create_error: Mutex::new(None),
            push_error: Mutex::new(None),
            created: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn push_listing(&self, listing: Listing) {
        self.listings.lock().push_back(listing);
    }

    /// The grant script, replaced rather than queued.
    ///
    /// A queue would put a test's refusal *behind* the repeating success the fake starts with,
    /// which is right for a pass that must succeed first and wrong for a test that wants the
    /// refresh itself refused.
    fn set_grants(&self, grants: Vec<Grant>) {
        *self.grants.lock() = grants.into();
    }

    fn set_calendars(&self, calendars: Vec<CalendarMeta>) {
        *self.calendars.lock() = calendars;
    }

    fn fail_calendars(&self, error: CalendarError) {
        *self.calendars_error.lock() = Some(error);
    }

    /// Make every later `upsert_event` fail.
    ///
    /// Replaced rather than queued, like [`FakeCalendar::set_grants`]: a queue would put a
    /// test's refusal behind calls it did not intend to script.
    fn fail_pushes(&self, error: CalendarError) {
        *self.push_error.lock() = Some(error);
    }

    /// The summaries `create_calendar` was asked for, in order.
    fn created(&self) -> Vec<String> {
        self.created.lock().clone()
    }

    fn calls(&self) -> Vec<FakeCall> {
        self.calls.lock().clone()
    }

    /// The events the outbound half pushed, in order: the push's own record.
    fn pushes(&self) -> Vec<RemoteEvent> {
        self.calls()
            .into_iter()
            .filter_map(|call| call.event)
            .collect()
    }

    /// The cursor each `list_events` call carried: what "incremental" means, as a value.
    fn cursors(&self) -> Vec<Option<String>> {
        self.calls()
            .into_iter()
            .filter(|call| call.method == "list_events")
            .map(|call| call.sync_token)
            .collect()
    }
}

#[async_trait::async_trait]
impl CalendarApi for FakeCalendar {
    async fn list_events(
        &self,
        calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<EventPage, CalendarError> {
        self.calls.lock().push(FakeCall {
            method: "list_events",
            calendar_id: Some(calendar_id.to_string()),
            sync_token: sync_token.map(str::to_string),
            refresh_token: None,
            event: None,
        });

        let answer = {
            let mut script = self.listings.lock();
            if script.len() > 1 {
                script.pop_front().unwrap()
            } else {
                script
                    .front()
                    .cloned()
                    .unwrap_or(Listing::Page(Vec::new(), None))
            }
        };

        match answer {
            Listing::Page(events, token) => Ok(EventPage {
                events,
                next_sync_token: token.map(str::to_string),
            }),
            Listing::Gone => Err(CalendarError::Gone),
            Listing::Unauthorized => Err(CalendarError::Unauthorized(
                "the student revoked the grant".to_string(),
            )),
            Listing::Transport => Err(CalendarError::Transport(
                "the connection was reset".to_string(),
            )),
        }
    }

    async fn list_calendars(&self) -> Result<Vec<CalendarMeta>, CalendarError> {
        self.calls.lock().push(FakeCall {
            method: "list_calendars",
            calendar_id: None,
            sync_token: None,
            refresh_token: None,
            event: None,
        });
        if let Some(error) = self.calendars_error.lock().clone() {
            return Err(error);
        }
        Ok(self.calendars.lock().clone())
    }

    async fn create_calendar(&self, summary: &str) -> Result<CalendarMeta, CalendarError> {
        self.calls.lock().push(FakeCall {
            method: "create_calendar",
            calendar_id: None,
            sync_token: None,
            refresh_token: None,
            event: None,
        });
        if let Some(error) = self.create_error.lock().clone() {
            return Err(error);
        }
        let mut created = self.created.lock();
        created.push(summary.to_string());
        Ok(CalendarMeta {
            id: format!("created-{}", created.len()),
            summary: summary.to_string(),
            primary: false,
        })
    }

    async fn upsert_event(
        &self,
        calendar_id: &str,
        event: &RemoteEvent,
    ) -> Result<RemoteEvent, CalendarError> {
        self.calls.lock().push(FakeCall {
            method: "upsert_event",
            calendar_id: Some(calendar_id.to_string()),
            sync_token: None,
            refresh_token: None,
            event: Some(event.clone()),
        });
        if let Some(error) = self.push_error.lock().clone() {
            return Err(error);
        }

        // Google's **answer**, not the request echoed back, and the difference is the whole
        // reason the write-back is assertable: an `events.insert` is given an id by Google, and
        // every accepted write carries a new etag. A fake that echoed the request would make a
        // pass that never recorded the answer look idempotent, and the no-churn assertion would
        // pass for the wrong reason.
        let mut accepted = event.clone();
        let accepted_count = self.pushes().len();
        if accepted.id.is_empty() {
            accepted.id = format!("g-{accepted_count}");
        }
        accepted.etag = format!("v{accepted_count}");
        Ok(accepted)
    }

    async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<AccessToken, CalendarError> {
        self.calls.lock().push(FakeCall {
            method: "refresh_access_token",
            calendar_id: None,
            sync_token: None,
            refresh_token: Some(refresh_token.to_string()),
            event: None,
        });

        let answer = {
            let mut script = self.grants.lock();
            if script.len() > 1 {
                script.pop_front().unwrap()
            } else {
                script.front().cloned().unwrap_or(Grant::Token)
            }
        };

        match answer {
            Grant::Token => Ok(AccessToken {
                token: "access-1".to_string(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            }),
            Grant::Unauthorized => Err(CalendarError::Unauthorized(
                "the grant was revoked".to_string(),
            )),
            Grant::Transport => Err(CalendarError::Transport(
                "the connection was reset".to_string(),
            )),
        }
    }
}

/// One live remote event, as Google's API sends it.
fn remote(id: &str, etag: &str, summary: &str, starts_at: &str, ends_at: &str) -> RemoteEvent {
    RemoteEvent {
        id: id.to_string(),
        etag: etag.to_string(),
        status: "confirmed".to_string(),
        summary: summary.to_string(),
        start: Some(at(starts_at)),
        end: Some(at(ends_at)),
        recurring_event_id: None,
    }
}

/// A cancellation: Google sends the id and the status, and no instants at all.
fn cancelled(id: &str, etag: &str) -> RemoteEvent {
    RemoteEvent {
        id: id.to_string(),
        etag: etag.to_string(),
        status: "cancelled".to_string(),
        summary: String::new(),
        start: None,
        end: None,
        recurring_event_id: None,
    }
}

/// An all-day event: a `date` and no `dateTime`, so no instant this build may invent.
fn all_day(id: &str, etag: &str, summary: &str) -> RemoteEvent {
    RemoteEvent {
        id: id.to_string(),
        etag: etag.to_string(),
        status: "confirmed".to_string(),
        summary: summary.to_string(),
        start: None,
        end: None,
        recurring_event_id: None,
    }
}

/// A connected Google row, written directly.
///
/// The consent flow's code exchange is the one step that needs Google, so a test that reached
/// this row through `/calendar/google/callback` would be reaching Google. What this writes is
/// what `store_connection` writes: a grant encrypted with the deployment key, and
/// `status = 'connected'`.
async fn google_connection(
    harness: &TestState,
    email: &str,
    study_calendar_id: Option<&str>,
) -> Uuid {
    let user_id = user_id_of(harness, email).await;
    let encrypted =
        stvdy_server::auth::oauth_state::encrypt_secret(&harness.state, "refresh-token-1").unwrap();
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into calendar_connections
            (id, user_id, provider, refresh_token_enc, status, study_calendar_id, updated_at)
         values ($1, $2, 'google', $3, 'connected', $4, now())",
    )
    .bind(id)
    .bind(user_id)
    .bind(&encrypted)
    .bind(study_calendar_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}

/// The connection columns the sync's own decisions are read from.
#[derive(Debug, sqlx::FromRow)]
struct ConnectionState {
    sync_token: Option<String>,
    last_sync_was_full: bool,
    status: String,
    error: Option<String>,
    study_calendar_id: Option<String>,
}

async fn connection_state(harness: &TestState, connection_id: Uuid) -> ConnectionState {
    sqlx::query_as(
        "select sync_token, last_sync_was_full, status, error, study_calendar_id
           from calendar_connections where id = $1",
    )
    .bind(connection_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// One `events` row's sync-owned columns.
#[derive(Debug, sqlx::FromRow)]
struct EventState {
    title: String,
    etag: Option<String>,
    source: String,
    kind: String,
    deleted: bool,
    starts_at: chrono::DateTime<chrono::Utc>,
}

async fn event_state(
    harness: &TestState,
    connection_id: Uuid,
    external_id: &str,
) -> Option<EventState> {
    sqlx::query_as(
        "select title, etag, source, kind, deleted_at is not null as deleted, starts_at
           from events where connection_id = $1 and external_id = $2",
    )
    .bind(connection_id)
    .bind(external_id)
    .fetch_optional(&harness.state.pool)
    .await
    .unwrap()
}

async fn event_updated_at(
    harness: &TestState,
    connection_id: Uuid,
    external_id: &str,
) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar(
        "select updated_at from events where connection_id = $1 and external_id = $2",
    )
    .bind(connection_id)
    .bind(external_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap()
}

/// One pass, with the row re-read first: the pass takes the connection it is handed, so a test
/// that reused the row it loaded before the last pass would be asserting against a stale cursor.
async fn run_pass(harness: &TestState, connection_id: Uuid) -> SyncOutcome {
    let connection = load(&harness.state.pool, connection_id)
        .await
        .unwrap()
        .expect("the connection");
    sync_google(&harness.state, &connection).await.unwrap()
}

/// A local row on a connection.
///
/// `external_id`, `etag` and `source` are the three columns the sync's decision reads, and A198
/// refuses a client all three — so a test writes them directly rather than through a route.
/// The first two are `Option` because both are nullable and the push half reads the difference:
/// a row with no `external_id` is one Google has never been told about, and a row with an `etag`
/// is one whose remote version is known.
#[allow(clippy::too_many_arguments)]
async fn insert_connected_event(
    harness: &TestState,
    user_id: Uuid,
    connection_id: Uuid,
    external_id: Option<&str>,
    etag: Option<&str>,
    source: &str,
    title: &str,
    starts_at: &str,
    ends_at: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into events
            (id, user_id, kind, title, starts_at, ends_at, external_id, etag, connection_id,
             source)
         values ($1, $2, 'external', $3, $4::timestamptz, $5::timestamptz, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(user_id)
    .bind(title)
    .bind(starts_at)
    .bind(ends_at)
    .bind(external_id)
    .bind(etag)
    .bind(connection_id)
    .bind(source)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    id
}

/// The two properties the exit gate names: the sync is incremental, and a `410` is a full
/// resync. `last_sync_was_full` is what makes "silently stopped being incremental" assertable
/// at all (A236) — every pass here succeeds either way, which is exactly why the mode has to be
/// a stored value rather than an error.
#[sqlx::test(migrations = "../../migrations")]
async fn a_google_sync_is_incremental_and_a_410_resyncs_fully(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool)
        .with_calendar(fake.clone())
        .with_clock(Arc::new(FixedClock::at(at("2026-06-01T00:00:00Z"))));
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "google@example.test").await;
    let connection_id =
        google_connection(&harness, "google@example.test", Some("study-plan-1")).await;

    // Pass 1: a full listing. Pass 2: incremental, and it carries a cancellation. Pass 3: the
    // cursor has expired, so the same pass re-lists fully. Pass 4: incremental again, with an
    // event nothing has touched.
    fake.push_listing(Listing::Page(
        vec![
            remote(
                "evt-a",
                "e1",
                "Kriptografi",
                "2026-06-10T01:00:00Z",
                "2026-06-10T03:00:00Z",
            ),
            remote(
                "evt-b",
                "b1",
                "Kuliah umum",
                "2026-06-11T01:00:00Z",
                "2026-06-11T02:00:00Z",
            ),
            // A date-only event has no instant, and inventing one would place a Jakarta
            // all-day event at 07:00 local (see the client's `instant`).
            all_day("evt-libur", "l1", "Libur nasional"),
        ],
        Some("t1"),
    ));
    fake.push_listing(Listing::Page(
        vec![
            remote(
                "evt-a",
                "e2",
                "Kriptografi (revisi)",
                "2026-06-10T01:00:00Z",
                "2026-06-10T03:00:00Z",
            ),
            cancelled("evt-b", "b2"),
        ],
        Some("t2"),
    ));
    fake.push_listing(Listing::Gone);
    fake.push_listing(Listing::Page(
        vec![remote(
            "evt-a",
            "e3",
            "Kriptografi (revisi)",
            "2026-06-10T01:00:00Z",
            "2026-06-10T03:00:00Z",
        )],
        Some("t3"),
    ));
    fake.push_listing(Listing::Page(
        vec![remote(
            "evt-a",
            "e3",
            "Kriptografi (revisi)",
            "2026-06-10T01:00:00Z",
            "2026-06-10T03:00:00Z",
        )],
        Some("t4"),
    ));

    let first = run_pass(&harness, connection_id).await;
    assert!(first.was_full, "a pass with no cursor is a full listing");
    assert_eq!((first.inserted, first.updated, first.deleted), (2, 0, 0));

    let calls = fake.calls();
    // The grant is refreshed **first**, with the token the row holds decrypted: a pass that
    // never decrypted it could not have got past this line, and one that listed before
    // refreshing would have listed with a token an hour old.
    assert_eq!(calls[0].method, "refresh_access_token");
    assert_eq!(calls[0].refresh_token.as_deref(), Some("refresh-token-1"));
    assert_eq!(calls[1].method, "list_events");
    assert_eq!(
        calls[1].calendar_id.as_deref(),
        Some("study-plan-1"),
        "the pass lists the calendar the connect flow stored (A237)"
    );
    assert_eq!(calls[1].sync_token, None, "the first pass is full");

    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.sync_token.as_deref(), Some("t1"));
    assert!(state.last_sync_was_full, "the mode must be recorded (A236)");
    assert_eq!(state.status, "connected");
    assert_eq!(state.error, None);

    let adopted = event_state(&harness, connection_id, "evt-a")
        .await
        .expect("the adopted event");
    assert_eq!(adopted.source, "google");
    assert_eq!(adopted.kind, "external");
    assert_eq!(adopted.etag.as_deref(), Some("e1"));
    assert_eq!(adopted.title, "Kriptografi");
    assert_eq!(
        adopted.starts_at,
        at("2026-06-10T01:00:00Z"),
        "the instant Google sent, verbatim"
    );
    assert!(!adopted.deleted);
    // The date-only event is skipped rather than given an invented instant, and the two events
    // that can be placed are stored: one unplaceable event must not cost the student the rest.
    assert!(
        event_state(&harness, connection_id, "evt-libur")
            .await
            .is_none(),
        "an event with no instant must not be invented into one"
    );

    let second = run_pass(&harness, connection_id).await;
    assert!(!second.was_full, "a pass with a cursor is incremental");
    assert_eq!((second.inserted, second.updated, second.deleted), (0, 1, 1));
    assert_eq!(
        fake.cursors()[1].as_deref(),
        Some("t1"),
        "the stored cursor must be sent, or every pass is silently full"
    );

    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.sync_token.as_deref(), Some("t2"));
    assert!(
        !state.last_sync_was_full,
        "an incremental pass must record that it was one"
    );

    let adopted = event_state(&harness, connection_id, "evt-a").await.unwrap();
    assert_eq!(adopted.title, "Kriptografi (revisi)");
    assert_eq!(adopted.etag.as_deref(), Some("e2"));

    // The cancellation is a **tombstone**, not a missing row: `events` is +sync, and a row left
    // live would keep showing an event Google has cancelled, on every device.
    let cancelled_row = event_state(&harness, connection_id, "evt-b")
        .await
        .expect("the cancelled event's row");
    assert!(
        cancelled_row.deleted,
        "a cancelled remote event must be tombstoned"
    );

    let third = run_pass(&harness, connection_id).await;
    assert!(third.was_full, "a 410 pass re-lists fully");
    assert_eq!(fake.cursors()[2].as_deref(), Some("t2"));
    assert_eq!(
        fake.cursors()[3],
        None,
        "the pass after a 410 is a full listing"
    );
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.sync_token.as_deref(), Some("t3"));
    assert!(state.last_sync_was_full);
    assert_eq!(
        state.status, "connected",
        "a 410 is not an error: the same pass recovered and succeeded"
    );
    assert_eq!(
        event_state(&harness, connection_id, "evt-a")
            .await
            .unwrap()
            .etag
            .as_deref(),
        Some("e3"),
        "the full resync must take the remote's version"
    );
    assert!(
        event_state(&harness, connection_id, "evt-b")
            .await
            .unwrap()
            .deleted,
        "a full listing must not resurrect a cancelled event"
    );
    let after_resync = event_updated_at(&harness, connection_id, "evt-a").await;

    let fourth = run_pass(&harness, connection_id).await;
    assert!(!fourth.was_full);
    assert_eq!(
        (fourth.inserted, fourth.updated, fourth.deleted),
        (0, 0, 0),
        "an event whose etag has not moved must not be rewritten"
    );
    assert_eq!(
        fake.cursors()[4].as_deref(),
        Some("t3"),
        "the pass after a full resync is incremental again"
    );
    assert_eq!(
        event_updated_at(&harness, connection_id, "evt-a").await,
        after_resync,
        "an unchanged row must not be touched: every touched row is a change_log entry every \
         device pulls (A239)"
    );
}

/// A refused grant is not a failed pass. Retrying cannot un-revoke it, so the connection is
/// marked `revoked` and the pass **succeeds** (A247) — a job that failed would retry five times
/// with backoff and record the same refusal five times.
///
/// Both legs are exercised, because the refresh and the listing are two calls to one decision:
/// the listing leg first, then the refresh leg with the row put back to `connected` — which is
/// what a student who reconnected would have.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refused_google_grant_revokes_the_connection_and_the_pass_succeeds(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "revoked@example.test").await;
    let connection_id = google_connection(&harness, "revoked@example.test", None).await;

    fake.push_listing(Listing::Unauthorized);
    let outcome = run_pass(&harness, connection_id).await;
    assert_eq!(
        outcome,
        SyncOutcome::default(),
        "a refused grant is not a failure: nothing was imported and nothing is retryable"
    );
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.status, "revoked");
    assert_eq!(
        state.error.as_deref(),
        Some("the student revoked the grant"),
        "the reason is what the screen shows (A247)"
    );

    fake.set_grants(vec![Grant::Unauthorized]);
    sqlx::query("update calendar_connections set status = 'connected', error = null where id = $1")
        .bind(connection_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let outcome = run_pass(&harness, connection_id).await;
    assert_eq!(outcome, SyncOutcome::default());
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.status, "revoked");
    assert_eq!(state.error.as_deref(), Some("the grant was revoked"));
    assert_eq!(
        fake.cursors().len(),
        1,
        "a refresh that was refused must not be followed by a listing"
    );
}

/// A transport failure is the one a retry is worth, so the pass **fails** — and the connection
/// is not revoked. A network outage that marked the student's grant revoked would make them
/// reconnect for nothing, and the poll would stop examining the row (A236/A247).
#[sqlx::test(migrations = "../../migrations")]
async fn a_transport_failure_fails_the_pass_and_leaves_the_connection_connected(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "transport@example.test").await;
    let connection_id = google_connection(&harness, "transport@example.test", None).await;

    fake.push_listing(Listing::Transport);
    let connection = load(&harness.state.pool, connection_id)
        .await
        .unwrap()
        .expect("the connection");
    let error = sync_google(&harness.state, &connection)
        .await
        .expect_err("a transport failure must reach the worker so its backoff runs");
    assert!(
        error.to_string().contains("could not be reached"),
        "{error}"
    );

    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.status, "connected");
    assert_eq!(state.error, None);
    assert_eq!(
        state.sync_token, None,
        "nothing was read, so nothing advanced"
    );

    // The refresh leg, which is a transport failure too.
    fake.set_grants(vec![Grant::Transport]);
    let connection = load(&harness.state.pool, connection_id)
        .await
        .unwrap()
        .expect("the connection");
    sync_google(&harness.state, &connection)
        .await
        .expect_err("a refresh that cannot reach Google is retryable");
    assert_eq!(
        connection_state(&harness, connection_id).await.status,
        "connected"
    );
}

/// A234's rule 3, end to end: a remote tombstone is honoured for a row we **adopted** and
/// ignored for a row we **created**.
///
/// The scope pair is what makes this structural rather than a rule someone remembers — Google
/// cannot delete what `calendar.app.created` does not let it write — so a build that deleted the
/// student's study block on a remote cancellation would be deleting the one thing the scope pair
/// exists to protect.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cancelled_remote_event_tombstones_an_adopted_row_and_leaves_an_app_row_alone(
    pool: PgPool,
) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "origins@example.test").await;
    let user_id = user_id_of(&harness, "origins@example.test").await;
    let connection_id =
        google_connection(&harness, "origins@example.test", Some("study-plan-1")).await;

    insert_connected_event(
        &harness,
        user_id,
        connection_id,
        Some("evt-adopted"),
        Some("a1"),
        "google",
        "Rapat himpunan",
        "2026-06-13T01:00:00Z",
        "2026-06-13T02:00:00Z",
    )
    .await;
    insert_connected_event(
        &harness,
        user_id,
        connection_id,
        Some("evt-app"),
        Some("p1"),
        "app",
        "Blok belajar",
        "2026-06-14T01:00:00Z",
        "2026-06-14T02:00:00Z",
    )
    .await;

    fake.push_listing(Listing::Page(
        vec![cancelled("evt-adopted", "a2"), cancelled("evt-app", "p2")],
        Some("t1"),
    ));
    let outcome = run_pass(&harness, connection_id).await;
    assert_eq!(outcome.deleted, 1, "only the adopted row may be deleted");

    assert!(
        event_state(&harness, connection_id, "evt-adopted")
            .await
            .expect("the adopted row")
            .deleted,
        "an adopted row follows the remote, tombstone included"
    );

    let own = event_state(&harness, connection_id, "evt-app")
        .await
        .expect("the student's own row");
    assert!(
        !own.deleted,
        "a remote tombstone must not delete the student's own study block"
    );
    assert_eq!(own.title, "Blok belajar");
    assert_eq!(own.source, "app");
}

/// Task 7's outbound half: a row this connection owns is **pushed**, and Google's answer is
/// written back into it.
///
/// Four properties, each of which a plausible implementation gets wrong. The push targets the
/// calendar the app created (A237) — never `primary`, which the scope cannot write and which
/// would answer `403`. The returned `id` and `etag` land in the row, because that is the whole
/// reason the push is idempotent: without the `etag` every pass re-pushes the row and Google
/// stamps a new `updated` on each one, and without the `id` the second push is a second insert.
/// A remote change to a row we own is answered with **our** content, not the remote's (A234's
/// rule 2). And a pass whose listing already carries the `etag` we stored pushes **nothing**
/// and touches no row — A239's rule, which the ICS half's `304` obeys for the same reason.
#[sqlx::test(migrations = "../../migrations")]
async fn an_app_owned_event_is_pushed_once_and_a_second_pass_pushes_nothing(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "push@example.test").await;
    let user_id = user_id_of(&harness, "push@example.test").await;
    let connection_id =
        google_connection(&harness, "push@example.test", Some("study-plan-1")).await;

    // The student's own study block, mirrored to this connection and never pushed: no
    // `external_id`, because only Google assigns one, and no `etag` for the same reason.
    insert_connected_event(
        &harness,
        user_id,
        connection_id,
        None,
        None,
        "app",
        "Blok belajar",
        "2026-06-14T01:00:00Z",
        "2026-06-14T02:00:00Z",
    )
    .await;

    // Every listing up front, because the fake's script pops its front while more than one
    // answer is queued: pass 1 gets the empty calendar, and passes 2 and 3 get the event
    // Google accepted — with a **different summary and a newer `etag`**, which is what an
    // out-of-band edit in the Study Plan calendar looks like.
    //
    // Pass 1: Google's calendar holds nothing of ours.
    fake.push_listing(Listing::Page(Vec::new(), Some("t1")));
    let echoed = || {
        Listing::Page(
            vec![remote(
                "g-1",
                "v2",
                "Blok belajar (versi Google)",
                "2026-06-14T01:00:00Z",
                "2026-06-14T02:00:00Z",
            )],
            Some("t2"),
        )
    };
    fake.push_listing(echoed());
    fake.push_listing(echoed());

    let first = run_pass(&harness, connection_id).await;
    assert_eq!(
        first.pushed, 1,
        "the row is the student's and Google has never been told about it"
    );

    let pushes = fake.pushes();
    assert_eq!(pushes.len(), 1, "pushed exactly once");
    let call = fake
        .calls()
        .into_iter()
        .find(|call| call.method == "upsert_event")
        .expect("the push");
    assert_eq!(
        call.calendar_id.as_deref(),
        Some("study-plan-1"),
        "the push goes to the calendar the app created, never `primary` (A237)"
    );
    assert_eq!(
        pushes[0].id, "",
        "an empty id is what makes the client insert rather than patch"
    );
    assert_eq!(pushes[0].summary, "Blok belajar");
    assert_eq!(pushes[0].start, Some(at("2026-06-14T01:00:00Z")));
    assert_eq!(pushes[0].end, Some(at("2026-06-14T02:00:00Z")));

    // Google's answer, in the row: its own id, and the version it accepted.
    let pushed_row = event_state(&harness, connection_id, "g-1")
        .await
        .expect("the row Google's id now names");
    assert_eq!(
        pushed_row.etag.as_deref(),
        Some("v1"),
        "the etag is what makes the next pass see the row as unchanged"
    );
    assert_eq!(
        pushed_row.source, "app",
        "a push must not reclassify the row as Google's: the conflict policy reads the origin \
         on the next pass (A234)"
    );
    assert_eq!(pushed_row.title, "Blok belajar");
    assert_eq!(pushed_row.starts_at, at("2026-06-14T01:00:00Z"));

    // Pass 2: the remote has moved — a newer etag, and a summary someone edited in the Study
    // Plan calendar. The row is ours, so the policy answers `Winner::Local` and it is pushed
    // again, **with the local content**: the remote cannot hold "the newer version" of a row
    // only we write to (A234's rule 2).
    let second = run_pass(&harness, connection_id).await;
    assert_eq!(
        second.pushed, 1,
        "the remote moved, so the row is pushed again"
    );
    let pushes = fake.pushes();
    assert_eq!(pushes.len(), 2);
    assert_eq!(
        pushes[1].summary, "Blok belajar",
        "the push carries the student's title, never the remote's version of it"
    );
    assert_eq!(
        pushes[1].id, "g-1",
        "the row now has Google's id, so the second push is a patch of that event rather than \
         a second insert"
    );
    let repushed = event_state(&harness, connection_id, "g-1").await.unwrap();
    assert_eq!(repushed.etag.as_deref(), Some("v2"));
    assert_eq!(
        repushed.title, "Blok belajar",
        "the local copy is the student's: a listing that carries a different summary for a row \
         we own must not overwrite it (A234's rule 2)"
    );

    // Pass 3: the same listing as pass 2, and the row now holds the version Google accepted —
    // so nothing is pushed and no row is touched. Without the `etag` write-back this pass
    // would push again and Google would stamp a new `updated` on every pass (A239's rule, the
    // ICS half's `304`).
    let touched_at = event_updated_at(&harness, connection_id, "g-1").await;
    let third = run_pass(&harness, connection_id).await;
    assert_eq!(
        third.pushed, 0,
        "a row whose etag matches the remote's must not be pushed again"
    );
    assert_eq!(
        fake.pushes().len(),
        2,
        "the fake recorded one push per *change*, not one per pass"
    );
    assert_eq!(
        event_updated_at(&harness, connection_id, "g-1").await,
        touched_at,
        "a pass with no local change must not touch the row: every touched row is a change_log \
         entry every device pulls (A239)"
    );
}

/// The push's failures get the listing's treatment (see `stop`), and the half that already
/// landed is not undone.
///
/// That second half is the part a copy of the listing's two arms would get wrong: the inbound
/// half is committed before the first push is attempted, so a refused or unreachable push must
/// not cost the student the page this pass imported.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refused_push_revokes_the_connection_and_a_transport_push_fails_the_pass(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "push-refused@example.test").await;
    let user_id = user_id_of(&harness, "push-refused@example.test").await;
    let connection_id =
        google_connection(&harness, "push-refused@example.test", Some("study-plan-1")).await;
    insert_connected_event(
        &harness,
        user_id,
        connection_id,
        None,
        None,
        "app",
        "Blok belajar",
        "2026-06-14T01:00:00Z",
        "2026-06-14T02:00:00Z",
    )
    .await;

    // The listing itself succeeded, so the pass has something a push failure could cost it.
    fake.push_listing(Listing::Page(
        vec![remote(
            "evt-a",
            "e1",
            "Kriptografi",
            "2026-06-10T01:00:00Z",
            "2026-06-10T03:00:00Z",
        )],
        Some("t1"),
    ));
    fake.fail_pushes(CalendarError::Unauthorized(
        "the grant was revoked".to_string(),
    ));

    let outcome = run_pass(&harness, connection_id).await;
    assert_eq!(outcome.inserted, 1, "the inbound half landed");
    assert_eq!(outcome.pushed, 0);
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(
        state.status, "revoked",
        "a refused push is a refused grant: retrying cannot un-revoke it (A247)"
    );
    assert_eq!(state.error.as_deref(), Some("the grant was revoked"));
    assert_eq!(
        state.sync_token.as_deref(),
        Some("t1"),
        "the cursor advanced with the events it covers"
    );
    assert!(
        event_state(&harness, connection_id, "evt-a")
            .await
            .is_some(),
        "a push failure must not cost the student the events this pass imported"
    );

    // A transport failure is the one a retry is worth, so the pass fails and the worker's
    // backoff runs.
    fake.fail_pushes(CalendarError::Transport(
        "the connection was reset".to_string(),
    ));
    fake.push_listing(Listing::Page(Vec::new(), Some("t2")));
    let connection = load(&harness.state.pool, connection_id)
        .await
        .unwrap()
        .expect("the connection");
    let error = sync_google(&harness.state, &connection)
        .await
        .expect_err("an unreachable push is retryable");
    assert!(
        error.to_string().contains("could not be reached"),
        "{error}"
    );
}

/// A connection with no Study Plan calendar pushes **nothing**, and in particular not to
/// `primary`.
///
/// The listing half falls back to `primary`, so a connect that could not resolve the calendar
/// still imports the student's own Google events; the push half must not follow it there,
/// because `calendar.app.created` can write only into a calendar the app created and a push
/// that tried would be a `403` that reads like a credential problem (A234/A237).
#[sqlx::test(migrations = "../../migrations")]
async fn a_connection_without_a_study_calendar_pushes_nothing_to_primary(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "no-calendar@example.test").await;
    let user_id = user_id_of(&harness, "no-calendar@example.test").await;
    let connection_id = google_connection(&harness, "no-calendar@example.test", None).await;
    insert_connected_event(
        &harness,
        user_id,
        connection_id,
        None,
        None,
        "app",
        "Blok belajar",
        "2026-06-14T01:00:00Z",
        "2026-06-14T02:00:00Z",
    )
    .await;

    fake.push_listing(Listing::Page(Vec::new(), Some("t1")));
    let outcome = run_pass(&harness, connection_id).await;
    assert_eq!(
        outcome.pushed, 0,
        "there is no calendar this app is allowed to write to"
    );
    assert!(
        fake.pushes().is_empty(),
        "a push at `primary` is the 403 the scope pair exists to prevent: {:?}",
        fake.pushes()
    );
    let listed = fake
        .calls()
        .into_iter()
        .find(|call| call.method == "list_events")
        .expect("the listing");
    assert_eq!(
        listed.calendar_id.as_deref(),
        Some("primary"),
        "the inbound half's own fallback is unchanged: it reads `primary` and writes nowhere"
    );
}

/// A row the student deleted locally is not pushed, whatever the remote says about it.
///
/// `CalendarApi` has no delete, so the only write available would **resurrect** the block in
/// Google. A234's rule 3 — a remote tombstone is ignored for a row we own — is about the
/// remote's cancellation, not about a row that no longer exists locally: there is nothing to
/// push, and the deletion waits for a push this seam cannot express.
#[sqlx::test(migrations = "../../migrations")]
async fn a_locally_deleted_owned_row_is_not_pushed(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "deleted@example.test").await;
    let user_id = user_id_of(&harness, "deleted@example.test").await;
    let connection_id =
        google_connection(&harness, "deleted@example.test", Some("study-plan-1")).await;
    insert_connected_event(
        &harness,
        user_id,
        connection_id,
        Some("evt-app"),
        Some("p1"),
        "app",
        "Blok belajar",
        "2026-06-14T01:00:00Z",
        "2026-06-14T02:00:00Z",
    )
    .await;

    // The student deletes it on the phone: a tombstone (`events` is `+sync`), with the
    // `external_id` and `etag` a previous push left behind.
    sqlx::query(
        "update events set deleted_at = $2
          where connection_id = $1 and external_id = 'evt-app'",
    )
    .bind(connection_id)
    .bind(at("2026-06-02T00:00:00Z"))
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // The listing says the remote moved on — the state that pushes a live row. Without the
    // `deleted_at is null` filter the policy answers `Winner::Local` and the block comes back
    // from the dead in Google.
    fake.push_listing(Listing::Page(vec![cancelled("evt-app", "p2")], Some("t1")));
    let outcome = run_pass(&harness, connection_id).await;
    assert_eq!(
        outcome.pushed, 0,
        "a row that no longer exists locally is not pushed"
    );
    assert!(fake.pushes().is_empty(), "{:?}", fake.pushes());
    assert!(
        event_state(&harness, connection_id, "evt-app")
            .await
            .expect("the tombstone")
            .deleted,
        "the tombstone stands: the push half must not clear it"
    );
}

/// The path a student who connects Google Calendar takes today: the poll enqueues a
/// `sync_calendar` for the row and the worker's handler runs it.
///
/// Before Task 7 that arm was a `Failure::permanent` naming a missing provider, so a connected
/// Google row produced a job that failed on every poll interval. This asserts the row is no
/// longer that, and that the events actually land through the handler rather than only through a
/// direct call.
#[sqlx::test(migrations = "../../migrations")]
async fn a_google_connection_syncs_through_the_job_handler(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "handler@example.test").await;
    let connection_id =
        google_connection(&harness, "handler@example.test", Some("study-plan-1")).await;

    fake.push_listing(Listing::Page(
        vec![remote(
            "evt-a",
            "e1",
            "Kriptografi",
            "2026-06-10T01:00:00Z",
            "2026-06-10T03:00:00Z",
        )],
        Some("t1"),
    ));

    let scheduled = stvdy_server::jobs::schedule::calendar_pass(&harness.state)
        .await
        .unwrap();
    assert_eq!(scheduled, 1, "a fresh Google row is polled");
    common::drain_jobs(&harness).await;

    let status: String = sqlx::query_scalar(
        "select status from jobs where kind = 'sync_calendar' order by created_at desc limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        status, "done",
        "the google arm must run the sync, not refuse the provider"
    );

    assert!(
        event_state(&harness, connection_id, "evt-a")
            .await
            .is_some(),
        "the handler's pass must import the event"
    );
    assert_eq!(
        connection_state(&harness, connection_id)
            .await
            .sync_token
            .as_deref(),
        Some("t1")
    );
}

/// A237: the "Study Plan" calendar is found or created **at connect**, and a failure to resolve
/// it is recorded on the connection rather than raised — a student whose token was stored must
/// not see a 500, and the connection must not be lost.
#[sqlx::test(migrations = "../../migrations")]
async fn the_connect_flow_finds_or_creates_the_study_plan_calendar(pool: PgPool) {
    let fake = Arc::new(FakeCalendar::new());
    let harness = TestState::new(pool.clone()).with_calendar(fake.clone());
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    signed_up(&app, &harness, "study-plan@example.test").await;
    let connection_id = google_connection(&harness, "study-plan@example.test", None).await;

    // Found: the summary matches exactly, and nothing is created.
    fake.set_calendars(vec![
        CalendarMeta {
            id: "personal".to_string(),
            summary: "Personal".to_string(),
            primary: true,
        },
        CalendarMeta {
            id: "sp-9".to_string(),
            summary: "Study Plan".to_string(),
            primary: false,
        },
    ]);
    stvdy_server::calendar::oauth::ensure_study_calendar(&harness.state, connection_id)
        .await
        .unwrap();
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.study_calendar_id.as_deref(), Some("sp-9"));
    assert_eq!(state.error, None);
    assert!(
        fake.created().is_empty(),
        "an existing calendar must not be duplicated"
    );

    // Not found: it is created, with exactly the name A237 names.
    fake.set_calendars(vec![CalendarMeta {
        id: "personal".to_string(),
        summary: "Personal".to_string(),
        primary: true,
    }]);
    sqlx::query("update calendar_connections set study_calendar_id = null where id = $1")
        .bind(connection_id)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    stvdy_server::calendar::oauth::ensure_study_calendar(&harness.state, connection_id)
        .await
        .unwrap();
    assert_eq!(fake.created(), vec!["Study Plan".to_string()]);
    assert_eq!(
        connection_state(&harness, connection_id)
            .await
            .study_calendar_id
            .as_deref(),
        Some("created-1")
    );

    // The API refuses: the connection survives with the reason on it, and the call does **not**
    // return an error — which is what keeps the callback's answer a `connected` deep link.
    fake.fail_calendars(CalendarError::Unauthorized("the grant is gone".to_string()));
    sqlx::query(
        "update calendar_connections set study_calendar_id = null, error = null where id = $1",
    )
    .bind(connection_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
    stvdy_server::calendar::oauth::ensure_study_calendar(&harness.state, connection_id)
        .await
        .unwrap();
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(
        state.status, "connected",
        "a missing study calendar must not fail the connect"
    );
    assert!(
        state
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("could not be read"),
        "{:?}",
        state.error
    );
    assert_eq!(state.study_calendar_id, None);

    // A deployment with no client at all: the same outcome, with the honest reason rather than
    // a panic or a 500.
    let bare = TestState::new(pool);
    stvdy_server::calendar::oauth::ensure_study_calendar(&bare.state, connection_id)
        .await
        .unwrap();
    let state = connection_state(&harness, connection_id).await;
    assert_eq!(state.status, "connected");
    assert!(
        state
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("no Google Calendar client"),
        "{:?}",
        state.error
    );
}

/// The client's **request** is the contract, and a local stub is the only way to assert it
/// without a Google credential.
///
/// Four assertions, and each is one a plausible implementation gets wrong: the cursor is absent
/// on a full listing and present on an incremental one (a client that always sent one, or never
/// did, still passes every "it parsed the response" test), `showDeleted=true` is sent (without
/// it a cancellation never arrives and the local row stays live forever), the bearer header
/// carries the refreshed token, and the calendar id is percent-encoded into the path rather than
/// concatenated into a query.
#[tokio::test]
async fn the_google_client_sends_the_cursor_and_the_bearer_header() {
    let token_stub = stub_http_json(200, r#"{"access_token": "at-1", "expires_in": 3600}"#).await;
    let api_stub = stub_http_json(200, r#"{"items": [], "nextSyncToken": "t1"}"#).await;
    let client = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        api_stub.base_url().to_string(),
        format!("{}/token", token_stub.base_url()),
    )
    .unwrap();

    let access = client.refresh_access_token("rt-1").await.unwrap();
    assert_eq!(access.token, "at-1");

    // The refresh request, as a form: the endpoint takes no JSON, and a client that posted one
    // would be answered `400 unsupported_grant_type`.
    let token_requests = token_stub.requests();
    assert_eq!(token_requests.len(), 1);
    assert_eq!(token_requests[0].method, "POST");
    assert_eq!(token_requests[0].path, "/token");
    for field in [
        "grant_type=refresh_token",
        "client_id=client-id",
        "refresh_token=rt-1",
    ] {
        assert!(
            token_requests[0].body.contains(field),
            "{field} is missing from {}",
            token_requests[0].body
        );
    }

    // A full listing: no cursor, and the calendar id is a path segment.
    let page = client
        .list_events("study@group.calendar.google.com", None)
        .await
        .unwrap();
    assert_eq!(page.next_sync_token.as_deref(), Some("t1"));

    let full = &api_stub.requests()[0];
    assert_eq!(full.method, "GET");
    // Parsed rather than string-matched, because the claim is about *where* the id went: a
    // client that concatenated it into a query string would still contain it somewhere.
    let target = url::Url::parse(&format!("http://stub{}", full.path)).expect("the request target");
    assert_eq!(
        target.path_segments().map(Iterator::collect::<Vec<_>>),
        Some(vec![
            "calendars",
            "study@group.calendar.google.com",
            "events"
        ]),
        "the calendar id is a path segment, not a query parameter: {}",
        full.path
    );
    let query: Vec<(String, String)> = target
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert!(
        query.contains(&("showDeleted".to_string(), "true".to_string())),
        "a cancellation arrives only through a listing that asks for deleted rows: {query:?}"
    );
    assert!(
        query.contains(&("singleEvents".to_string(), "true".to_string())),
        "the occurrence is the row, so the series must arrive expanded (A231): {query:?}"
    );
    assert!(
        !query.iter().any(|(key, _)| key == "syncToken"),
        "a full listing must not send a cursor: {query:?}"
    );
    assert_eq!(full.header("authorization"), Some("Bearer at-1"));

    // An incremental one: the cursor is in the query, and everything else is unchanged.
    client
        .list_events("study@group.calendar.google.com", Some("t1"))
        .await
        .unwrap();
    let incremental = &api_stub.requests()[1];
    let query: Vec<(String, String)> = url::Url::parse(&format!("http://stub{}", incremental.path))
        .expect("the request target")
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert!(
        query.contains(&("syncToken".to_string(), "t1".to_string())),
        "an incremental listing must carry the stored cursor: {query:?}"
    );
    assert!(
        query.contains(&("showDeleted".to_string(), "true".to_string())),
        "an incremental listing is the *only* one that can deliver a cancellation: {query:?}"
    );
    assert_eq!(incremental.header("authorization"), Some("Bearer at-1"));
}

/// The status mapping and the paging rule.
///
/// A `410` is `Gone` — the one failure whose remedy is a *different request* — and a `401` is
/// `Unauthorized`; an unreachable host is `Transport`, which is the only one a retry is worth.
/// The paging half is the trap the trait's own comment names: `nextSyncToken` is present on the
/// **last** page only, so a client that read the first page's would store `None` and turn every
/// later pass into a full listing without anyone noticing (A236).
#[tokio::test]
async fn the_google_client_maps_a_410_to_gone_and_reads_the_last_page_s_cursor() {
    let token_stub = stub_http_json(200, r#"{"access_token": "at-1", "expires_in": 3600}"#).await;
    let token_url = format!("{}/token", token_stub.base_url());

    let gone = stub_http_json(
        410,
        r#"{"error": {"code": 410, "message": "Sync token is no longer valid"}}"#,
    )
    .await;
    let client = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        gone.base_url().to_string(),
        token_url.clone(),
    )
    .unwrap();
    client.refresh_access_token("rt-1").await.unwrap();
    match client.list_events("primary", Some("stale")).await {
        Err(CalendarError::Gone) => {}
        other => panic!("a 410 must be Gone, not {other:?}"),
    }

    let unauthorized = stub_http_json(
        401,
        r#"{"error": {"code": 401, "message": "Invalid Credentials"}}"#,
    )
    .await;
    let client = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        unauthorized.base_url().to_string(),
        token_url.clone(),
    )
    .unwrap();
    client.refresh_access_token("rt-1").await.unwrap();
    match client.list_events("primary", None).await {
        Err(CalendarError::Unauthorized(reason)) => assert_eq!(reason, "Invalid Credentials"),
        other => panic!("a 401 must be Unauthorized, not {other:?}"),
    }

    // A host that refuses the connection: `Transport`, never `Provider` — the distinction is
    // what decides whether the worker retries or the connection records an error.
    let unreachable = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        "http://127.0.0.1:1".to_string(),
        "http://127.0.0.1:1/token".to_string(),
    )
    .unwrap();
    match unreachable.refresh_access_token("rt-1").await {
        Err(CalendarError::Transport(_)) => {}
        other => panic!("an unreachable host must be Transport, not {other:?}"),
    }

    // The paging rule, with a stub that answers differently per request: a one-body stub cannot
    // express it at all — every request would carry the same `nextPageToken` and the client would
    // page until the test timed out.
    let pages = PageStub::start(vec![
        r#"{"items": [
             {"id": "evt-a", "etag": "e1", "status": "confirmed", "summary": "A",
              "start": {"dateTime": "2026-06-10T01:00:00Z"},
              "end": {"dateTime": "2026-06-10T02:00:00Z"}}
           ], "nextPageToken": "p2"}"#
            .to_string(),
        r#"{"items": [
             {"id": "evt-b", "etag": "e2", "status": "confirmed", "summary": "B",
              "start": {"dateTime": "2026-06-11T01:00:00Z"},
              "end": {"dateTime": "2026-06-11T02:00:00Z"}},
             {"id": "evt-libur", "etag": "l1", "status": "confirmed", "summary": "Libur",
              "start": {"date": "2026-06-12"}, "end": {"date": "2026-06-13"}}
           ], "nextSyncToken": "t9"}"#
            .to_string(),
    ])
    .await;
    let client = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        pages.base_url().to_string(),
        token_url,
    )
    .unwrap();
    client.refresh_access_token("rt-1").await.unwrap();

    let page = client.list_events("primary", None).await.unwrap();
    assert_eq!(
        page.events.len(),
        3,
        "both pages must be read: one event on the first, two on the second"
    );
    assert_eq!(
        page.next_sync_token.as_deref(),
        Some("t9"),
        "the cursor is the last page's, because the intermediate pages carry none"
    );
    let paths = pages.paths();
    assert_eq!(paths.len(), 2);
    assert!(!paths[0].contains("pageToken"), "{}", paths[0]);
    assert!(
        paths[1].contains("pageToken=p2"),
        "the second request must ask for the second page: {}",
        paths[1]
    );
    // A date-only event has no instant in it, and inventing midnight UTC would place a Jakarta
    // all-day event at 07:00 local.
    let all_day = page
        .events
        .iter()
        .find(|event| event.id == "evt-libur")
        .expect("the all-day event");
    assert!(
        all_day.start.is_none() && all_day.end.is_none(),
        "a `date` is not an instant: {all_day:?}"
    );
    assert!(
        !all_day.is_cancelled(),
        "a missing status is not a cancellation"
    );
}

/// A request target's path segments, parsed rather than string-matched: the claim is about
/// *where* a value went, and a client that concatenated the calendar id into a query string
/// would still contain it somewhere in the target.
fn path_segments(target: &str) -> Vec<String> {
    url::Url::parse(&format!("http://stub{target}"))
        .expect("the request target")
        .path_segments()
        .map(|segments| segments.map(str::to_string).collect())
        .unwrap_or_default()
}

/// The outbound client's **request** is the contract, and nothing asserted it before: the push
/// half's own slice reported `upsert_event` as tested for nothing, and a client that posted to
/// `primary`, or to a path without the calendar id, or with no bearer header, would have passed
/// every test in this file.
///
/// Four assertions. The method is `POST` for a row Google has never seen — an empty id is what
/// makes Google assign one — and `PATCH` for a row it has, with the id as the last path
/// segment. The calendar id is a **path segment** on both, and it is the id the caller passed
/// rather than a hard-coded `primary`. The bearer header carries the refreshed token. And the
/// body is the row's content, with instants as `dateTime` rather than as a `date`.
#[tokio::test]
async fn the_google_client_inserts_an_unpushed_event_and_patches_a_known_one() {
    let token_stub = stub_http_json(200, r#"{"access_token": "at-1", "expires_in": 3600}"#).await;
    let api_stub = stub_http_json(
        200,
        r#"{"id": "g-1", "etag": "v1", "status": "confirmed", "summary": "Blok belajar",
            "start": {"dateTime": "2026-06-14T01:00:00Z"},
            "end": {"dateTime": "2026-06-14T02:00:00Z"}}"#,
    )
    .await;
    let client = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        api_stub.base_url().to_string(),
        format!("{}/token", token_stub.base_url()),
    )
    .unwrap();
    client.refresh_access_token("rt-1").await.unwrap();

    let created = client
        .upsert_event(
            "study@group.calendar.google.com",
            &remote(
                "",
                "",
                "Blok belajar",
                "2026-06-14T01:00:00Z",
                "2026-06-14T02:00:00Z",
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        created.id, "g-1",
        "the answer's id is what the write-back stores, and what makes the next push a patch"
    );

    let insert = &api_stub.requests()[0];
    assert_eq!(insert.method, "POST", "an event with no id is an insert");
    assert_eq!(
        path_segments(&insert.path),
        vec!["calendars", "study@group.calendar.google.com", "events"],
        "the calendar id is a path segment, not a query parameter: {}",
        insert.path
    );
    assert_eq!(insert.header("authorization"), Some("Bearer at-1"));
    let body: serde_json::Value = serde_json::from_str(&insert.body).expect("a JSON body");
    assert_eq!(body["summary"], "Blok belajar");
    assert_eq!(body["start"]["dateTime"], "2026-06-14T01:00:00Z");
    assert_eq!(body["end"]["dateTime"], "2026-06-14T02:00:00Z");

    // The row Google already has: the id names the event, and the method is a patch.
    client
        .upsert_event(
            "study@group.calendar.google.com",
            &remote(
                "g-1",
                "v1",
                "Blok belajar",
                "2026-06-14T01:00:00Z",
                "2026-06-14T02:00:00Z",
            ),
        )
        .await
        .unwrap();
    let patch = &api_stub.requests()[1];
    assert_eq!(
        patch.method, "PATCH",
        "an event Google knows is patched, never inserted a second time"
    );
    assert_eq!(
        path_segments(&patch.path),
        vec![
            "calendars",
            "study@group.calendar.google.com",
            "events",
            "g-1"
        ],
        "the event id is the last path segment: {}",
        patch.path
    );
    assert_eq!(patch.header("authorization"), Some("Bearer at-1"));
}

/// The push's status mapping, which is the listing's: `410` is `Gone`, and `401` and `403` are
/// both `Unauthorized`.
///
/// Asserted on `upsert_event` rather than only on `list_events`, because they are different call
/// sites and the consequences differ: a `410` mapped to `Transport` would retry a request that
/// cannot succeed, and a revoked grant mapped to `Provider` would record an error on the
/// connection instead of marking it `revoked` (A247).
#[tokio::test]
async fn the_google_client_maps_a_refused_upsert_to_gone_or_unauthorized() {
    let token_stub = stub_http_json(200, r#"{"access_token": "at-1", "expires_in": 3600}"#).await;
    let token_url = format!("{}/token", token_stub.base_url());

    let gone = stub_http_json(
        410,
        r#"{"error": {"code": 410, "message": "Resource has been deleted"}}"#,
    )
    .await;
    let client = GoogleCalendarApi::with_endpoints(
        "client-id".to_string(),
        "client-secret".to_string(),
        gone.base_url().to_string(),
        token_url.clone(),
    )
    .unwrap();
    client.refresh_access_token("rt-1").await.unwrap();
    match client
        .upsert_event(
            "study-plan-1",
            &remote(
                "",
                "",
                "Blok belajar",
                "2026-06-14T01:00:00Z",
                "2026-06-14T02:00:00Z",
            ),
        )
        .await
    {
        Err(CalendarError::Gone) => {}
        other => panic!("a 410 must be Gone, not {other:?}"),
    }

    for (status, body) in [
        (
            401,
            r#"{"error": {"code": 401, "message": "Invalid Credentials"}}"#,
        ),
        (
            403,
            r#"{"error": {"code": 403, "message": "Request had insufficient authentication scopes."}}"#,
        ),
    ] {
        let refused = stub_http_json(status, body).await;
        let client = GoogleCalendarApi::with_endpoints(
            "client-id".to_string(),
            "client-secret".to_string(),
            refused.base_url().to_string(),
            token_url.clone(),
        )
        .unwrap();
        client.refresh_access_token("rt-1").await.unwrap();
        match client
            .upsert_event(
                "study-plan-1",
                &remote(
                    "g-1",
                    "v1",
                    "Blok belajar",
                    "2026-06-14T01:00:00Z",
                    "2026-06-14T02:00:00Z",
                ),
            )
            .await
        {
            Err(CalendarError::Unauthorized(reason)) => {
                assert!(
                    !reason.is_empty(),
                    "the reason is what the screen shows: {reason}"
                )
            }
            other => panic!("a {status} must be Unauthorized, not {other:?}"),
        }
    }
}

/// A stub that answers each request with the next body in its script, and records the request
/// targets it was asked for.
///
/// [`stub_http_json`] answers one body forever, which cannot express a paged listing. Every
/// request this client makes here is a `GET` with no body, so the reader stops at the blank line
/// and never reads a payload.
struct PageStub {
    base_url: String,
    paths: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl PageStub {
    async fn start(bodies: Vec<String>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a stub listener");
        let address = listener.local_addr().expect("a stub address");
        let paths = Arc::new(Mutex::new(Vec::new()));
        let recorded = paths.clone();

        let handle = tokio::spawn(async move {
            let mut script: VecDeque<String> = bodies.into();
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    }
                }
                let head = String::from_utf8_lossy(&buffer).to_string();
                let target = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                recorded.lock().push(target);
                // The last answer repeats, so a client that pages once more than the script
                // covers is answered rather than hung.
                let body = if script.len() > 1 {
                    script.pop_front().unwrap_or_default()
                } else {
                    script.front().cloned().unwrap_or_default()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                     connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });

        Self {
            base_url: format!("http://{address}"),
            paths,
            handle,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Every request target received, in order.
    fn paths(&self) -> Vec<String> {
        self.paths.lock().clone()
    }
}

impl Drop for PageStub {
    fn drop(&mut self) {
        // Aborted rather than leaked, for the reason `StubHttp`'s drop gives: a listener per
        // case that outlives the test is a file descriptor per case.
        self.handle.abort();
    }
}
