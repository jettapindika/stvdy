//! Roadmap gate 6, the money half: the invoice, the period end and the copy, in the zone
//! the product sells in.
//!
//! Phase 11's `planner_jakarta.rs` is the same gate on the planner, and it states the rule
//! this file inherits: **every assertion is made through `chrono_tz`, never through a fixed
//! offset.** An offset is right for Jakarta and wrong for every zone with a DST transition,
//! and the product's local-date maths (a reminder at the student's own 09:00, a period that
//! ends on the student's own day) is exactly where that difference lands.
//!
//! Three things this file pins, each with a way of being silently wrong:
//!
//!  1. **`expires_at` is an absolute instant, and the Jakarta user's clock reads it as
//!     `06:15` on a day the UTC clock calls the 31st.** The clock is fixed at
//!     `2026-08-31T23:00:00Z`, which is **2026-09-01 06:00 in Jakarta** — the sharp case.
//!     A client that rendered the UTC wall time would show a student a code expiring "at
//!     23:15 yesterday", and a server that computed local dates by bucketing the UTC date
//!     would file the payment under August.
//!  2. **A 30-day period that starts on 2026-09-01 Jakarta ends on 2026-10-01 Jakarta**,
//!     read back through the zone. That crosses a Jakarta month boundary, which is where a
//!     30-day period and a "same day next month" period diverge — and the local end date is
//!     a day later than the UTC end date for this payment, which is the reading a
//!     UTC-bucketed implementation reports.
//!  3. **The copy.** The keys this phase added are asserted in both locales with identical
//!     placeholder sets and rendered with values, because an Indonesian student who gets
//!     `Kode kedaluwarsa dalam {{time}}` is reading an app bug rather than a countdown. The
//!     rendered sentences are printed, so the gate's output is the sentence and not a green
//!     word.
//!
//! **What this file cannot assert, recorded rather than implied.** `renewal::PLAN_ENDING_KEY`
//! (`"reminder.planEnding"`) is the key the server sends beside a renewal push body, and it
//! has **no entry in `en.json` or `id.json` and no renderer in the SPA** — the feed row the
//! renewal writes (`kind = 'renewal'`, `subject_id = null`) is dropped by
//! `useLocalNotifications.ts::feedKey`, which returns `null` for a row with no subject. So
//! the reminder copy in Indonesian does not exist yet, and asserting it here would be
//! asserting a contract the tree does not implement. It is reported as a gate-6 finding with
//! the exact fix (two locale entries plus one renderer, or the server stops sending a key
//! nothing reads); what *is* asserted below is the copy the phase's screens actually render,
//! including the in-app renewal line (`plan.renewalDate`), which is the label a student
//! sees for a period ending.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, TimeZone as _, Utc};
use chrono_tz::Tz;
use sqlx::PgPool;
use stvdy_core::traits::testing::FixedClock;
use stvdy_server::testing::TestState;

mod common;
use common::{json, signed_up_on};

