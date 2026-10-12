package app.stvdy.mobile

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters

/**
 * The periodic background sync (A311, A315).
 *
 * It runs with no activity and no window — Android may have killed the process and
 * restarted it just for this job — so there is nothing here to notify and no webview
 * to reach. The pass goes through [StvdySyncSeam], which is also where the one hop
 * Android cannot currently make is documented.
 *
 * **`Result.success()` on [SyncOutcome.UNAVAILABLE], not `retry()`.** A retry means
 * "try again and it may work"; nothing about a missing Rust entry point changes
 * between attempts, and `WorkManager` would retry for ever, which is a battery drain
 * dressed up as persistence. `retry()` is reserved for [SyncOutcome.FAILED], where a
 * retry is the right response to a network that was not there.
 */
class BackgroundSyncWorker(
    context: Context,
    params: WorkerParameters,
) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result = when (StvdySyncSeam.backgroundSync("periodic")) {
        SyncOutcome.DONE -> Result.success()
        SyncOutcome.UNAVAILABLE -> Result.success()
        SyncOutcome.FAILED -> Result.retry()
    }

    companion object {
        const val TAG = "StvdyBackgroundSync"
    }
}
