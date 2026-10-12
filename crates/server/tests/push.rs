//! `crates/server/src/integrations/push.rs`: the two payload shapes, the two token shapes,
//! and the one classification that decides whether a token row is deleted.
//!
//! Every case here is a *pure* assertion on a value the module produces, because that is what
//! the interesting part is: the wire shape and the error classification. The network call is
//! the boring half, and a test that needed a vendor would be a test that never runs.
//!
//! The classification case is the one this suite exists for. `Unregistered` is the only
//! `PushError` a caller may act on by deleting a `push_tokens` row, so a `503` misread as a
//! dead token unregisters a phone that was merely asleep — the student stops getting
//! reminders and nothing anywhere reports an error.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use sqlx::PgPool;
use stvdy_core::traits::push::{PushError, PushMessage, PushPlatform};
use stvdy_server::integrations::push::{ApnsPush, FcmPush, PushRouter};
use stvdy_server::testing::TestState;

mod common;

fn message() -> PushMessage {
    let mut data = BTreeMap::new();
    data.insert(
        "reminder_id".to_string(),
        "5b2f0e0e-0000-0000-0000-000000000001".to_string(),
    );
    data.insert("kind".to_string(), "task_due".to_string());
    PushMessage {
        title: "Essay is due in an hour".to_string(),
        body: "Algorithms — 23:00".to_string(),
        data,
        collapse_key: Some("reminder-5b2f0e0e".to_string()),
        badge: Some(3),
    }
}

/// A PKCS#8 EC private key, generated for this test and used by nothing else.
const TEST_EC_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgYvn/Q1/vA5qs1Oys\n\
y6LD8XKcr/85vBhalUIapCoLEAqhRANCAARYnSgMLi5k9uhguo6KHjGuOiBYnWPx\n\
DCSdXbByNSRpaCMvu9qD/JZGvuyl2h8NbrbCnzb64FIULZGX7LcbouUe\n\
-----END PRIVATE KEY-----\n";

/// A PKCS#8 RSA key, likewise generated for this test alone.
const TEST_RSA_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQC0Q3GMhD4FisXn\n\
TQFvQlA/0kEEyBAb/OnK3mXXfOcit+AAAJtuoF9HLLppm2p6bwcIRBZ3P7TnY680\n\
YIbsXrav1z8na7Oq1qulCIFrc//mIowXJxUP2k67C3ghnuJDbqa2Dy9H4EWMNenC\n\
G1axIYhNIeLcG6AzmKpqZr91UWT5MWQlJet5M9lgBqwmXqQvXHW1QNW5KjzSsAMN\n\
pmM5k3YzRohZ2lfdsOc/0/n1EP+aUv2ST+mh2sotImnCFAQqka+K6QDjXHswvQTz\n\
CyLOuQNKVx6G7kpRaG3yciPEu4qjT6Og6aHywBvPcHJGhFkshuvTZDryq15eYamI\n\
p0Qbc2xFAgMBAAECggEAQf/rhBIrDyZnHS/RlaXG8gcOLSz+0PCBvPg+e25MxiZi\n\
ij2uqwtmtw2GVwJDMv6RqZIcU57sokeaL2gkHkjzRGBfzBQf3S3p/GunzurSx7Tg\n\
a92wVXSNnOgvHMgqeZv6x++5VOylKhfXNW2DCSmI2QzOHd6zghCkXH6wHMknWiEA\n\
g0IBRJUKydIjGQzZAvQRxbxRDMm/wNuICvlerWO0qR0qvuqqR+0xu8i8YJhmV5tc\n\
PLFPqTFciutsgy/+S5xZOZPO/WJKlo8IVP/ScCEEWHd3gwGWSkWANp8KFRkJ3XGz\n\
n0q79ajdGOOtoFOGp7P9YmKzhqTccOqgg1p5nOgFpQKBgQDo3ippBd6UAE1MzlLY\n\
hRWyWBVJAvwr74X/xJ1+c6l2Z9hSZ6tgs0a+fhA1E36ztzEiobv30100IWD+PuMu\n\
8xquA0AwFUsQecoT28KadyzJh7digKLVPgtW11y6B9ziOrLYPdIL1cLxNN6Wx55M\n\
KuR0oWzSKHQPghSuHaI7zZUm4wKBgQDGK4wWuHFeb8Vavl85wnDKcz47sOPsDUgn\n\
TE5BfQJrdNKCrQCC02eiYekQ1xCjWci9WreCx+Aop6DdVRMNBOinGeiAu55XrtWB\n\
6P1HvgyyAQ9wHv3/fs00VlYBcqqVWU6nor0qNDIWMmP0T72Sv8eEibeWx6FKuTBM\n\
ruUvqFrgtwKBgCr7vzZNiA0nlR0lRHOE9UFLMvqcG3H8r25Lm4qYzOH3Px3O1vpt\n\
fRRybolthMileuNamTiggia6No4Xi5XB993A6ds2QvnSkTNjMCOU9eZpL7d16RiV\n\
cEBiVlJcWzBDB3oUxy26Ela1nJSWWijTDgJdPZn5L5shNrIA9E43n11jAoGAOipl\n\
//U7MWDVKIFgKe/scz/IMHUHMS3k08QN5CLxJuEvOLPWxo+Oh5b9dbfqZkfgJ9QM\n\
sRJBtS0ehliGMvgR4Zo1KDPpkgr7GBpu3TPnPggZo+s02l05lfcRX3NsplGESKf3\n\
pf+UA/feZsNQ/VXVFg3mGReXO/uHNbrQAmED65kCgYA5p2RDKwAVy5hP7Gb0gFfn\n\
fg3WZC218tyD41KSgDZFHoxQ8C4nkbuqJjyEPBiXwio5GTEWieWZT/k6QENiAG2v\n\
iVFv47fFrJrkEEU2zSS4k+8qBzRcJux1Nbk0LxHNRazK6S3anthaMXHeZZaTb1IU\n\
Y/2Js6fL/4m21l2/r0h8KQ==\n\
-----END PRIVATE KEY-----\n";

