//! The calendar's four surfaces: the feed out, the ICS import, the consent flow and
//! the two-way sync. Split by surface, not by module, because each has a failure the
//! others cannot produce: a feed a client refuses, a private host reached, a token in a
//! deep link, a sync that stopped being incremental.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use sqlx::PgPool;
use std::sync::Arc;
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::testing::{Scripted, ScriptedIcsFetch, TestState};
use tower::ServiceExt;
use uuid::Uuid;

mod common;
use common::{json, signed_up};

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
