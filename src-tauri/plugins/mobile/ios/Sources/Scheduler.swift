import BackgroundTasks
import Foundation
import os.log

/// The iOS background scheduler seam (A315).
///
/// **Nothing in this file touches a webview, a window or the UI.** Every symbol it
/// uses is `BGTaskScheduler` and `os_log`. `BGAppRefreshTask` runs an app that may
/// have no window at all, so the handlers do exactly one thing: hand a pass to the
/// Rust sync engine. `tests/scheduler.rs` asserts the webview rule structurally,
/// because there is no runtime here in which a background task could render JS to
/// observe.
///
/// ## The one hop that is not wired, and why
///
/// The *scheduling* half is real: `BGTaskScheduler.register` and `.submit` are public
/// APIs and both run. The *pass* is not, and the reason is a measurement rather than a
/// preference. Reaching the Rust command from a background handler needs
/// `PluginManager.invoke(name:invoke:)`, and Tauri 2.12.3 declares that method
/// `internal` — as it does `PluginManager.shared`:
///
/// ```text
/// error: 'invoke' is inaccessible due to 'internal' protection level
/// error: 'shared' is inaccessible due to 'internal' protection level
/// ```
///
/// The two public routes out are both closed for a reason:
///
///   - `Plugin.trigger(_:data:)` is public, and it **emits a frontend event** — the
///     exact thing A315 forbids, and pointless besides, because a background launch
///     may have no webview listening.
///   - A `@_cdecl` entry point that Rust exports would need
///     `#[unsafe(no_mangle)]`, which this workspace's lint block refuses
///     (`unsafe_code = "forbid"`; see `android/src/main/java/app/stvdy/mobile/StvdySyncSeam.kt`,
///     where the same wall is documented from the other side).
///
/// So the handler reports the deferral instead of pretending: the schedule is real,
/// and the pass is the one the app runs on its next foreground, through the same
/// `sync::sync_once`. That is A316's own position — a background window is
/// opportunistic and the foreground sync is what makes a device correct (A317 for the
/// inaccuracy) — so the outcome is degraded and honest rather than silent.
enum Scheduler {
  /// The identifiers, which must match `scheduler.rs`, `StvdySyncWork.kt` and
  /// `BGTaskSchedulerPermittedIdentifiers` in `gen/apple/Info.ios.plist`. A mismatch
  /// is a task that never fires and never says so.
  static let syncWorkName = "app.stvdy.sync"
  static let refreshTaskId = "app.stvdy.sync.refresh"
  static let processingTaskId = "app.stvdy.sync.processing"

  /// The minimum Android enforces, mirrored here so both platforms report the same
  /// number back to the SPA. iOS has no such floor, but a schedule that means
  /// different things per platform is one the SPA would have to special-case.
  static let minIntervalMinutes = 15

  private static let log = OSLog(subsystem: "app.stvdy", category: "mobile")

  /// Registers both handlers. Called from the plugin's `load(webview:)`, which runs
  /// inside `application(_:didFinishLaunchingWithOptions:)` — the only window in
  /// which a `BGTaskScheduler` handler may be registered, and the reason A311 puts
  /// the scheduler in a plugin.
  static func registerHandlers() {
    BGTaskScheduler.shared.register(
      forTaskWithIdentifier: refreshTaskId, using: nil
    ) { task in
      handle(task: task, window: "refresh")
    }
    BGTaskScheduler.shared.register(
      forTaskWithIdentifier: processingTaskId, using: nil
    ) { task in
      handle(task: task, window: "processing")
    }
    os_log("background task handlers registered", log: log, type: .info)
  }

  /// Submits both requests. Resubmitting from inside the handler is what keeps the
  /// schedule alive: a `BGTaskScheduler` request fires once and is then gone.
  static func submit(intervalMinutes: Int) {
    let interval = max(intervalMinutes, minIntervalMinutes)
    let refresh = BGAppRefreshTaskRequest(identifier: refreshTaskId)
    refresh.earliestBeginDate = Date(timeIntervalSinceNow: Double(interval) * 60)
    let processing = BGProcessingTaskRequest(identifier: processingTaskId)
    processing.earliestBeginDate = Date(timeIntervalSinceNow: Double(interval) * 60)
    // The processing window is the one that may need the network and may need to run
    // longer than the refresh budget, so it asks for both.
    processing.requiresNetworkConnectivity = true
    processing.requiresExternalPower = false

    for request in [refresh as BGTaskRequest, processing as BGTaskRequest] {
      do {
        try BGTaskScheduler.shared.submit(request)
        os_log("scheduled %{public}@", log: log, type: .info, request.identifier)
      } catch {
        // `BGTaskScheduler.Error.Code.notPermitted` here almost always means the
        // identifier is missing from `BGTaskSchedulerPermittedIdentifiers`, or that
        // this is a simulator build (the scheduler does not run there). Logged rather
        // than thrown: a failed schedule degrades to the next foreground sync
        // (A316, A317), it does not break the app.
        os_log("could not schedule %{public}@: %{public}@", log: log, type: .error,
               request.identifier, error.localizedDescription)
      }
    }
  }

  /// The handler body: resubmit, report what can be done, and complete the task.
  ///
  /// `setTaskCompleted(success: true)` even though no pass ran, and that is deliberate
  /// rather than sloppy: `false` tells the system the *task* failed, which counts
  /// against this app's background budget for a failure that is not transient — the
  /// entry point does not exist in this build. The deferral is logged instead, which
  /// is where a reader finds it.
  private static func handle(task: BGTask, window: String) {
    os_log("background window %{public}@ opened", log: log, type: .info, window)
    // Resubmitted first, so a window that is cut off still leaves the next one
    // scheduled.
    submit(intervalMinutes: minIntervalMinutes)
    task.expirationHandler = {
      os_log("background window %{public}@ expired", log: log, type: .default, window)
    }
    runBackgroundSync(window: window) { outcome in
      os_log("background sync %{public}@: %{public}@", log: log, type: .info, window, outcome)
      task.setTaskCompleted(success: true)
    }
  }

  /// One background pass, or the honest report that this build cannot start one.
  ///
  /// The completion is called synchronously: there is no bridge to await, and a
  /// completion that is never called is a `BGTask` that runs until its expiry, which
  /// the OS counts against the app.
  static func runBackgroundSync(
    window: String, completion: @escaping (String) -> Void
  ) {
    completion("deferred to the next foreground sync (no Rust entry point in this build)")
  }
}