/// Signs the same account in **from a second device**, which is the exit gate's own
/// sentence ("plan active on the second device").
///
/// A login rather than a second signup: the address already exists, so a second signup is
/// a 409 — and a "second device" that had to invent a second account would be proving
/// nothing about this one.
async fn signed_in_on(app: &Router, email: &str, platform: &str) -> String {
    let (status, body) = json(
        app,
        "POST",
        "/auth/login",
        "",
        serde_json::json!({
            "email": email,
            "password": "correct horse battery",
            "device": { "name": "Second device", "platform": platform }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"]
        .as_str()
        .expect("an access token")
        .to_string()
}

/// The instant this gate is about: **2026-09-01 06:00 in Jakarta**, which is
/// `2026-08-31T23:00Z`. A Jakarta morning that the UTC clock still calls the previous
/// evening, and therefore the case a UTC-bucketed implementation gets wrong while an
/// evening instant would have passed by accident.
fn jakarta_morning() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 31, 23, 0, 0).unwrap()
}

/// The student's zone, as the product defaults it.
fn jakarta() -> Tz {
    "Asia/Jakarta".parse().unwrap()
}

/// An instant as the Jakarta user's own clock reads it, through `chrono_tz`.
fn local(instant: DateTime<Utc>) -> String {
    instant
        .with_timezone(&jakarta())
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// The local calendar day, through `chrono_tz` — the form every "which day is this" claim
/// in the product takes.
fn local_day(instant: DateTime<Utc>) -> String {
    instant.with_timezone(&jakarta()).date_naive().to_string()
}

fn router(harness: &TestState) -> Router {
    stvdy_server::routes::router(harness.state.clone()).unwrap()
}

/// A signed-in account on the web, which is the platform checkout is sold on (A151).
async fn account(pool: PgPool) -> (TestState, Router, String) {
    let harness = TestState::new(pool).with_clock(Arc::new(FixedClock::at(jakarta_morning())));
    harness.seed_plans().await.unwrap();
    let app = router(&harness);
    let token = signed_up_on(&app, &harness, "jakarta-payer@example.test", "web").await;
    (harness, app, token)
}

/// One locale file, read from the repository.
///
/// The copy half of the gate lives in `app/src/i18n/locales/*.json`, and the sentence a
/// student reads is rendered from there. Reading the file here is the only way a Rust test
/// can assert that the sentence has no unfilled placeholder left in it.
fn locale(name: &str) -> serde_json::Value {
    let path = format!(
        "{}/../../app/src/i18n/locales/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parsing {path}: {error}"))
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

/// Every leaf key beneath a namespace, dotted.
fn keys_below(resource: &serde_json::Value, namespace: &str) -> Vec<String> {
    fn walk(node: &serde_json::Value, prefix: String, out: &mut Vec<String>) {
        match node {
            serde_json::Value::String(_) => out.push(prefix),
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    walk(child, format!("{prefix}.{key}"), out);
                }
            }
            other => panic!("{prefix} is neither a string nor an object: {other}"),
        }
    }

    let root = resource
        .get(namespace)
        .unwrap_or_else(|| panic!("the locale file has no {namespace} namespace"));
    let mut out = Vec::new();
    walk(root, namespace.to_string(), &mut out);
    out.sort();
    out
}

/// Fills every `{{name}}` with a value, so a surviving placeholder is visible.
fn render(template: &str) -> String {
    let mut rendered = template.to_string();
    for name in placeholders(template) {
        rendered = rendered.replace(&format!("{{{{{name}}}}}"), &value_for(&name));
    }
    rendered
}

/// The value a placeholder gets in the rendered sentence. The number is a Jakarta-shaped
/// one (`1.500` is `id-ID`'s grouping, and the locale strings carry `{{count, number}}`),
/// and the date is the day the gate's own instant falls on.
fn value_for(name: &str) -> String {
    match name {
        "time" => "06:15".to_string(),
        "date" => "1 Oktober 2026".to_string(),
        "percent" => "50".to_string(),
        "plan" => "Pro".to_string(),
        "orderId" => "stvdy_01j0example".to_string(),
        "code" => "KAMPUS50".to_string(),
        other => format!("<{other}>"),
    }
}

