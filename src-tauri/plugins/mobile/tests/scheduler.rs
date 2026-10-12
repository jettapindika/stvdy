//! A315's mechanical defence: the background entry points call Rust and never the
//! webview. This is a source assertion because the property is structural — there
//! is no runtime in which a background task could render JS to observe.
//!
//! `unwrap`/`expect`/`panic` are denied in non-test code by the workspace lints;
//! this is test code, where a failed precondition should stop the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// The source with `//` comment lines removed.
///
/// The assertions below are about what the code *does*, and every one of these files
/// explains in a comment what it must not do — so a naive substring check would fail on
/// the explanation. Stripping the comment lines keeps the assertion pointed at code,
/// and the doc comments themselves are covered by the plan and by review.
fn code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_scheduler_seam_never_references_the_webview() {
    let source = include_str!("../src/scheduler.rs");
    let source = code(source);
    for forbidden in ["WebviewWindow", "emit(", "eval(", "get_webview"] {
        assert!(
            !source.contains(forbidden),
            "the background seam must not touch the frontend: found {forbidden}"
        );
    }
    // The plan's literal assertion here is `source.contains("sync_once")`. It cannot
    // hold, and the reason is a cycle rather than an oversight: the plugin cannot
    // depend on the shell (the shell depends on the plugin), so `sync_once` is the
    // shell's and the seam *invokes the pass the shell supplied*. What replaces it is
    // the property that matters — the seam calls exactly that pass and defines no
    // second implementation. `the_seam_runs_the_shells_pass_rather_than_its_own` below
    // asserts the other half.
    assert!(
        source.contains("(state.pass)()"),
        "the seam's whole job is to run the pass the shell supplied"
    );
}

/// The Android intake is the second half of A311's one plugin and the same rule
/// applies to it: a share arrives while the app is not running, so an intake that
/// reached for the frontend would be reaching for a window that does not exist.
///
/// The Kotlin half is asserted at the same time because it is the *other* entry
/// point the OS calls with no app process (`FirebaseMessagingService`,
/// `WorkManager`); a Kotlin file that posted to a webview would be the same defect
/// one language over.
#[test]
fn the_android_intake_never_references_the_webview() {
    let intake = include_str!("../src/intake.rs");
    let intake = code(intake);
    for forbidden in ["WebviewWindow", "emit(", "eval(", "get_webview"] {
        assert!(
            !intake.contains(forbidden),
            "the intake must not touch the frontend: found {forbidden}"
        );
    }
}

/// The Kotlin sources are read as text and held to the same rule, so the native
/// half cannot quietly become the place the frontend is reached from.
#[test]
fn the_kotlin_sources_do_not_post_to_a_webview() {
    for (name, source) in [
        (
            "MobilePlugin.kt",
            include_str!("../android/src/main/java/app/stvdy/mobile/MobilePlugin.kt"),
        ),
        (
            "BackgroundSyncWorker.kt",
            include_str!("../android/src/main/java/app/stvdy/mobile/BackgroundSyncWorker.kt"),
        ),
        (
            "StvdyMessagingService.kt",
            include_str!("../android/src/main/java/app/stvdy/mobile/StvdyMessagingService.kt"),
        ),
        (
            "ShareIntakeActivity.kt",
            include_str!("../android/src/main/java/app/stvdy/mobile/ShareIntakeActivity.kt"),
        ),
    ] {
        let source = code(source);
        for forbidden in ["WebView", "evaluateJavascript", "loadUrl"] {
            assert!(
                !source.contains(forbidden),
                "{name} must not touch a webview: found {forbidden}"
            );
        }
    }
}

/// The Swift seam, for the same reason: `BGTaskScheduler` runs an app that may have
/// no window at all.
///
/// `Scheduler.swift` is the file asserted, not `MobilePlugin.swift`: the plugin class
/// overrides Tauri's own `load(webview: WKWebView)` signature, which is the hook that
/// must exist for the registration to happen inside the launch sequence. The *seam* —
/// the file the background handlers live in — is the one held to the rule.
#[test]
fn the_swift_seam_does_not_touch_the_webview() {
    let scheduler = code(include_str!("../ios/Sources/Scheduler.swift"));
    for forbidden in ["WebviewWindow", "evaluateJavaScript", "WKWebView", "WebKit"] {
        assert!(
            !scheduler.contains(forbidden),
            "the iOS background seam must not touch a webview: found {forbidden}"
        );
    }
}

