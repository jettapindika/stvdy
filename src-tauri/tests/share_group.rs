//! The App Group identifier is the contract between the extension and the app. A
//! second spelling on either side is a silent "the file never arrives": a container
//! that does not exist is a directory that reads empty, and nothing fails.
//!
//! `unwrap`/`expect`/`panic` are denied in non-test code by this crate's lint block;
//! this is test code, where a failed precondition should stop the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The one shared container. Compared as a set everywhere below, never counted as a
/// substring.
const APP_GROUP: &str = "group.app.stvdy";

#[test]
fn the_app_group_identifier_is_declared_in_exactly_one_place() {
    let app = include_str!("../src/share.rs");
    let ext = include_str!("../gen/apple/ShareExtension/ShareViewController.swift");
    assert!(app.contains(APP_GROUP), "the app side names the group");
    assert!(
        ext.contains(APP_GROUP),
        "the extension side names the group"
    );

    // Both files are read whole, so the check is that the *same* literal is the only
    // group identifier in each, not that a substring exists.
    //
    // The plan's snippet counts `"group."`, which is a substring of ordinary prose
    // ("…the App Group. The extension…"), so it would fail on a correct file with a
    // second paragraph. What is counted here is the token: `group.` followed by a
    // letter or digit, which is what a container identifier looks like and what prose
    // does not.
    for (side, source) in [("the app", app), ("the extension", ext)] {
        assert_eq!(
            group_identifiers(source),
            BTreeSet::from([APP_GROUP.to_string()]),
            "{side} names exactly one shared container"
        );
    }
}

/// The App Store validator rejects an extension whose version does not match its host,
/// and the extension's version comes from `project.yml.template`'s `info.properties` —
/// the CLI's `bundle.iOS.bundleVersion` merge reaches the app's plist only. So the two
/// are compared against `tauri.conf.json` rather than trusted to stay in step.
#[test]
fn the_extension_version_matches_the_apps() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses");
    let short = config["version"].as_str().expect("a version");
    let build = config["bundle"]["iOS"]["bundleVersion"]
        .as_str()
        .expect("a bundle version");

    let template = include_str!("../gen/apple/project.yml.template");
    assert!(
        template.contains(&format!("CFBundleShortVersionString: {short}")),
        "the extension's short version must be the app's ({short})"
    );
    assert!(
        template.contains(&format!("CFBundleVersion: \"{build}\"")),
        "the extension's build number must be the app's ({build})"
    );
}

/// The background identifiers are one contract written three times: the plugin's Rust
/// constants, the iOS handlers that register for them, and `Info.ios.plist`'s
/// `BGTaskSchedulerPermittedIdentifiers`. A launch the scheduler woke for an identifier
/// the plist does not permit throws `NSInternalInconsistencyException` and kills the
/// app, so the halves are compared rather than trusted.
///
/// The plugin crate cannot be named here — it is a target-gated dependency, so on the
/// machine these tests run on there is no such crate — so the Swift file is read as
/// text, which is the same trick `tests/push.rs` uses for the server's provider list.
#[test]
fn the_background_identifiers_match_the_permitted_list() {
    let plist = include_str!("../gen/apple/Info.ios.plist");
    let swift = include_str!("../plugins/mobile/ios/Sources/Scheduler.swift");

    for name in ["refreshTaskId", "processingTaskId"] {
        let declaration = swift
            .lines()
            .find(|line| line.contains(&format!("let {name} = ")))
            .unwrap_or_else(|| panic!("Scheduler.swift still declares {name}"));
        let identifier = declaration
            .split('"')
            .nth(1)
            .unwrap_or_else(|| panic!("{name} is a string literal"));
        assert!(
            plist.contains(&format!("<string>{identifier}</string>")),
            "{identifier} is registered but not permitted in Info.ios.plist"
        );
    }

    // And the background modes that make the tasks schedulable at all.
    for mode in ["fetch", "processing", "remote-notification"] {
        assert!(
            plist.contains(&format!("<string>{mode}</string>")),
            "{mode} is missing from UIBackgroundModes"
        );
    }
}

/// The two files above are the ones the plan names; this walks the shell's own sources
/// so a *third* spelling — in the XcodeGen template, the entitlements, the Kotlin or a
/// comment — is caught wherever it is written.
///
/// The set is compared, not a count: the identifier legitimately appears in several
/// files (the reader, the extension, both entitlements blocks, the template), and what
/// must never happen is a *different* container name joining it.
#[test]
fn no_second_shared_container_name_exists_in_the_shell() {
    let mut found = BTreeSet::new();
    for root in source_roots() {
        collect(&root, &mut found);
    }
    assert_eq!(
        found,
        BTreeSet::from([APP_GROUP.to_string()]),
        "the shell must name exactly one shared container"
    );
}

/// The identifiers in a source string: `group.` followed by a letter or digit, then
/// only identifier characters, and at least one further dot — `group.app.stvdy` is one,
/// `group.` at the end of a sentence and `share_group.rs` (a file name, where the dot
/// separates a stem from an extension) are not.
fn group_identifiers(source: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = source;
    while let Some(index) = rest.find("group.") {
        let tail = &rest[index + "group.".len()..];
        let mut identifier = String::from("group.");
        identifier.extend(
            tail.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
        );
        // A container name has a reverse-DNS shape; `group.rs` and `group.` do not.
        if identifier.matches('.').count() >= 2 {
            found.insert(identifier);
        }
        rest = tail;
    }
    found
}

/// The shell's own sources. `target/`, `build/`, `Externals/`, `Pods/` and the
/// generated `.xcodeproj` are excluded: they are build products of the files below, and
/// a stale one is not a second spelling.
fn source_roots() -> Vec<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        "src",
        "tests",
        "capabilities",
        "tauri.conf.json",
        "plugins/mobile/src",
        "plugins/mobile/tests",
        "plugins/mobile/ios",
        "plugins/mobile/android",
        "plugins/mobile/Cargo.toml",
        "plugins/mobile/build.rs",
        "gen/apple/project.yml",
        "gen/apple/project.yml.template",
        "gen/apple/Info.ios.plist",
        "gen/apple/ShareExtension",
        "gen/apple/stvdy-shell_iOS",
    ]
    .iter()
    .map(|relative| here.join(relative))
    .filter(|path| path.exists())
    .collect()
}

fn collect(path: &Path, found: &mut BTreeSet<String>) {
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some("target" | "build" | "Externals" | "Pods" | "node_modules" | ".git")
            ) {
                continue;
            }
            collect(&entry.path(), found);
        }
        return;
    }
    // A binary (an archive, a compiled plist) is not a spelling; `read_to_string`
    // refuses it, which is the check.
    let Ok(source) = std::fs::read_to_string(path) else {
        return;
    };
    found.extend(group_identifiers(&source));
}
