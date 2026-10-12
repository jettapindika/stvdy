package app.stvdy.mobile

import android.app.Activity
import android.content.Intent
import android.os.Build
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import java.util.concurrent.TimeUnit

/**
 * The Android half of the mobile plugin (A311).
 *
 * Two jobs and nothing else: it registers the periodic `WorkManager` request, and it
 * asks the OS for the notification permission. The share intake is [ShareIntake]'s and
 * the background pass is [StvdySyncSeam]'s, which documents the one hop Android cannot
 * make today. **Nothing here touches a webview** — a worker, an FCM message and a
 * share can all arrive with no window at all (A315), and `tests/scheduler.rs` asserts
 * that from the Rust side.
 */
@TauriPlugin(
    permissions = [
        // Declared here so the plugin can ask for it; the manifest declares the same
        // string, and both are needed (a runtime request for an undeclared permission
        // is denied without a prompt).
        Permission(strings = [android.Manifest.permission.POST_NOTIFICATIONS], alias = "notifications"),
    ],
)
class MobilePlugin(private val activity: Activity) : Plugin(activity) {

    companion object {
        /**
         * One unique periodic job, not a new job per schedule call. `KEEP` rather
         * than `REPLACE`: re-registering on every foreground would otherwise reset
         * the period and the job would never fire.
         */
        const val SYNC_WORK_NAME = StvdySyncWork.NAME

        /**
         * `WorkManager`'s own minimum. Asking for less throws
         * `IllegalArgumentException` on a device, where nothing tests it, so the Rust
         * side clamps before it gets here.
         */
        const val MIN_INTERVAL_MINUTES = StvdySyncWork.MIN_INTERVAL_MINUTES
    }

    /**
     * The **cold-start** half of the share intake.
     *
     * `load` runs while the activity is being created, and `activity.intent` is the
     * intent that created it — so a share that *launched* the app is filed here. This
     * is the only seam that sees it: Tauri's plugin lifecycle has `onNewIntent` and no
     * `onCreate(intent)`, so a warm share and a cold one arrive by different routes and
     * both must be handled, or half of all shares are silently dropped.
     */
    override fun load(webView: android.webkit.WebView) {
        if (ShareIntake.handle(activity, activity.intent)) {
            // Consume it, so a later configuration change that recreates the activity
            // does not file the same file a second time.
            activity.intent = null
        }
    }

    /**
     * A share that arrived while the activity was alive (`launchMode="singleTask"`
     * means the running activity is reused rather than a second one started).
     */
    override fun onNewIntent(intent: Intent) {
        ShareIntake.handle(activity, intent)
    }

    /**
     * Registers the periodic sync. `intervalMinutes` arrives already clamped by
     * `mobile_scheduler_register`.
     */
    @Command
    fun mobileSchedulerRegister(invoke: Invoke) {
        val args = invoke.parseArgs(SchedulerArgs::class.java)
        val interval = args.intervalMinutes.coerceAtLeast(MIN_INTERVAL_MINUTES)

        val request = PeriodicWorkRequestBuilder<BackgroundSyncWorker>(
            interval, TimeUnit.MINUTES,
        )
            // A background sync with no network is a pass that can only report
            // offline; `CONNECTED` lets the OS wait for a link instead of spending
            // the window on a request that cannot succeed.
            .setConstraints(
                Constraints.Builder()
                    .setRequiredNetworkType(NetworkType.CONNECTED)
                    .build(),
            )
            .build()

        WorkManager.getInstance(activity).enqueueUniquePeriodicWork(
            SYNC_WORK_NAME,
            ExistingPeriodicWorkPolicy.KEEP,
            request,
        )

        val result = JSObject()
        result.put("intervalMinutes", interval)
        invoke.resolve(result)
    }

    /**
     * Asks for `POST_NOTIFICATIONS`, which is runtime-granted from API 33. Below that
     * the permission does not exist and notifications are granted at install time, so
     * the answer is `granted` without a prompt.
     */
    @Command
    fun mobileRequestNotificationPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            requestPermissionForAlias("notifications", invoke, "notificationsResult")
        } else {
            val result = JSObject()
            result.put("granted", true)
            invoke.resolve(result)
        }
    }

    @PermissionCallback
    fun notificationsResult(invoke: Invoke) {
        val result = JSObject()
        result.put(
            "granted",
            getPermissionState("notifications") == app.tauri.PermissionState.GRANTED,
        )
        invoke.resolve(result)
    }

    /**
     * One background pass, for the FCM silent-push path and for a manual trigger.
     * The periodic job goes through the worker; both end at the same seam.
     */
    @Command
    fun mobileBackgroundSync(invoke: Invoke) {
        val args = invoke.parseArgs(WindowArgs::class.java)
        val window = if (args.window == "processing") "processing" else "refresh"
        val result = JSObject()
        result.put("outcome", StvdySyncSeam.backgroundSync(window).name)
        invoke.resolve(result)
    }

    /** `mobile_scheduler_register`'s argument. */
    @InvokeArg
    internal class SchedulerArgs {
        var intervalMinutes: Long = MIN_INTERVAL_MINUTES
    }

    /** `mobile_background_sync`'s argument. */
    @InvokeArg
    internal class WindowArgs {
        var window: String? = null
    }
}