fn apns() -> ApnsPush {
    ApnsPush::new(&stvdy_server::config::ApnsConfig {
        key_p8: secrecy::SecretString::from(TEST_EC_KEY),
        key_id: "ABC123DEFG".to_string(),
        team_id: "TEAM123456".to_string(),
        topic: "app.stvdy.study".to_string(),
        sandbox: false,
    })
    .expect("the test EC key must be a valid PKCS#8 key")
}

fn fcm() -> FcmPush {
    FcmPush::new(&stvdy_server::config::FcmConfig {
        project_id: "stvdy-test".to_string(),
        client_email: "push@stvdy-test.iam.gserviceaccount.com".to_string(),
        private_key: TEST_RSA_KEY.to_string(),
    })
    .expect("the test RSA key must be a valid PKCS#8 key")
}

/// The base64url payload of a JWT, decoded — so a test asserts on the claims rather than on a
/// string it cannot read.
fn jwt_claims(token: &str) -> serde_json::Value {
    use base64::Engine;
    let payload = token.split('.').nth(1).expect("a JWT has three parts");
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("the payload must be base64url");
    serde_json::from_slice(&decoded).expect("the payload must be JSON")
}

fn jwt_header(token: &str) -> serde_json::Value {
    use base64::Engine;
    let header = token.split('.').next().expect("a JWT has three parts");
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header)
        .expect("the header must be base64url");
    serde_json::from_slice(&decoded).expect("the header must be JSON")
}

/// APNs' provider token is ES256 with `kid`, `iss` and `iat`, and it is short-lived.
///
/// The `kid` is the key id and the `iss` is the team id, in that order — Apple's own
/// documentation is explicit that getting them the wrong way round is a `403` that reads like
/// a bad key, so the two are asserted against values that are distinguishable.
#[test]
fn the_apns_provider_token_has_the_claims_apns_requires() {
    let push = apns();
    let now: chrono::DateTime<chrono::Utc> = "2026-10-12T03:00:00Z".parse().unwrap();
    let token = push.provider_token(now).expect("signing");

    let header = jwt_header(&token);
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["kid"], "ABC123DEFG");

    let claims = jwt_claims(&token);
    assert_eq!(claims["iss"], "TEAM123456");
    assert_eq!(claims["iat"], now.timestamp());
    // No `exp`: APNs reads the token's age from `iat` and rejects one more than an hour old,
    // and a token that carried its own `exp` would be a second answer to the same question.
    assert!(claims.get("exp").is_none(), "{claims}");

    // A different instant is a different `iat`, so the token is not a constant.
    let later = push
        .provider_token(now + chrono::Duration::minutes(5))
        .unwrap();
    assert_ne!(token, later);
}

