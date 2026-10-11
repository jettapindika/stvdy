//! Tests for the per-platform billing policy. The point of these is that a
//! config edit cannot enable selling on iOS/Android, which the spec treats as an
//! app-store rejection risk.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use stvdy_core::config::{Platform, load_billing_policy};
use stvdy_core::error::CoreError;

const POLICY_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../config/billing_policy.toml"
);

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("stvdy-bp-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shipped_policy_allows_selling_on_web_and_desktop_only() {
    let policy = load_billing_policy(Path::new(POLICY_PATH)).unwrap();

    for platform in [Platform::Web, Platform::Macos, Platform::Windows] {
        let p = policy.for_platform(platform);
        assert!(p.allow_purchase_ui, "{platform:?} must allow purchase UI");
        assert!(p.show_prices, "{platform:?} must show prices");
        assert!(
            p.allow_purchase_cta_in_locked_messages,
            "{platform:?} must allow the upgrade CTA"
        );
        assert!(policy.purchase_allowed(platform));
    }

    for platform in [Platform::Ios, Platform::Android] {
        let p = policy.for_platform(platform);
        assert!(
            !p.allow_purchase_ui,
            "{platform:?} must not allow purchase UI"
        );
        assert!(!p.show_prices, "{platform:?} must not show prices");
        assert!(
            !p.allow_purchase_cta_in_locked_messages,
            "{platform:?} must not allow the upgrade CTA"
        );
        assert!(!policy.purchase_allowed(platform));
    }
}

#[test]
fn every_platform_must_be_declared() {
    let dir = scratch_dir();
    let path = dir.join("billing_policy.toml");
    std::fs::write(
        &path,
        "[platforms.web]\nallow_purchase_ui = true\nshow_prices = true\n\
         allow_purchase_cta_in_locked_messages = true\n",
    )
    .unwrap();

    match load_billing_policy(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("macos"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn enabling_purchase_ui_on_mobile_is_rejected() {
    let dir = scratch_dir();
    let path = dir.join("billing_policy.toml");
    let mut body = String::new();
    for (name, allow) in [
        ("web", true),
        ("macos", true),
        ("windows", true),
        ("ios", true), // the violation
        ("android", false),
    ] {
        body.push_str(&format!(
            "[platforms.{name}]\nallow_purchase_ui = {allow}\nshow_prices = {allow}\n\
             allow_purchase_cta_in_locked_messages = {allow}\n"
        ));
    }
    std::fs::write(&path, body).unwrap();

    match load_billing_policy(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("ios"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn showing_prices_on_mobile_is_rejected_even_without_purchase_ui() {
    let dir = scratch_dir();
    let path = dir.join("billing_policy.toml");
    let mut body = String::new();
    for name in ["web", "macos", "windows"] {
        body.push_str(&format!(
            "[platforms.{name}]\nallow_purchase_ui = true\nshow_prices = true\n\
             allow_purchase_cta_in_locked_messages = true\n"
        ));
    }
    body.push_str(
        "[platforms.ios]\nallow_purchase_ui = false\nshow_prices = true\n\
         allow_purchase_cta_in_locked_messages = false\n",
    );
    body.push_str(
        "[platforms.android]\nallow_purchase_ui = false\nshow_prices = false\n\
         allow_purchase_cta_in_locked_messages = false\n",
    );
    std::fs::write(&path, body).unwrap();

    match load_billing_policy(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("prices"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn a_cta_without_purchase_ui_is_rejected_on_any_platform() {
    let dir = scratch_dir();
    let path = dir.join("billing_policy.toml");
    let mut body = String::new();
    // web has a CTA but no purchase UI: internally inconsistent.
    body.push_str(
        "[platforms.web]\nallow_purchase_ui = false\nshow_prices = false\n\
         allow_purchase_cta_in_locked_messages = true\n",
    );
    for name in ["macos", "windows"] {
        body.push_str(&format!(
            "[platforms.{name}]\nallow_purchase_ui = true\nshow_prices = true\n\
             allow_purchase_cta_in_locked_messages = true\n"
        ));
    }
    for name in ["ios", "android"] {
        body.push_str(&format!(
            "[platforms.{name}]\nallow_purchase_ui = false\nshow_prices = false\n\
             allow_purchase_cta_in_locked_messages = false\n"
        ));
    }
    std::fs::write(&path, body).unwrap();

    match load_billing_policy(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("web"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn an_undeclared_platform_denies_by_default() {
    // `for_platform` must never fall back to a permissive policy: a future
    // platform added to the enum but missing from the file must deny.
    let dir = scratch_dir();
    let path = dir.join("billing_policy.toml");
    let mut body = String::new();
    for (name, allow) in [
        ("web", true),
        ("macos", true),
        ("windows", true),
        ("ios", false),
        ("android", false),
    ] {
        body.push_str(&format!(
            "[platforms.{name}]\nallow_purchase_ui = {allow}\nshow_prices = {allow}\n\
             allow_purchase_cta_in_locked_messages = {allow}\n"
        ));
    }
    std::fs::write(&path, body).unwrap();

    // Loads fine; the deny-by-default path is exercised through a policy that
    // is missing a key.
    let policy = load_billing_policy(&path).unwrap();
    let mut sparse = policy.clone();
    sparse.platforms.remove(&Platform::Android);
    assert!(
        !sparse.purchase_allowed(Platform::Android),
        "a missing platform entry must deny, not allow"
    );
    assert!(!sparse.for_platform(Platform::Android).show_prices);
}
