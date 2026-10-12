package app.stvdy.mobile

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * The FCM receive seam (A310's Android half, A311's one plugin).
 *
 * Two messages matter and they are different:
 *
 *   - **A data-only (silent) push** arrives in `onMessageReceived` and is a hint to
 *     sync. It goes to [StvdySyncSeam], the same place the periodic worker goes, and
 *     never to a webview — the app may be backgrounded and its webview suspended
 *     (A315, A316).
 *   - **A notification push** is rendered by the OS itself and never reaches this
 *     class; the tap opens the app and the SPA routes from the payload (A316:
 *     tap-to-open is the only guaranteed JS surface).
 *
 * A new token is the one thing here that must reach Rust, because the token is what
 * the server sends to. It is logged by length only — the token is a device
 * credential — and the app re-registers it on its next launch (A329).
 */
class StvdyMessagingService : FirebaseMessagingService() {

    companion object {
        const val TAG = "StvdyMessaging"
    }

    override fun onNewToken(token: String) {
        super.onNewToken(token)
        android.util.Log.i(TAG, "a new FCM token arrived (${token.length} chars)")
    }

    override fun onMessageReceived(message: RemoteMessage) {
        super.onMessageReceived(message)
        val wantsSync = message.data["sync"] == "1" || message.data["sync"] == "true"
        if (wantsSync) {
            val outcome = StvdySyncSeam.backgroundSync("periodic")
            android.util.Log.i(TAG, "silent push asked for a sync: ${outcome.name}")
        }
    }
}