/// The APNs payload is `{"aps": {"alert": {"title", "body"}, "badge", "thread-id"}}` plus
/// the data keys, which is the shape APNs accepts and the shape the client reads
/// `reminder_id` out of.
#[test]
fn the_apns_payload_carries_the_reminder_id() {
    let payload = ApnsPush::payload(&message());

    assert_eq!(payload["aps"]["alert"]["title"], "Essay is due in an hour");
    assert_eq!(payload["aps"]["alert"]["body"], "Algorithms — 23:00");
    assert_eq!(payload["aps"]["badge"], 3);
    assert_eq!(payload["aps"]["thread-id"], "reminder-5b2f0e0e");

    // The data keys are top level, which is where APNs puts custom keys — the client reads
    // `reminder_id` to suppress a notification it has already shown on another device (A285).
    assert_eq!(
        payload["reminder_id"],
        "5b2f0e0e-0000-0000-0000-000000000001"
    );
    assert_eq!(payload["kind"], "task_due");

    // A message with no badge and no collapse key is still a valid alert, and carries no
    // empty `aps` entries: APNs rejects `"badge": null`.
    let bare = PushMessage {
        title: "t".to_string(),
        body: "b".to_string(),
        data: BTreeMap::new(),
        collapse_key: None,
        badge: None,
    };
    let payload = ApnsPush::payload(&bare);
    assert!(payload["aps"].get("badge").is_none(), "{payload}");
    assert!(payload["aps"].get("thread-id").is_none(), "{payload}");

    // And a data key named `aps` cannot replace the notification dictionary: the dictionary is
    // inserted last, so a caller's `aps` data key is overridden rather than becoming the alert.
    let mut data = BTreeMap::new();
    data.insert("aps".to_string(), "oops".to_string());
    let hostile = PushMessage {
        title: "t".to_string(),
        body: "b".to_string(),
        data,
        collapse_key: None,
        badge: None,
    };
    assert_eq!(ApnsPush::payload(&hostile)["aps"]["alert"]["title"], "t");
}

/// FCM v1 wants `{"message": {"token", "notification": {...}, "data": {...},
/// "android": {"collapse_key": ...}}}`.
#[test]
fn the_fcm_payload_is_a_v1_message() {
    let payload = FcmPush::payload("device-token-123", &message());

    let message = &payload["message"];
    assert_eq!(message["token"], "device-token-123");
    assert_eq!(message["notification"]["title"], "Essay is due in an hour");
    assert_eq!(message["notification"]["body"], "Algorithms — 23:00");
    // `data` is a map of strings, which is what FCM requires: a non-string value is an
    // `INVALID_ARGUMENT` that names the field but not the type.
    assert_eq!(
        message["data"]["reminder_id"],
        "5b2f0e0e-0000-0000-0000-000000000001"
    );
    assert!(message["data"]["kind"].is_string(), "{payload}");
    // Android's collapse key is under `android`, not at the top level: the iOS equivalent is
    // APNs' `thread-id` and the two are not the same parameter.
    assert_eq!(message["android"]["collapse_key"], "reminder-5b2f0e0e");
    assert!(message.get("collapse_key").is_none(), "{payload}");

    // A bare message has no `android` block at all rather than an empty one.
    let bare = PushMessage {
        title: "t".to_string(),
        body: "b".to_string(),
        data: BTreeMap::new(),
        collapse_key: None,
        badge: None,
    };
    let payload = FcmPush::payload("t", &bare);
    assert!(payload["message"].get("android").is_none(), "{payload}");
}

