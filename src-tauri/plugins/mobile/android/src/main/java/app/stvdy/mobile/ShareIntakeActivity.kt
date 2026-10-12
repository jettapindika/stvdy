package app.stvdy.mobile

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File
import java.util.UUID

/**
 * The Android share intake (A313, A314).
 *
 * **The copy happens here, before anything else, and that is the point.** A shared
 * file arrives as a `content://` URI whose read grant is transient: `ACTION_GET_CONTENT`
 * grants die when the receiving activity finishes, and nothing in the dialog plugin
 * calls `takePersistableUriPermission`. So the bytes are read through the content
 * resolver and written into app storage *first*; the copied path is what the upload
 * queue holds and the original grant is allowed to die.
 *
 * Nothing here touches a webview: a share can arrive with no window at all, so the
 * intake writes a file and returns. The SPA reads the result later.
 */
object ShareIntakeActivity {

    private const val TAG = "StvdyShareIntake"

    /** Where a copy lands before Rust files it. App-private, so no permission needed. */
    private const val STAGING = "share-staging"

    /**
     * A warm share: the activity exists and the intent arrived while it was alive.
     * Returns whether the intent was one of ours.
     */
    fun dispatch(activity: Activity, intent: Intent?): Boolean {
        val uri = sharedUri(intent) ?: return false
        copyAndFile(activity, uri, intent)
        return true
    }

    /**
     * The `ACTION_SEND` / `ACTION_VIEW` URI, if this intent carries one. A concrete
     * MIME type in `bundle.fileAssociations` is what makes the OS offer this app for
     * a PDF and not for every file on the device (A313).
     */
    fun sharedUri(intent: Intent?): Uri? {
        if (intent == null) return null
        val action = intent.action
        if (action != Intent.ACTION_SEND && action != Intent.ACTION_VIEW) return null
        return @Suppress("DEPRECATION")
        if (action == Intent.ACTION_SEND) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        } else {
            intent.data
        }
    }

    /**
     * Reads the URI through the content resolver and writes a copy into the staging
     * directory. **The URL-based read, never `stat`/`exists`** — those fail on a
     * `content://` URI, which is why this is Kotlin and not a `std::fs` call.
     */
    private fun copyAndFile(context: Context, uri: Uri, intent: Intent) {
        val name = intent.getStringExtra(Intent.EXTRA_TITLE)
            ?: uri.lastPathSegment?.substringAfterLast('/')
            ?: "shared"
        val staging = File(context.filesDir, STAGING).apply { mkdirs() }
        // The staged name is a fresh UUID, never the sender's string: a name from
        // another app joined onto a path is a path traversal waiting to happen.
        val staged = File(staging, UUID.randomUUID().toString())

        try {
            context.contentResolver.openInputStream(uri)?.use { input ->
                staged.outputStream().use { output -> input.copyTo(output) }
            } ?: run {
                android.util.Log.w(TAG, "the shared URI could not be opened: $uri")
                return
            }
        } catch (error: Throwable) {
            android.util.Log.w(TAG, "the shared file could not be copied", error)
            return
        }

        file(context, staged.absolutePath, name, intent.type, null)
    }

    /**
     * Hands an already-copied path to Rust, which files it into the inbox.
     *
     * `invoke` is `null` on the cold-start path, where there is no command in flight
     * to answer — the file is simply left in the inbox for the SPA to find.
     */
    fun file(
        context: Context,
        path: String,
        name: String,
        mime: String?,
        invoke: Invoke?,
    ) {
        // The plugin's own Rust command is the only writer of the inbox, so the
        // layout has one author (see the Rust `intake` module). Reached through the
        // plugin manager's command bridge, which is a Kotlin→Rust direction that
        // exists only while a command is in flight; on the cold-start path the file
        // stays staged and is filed by the next `mobile_share_intake` call.
        val staged = File(path)
        if (!staged.isFile) {
            android.util.Log.w(TAG, "nothing to file at $path")
            invoke?.reject("the shared file was not staged")
            return
        }
        val result = JSObject()
        result.put("path", staged.absolutePath)
        result.put("name", name)
        result.put("mime", mime)
        invoke?.resolve(result)
    }
}
