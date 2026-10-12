//! The background scheduler seam (A315): the boundary the native schedulers call.
//!
//! **Why this file is a separate module and not a command.** The iOS
//! `BGAppRefreshTask`/`BGProcessingTask` handlers and the Android `WorkManager`
//! worker run in a process where the SPA may never have run and the webview may not
//! exist. Their whole job is to reach the Rust sync engine, so this module never
//! names a window, never emits an event and never evaluates JavaScript.
//! `tests/scheduler.rs` asserts that structurally, because there is no runtime here in
//! which a background task could render JS to observe.
//!
//! **The pass itself is injected, and that is not indirection for its own sake.** The
//! plan's file list has this crate depend on `stvdy-shell` *and* the shell depend on
//! this crate, which is a cycle cargo refuses outright:
//!
//! ```text
//! error: cyclic package dependency: package `stvdy-shell` depends on itself
//! ```
//!
//! One direction had to go, and the one that went is this crate's: the shell owns the
//! engine (`SyncShell::sync_once`), so the shell is what supplies the pass, and this
//! crate supplies the native boundary it is supplied *to*. `SyncPass` is the whole
//! interface between them — a boxed async closure returning [`PassOutcome`] — so the
//! shell's types (`SyncShell`, `SyncReport`, `ShellState`) stay on the shell's side
//! and this crate keeps only what it needs to log and to answer a native caller.
//!
//! **What it can and cannot promise.** The shell's credentials are pushed by the SPA
//! and never stored, so a background pass that runs before the app has been opened
//! finds no session and reports `offline` — the engine's own word for "nothing was
//! reached, waiting is the remedy". That is A316 by design: push delivery and
//! background execution are opportunistic hints, and the sync that makes a device
//! correct is the one on the next foreground. The scheduler's inaccuracy is A317: iOS
//! scheduling is discretionary and Android periodic work is inexact and ≥15 minutes,
//! so the honest thing to log is what actually happened.

use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

/// The `WorkManager` unique work name and the `BGTaskScheduler` identifiers, in one
/// place because they are also written into the native halves (the Kotlin
/// `StvdySyncWork` object and the iOS `Info.plist`'s
/// `BGTaskSchedulerPermittedIdentifiers`). A mismatch between them is a scheduled task
/// that never fires and never says so, so they are asserted equal by
/// `tests/scheduler.rs` rather than copied by hand four times.
pub const SYNC_WORK_NAME: &str = "app.stvdy.sync";

/// The refresh identifier. `BGAppRefreshTask` has a ~30 second budget, which is what
/// a page of the outbox plus a pull of a small account fits in.
pub const REFRESH_TASK_ID: &str = "app.stvdy.sync.refresh";

/// The processing identifier, for work that must not be cut off at 30 seconds.
/// `BGProcessingTask` is the OS's discretionary window for exactly this.
pub const PROCESSING_TASK_ID: &str = "app.stvdy.sync.processing";

/// The Android minimum. `WorkManager` rejects anything shorter, so the value is
/// pinned rather than configurable — a caller asking for 5 minutes would get an
/// `IllegalArgumentException` at enqueue time, on a device, with no test to catch it.
pub const MIN_INTERVAL_MINUTES: u32 = 15;

/// What one pass did, in the shape the native halves log and the SPA reads.
///
/// The fields are the shell's `SyncReport`'s, narrowed to what a background caller
/// needs: a background window leaves no UI, so the log line and this value are the
/// only record that anything ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PassOutcome {
    /// `synced | syncing | offline | needs_attention | revoked`.
    pub state: String,
    /// How many mutations were still queued at the end of the pass.
    pub outbox: usize,
}

/// The shell's pass, as this crate sees it: one async call, no shell types.
///
/// `Send + Sync` because it is managed as app state and read from whichever thread
/// the OS woke the app on. Boxed because a background entry point has no place to
/// thread a generic through.
pub type SyncPass = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<PassOutcome, String>> + Send>> + Send + Sync,
>;

/// The injected pass, as Tauri manages it.
pub struct BackgroundSync {
    pass: SyncPass,
}

impl BackgroundSync {
    pub fn new(pass: SyncPass) -> Self {
        Self { pass }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// No pass was injected, which means the shell did not register this plugin. A
    /// real failure rather than a default: a scheduler that returns `Ok` when it did
    /// nothing is the one whose silence reads as "sync is broken".
    #[error("the sync pass was never injected, so there is nothing to run")]
    NoPass,
    #[error("the sync pass failed: {0}")]
    Pass(String),
}

/// What one background pass did, in the shape the native halves log.
///
/// A struct rather than a bare [`PassOutcome`] because the background caller also
/// needs to say *which* window ran (refresh vs processing vs the periodic worker), and
/// a log line that says "synced" without saying what ran it is the one that cannot be
/// read six months later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackgroundReport {
    /// `synced | syncing | offline | needs_attention | revoked`.
    pub state: String,
    pub outbox: usize,
    /// Which scheduler window ran the pass: `refresh`, `processing` or `periodic`.
    pub window: &'static str,
}

/// One background sync pass, through the pass the shell injected.
///
/// This is the single function every native entry point calls — the Kotlin
/// `CoroutineWorker.doWork`, the Swift `BGTaskScheduler` handlers and the FCM
/// service's silent-push path — so there is one answer to "what does a background
/// sync do".
///
/// `window` is the label above, not a parameter with a default: the caller knows which
/// of its own entry points is running and the seam does not.
pub async fn run_background_sync<R: Runtime>(
    app: &AppHandle<R>,
    window: &'static str,
) -> Result<BackgroundReport, SchedulerError> {
    let state = app.state::<BackgroundSync>();
    let outcome = (state.pass)().await.map_err(SchedulerError::Pass)?;
    // Logged here rather than returned only: a background window leaves no UI, so the
    // device log is the only place this is ever read.
    tracing::info!(
        window,
        state = outcome.state,
        outbox = outcome.outbox,
        "background sync finished"
    );
    Ok(BackgroundReport {
        state: outcome.state,
        outbox: outcome.outbox,
        window,
    })
}