/// A dead token is `Unregistered` and a 503 is `Transport`, because only one of them
/// deletes a row.
///
/// The three cases that *do* delete are the three that mean the token can never work, and the
/// two that must not are the ones where nothing is known about the token at all.
#[test]
fn a_dead_token_and_an_outage_are_different_errors() {
    // APNs: 410 is the canonical "this token is gone".
    assert!(matches!(
        ApnsPush::classify(410, Some("Unregistered")),
        PushError::Unregistered
    ));
    // Apple's own guidance for these two is to stop using the token, so they are the same
    // fact from the caller's point of view — the token is dead, not the service.
    assert!(matches!(
        ApnsPush::classify(400, Some("BadDeviceToken")),
        PushError::Unregistered
    ));
    assert!(matches!(
        ApnsPush::classify(400, Some("DeviceTokenNotForTopic")),
        PushError::Unregistered
    ));

    // Our own problem: a bad key or topic must not unregister anybody's phone.
    assert!(matches!(
        ApnsPush::classify(403, Some("InvalidProviderToken")),
        PushError::Invalid(_)
    ));
    // A bad payload is the same: the token is fine.
    assert!(matches!(
        ApnsPush::classify(400, Some("BadCollapseId")),
        PushError::Invalid(_)
    ));
    // An outage is a retry, not a deletion. This is the case a caller that treated every
    // non-200 as a dead token would get wrong.
    assert!(matches!(
        ApnsPush::classify(503, None),
        PushError::Transport(_)
    ));
    assert!(matches!(
        ApnsPush::classify(500, Some("InternalServerError")),
        PushError::Transport(_)
    ));

    // FCM: `404 NOT_FOUND` with an `UNREGISTERED` detail is the v1 spelling.
    let unregistered = serde_json::json!({
        "error": {
            "status": "NOT_FOUND",
            "details": [{ "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                          "errorCode": "UNREGISTERED" }]
        }
    });
    assert!(matches!(
        FcmPush::classify(404, &unregistered),
        Some(PushError::Unregistered)
    ));

    // A 404 that does *not* name `UNREGISTERED` is not a dead token: it could be a typo'd
    // project id, which is our configuration rather than the device's.
    let wrong_project = serde_json::json!({ "error": { "status": "NOT_FOUND" } });
    assert!(matches!(
        FcmPush::classify(404, &wrong_project),
        Some(PushError::Transport(_))
    ));

    // The body-less 400 form some FCM versions answer with.
    let bare = serde_json::json!({ "error": { "status": "UNREGISTERED" } });
    assert!(matches!(
        FcmPush::classify(400, &bare),
        Some(PushError::Unregistered)
    ));

    // Credentials and a malformed message are ours.
    let unauthorized = serde_json::json!({ "error": { "status": "UNAUTHENTICATED" } });
    assert!(matches!(
        FcmPush::classify(401, &unauthorized),
        Some(PushError::Invalid(_))
    ));
    let invalid = serde_json::json!({ "error": { "status": "INVALID_ARGUMENT" } });
    assert!(matches!(
        FcmPush::classify(400, &invalid),
        Some(PushError::Invalid(_))
    ));

    // An outage is a retry.
    let unavailable = serde_json::json!({ "error": { "status": "UNAVAILABLE" } });
    assert!(matches!(
        FcmPush::classify(503, &unavailable),
        Some(PushError::Transport(_))
    ));
    assert!(matches!(
        FcmPush::classify(429, &unavailable),
        Some(PushError::Transport(_))
    ));

    // A success is no error at all, which is the one case that must not be classified.
    assert!(FcmPush::classify(200, &serde_json::json!({})).is_none());
    assert!(FcmPush::classify(204, &serde_json::json!({})).is_none());

    // The classification is only useful if a caller can act on it, so the two are asserted to
    // be **distinguishable** rather than merely both present: different variants, different
    // messages, and the deletion decision readable off the variant alone. A single
    // `PushError::Transport` for both would make every case above pass while a dead token was
    // retried forever and the device was never pruned — the failure this file exists for.
    let dead = ApnsPush::classify(410, Some("Unregistered"));
    let outage = ApnsPush::classify(503, None);
    assert!(
        !matches!(&outage, PushError::Unregistered),
        "an outage must not be the variant that deletes a token"
    );
    assert!(
        !matches!(&dead, PushError::Transport(_)),
        "a dead token must not be the variant a caller retries"
    );
    assert_ne!(
        std::mem::discriminant(&dead),
        std::mem::discriminant(&outage),
        "the two failures are different variants, not one variant with two messages"
    );
    assert_ne!(
        dead.to_string(),
        outage.to_string(),
        "and the operator reading the log can tell them apart: {dead} vs {outage}"
    );
    assert_eq!(
        dead.to_string(),
        "the push token is no longer registered",
        "the dead-token message names the token, not the vendor"
    );
    assert!(
        outage.to_string().contains("503"),
        "the transport message carries the status: {outage}"
    );
}

/// The OAuth2 assertion names the token endpoint and the messaging scope.
///
/// A wrong `aud` is the classic failure — Google answers `invalid_grant` with no hint that the
/// audience was the problem — so both values are asserted rather than assumed.
#[test]
fn the_fcm_assertion_names_the_token_endpoint_and_scope() {
    let push = fcm();
    let now: chrono::DateTime<chrono::Utc> = "2026-10-12T03:00:00Z".parse().unwrap();
    let token = push.assertion(now).expect("signing");

    let header = jwt_header(&token);
    assert_eq!(header["alg"], "RS256");

    let claims = jwt_claims(&token);
    assert_eq!(claims["iss"], "push@stvdy-test.iam.gserviceaccount.com");
    assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/firebase.messaging"
    );
    assert_eq!(claims["iat"], now.timestamp());
    // The assertion lives an hour, which is what Google's own contract says.
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        3600
    );
}