/// **Gate 6, the copy half.** Every key this phase added, in both languages, with its
/// placeholders filled.
///
/// A `#[test]` rather than a `#[sqlx::test]`: this half is about the locale files and needs
/// no database.
#[test]
fn the_money_copy_is_complete_in_both_languages() {
    let en = locale("en");
    let id = locale("id");

    // The namespaces this phase added (`checkout`, `pricing`, `billing`) plus the two
    // `plan` keys the plan screen renders for a paid period. `plan` is Phase 7's namespace
    // and only these two keys are this phase's, so it is named rather than swept.
    let mut keys: Vec<String> = Vec::new();
    for namespace in ["checkout", "pricing", "billing"] {
        keys.extend(keys_below(&en, namespace));
    }
    keys.extend([
        // The label the plan screen renders for a paid period. It replaced
        // `plan.periodEnd`, which was a whole sentence built from the instant; the
        // design labels the row and prints the date beside it through `formatInZone`,
        // which is the behaviour this file exists to check.
        "plan.renewalDate".to_string(),
        "plan.subscription".to_string(),
    ]);
    // Indonesian has no plural form, so `_one` exists only in English — the same rule
    // `app/src/i18n/i18n.test.ts` states. Walking English's keys would otherwise report
    // `pricing.creditsIncluded_one` as missing from `id.json`, which is correct copy rather
    // than a defect. The `_other` form is in both files and is what this half checks.
    keys.retain(|key| !key.ends_with("_one"));

    assert!(
        keys.len() > 40,
        "the phase added a screen's worth of copy; {keys:?} is too few keys to be it"
    );

    for key in &keys {
        let english = copy(&en, key);
        let indonesian = copy(&id, key);

        assert!(
            !english.is_empty() && !indonesian.is_empty(),
            "{key} must carry real copy in both languages"
        );
        assert_eq!(
            placeholders(&english),
            placeholders(&indonesian),
            "{key}: the two languages must interpolate the same values, or one of them \
             renders the placeholder literally"
        );

        // **A sentence has to be translated; a loanword does not.** `checkout.line.total` is
        // "Total" in both languages and `billing.historyPlan` is a template with no words in
        // it at all, so a blanket "the strings must differ" would fail correct copy. The rule
        // is about sentences: an English sentence of four words or more that survives
        // unchanged into `id.json` is copy nobody translated, which is exactly the failure
        // this half is here for.
        let words = english.split_whitespace().count();
        if words >= 4 {
            assert_ne!(
                english, indonesian,
                "{key}: a {words}-word English sentence is identical in Indonesian, which is \
                 copy nobody translated"
            );
        }

        // And rendered, because a placeholder that survives into the sentence is the
        // failure this half exists for.
        let sentence = render(&indonesian);
        assert!(
            !sentence.contains("{{") && !sentence.contains("}}"),
            "{key} rendered in Indonesian with a placeholder left in it: {sentence}"
        );
        assert!(
            sentence.chars().any(|c| c.is_alphanumeric()),
            "{key} rendered as nothing: {sentence}"
        );
    }

    // The sentences gate 6 asks to be quoted, with the values the screens pass. The
    // countdown is the one that matters most: it is the number a student watches while
    // deciding whether they still have time to pay.
    let quoted = [
        (
            "id.checkout.expiresIn",
            render(&copy(&id, "checkout.expiresIn")),
        ),
        (
            "en.checkout.expiresIn",
            render(&copy(&en, "checkout.expiresIn")),
        ),
        (
            "id.pricing.studentBanner",
            render(&copy(&id, "pricing.studentBanner")),
        ),
        (
            "id.checkout.line.total",
            render(&copy(&id, "checkout.line.total")),
        ),
        (
            "id.checkout.state.review.detail",
            render(&copy(&id, "checkout.state.review.detail")),
        ),
        (
            "id.plan.renewalDate",
            render(&copy(&id, "plan.renewalDate")),
        ),
    ];
    for (name, sentence) in quoted {
        assert!(
            !sentence.contains("{{") && !sentence.contains("}}"),
            "{name} rendered with a placeholder left in it: {sentence}"
        );
        println!("{name} -> {sentence}");
    }

    // The five spec checkout states are **spoken**, not coloured: each carries a title, a
    // chip word and a sentence, and the words are what a screen reader announces. The
    // assertion is on the words because that is the accessibility contract (gate 5d), and
    // it is asserted here as well because a state whose Indonesian copy lost its title is
    // a state a student cannot read.
    for state in ["waiting", "paid", "expired", "review", "failed"] {
        for field in ["chip", "title", "detail"] {
            let key = format!("checkout.state.{state}.{field}");
            let sentence = render(&copy(&id, &key));
            assert!(
                sentence.chars().filter(|c| c.is_alphabetic()).count() >= 3,
                "{key} is too short to say anything: {sentence}"
            );
        }
    }
    println!(
        "the five checkout states carry a chip, a title and a sentence in Indonesian \
         ({} strings), plus `offline`",
        5 * 3
    );
}

