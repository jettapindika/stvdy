//! Roadmap gate 6, the `Asia/Jakarta` half: the whole flow, in the zone the product ships in.
//!
//! The gate is called out for this phase specifically, and it has two halves that fail in
//! different ways.
//!
//! **The data half is sharp, and it is the reason this test exists.** An exam at 23:00 Jakarta
//! on the 16th is `16:00Z` on the 16th, but one at **06:00 Jakarta on the 16th is `23:00Z` on
//! the 15th**. An implementation that buckets instants by their UTC date gets the second one
//! wrong and the first one right, so a test that only looked at an evening exam would pass
//! while the student's exam sat a day early on their calendar. Every assertion below is made
//! through `chrono_tz`, never through a fixed offset.
//!
//! **The copy half is the locale files.** The proposal's warnings are sentences with a count in
//! them (`planner.overloaded`) and a goal name (`planner.pastDeadline`), and a sentence whose
//! placeholder is missing renders as `{{count}} days are over …` — a string that reads as an app
//! bug rather than as a warning. Both languages are asserted, with the same placeholder set,
//! because an Indonesian student is the one this zone default exists for.
//!
//! Jakarta has no DST, which is exactly why the DST arithmetic is tested on `Europe/London` in
//! `crates/server/tests/planner.rs` and this test is about the zone the product ships in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, NaiveTime, TimeZone as _, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use stvdy_core::traits::testing::{FixedClock, ScriptedLlm};
use stvdy_server::testing::TestState;

mod common;
use common::{drain_jobs, json, multipart, post_json, signed_up};

/// The extraction: one exam at **06:00 Jakarta**, which is the sharp case, and one deadline at
/// an evening Jakarta time, which is the case a UTC implementation gets right by accident.
fn extraction_reply() -> String {
    r#"{"course":{"name":"Jaringan Komputer","code":"IF2230","instructor":null},
        "meetings":[],
        "deadlines":[
          {"title":"Tugas 1","due":"2026-11-12T23:59:00+07:00","weight_percent":15.0,
           "evidence":"Tugas 1 dikumpulkan 12 Nov","confidence":0.9}],
        "exams":[
          {"title":"UAS","starts_at":"2026-11-16T06:00:00+07:00","ends_at":null,
           "topics":["Graphs"],"evidence":"UAS 16 November pukul 06:00","confidence":0.95}],
        "notes":[]}"#
        .to_string()
}

/// The effort reply, one entry per topic, named after the source it estimates.
fn effort_reply(topics: &[(&str, i64)]) -> String {
    let entries: Vec<String> = topics
        .iter()
        .map(|(name, minutes)| {
            format!(
                "{{\"topic\":\"{name}\",\"estimate_min\":{minutes},\"reason\":\"large source\"}}"
            )
        })
        .collect();
    let total: i64 = topics.iter().map(|(_, minutes)| minutes).sum();
    format!(
        "{{\"topics\":[{}],\"total_estimate_min\":{total}}}",
        entries.join(",")
    )
}

fn router(harness: &TestState) -> Router {
    stvdy_server::routes::router(harness.state.clone()).unwrap()
}

