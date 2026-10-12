//! The deep-link round trip (A320, A329): a tapped notification's universal link and a
//! `stvdy://` URL must reach the same handler, including on a cold start.
//!
//! A320's whole claim is that the two arrive as the *same* `DeepLinkIntent`. If they do
//! not, the app has two deep-link vocabularies and only one of them is tested — which is
//! the failure this file exists to make impossible.
//!
//! `unwrap`/`expect`/`panic` are denied in non-test code by this crate's lint block;
//! this is test code, where a failed precondition should stop the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_shell_lib::deeplink::{DeepLinkIntent, IgnoreReason, Ignored, classify};

/// The host `bundle.iOS.associatedDomains` claims and the one the universal link
/// carries. Named once, here, so the test says which host it is asserting about.
const APP_HOST: &str = "app.stvdy";

#[test]
fn a_universal_link_and_the_custom_scheme_classify_alike() {
    for route in [
        "auth/callback?code=abc123&state=xyz",
        "auth/callback?ticket=one-time",
        "oauth/calendar?code=c&state=s",
    ] {
        let custom = classify(&format!("stvdy://{route}"));
        let universal = classify(&format!("https://{APP_HOST}/{route}"));
        assert_eq!(
            custom, universal,
            "stvdy://{route} and https://{APP_HOST}/{route} must be one intent"
        );
        assert!(
            !matches!(custom, DeepLinkIntent::Ignored(_)),
            "{route} must be actionable"
        );
    }
}

/// The two credentials keep their precedence through the universal-link path too: a
/// complete `code`+`state` pair is the OAuth return, a lone `ticket` is the Google
/// sign-in return.
#[test]
fn the_universal_link_keeps_the_credential_precedence() {
    let callback = classify(&format!(
        "https://{APP_HOST}/auth/callback?code=c&state=s&ticket=t"
    ));
    assert!(matches!(callback, DeepLinkIntent::AuthCallback(_)));

    let ticket = classify(&format!("https://{APP_HOST}/auth/callback?ticket=t"));
    assert!(matches!(ticket, DeepLinkIntent::AuthTicket(_)));
}

/// A cold start hands the URL to the shell before any listener exists, and the SPA
/// reads it through `deep_link_current`. That path classifies the same URL, so the
/// property has to hold for a bare classify call rather than only for the event.
#[test]
fn a_cold_start_universal_link_is_still_an_intent() {
    let intent = classify(&format!("https://{APP_HOST}/oauth/calendar?code=c&state=s"));
    match intent {
        DeepLinkIntent::Calendar(calendar) => {
            assert_eq!(calendar.params.get("code").map(String::as_str), Some("c"));
        }
        other => panic!("expected a calendar intent, got {other:?}"),
    }
}

/// The host is what makes the link ours, and it is written twice: once in
/// `plugins.deep-link.mobile[]` (which the plugin's build script turns into
/// `applinks:<host>` in the app's entitlements and into an `<intent-filter
/// android:autoVerify="true">` in the Android manifest) and once in
/// `deeplink::APP_LINK_HOST`, which is what actually accepts or refuses the URL.
///
/// A drift between them is the worst kind of silent failure: the OS would hand the app
/// links for one host and `classify` would refuse every one of them, and nothing in the
/// build would complain. So they are compared.
#[test]
fn the_app_link_host_is_the_one_the_classifier_accepts() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses");
    let mobile = config["plugins"]["deep-link"]["mobile"]
        .as_array()
        .expect("a mobile deep-link list");

    let app_links: Vec<&str> = mobile
        .iter()
        .filter(|entry| entry["appLink"].as_bool() == Some(true))
        .filter_map(|entry| entry["host"].as_str())
        .collect();
    assert_eq!(
        app_links,
        vec![stvdy_shell_lib::deeplink::APP_LINK_HOST],
        "the universal-link host must be declared exactly once, and be the classifier's"
    );

    // `https` only: an `http` app link is a credential on an unverified transport.
    for entry in mobile {
        if entry["appLink"].as_bool() == Some(true) {
            assert_eq!(entry["scheme"], serde_json::json!(["https"]));
        }
    }
}

/// `bundle.iOS.associatedDomains` does **not** exist and must not be invented: the
/// CLI's `IosConfig` is `deny_unknown_fields` and has no such key (verified against
/// `tauri-utils-2.10.1`'s `IosConfig` and the CLI's own `config.schema.json`), so
/// writing one is a config the CLI refuses to load rather than an entitlement.
///
/// The mechanism is the deep-link plugin's build script, which writes
/// `com.apple.developer.associated-domains: ["applinks:<host>"]` into the app's
/// entitlements from exactly the config asserted above. This test pins the *absence*
/// so a later reader does not "fix" the plan's Task 6 Step 4 by adding a key the CLI
/// rejects.
#[test]
fn the_ios_config_has_no_associated_domains_key() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses");
    assert!(
        config["bundle"]["iOS"]["associatedDomains"].is_null(),
        "bundle.iOS is deny_unknown_fields and has no associatedDomains key; the \
         entitlement comes from plugins.deep-link.mobile[] instead"
    );
}

/// The host is what makes the link ours. Anything else is a web page, not an action —
/// and it must be refused *without* the URL reaching a handler.
#[test]
fn a_foreign_host_is_not_an_intent() {
    for url in [
        "https://evil.example/auth/callback?code=c&state=s",
        "https://app.stvdy.evil.example/auth/callback?code=c&state=s",
        "https://notapp.stvdy/auth/callback?code=c&state=s",
    ] {
        assert!(
            matches!(classify(url), DeepLinkIntent::Ignored(_)),
            "{url} must not be actionable"
        );
    }
}

/// `http://` is a downgrade: the link could be forged in transit, and a universal link
/// is `https` by definition (the AASA file is fetched over TLS). It is refused as
/// *not our scheme* rather than as a foreign host — the scheme really is neither
/// `stvdy://` nor `https://`, and saying so is the more accurate of the two reasons.
#[test]
fn an_insecure_universal_link_is_refused() {
    let intent = classify(&format!("http://{APP_HOST}/auth/callback?code=c&state=s"));
    assert!(matches!(
        intent,
        DeepLinkIntent::Ignored(Ignored {
            reason: IgnoreReason::NotStvdyScheme
        })
    ));
}

/// A fragment is never meaningful to an OAuth return, on either scheme — the token
/// exchange reads the query, and a fragment that survived would be a second vocabulary.
#[test]
fn the_universal_link_drops_its_fragment_too() {
    let with = classify(&format!(
        "https://{APP_HOST}/auth/callback?code=c&state=s#x"
    ));
    let without = classify(&format!("https://{APP_HOST}/auth/callback?code=c&state=s"));
    assert_eq!(with, without);
}

/// The trailing slash and the case of the host are both the OS's to normalise, and a
/// URL that classified differently because of either would be a route that works from
/// one app and not another.
#[test]
fn the_host_is_matched_case_insensitively_and_the_path_tolerates_a_slash() {
    let plain = classify(&format!("https://{APP_HOST}/auth/callback?code=c&state=s"));
    let shouty = classify(&format!(
        "https://{}/auth/callback?code=c&state=s",
        APP_HOST.to_uppercase()
    ));
    let slashed = classify(&format!("https://{APP_HOST}//auth/callback?code=c&state=s"));
    assert_eq!(plain, shouty);
    assert_eq!(plain, slashed);
}