/// Confirms a payment through the one function that activates one.
///
/// **Not through the poller, and the reason is this suite's clock.** `account` freezes the
/// clock at `jakarta_morning()`, so the invoice's own `expires_at` is in the past as far as
/// the database is concerned — the sweep's `expires_at > now()` window excludes it and the
/// client's poll would expire it, both correctly. That makes the ingress untestable here
/// rather than broken, and `tests/payment_jobs.rs` covers it with a real clock: the client's
/// poll activating, the sweep recovering, a replay activating once.
///
/// What this file is about is the money path's *arithmetic and copy* under a frozen instant,
/// and the event built below is byte-for-byte the one `billing::poll` synthesises from the
/// gateway's answer — same kind, same amount, same `paid_at` — so the period and the receipt
/// are the ones a real settlement produces.
async fn settle(harness: &TestState, payment: &serde_json::Value, paid_at: DateTime<Utc>) {
    let amount = payment["amount_idr"].as_i64().unwrap();
    let event = stvdy_core::traits::payment::GatewayEvent {
        event_id: format!(
            "jakarta:{}:{}",
            payment["order_id"].as_str().unwrap(),
            paid_at
        ),
        kind: stvdy_core::traits::payment::GatewayEventKind::Paid,
        order_id: payment["order_id"].as_str().unwrap().to_string(),
        amount_idr: amount,
        currency: "IDR".to_string(),
        paid_at: Some(paid_at),
        provider_payment_id: payment["order_id"]
            .as_str()
            .map(|order| format!("mock-{order}")),
        fee_idr: None,
        raw: serde_json::json!({ "source": "test", "status": "paid" }),
    };

    let mut conn = harness.state.pool.acquire().await.unwrap();
    let applied = stvdy_server::billing::apply::apply_paid(
        &mut conn,
        &harness.state,
        payment["id"].as_str().unwrap().parse().unwrap(),
        &event,
    )
    .await
    .expect("the apply runs");

    assert!(
        matches!(
            applied,
            stvdy_server::billing::apply::Applied::Activated { .. }
        ),
        "the gateway's paid answer did not activate the payment: {applied:?}"
    );
}