/// An unconfigured platform is absent, so the settings screen never offers a channel that
/// cannot fire (A288).
#[test]
fn an_unconfigured_platform_is_not_available() {
    // Nothing configured: no platform, and `channels_available` therefore omits `push`.
    let empty = PushRouter::default();
    assert!(empty.platforms().is_empty());
    assert!(!empty.is_configured());
    assert!(empty.for_provider("apns").is_none());
    assert!(empty.for_provider("fcm").is_none());
    // A provider string that is neither vendor is absent rather than a panic, because
    // `push_tokens.provider` is free text and a stale row must not take the job down.
    assert!(empty.for_provider("telegram").is_none());

    // A configured APNs but no FCM is exactly one platform, and the *other* one is absent
    // rather than defaulted — which is what makes the settings screen's list truthful.
    let apns_only = PushRouter::with(Some(std::sync::Arc::new(apns())), None);
    assert_eq!(apns_only.platforms(), vec![PushPlatform::Apns]);
    assert!(apns_only.is_configured());
    assert!(apns_only.for_provider("apns").is_some());
    assert!(apns_only.for_provider("fcm").is_none());

    let both = PushRouter::with(
        Some(std::sync::Arc::new(apns())),
        Some(std::sync::Arc::new(fcm())),
    );
    assert_eq!(
        both.platforms(),
        vec![PushPlatform::Apns, PushPlatform::Fcm],
        "the order is the matrix's, so the screen's list is a contract"
    );
}

/// `GET /settings` reports only the channels this deployment can deliver on (A288).
///
/// The field is what stops the settings screen offering `telegram` and `discord`: the plan
/// matrix grants them to Power and `PUT /settings` accepts them, but nothing sends them, and a
/// channel that is granted, selectable and silently never fires is worse than one that is
/// visibly unavailable.
#[sqlx::test(migrations = "../../migrations")]
async fn channels_available_reports_the_transports_that_exist(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = common::signed_up(&app, &harness, "channels@example.test").await;

    let (status, body) =
        common::json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(
        body["channels_available"],
        serde_json::json!(["web", "email"]),
        "with no push transport configured, push must not be offered: {body}"
    );

    // With a transport installed, `push` appears — and the order stays the matrix's.
    let with_push = TestState::new(harness.state.pool.clone());
    with_push.seed_plans().await.unwrap();
    let with_push = with_push.with_push(std::sync::Arc::new(PushRouter::with(
        Some(std::sync::Arc::new(apns())),
        None,
    )));
    let app = stvdy_server::routes::router(with_push.state.clone()).unwrap();
    let token = common::signed_up(&app, &with_push, "channels-push@example.test").await;

    let (status, body) =
        common::json(&app, "GET", "/settings", &token, serde_json::Value::Null).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(
        body["channels_available"],
        serde_json::json!(["web", "push", "email"]),
        "{body}"
    );
    // And the field is a sibling of `settings` and `timezone`, which is the shape the screen
    // reads.
    assert!(body["settings"].is_object(), "{body}");
    assert!(body["timezone"].is_string(), "{body}");
}