/// Moves an account onto a paid plan with a live subscription, which is what `effective_plan`
/// requires (A146): a bare `users.plan_id` edit is not enough, and the free plan refuses the
/// planner outright.
async fn upgrade(harness: &TestState, email: &str, plan: &str) {
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    sqlx::query("update users set plan_id = $2 where id = $1")
        .bind(user_id)
        .bind(plan)
        .execute(&harness.state.pool)
        .await
        .unwrap();
    let now = harness.state.clock.now();
    sqlx::query(
        "insert into subscriptions
           (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly', $3, $4)",
    )
    .bind(user_id)
    .bind(plan)
    .bind(now - Duration::days(1))
    .bind(now + Duration::days(29))
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

/// The student's own three knobs, stated here so the window the assertions read against is a
/// number this test chose: 08:00–22:00 Jakarta, 240 minutes a day.
async fn set_settings(harness: &TestState, user_id: Uuid) {
    sqlx::query(
        "insert into user_settings
           (user_id, study_start_hour, study_end_hour, max_study_min_per_day, block_len_min, break_min)
         values ($1, 8, 22, 240, 50, 10)
         on conflict (user_id) do update set
           study_start_hour = 8, study_end_hour = 22, max_study_min_per_day = 240,
           block_len_min = 50, break_min = 10",
    )
    .bind(user_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();
}

async fn user_id_of(harness: &TestState, email: &str) -> Uuid {
    sqlx::query_scalar("select id from users where email = $1")
        .bind(email)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap()
}

/// The student's timezone, as the product defaults it.
fn jakarta() -> chrono_tz::Tz {
    "Asia/Jakarta".parse().unwrap()
}

/// One locale file, read from the repository.
///
/// The copy half of the gate lives in `app/src/i18n/locales/*.json`, and the sentence a student
/// sees for an overloaded week is rendered from there. Reading the file here is the only way a
/// Rust test can assert that the sentence has no unfilled placeholder left in it, and it is
/// what makes this gate about the copy rather than about the warning enum.
fn locale(name: &str) -> serde_json::Value {
    let path = format!(
        "{}/../../app/src/i18n/locales/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parsing {path}: {error}"))
}

/// Every `{{name}}` in a locale string.
fn placeholders(value: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        found.push(after[..close].to_string());
        rest = &after[close + 2..];
    }
    found.sort();
    found
}

/// One locale string by its dotted key, or a panic naming the key.
fn copy(resource: &serde_json::Value, key: &str) -> String {
    let mut node = resource;
    for part in key.split('.') {
        node = node
            .get(part)
            .unwrap_or_else(|| panic!("{key} is missing from the locale file"));
    }
    node.as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
        .to_string()
}

/// The copy half of gate 6: the proposal's warnings, in both languages, with every placeholder
/// filled.
#[test]
fn the_planner_warning_copy_is_complete_in_both_languages() {
    let en = locale("en");
    let id = locale("id");

    // The three sentences a proposal can put in front of the student. Each is a sentence with
    // a value in it: `planner.overloaded` carries the count of days the week could not hold,
    // `planner.pastDeadline` the goal whose date has gone, and `planner.deficit` the minutes
    // that did not fit.
    for key in [
        "planner.overloaded_other",
        "planner.pastDeadline",
        "planner.deficit",
    ] {
        let english = copy(&en, key);
        let indonesian = copy(&id, key);

        assert!(
            !english.is_empty() && !indonesian.is_empty(),
            "{key} must carry real copy in both languages"
        );
        assert_eq!(
            placeholders(&english),
            placeholders(&indonesian),
            "{key}: the two languages must interpolate the same values, or one of them renders \
             the placeholder literally"
        );
        assert!(
            !placeholders(&english).is_empty(),
            "{key} is a sentence with a value in it; a string with no placeholder would be a \
             different warning"
        );
    }

    // The rendered sentences, with the values the screen passes. A missing placeholder shows up
    // here as a `{{` that survived into the sentence the student reads.
    let rendered = [
        (
            "en.planner.overloaded",
            copy(&en, "planner.overloaded_other").replace("{{count}}", "3"),
        ),
        (
            "id.planner.pastDeadline",
            copy(&id, "planner.pastDeadline").replace("{{goal}}", "UAS Jaringan Komputer"),
        ),
        (
            "en.planner.deficit",
            copy(&en, "planner.deficit").replace("{{minutes}}", "480"),
        ),
    ];
    for (name, sentence) in rendered {
        assert!(
            !sentence.contains("{{") && !sentence.contains("}}"),
            "{name} rendered with a placeholder left in it: {sentence}"
        );
        assert!(
            sentence.chars().any(|c| c.is_alphanumeric()),
            "{name} rendered as nothing: {sentence}"
        );
        println!("{name} -> {sentence}");
    }
}

/// **Gate 6.** The whole flow — upload, extract, confirm, plan, accept — in `Asia/Jakarta`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_planner_and_its_copy_are_correct_in_asia_jakarta(pool: PgPool) {
    // A fixed Monday morning in Jakarta, so the horizon and the week are the same on every run.
    let clock = FixedClock::at(Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 0).unwrap());
    let harness = TestState::new(pool)
        .with_clock(Arc::new(clock))
        .with_llm(Arc::new(ScriptedLlm::new(vec![
            extraction_reply(),
            effort_reply(&[("Silabus Jaringan", 300)]),
        ])));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up(&app, &harness, "jakarta@example.test").await;
    upgrade(&harness, "jakarta@example.test", "pro").await;
    let user_id = user_id_of(&harness, "jakarta@example.test").await;
    set_settings(&harness, user_id).await;

    let timezone: String = sqlx::query_scalar("select timezone from users where id = $1")
        .bind(user_id)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(
        timezone, "Asia/Jakarta",
        "the default zone is what this gate is about, and it must be the stored one"
    );

    let (status, body) = json(
        &app,
        "POST",
        "/workspaces",
        &token,
        serde_json::json!({ "name": "Jaringan" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let workspace_id = Uuid::parse_str(body["workspace"]["id"].as_str().unwrap()).unwrap();

    // --- the syllabus, uploaded as pasted text ------------------------------------------

    let (content_type, upload_body) = multipart(&[
        (
            "text",
            "UAS 16 November pukul 06:00\nTugas 1 dikumpulkan 12 Nov",
            None,
        ),
        ("title", "Silabus Jaringan", None),
    ]);
    let (status, uploaded) = post_json(
        &app,
        "POST",
        &format!("/workspaces/{workspace_id}/syllabus"),
        &token,
        Some(&content_type),
        upload_body,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{uploaded}");
    let draft_id = Uuid::parse_str(uploaded["draft_id"].as_str().unwrap()).unwrap();

    drain_jobs(&harness).await;

    let (status, draft) = json(
        &app,
        "GET",
        &format!("/syllabus/{draft_id}/draft"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{draft}");
    assert_eq!(draft["status"], "ready", "{draft}");

    let items = draft["items"].as_array().expect("items");
    let exam = items
        .iter()
        .find(|item| item["kind"] == "exam")
        .expect("the exam");
    let deadline = items
        .iter()
        .find(|item| item["kind"] == "deadline")
        .expect("the deadline");

    // **The sharp pair.** 06:00 on the 16th in Jakarta is 23:00Z on the **15th**, and a
    // UTC-bucketing implementation puts it on the 16th. The evening deadline is the case that
    // same implementation gets right by accident, which is why both are asserted.
    assert_eq!(
        exam["when"], "2026-11-15T23:00:00Z",
        "an exam at 06:00 Jakarta on the 16th is 23:00Z on the 15th: {exam}"
    );
    assert_eq!(
        deadline["when"], "2026-11-12T16:59:00Z",
        "23:59 Jakarta on the 12th is 16:59Z on the 12th: {deadline}"
    );
    let exam_local = exam["when"]
        .as_str()
        .unwrap()
        .parse::<DateTime<Utc>>()
        .unwrap()
        .with_timezone(&jakarta());
    println!(
        "Jakarta pair: 2026-11-16T06:00+07:00 -> {} (stored {}), local day {}",
        exam["when"].as_str().unwrap(),
        exam["when"].as_str().unwrap(),
        exam_local.date_naive()
    );
    assert_eq!(
        exam_local.date_naive().to_string(),
        "2026-11-16",
        "read back through chrono_tz, the exam is still on Jakarta's 16th: {exam_local}"
    );

    // The list arrives pre-ordered with a `needs_check` flag and **no score** (A261).
    for item in items {
        assert!(
            item.get("confidence").is_none(),
            "the review list must not carry the extraction's score: {item}"
        );
        assert!(
            item.get("needs_check").is_some(),
            "every item says whether it needs a check: {item}"
        );
    }

    // --- the confirm: the only writer of the rows ----------------------------------------

    let kept: Vec<serde_json::Value> = items
        .iter()
        .map(|item| serde_json::json!({ "index": item["index"], "keep": true }))
        .collect();
    let (status, confirmed) = json(
        &app,
        "POST",
        &format!("/syllabus/{draft_id}/confirm"),
        &token,
        serde_json::json!({ "items": kept }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(
        confirmed["goals"], 1,
        "the kept exam is one goal: {confirmed}"
    );
    assert_eq!(
        confirmed["tasks"], 1,
        "the kept deadline is one task: {confirmed}"
    );

    // The stored instants, read back through the database rather than through the response:
    // a UTC-stored, offset-rendered date survives a response assertion and fails here.
    let goal: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "select id, target_date from goals where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        goal.1,
        Utc.with_ymd_and_hms(2026, 11, 15, 23, 0, 0).unwrap(),
        "the goal's target_date is the exam's own instant"
    );
    let stored_goal: String = sqlx::query_scalar(
        "select to_char(target_date at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')
           from goals where id = $1",
    )
    .bind(goal.0)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        stored_goal, "2026-11-15T23:00:00Z",
        "the stored instant moved"
    );
    assert_eq!(
        goal.1.with_timezone(&jakarta()).date_naive().to_string(),
        "2026-11-16",
        "the goal's local day is Jakarta's 16th, not the 15th"
    );

    let stored_due: String = sqlx::query_scalar(
        "select to_char(due_at at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')
           from tasks where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        stored_due, "2026-11-12T16:59:00Z",
        "a due of 2026-11-12T23:59:00+07:00 survives the round trip unchanged"
    );

    // --- the plan, and the day boundaries the planner works in ---------------------------

    let goal_id = goal.0;

    let (status, requested) = json(
        &app,
        "POST",
        &format!("/goals/{goal_id}/plan"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{requested}");
    let proposal_id = Uuid::parse_str(requested["proposal_id"].as_str().unwrap()).unwrap();
    drain_jobs(&harness).await;

    let (status, polled) = json(
        &app,
        "GET",
        &format!("/plans/{proposal_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    let proposal = &polled["proposal"];
    assert_eq!(proposal["status"], "ready", "{proposal}");

    let blocks = proposal["proposal"]["blocks"].as_array().expect("blocks");
    assert!(!blocks.is_empty(), "the gate needs a plan: {proposal}");

    // The day buckets are **Jakarta's** days: every `day_load` date is the local date of the
    // blocks that fall on it, which a UTC-bucketed plan cannot satisfy.
    let day_load = proposal["proposal"]["day_load"]
        .as_array()
        .expect("day_load");
    assert!(!day_load.is_empty(), "a plan has a day load: {proposal}");
    for day in day_load {
        let date = day["date"].as_str().unwrap();
        let on_that_day: i64 = blocks
            .iter()
            .filter(|block| {
                block["starts_at"]
                    .as_str()
                    .unwrap()
                    .parse::<DateTime<Utc>>()
                    .unwrap()
                    .with_timezone(&jakarta())
                    .date_naive()
                    .to_string()
                    == date
            })
            .count() as i64;
        assert!(
            on_that_day > 0,
            "day_load names {date}, which holds no block in Jakarta's own days: {proposal}"
        );
    }

    let accepted: Vec<serde_json::Value> = blocks
        .iter()
        .map(|block| {
            serde_json::json!({
                "starts_at": block["starts_at"],
                "ends_at": block["ends_at"],
                "goal_id": block["goal_id"],
                "topic": block["topic"],
                "kind": if block["kind"] == "Review" { "review" } else { "new_material" },
            })
        })
        .collect();
    let (status, accepted_body) = json(
        &app,
        "POST",
        "/plans/accept",
        &token,
        serde_json::json!({ "proposal_id": proposal_id, "blocks": accepted }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted_body}");

    // --- every written block, read back through chrono_tz --------------------------------

    let written: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "select starts_at, ends_at from events
          where user_id = $1 and kind = 'study_block' and deleted_at is null
          order by starts_at",
    )
    .bind(user_id)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();
    assert!(!written.is_empty(), "the accept wrote the blocks");

    let open = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
    let close = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    for (start, end) in &written {
        let local_start = start.with_timezone(&jakarta());
        let local_end = end.with_timezone(&jakarta());
        assert!(
            local_start.time() >= open,
            "a block starts before 08:00 Jakarta: {local_start}"
        );
        assert!(
            local_end.time() <= close || local_end.time() == midnight,
            "a block ends after 22:00 Jakarta: {local_end}"
        );
    }

    println!(
        "Jakarta gate: {} block(s) written, all inside 08:00-22:00 Asia/Jakarta; goal local day {}",
        written.len(),
        goal.1.with_timezone(&jakarta()).date_naive()
    );
}