/// **Gate 6, the money half.** A checkout, the gateway's own report that it settled, and the
/// period it buys — read back through `Asia/Jakarta` at every step.
#[sqlx::test(migrations = "../../migrations")]
async fn the_money_copy_is_correct_in_indonesia(pool: PgPool) {
    let (harness, app, token) = account(pool).await;
    let clock = jakarta_morning();

    // --- the invoice, and the instant it expires -----------------------------------------

    let (status, payment) = json(
        &app,
        "POST",
        "/billing/checkout",
        &token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payment}");

    let expires_at: DateTime<Utc> = payment["expires_at"].as_str().unwrap().parse().unwrap();
    let amount = payment["amount_idr"].as_i64().unwrap();

    // The amount is the config's number, not this test's: `config/plans.toml` says pro
    // monthly is 10,000 IDR, and a literal here would pass against a server whose config
    // disagrees with it.
    let plans = stvdy_server::testing::plans();
    let expected = stvdy_core::billing::price(
        plans.get(stvdy_core::config::PlanId::Pro),
        stvdy_core::billing::BillingPeriod::Monthly,
        false,
        None,
    );
    assert_eq!(
        amount, expected.due_idr,
        "the checkout quotes `billing::price`'s number, which is what the settlement is \
         checked against"
    );
    assert_eq!(
        amount, 10_000,
        "pro monthly is 10,000 IDR in config/plans.toml"
    );
    assert_eq!(
        payment["payable_amount_idr"].as_i64().unwrap(),
        amount,
        "the gateway charged the price exactly; the payable is what the student transfers"
    );

    // **The sharp pair.** The expiry is 15 minutes after the clock, and the Jakarta user's
    // clock reads it as `06:15` on the 1st while the UTC clock still says the 31st. A
    // client that rendered the UTC wall time shows the student "23:15" on a day that has
    // already passed for them.
    assert_eq!(
        local(expires_at),
        "2026-09-01 06:15",
        "the expiry, read back through chrono_tz, is the same instant the Jakarta clock shows"
    );
    assert_eq!(
        expires_at.format("%Y-%m-%d %H:%M").to_string(),
        "2026-08-31 23:15",
        "and the UTC reading is the previous evening: this is the pair a UTC-rendering client \
         gets wrong"
    );
    assert_eq!(
        expires_at
            .with_timezone(&jakarta())
            .format("%:z")
            .to_string(),
        "+07:00",
        "the offset comes from the zone, so a zone with DST would read differently"
    );
    println!(
        "checkout: amount {} IDR, expires_at {} (Jakarta {} / UTC {})",
        amount,
        expires_at.to_rfc3339(),
        local(expires_at),
        expires_at.format("%Y-%m-%d %H:%M")
    );

    // --- the gateway's own report, applied by the one function that acts on it -------------

    settle(&harness, &payment, clock).await;

    // --- the period it bought, on the student's own calendar ------------------------------

    let period_end: DateTime<Utc> = sqlx::query_scalar(
        "select current_period_end from subscriptions
          where user_id = (select id from users where email = 'jakarta-payer@example.test')
            and status = 'active'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    assert_eq!(
        period_end,
        clock + Duration::days(30),
        "a monthly period is 30 days from the payment (A296: no live period, so it starts at \
         `paid_at`)"
    );

    // **The month boundary.** Paid 06:00 Jakarta on 1 September; the period ends 06:00
    // Jakarta on 1 October. The local end day is one day *later* than the UTC end day,
    // which is exactly the reading a UTC-bucketed implementation reports — and it is the
    // date the UI prints, because `PlanUsageSections` renders `plan.renewalDate` beside
    // `formatInZone(instant, 'Asia/Jakarta')`.
    assert_eq!(
        local_day(clock),
        "2026-09-01",
        "the payment's local day is Jakarta's 1st"
    );
    assert_eq!(
        clock.format("%Y-%m-%d").to_string(),
        "2026-08-31",
        "while its UTC day is still August — the divergence this gate is about"
    );
    assert_eq!(
        local_day(period_end),
        "2026-10-01",
        "the period end lands on Jakarta's 1 October, one local month after the payment"
    );
    assert_eq!(
        period_end.format("%Y-%m-%d").to_string(),
        "2026-09-30",
        "and its UTC day is 30 September: a UTC-dated implementation reports the 30th while \
         the student's calendar says 1 October"
    );
    assert_eq!(
        period_end.with_timezone(&jakarta()).date_naive()
            - clock.with_timezone(&jakarta()).date_naive(),
        Duration::days(30),
        "the local span is exactly the 30 days the period buys"
    );
    println!(
        "period: paid_at {} (Jakarta {}) -> current_period_end {} (Jakarta {})",
        clock.to_rfc3339(),
        local_day(clock),
        period_end.to_rfc3339(),
        local_day(period_end)
    );

    // --- the second device -----------------------------------------------------------------

    // The plan is active on a device that did none of the paying, and the date it reports
    // is the same local day. This is the exit gate's own sentence ("plan active on the
    // second device") read in the zone the product sells in.
    let second = signed_in_on(&app, "jakarta-payer@example.test", "macos").await;
    let (status, usage) = json(&app, "GET", "/usage", &second, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{usage}");
    assert_eq!(usage["plan"], "pro", "the second device sees the paid plan");
    let reported: DateTime<Utc> = usage["subscription"]["period_end"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        reported, period_end,
        "the date the plan screen renders is the stored period end"
    );
    assert_eq!(
        local_day(reported),
        "2026-10-01",
        "and the plan screen's `plan.renewalDate` row prints Jakarta's 1 October"
    );
    assert_eq!(
        usage["subscription"]["status"], "active",
        "an active subscription is what makes `users.plan_id` grant anything (A146)"
    );
    println!(
        "second device: plan {}, subscription {}, period_end {} (Jakarta {})",
        usage["plan"],
        usage["subscription"]["status"],
        reported.to_rfc3339(),
        local_day(reported)
    );
}

/// The money path in one step: the checkout, then the gateway's own report that it settled,
/// returning the amount paid and the period end the subscription was given.
///
/// The two receipt cases below are about the **mail**, and the checkout half is
/// `the_money_copy_is_correct_in_indonesia`'s; sharing the steps keeps each case about the
/// one thing it is named for rather than re-proving the money path.
async fn paid_in_indonesia(harness: &TestState, app: &Router, token: &str) -> (i64, DateTime<Utc>) {
    let clock = jakarta_morning();

    let (status, payment) = json(
        app,
        "POST",
        "/billing/checkout",
        token,
        serde_json::json!({ "plan_id": "pro", "billing_period": "monthly" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payment}");

    let amount = payment["amount_idr"].as_i64().unwrap();
    settle(harness, &payment, clock).await;

    let period_end: DateTime<Utc> = sqlx::query_scalar(
        "select current_period_end from subscriptions
          where user_id = (select id from users where email = 'jakarta-payer@example.test')
            and status = 'active'",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    (amount, period_end)
}

/// The payment receipt the `RecordingMailer` captured.
///
/// Found by subject rather than by position: the signup verification mail is in the same
/// queue, so "the last message" would be asserting about the fixture. The prefix is the
/// receipt's own subject stem, which is the same in both arms of the match that produces it.
fn receipt(harness: &TestState) -> stvdy_core::traits::mailer::Email {
    harness
        .mailer
        .sent()
        .into_iter()
        .find(|message| message.subject.starts_with("Your Stvdy"))
        .expect("a payment receipt was mailed")
}

/// **The receipt, in the student's own zone and their own plan name.**
///
/// `billing::apply::notify_paid` mails the period-end sentence, and it used to render
/// `period_end.to_rfc3339()`: a raw UTC instant, so a Jakarta student was told their plan ran
/// until `2026-09-30T23:00:00+00:00` while the plan screen said 1 October — the day *before*,
/// which is precisely the class this file exists to catch and the receipt was the one place
/// it was not caught. The same sentence named the machine's `pro` where the app says `Pro`.
///
/// The instant is the suite's own sharp pair: paid `2026-08-31T23:00Z`, the period ends
/// `2026-09-30T23:00Z`, which is **1 October** in Jakarta and **30 September** in UTC. A
/// different day in the two zones is what makes this a test rather than a coincidence, and
/// both readings are asserted before the mail is, so the assertion cannot drift into
/// agreeing with a UTC rendering by accident.
#[sqlx::test(migrations = "../../migrations")]
async fn the_receipt_names_the_students_own_day_and_plan(pool: PgPool) {
    let (harness, app, token) = account(pool).await;
    let (amount, period_end) = paid_in_indonesia(&harness, &app, &token).await;
    let mail = receipt(&harness);

    assert_eq!(
        local_day(period_end),
        "2026-10-01",
        "the period ends on the student's own 1 October"
    );
    assert_eq!(
        period_end.format("%Y-%m-%d").to_string(),
        "2026-09-30",
        "and on 30 September in UTC — the reading a UTC-rendered receipt reports"
    );

    assert_eq!(mail.subject, "Your Stvdy Pro plan is active");
    assert!(
        mail.text
            .contains("Your Pro plan is active until 1 October 2026."),
        "the receipt names the student's own day, not the UTC one: {}",
        mail.text
    );
    assert!(
        !mail.text.contains("2026-09-30") && !mail.text.contains("T23:00:00"),
        "and carries no raw instant: {}",
        mail.text
    );
    assert!(
        !mail.text.contains(" pro "),
        "nor the machine's plan name where the app says `Pro`: {}",
        mail.text
    );
    assert!(
        mail.text
            .contains(&format!("Your payment of {amount} IDR went through.")),
        "the amount is still the payment's own number: {}",
        mail.text
    );

    println!("receipt subject: {}", mail.subject);
    println!("receipt body:\n{}", mail.text);
}

/// **The fallback.** `users.timezone` is `not null default 'Asia/Jakarta'`
/// (`0003_users.sql`), so an absent zone is not reachable through the app — but a column that
/// is empty or unparseable must still produce a date rather than an instant, a panic, or a
/// silent UTC day. The receipt falls back to the product's default zone, the same choice
/// `calendar::sync` makes and for the same reason: rendering a Jakarta student's period end in
/// UTC names the day before.
#[sqlx::test(migrations = "../../migrations")]
async fn the_receipt_falls_back_to_the_default_zone(pool: PgPool) {
    let (harness, app, token) = account(pool).await;
    sqlx::query("update users set timezone = '' where email = 'jakarta-payer@example.test'")
        .execute(&harness.state.pool)
        .await
        .unwrap();

    let (_, period_end) = paid_in_indonesia(&harness, &app, &token).await;
    let mail = receipt(&harness);

    assert_eq!(
        local_day(period_end),
        "2026-10-01",
        "the period still ends on the default zone's 1 October"
    );
    assert!(
        mail.text.contains("until 1 October 2026."),
        "an account with no usable zone still gets a date, in the default zone: {}",
        mail.text
    );
    assert!(
        !mail.text.contains("T23:00:00") && !mail.text.contains("2026-09-30"),
        "and not an instant: {}",
        mail.text
    );

    println!("receipt body with no stored zone:\n{}", mail.text);
}

/// **The zone rule, proved where it bites.** `chrono_tz` and a fixed offset agree in
/// Jakarta and disagree in every DST zone, so an assertion made only in Jakarta cannot tell
/// them apart.
///
/// This is a `#[test]`: it is arithmetic on `renewal::boundary_instant`, which is the
/// local-hour maths the reminder producer uses (A300), and it needs no database.
#[test]
fn a_local_hour_is_read_through_the_zone_and_not_through_an_offset() {
    // The renewal reminder fires at the student's own 09:00, `days` before the period end.
    // The period end is 2026-11-01T00:00Z, which is 07:00 Jakarta on the 1st.
    let end = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();

    // Jakarta: no DST, so the offset reading and the zone reading agree — which is why the
    // sharp case below is London.
    let jakarta_instant =
        stvdy_server::jobs::handlers::renewal::boundary_instant("Asia/Jakarta", end, 3);
    let jakarta_local = jakarta_instant.with_timezone(&jakarta());
    assert_eq!(
        jakarta_local.format("%Y-%m-%d %H:%M").to_string(),
        "2026-10-29 09:00",
        "three days before the period end is 09:00 Jakarta on the 29th"
    );
    println!(
        "Asia/Jakarta: period end {} -> reminder {} ({})",
        end.to_rfc3339(),
        jakarta_instant.to_rfc3339(),
        jakarta_local.format("%Y-%m-%d %H:%M")
    );

    // **Europe/London, across its own DST change.** British Summer Time ends on
    // 2026-10-25, so a reminder computed for a date after it is at UTC+0 and one before it
    // is at UTC+1. A fixed-offset implementation cannot be right for both, and this pair is
    // the proof that the arithmetic reads the zone.
    let london: Tz = "Europe/London".parse().unwrap();
    let before_change = stvdy_server::jobs::handlers::renewal::boundary_instant(
        "Europe/London",
        Utc.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap(),
        3,
    );
    let after_change = stvdy_server::jobs::handlers::renewal::boundary_instant(
        "Europe/London",
        Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
        3,
    );

    assert_eq!(
        before_change
            .with_timezone(&london)
            .format("%H:%M")
            .to_string(),
        "09:00",
        "the local hour is the student's own 09:00 on the summer side of the change"
    );
    assert_eq!(
        after_change
            .with_timezone(&london)
            .format("%H:%M")
            .to_string(),
        "09:00",
        "and on the winter side"
    );
    assert_eq!(
        before_change
            .with_timezone(&london)
            .format("%:z")
            .to_string(),
        "+01:00",
        "before 2026-10-25 London is UTC+1"
    );
    assert_eq!(
        after_change
            .with_timezone(&london)
            .format("%:z")
            .to_string(),
        "+00:00",
        "after it, UTC+0 — so a single hardcoded offset would put one of these at 08:00 or \
         10:00 local"
    );
    println!(
        "Europe/London: reminder {} ({} {}) vs {} ({} {}) — the offset moves, the local hour \
         does not",
        before_change.to_rfc3339(),
        before_change
            .with_timezone(&london)
            .format("%Y-%m-%d %H:%M"),
        before_change.with_timezone(&london).format("%:z"),
        after_change.to_rfc3339(),
        after_change.with_timezone(&london).format("%Y-%m-%d %H:%M"),
        after_change.with_timezone(&london).format("%:z")
    );
}

/// The DST pair above is only a control if the two reminders really land on **different UTC
/// hours**: 09:00 local on each side of the change is 08:00Z in summer and 09:00Z in winter,
/// so a fixed `+01:00` reading would call the second one 10:00 local. Asserted as its own
/// case so the control cannot be quietly weakened into two instants that agree.
///
/// The two period ends are different dates, so the *difference* between the instants is not
/// the interesting number and is deliberately not asserted: what matters is that each is the
/// student's own 09:00 and that their UTC hours differ by the offset change.
#[test]
fn the_dst_control_compares_two_different_utc_hours() {
    let london: Tz = "Europe/London".parse().unwrap();
    let summer = stvdy_server::jobs::handlers::renewal::boundary_instant(
        "Europe/London",
        Utc.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap(),
        3,
    );
    let winter = stvdy_server::jobs::handlers::renewal::boundary_instant(
        "Europe/London",
        Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
        3,
    );

    assert_eq!(summer.format("%H:%M").to_string(), "08:00", "{summer}");
    assert_eq!(winter.format("%H:%M").to_string(), "09:00", "{winter}");
    assert_eq!(
        summer.format("%Y-%m-%d").to_string(),
        "2026-10-17",
        "three days before a period end of the 20th, on the summer side of the change"
    );
    assert_eq!(
        winter.format("%Y-%m-%d").to_string(),
        "2026-10-29",
        "three days before a period end of 1 November, on the winter side"
    );

    // The control: a fixed +01:00 reading of the winter reminder is 10:00 local, which is
    // wrong by an hour. This is the assertion that makes the pair above a control rather
    // than two numbers that happen to differ.
    let fixed = chrono::FixedOffset::east_opt(3600).unwrap();
    assert_eq!(
        winter.with_timezone(&fixed).format("%H:%M").to_string(),
        "10:00",
        "a hardcoded +01:00 puts the winter reminder at 10:00 local"
    );
    assert_eq!(
        summer.with_timezone(&fixed).format("%H:%M").to_string(),
        "09:00",
        "and the summer one at 09:00 — so one fixed offset cannot serve both"
    );
    assert_eq!(
        summer.with_timezone(&london).format("%:z").to_string(),
        "+01:00",
        "the summer reminder is at UTC+1"
    );
    assert_eq!(
        winter.with_timezone(&london).format("%:z").to_string(),
        "+00:00",
        "the winter one at UTC+0"
    );
}
