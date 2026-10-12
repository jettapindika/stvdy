import Tauri
import WebKit

/// The plugin class: the Tauri lifecycle hook, and the three commands.
///
/// Everything the background path does is in `Scheduler.swift`, which touches no
/// webview. This file is the part that must, because `load(webview:)` is Tauri's own
/// plugin signature — the hook that runs inside the launch sequence, which is what
/// `BGTaskScheduler` registration requires.
class MobilePlugin: Plugin {
  /// Registers the `BGTaskScheduler` handlers. This runs before
  /// `application(_:didFinishLaunchingWithOptions:)` returns; a launch the scheduler
  /// woke for an unregistered identifier throws `NSInternalInconsistencyException`
  /// and the app is killed.
  @objc public override func load(webview: WKWebView) {
    Scheduler.registerHandlers()
  }

  /// Asks the scheduler for both windows.
  @objc public func mobileSchedulerRegister(_ invoke: Invoke) {
    do {
      let args = try invoke.parseArgs(ScheduleArgs.self)
      let interval = max(args.intervalMinutes ?? Scheduler.minIntervalMinutes,
                         Scheduler.minIntervalMinutes)
      Scheduler.submit(intervalMinutes: interval)
      invoke.resolve(["intervalMinutes": interval])
    } catch {
      invoke.reject(error.localizedDescription)
    }
  }

  /// One pass, on demand: the silent-push path, and the Android worker's iOS
  /// counterpart. Both reach the same seam.
  ///
  /// It resolves with the *outcome* rather than the report, because the report is the
  /// shell's and this build's seam cannot start a pass — see `Scheduler.swift` for the
  /// measured reason. A resolution that carried a fabricated `state` would be a
  /// background sync the SPA could not tell from a real one.
  @objc public func mobileBackgroundSync(_ invoke: Invoke) {
    let args = (try? invoke.parseArgs(WindowArgs.self)) ?? WindowArgs(window: nil)
    let window = args.window == "processing" ? "processing" : "refresh"
    Scheduler.runBackgroundSync(window: window) { outcome in
      invoke.resolve(["window": window, "outcome": outcome])
    }
  }

  /// Rejected on purpose. An iOS share arrives through the Share Extension and the
  /// App Group (A312), never as a `content://` URI, so there is nothing for this
  /// command to copy — and a caller that invoked it on iOS has the wrong platform.
  /// Rejecting is the honest answer; resolving with an empty result would look like
  /// a share that was filed.
  @objc public func mobileShareIntake(_ invoke: Invoke) {
    invoke.reject("the iOS share path is the App Group import, not the intake command")
  }
}

struct ScheduleArgs: Decodable {
  let intervalMinutes: Int?
}

struct WindowArgs: Decodable {
  let window: String?

  init(window: String?) {
    self.window = window
  }
}

/// The plugin's registration point, called by the generated Rust glue.
@_cdecl("init_plugin_stvdy_mobile")
func initPluginMobile() -> Plugin {
  return MobilePlugin()
}
