//! The mobile bundle config, asserted as data.
//!
//! A missing `bundle.android` block does not fail a build — it silently takes the
//! defaults — so the values that matter to a store submission are pinned here.
//!
//! `expect` is allowed here for the same reason it is in `tests/cache.rs`: this is
//! test code, where a failed precondition should stop the test rather than be
//! reported as a value, and the shell's lint block denies it in non-test code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn the_mobile_bundle_declares_its_floor_and_its_deep_link_domain() {
    let raw = include_str!("../tauri.conf.json");
    let config: serde_json::Value = serde_json::from_str(raw).expect("tauri.conf.json parses");
    let bundle = &config["bundle"];
    assert_eq!(bundle["iOS"]["minimumSystemVersion"], "15.0");
    assert_eq!(bundle["android"]["minSdkVersion"], 24);
    assert_eq!(
        bundle["iOS"]["infoPlist"].as_str(),
        Some("gen/apple/Info.ios.plist"),
        "the merged plist is where the App Group and the background modes are declared"
    );
}
