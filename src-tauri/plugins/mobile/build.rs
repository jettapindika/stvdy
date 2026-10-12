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

const COMMANDS: &[&str] = &["mobile_scheduler_register", "mobile_background_sync"];

fn main() {
    // `expect` is denied in this crate: the shell's lint block reaches it through
    // `[lints] workspace = true`, and the project's rule is "no `expect` in non-test
    // code". A build script that panics also prints a Rust backtrace, where the reader
    // needs the ACL generator's own error — so the cause is printed and the build
    // fails, which is the same shape `tauri_plugin::Builder::build` uses internally.
    if let Err(error) = tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .try_build()
    {
        eprintln!("stvdy-mobile: the plugin's ACL could not be generated: {error:#}");
        std::process::exit(1);
    }
}
