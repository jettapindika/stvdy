//! The Stvdy mobile plugin (A311): one crate for the native work no plugin layer can
//! do — the iOS `BGTaskScheduler` registration and handlers, the Android
//! `WorkManager` worker, the FCM service seam and the Android share intake.
//!
//! **One plugin, three mechanisms, and the split is by platform mechanism.** The
//! capability matrix budgets "one native-work package"; it is split only where the
//! platform forces it — the iOS Share Extension is an app-extension *target* and
//! cannot be a plugin at all (A312), so it is hand-wired into the XcodeGen template;
//! push receive is a pinned crate (A310). Everything else shares this crate because
//! it shares a `setup()` hook and a Rust entry point, and three crates would be three
//! copies of the same lifecycle code.
//!
//! **The seam never touches the frontend (A315).** [`scheduler::run_background_sync`]
//! calls the shell's own pass and nothing else; [`intake`] writes a file and returns.
//! `tests/scheduler.rs` asserts that structurally, in this crate and in the Kotlin
//! and Swift halves, because a background launch has no window to assert against at
//! runtime.

pub mod commands;
pub mod intake;
pub mod scheduler;

use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

pub use commands::{ScheduleAck, mobile_background_sync, mobile_scheduler_register, mobile_share_intake};
pub use intake::{INBOX_DIR, IntakeError, SharedFile};
pub use scheduler::{
    BackgroundReport, BackgroundSync, MIN_INTERVAL_MINUTES, PROCESSING_TASK_ID, PassOutcome,
    REFRESH_TASK_ID, SYNC_WORK_NAME, SchedulerError, SyncPass,
};

/// Initializes the plugin with the shell's background pass.
///
/// **`setup()` is the only seam that runs before the launch sequence ends**, which is
/// what the iOS `BGTaskScheduler` registration requires: the handlers must be
/// registered before `application(_:didFinishLaunchingWithOptions:)` returns or a
/// launch that the scheduler woke for a background task throws
/// `NSInternalInconsistencyException`. The native halves do the registering; this
/// hook is what guarantees it happened, and it is why the scheduler lives in a plugin
/// rather than in a command.
///
/// `pass` is the shell's own `SyncShell::sync_once` behind a closure. It is an
/// argument rather than a path dependency because this crate and the shell depend on
/// each other the other way round: the shell registers this plugin, so this plugin
/// cannot name the shell (see `Cargo.toml`).
pub fn init<R: Runtime>(pass: SyncPass) -> TauriPlugin<R> {
    Builder::new("stvdy-mobile")
        .invoke_handler(tauri::generate_handler![
            commands::mobile_scheduler_register,
            commands::mobile_background_sync,
            commands::mobile_share_intake,
        ])
        .setup(move |app, _api| {
            // The pass is managed *here*, not by the shell: it is this plugin's state
            // and the background entry points read it from the app they were woken in.
            app.manage(BackgroundSync::new(pass));
            tracing::info!(
                work = SYNC_WORK_NAME,
                refresh = REFRESH_TASK_ID,
                processing = PROCESSING_TASK_ID,
                "mobile plugin initialized; the native scheduler registers from here"
            );
            Ok(())
        })
        .build()
}
