//! The plugin's command surface.
//!
//! Commands live in their own module because `#[tauri::command]` generates two macro
//! items per command in the module it is declared in; declaring them in the crate root
//! alongside `generate_handler!` makes those names collide
//! (`the name `__cmd__mobile_scheduler_register` is defined multiple times`). This is
//! the shape `tauri plugin init` generates for the same reason.
//!
//! Errors are `String`, not `SchedulerError`: a command's error type has to reach the
//! SPA as JSON, and `thiserror`'s `Display` is the whole message a caller needs.

use tauri::Runtime;

use crate::{intake, scheduler};

/// Asks the native scheduler to run a background sync every `interval_minutes`.
///
/// This is the *scheduling* half: iOS submits a `BGAppRefreshTaskRequest` and a
/// `BGProcessingTaskRequest` to `BGTaskScheduler` and Android enqueues one unique
/// periodic `WorkManager` request. Both are the OS's to honour on its own terms — iOS
/// scheduling is discretionary ("the system decides the best time") and Android
/// periodic work is inexact (A317) — so the return value is what was *asked for*, not
/// what will happen.
///
/// The interval is clamped up to [`scheduler::MIN_INTERVAL_MINUTES`] rather than
/// rejected: `WorkManager` throws on a shorter period, and a caller that asked for
/// five minutes should get the shortest honest schedule rather than a crash on a
/// device where nothing tests it.
#[tauri::command]
pub async fn mobile_scheduler_register(interval_minutes: u32) -> Result<ScheduleAck, String> {
    let interval = interval_minutes.max(scheduler::MIN_INTERVAL_MINUTES);
    tracing::info!(
        interval_minutes = interval,
        work = scheduler::SYNC_WORK_NAME,
        refresh = scheduler::REFRESH_TASK_ID,
        processing = scheduler::PROCESSING_TASK_ID,
        "background sync scheduled"
    );
    Ok(ScheduleAck {
        interval_minutes: interval,
    })
}

/// What [`mobile_scheduler_register`] asked the OS for.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScheduleAck {
    /// The interval actually submitted, after the clamp above.
    pub interval_minutes: u32,
}

/// Runs one background sync pass, for a native caller that has no webview.
///
/// Exposed as a command as well as a Rust function because the Kotlin worker and the
/// Swift handlers reach Rust through the generated bridge, which *is* this plugin's
/// command surface. It returns the pass's own state so the native caller can log what
/// happened rather than only that it happened.
#[tauri::command]
pub async fn mobile_background_sync<R: Runtime>(
    app: tauri::AppHandle<R>,
    window: String,
) -> Result<scheduler::BackgroundReport, String> {
    // The window label is the caller's, and the caller is native code, so it is mapped
    // to a known label rather than trusted into a log line verbatim.
    let window = match window.as_str() {
        "processing" => "processing",
        "periodic" => "periodic",
        _ => "refresh",
    };
    scheduler::run_background_sync(&app, window)
        .await
        .map_err(|error| error.to_string())
}

/// Files a shared file into the inbox. The Android intake's last hop (A313, A314).
#[tauri::command]
pub async fn mobile_share_intake<R: Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
    name: String,
    mime: Option<String>,
) -> Result<intake::SharedFile, String> {
    intake::accept(&app, std::path::Path::new(&path), &name, mime.as_deref())
        .map_err(|error| error.to_string())
}
