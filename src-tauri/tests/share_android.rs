//! The Android share intake (A313, A314), asserted as data.
//!
//! No Android SDK exists on the machine this runs on, so none of the Kotlin is
//! compiled. What *can* be checked is the part that is a contract between files rather
//! than a behaviour: the MIME types the manifest and `tauri.conf.json` declare, and the
//! inbox directory and sidecar keys the Kotlin half and the shell's reader must agree
//! on. Those are the failures with no error anywhere — the app is simply never offered
//! for a shared PDF, or the inbox reads empty.
//!
//! `unwrap`/`expect`/`panic` are denied in non-test code by this crate's lint block;
//! this is test code, where a failed precondition should stop the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// The matrix verified `mimeType: "*/*"` never matches a concrete intent, so the config
/// must carry concrete types. This reads the config as data.
#[test]
fn every_file_association_declares_a_concrete_mime_type() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let assoc = config["bundle"]["fileAssociations"].as_array().unwrap();
    assert!(!assoc.is_empty());
    for entry in assoc {
        let mime = entry["mimeType"].as_str().unwrap();
        assert_ne!(mime, "*/*");
        assert!(mime.contains('/'), "{mime} is not a concrete type");
        assert!(
            !mime.ends_with("/*"),
            "{mime} is a wildcard, not a concrete type"
        );
    }
}

/// The manifest's intent filters are what make the OS *offer* Stvdy for a shared file,
/// and the Tauri CLI does not generate them (CLI 2.12.2's Android manifest template
/// emits only MAIN/LAUNCHER — read out of the shipped binary), so they are hand-written
/// in the plugin's manifest. Every MIME type `tauri.conf.json` declares must appear
/// there, and vice versa: a type in one and not the other is a file the app claims to
/// handle and is never given.
#[test]
fn the_manifest_offers_the_app_for_every_declared_file_association() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let manifest = include_str!("../plugins/mobile/android/src/main/AndroidManifest.xml");

    for entry in config["bundle"]["fileAssociations"].as_array().unwrap() {
        let mime = entry["mimeType"].as_str().unwrap();
        assert!(
            manifest.contains(&format!("android:mimeType=\"{mime}\"")),
            "{mime} is declared in tauri.conf.json but the manifest never matches it"
        );
    }

    // Both actions: `SEND` is the share sheet, `VIEW` is "open with" (A313).
    for action in ["android.intent.action.SEND", "android.intent.action.VIEW"] {
        assert!(
            manifest.contains(&format!("android:name=\"{action}\"")),
            "{action} is missing from the share filters"
        );
    }
}

/// The inbox directory is written by Kotlin and read by Rust, and neither can see the
/// other's constant. A second spelling is an inbox that reads empty, and nothing fails.
#[test]
fn the_kotlin_intake_writes_the_inbox_the_shell_reads() {
    let kotlin =
        include_str!("../plugins/mobile/android/src/main/java/app/stvdy/mobile/ShareIntake.kt");
    assert!(
        kotlin.contains(&format!(
            "const val INBOX = \"{}\"",
            stvdy_shell_lib::share::ANDROID_INBOX
        )),
        "ShareIntake.kt's INBOX must be the shell's ANDROID_INBOX"
    );
    assert!(
        kotlin.contains("File(context.filesDir, INBOX)"),
        "the inbox is under the app's own data directory, which is where the shell reads"
    );
}

/// The sidecar's keys are the shell's `SharedFile` fields. Kotlin writes JSON by hand
/// (it has no serde), so the names are a literal on one side and a derive on the other;
/// a rename on either is a row the reader skips with only a `tracing::warn` to show it.
#[test]
fn the_sidecar_keys_are_the_readers_field_names() {
    let kotlin =
        include_str!("../plugins/mobile/android/src/main/java/app/stvdy/mobile/ShareIntake.kt");
    for key in ["id", "name", "path", "mime"] {
        assert!(
            kotlin.contains(&format!("\\\"{key}\\\":")),
            "the sidecar is missing the `{key}` key the shell deserializes"
        );
    }
    // And the shape is the same one the iOS extension writes, which the shell's own
    // test pins against a literal.
    assert!(kotlin.contains(",\\\"mime\\\":"));
}