/// The plugin class may hold Tauri's webview hook, but it must not evaluate anything
/// in it — the hook exists to register the `BGTaskScheduler` handlers, nothing else.
#[test]
fn the_plugin_class_only_holds_the_webview_for_its_lifecycle_hook() {
    let plugin = code(include_str!("../ios/Sources/MobilePlugin.swift"));
    for forbidden in ["evaluateJavaScript", "WebviewWindow"] {
        assert!(
            !plugin.contains(forbidden),
            "the plugin class must not evaluate in the webview: found {forbidden}"
        );
    }
    assert!(
        plugin.contains("Scheduler.registerHandlers"),
        "the webview hook's one job is to register the background handlers"
    );
}

/// Neither native seam may fall back to a frontend event. `Plugin.trigger` is the one
/// public route Tauri's Swift API leaves open for a headless caller, and using it would
/// be A315's prohibition with extra steps — a background launch has no listener, so the
/// event would be emitted to nobody and the pass would still not have run.
#[test]
fn neither_native_seam_emits_a_frontend_event() {
    for (name, source) in [
        ("Scheduler.swift", include_str!("../ios/Sources/Scheduler.swift")),
        (
            "MobilePlugin.swift",
            include_str!("../ios/Sources/MobilePlugin.swift"),
        ),
    ] {
        assert!(
            !code(source).contains(".trigger("),
            "{name} must not emit a frontend event"
        );
    }
}

/// The scheduler identifiers are written into four files — `scheduler.rs`, the Swift
/// seam, `StvdySyncWork.kt` and `Info.ios.plist`'s
/// `BGTaskSchedulerPermittedIdentifiers` — and a mismatch is a job that never fires
/// and never says so. This compares the copies rather than trusting them.
#[test]
fn the_scheduler_identifiers_agree_across_the_native_halves() {
    let rust = include_str!("../src/scheduler.rs");
    let swift = include_str!("../ios/Sources/Scheduler.swift");
    let kotlin = include_str!("../android/src/main/java/app/stvdy/mobile/StvdySyncWork.kt");

    for source in [rust, swift, kotlin] {
        assert!(
            source.contains("app.stvdy.sync"),
            "every half must name the same work identifier"
        );
    }
    for source in [rust, swift] {
        for identifier in ["app.stvdy.sync.refresh", "app.stvdy.sync.processing"] {
            assert!(
                source.contains(identifier),
                "the refresh and processing identifiers must match: missing {identifier}"
            );
        }
    }
    // The Android floor is `WorkManager`'s, and the shell clamps to it before the
    // value ever reaches Kotlin.
    assert!(rust.contains("MIN_INTERVAL_MINUTES: u32 = 15"));
    assert!(kotlin.contains("MIN_INTERVAL_MINUTES = 15L"));
}

/// The seam's whole job is to run the shell's own pass, and there is exactly one such
/// pass. This asserts the *inversion* the cycle forced: the plugin cannot name the
/// shell crate (the shell depends on the plugin), so the pass arrives as
/// [`scheduler::SyncPass`] and the shell is what calls `sync_once`. Asserting the
/// direction here is what stops a future edit from adding a second sync implementation
/// inside the plugin "because the closure was inconvenient".
#[test]
fn the_seam_runs_the_shells_pass_rather_than_its_own() {
    let source = include_str!("../src/scheduler.rs");
    assert!(
        source.contains("SyncPass"),
        "the seam must take the shell's pass as an injected closure"
    );
    assert!(
        !source.contains("stvdy_shell_lib::sync::SyncShell"),
        "the seam must not reach for the shell's types: that dependency is the cycle"
    );
    assert!(
        !source.contains("fn sync_once"),
        "the seam must not define a second pass: the shell owns `sync_once`"
    );
}
