//! The plugin's ACL and native-project wiring.
//!
//! `tauri_plugin::Builder` generates the permission files under `permissions/` and,
//! on a mobile target, copies the Tauri iOS/Android library into `ios/` and
//! `android/` and links the native sources (its `mobile::setup`: on iOS it
//! `link_apple_library`s the Swift package; on Android it emits
//! `cargo:android_library_path`, which is what makes Gradle compile the Kotlin).
//!
//! `try_build` rather than `build` so a docs.rs build of this crate does not need a
//! native project to exist — the same reason the pinned push plugin does it.

const COMMANDS: &[&str] = &[
    "mobile_scheduler_register",
    "mobile_background_sync",
    "mobile_share_intake",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .try_build()
        .expect("the mobile plugin's ACL could not be generated");
}
