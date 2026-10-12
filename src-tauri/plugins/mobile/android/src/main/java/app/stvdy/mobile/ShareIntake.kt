package app.stvdy.mobile

import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.util.UUID

/**
 * The Android share intake (A313, A314).
 *
 * **The copy happens here, before anything else, and that is the point.** A shared file
 * arrives as a `content://` URI whose read grant is transient: `ACTION_GET_CONTENT`
 * grants die when the receiving activity finishes, and nothing in the dialog plugin
 * calls `takePersistableUriPermission`. So the bytes are read through the content
 * resolver and written into app storage *first*; the copied path is what the upload
 * queue holds and the original grant is allowed to die.
 *
 * **Nothing here touches a webview, and nothing here calls Rust.** A share can arrive
 * on a cold start, before any plugin command is in flight — and the only Kotlin→Rust
 * direction the generated bridge provides answers a command Rust started. So the
 * intake writes the inbox itself, in the layout the shell's reader
 * (`src-tauri/src/share.rs`) owns: `<inbox>/<id>.<ext>` for the bytes and
 * `<inbox>/<id>.json` for the metadata. `tests/share_android.rs` compares this file's
 * literals against that reader's, so the two cannot drift.
 *
 * The inbox is `<filesDir>/share-inbox`. On Android `filesDir` *is* the app data
 * directory Tauri's `app_data_dir()` resolves to, so the shell's reader finds it
 * without either side knowing about the other.
 */
object ShareIntake {

    private const val TAG = "StvdyShareIntake"

    /**
     * The inbox directory name. **The shell's `share::ANDROID_INBOX` is the same
     * string**, asserted by `tests/share_android.rs`; a second spelling here is a
     * container that reads empty and nothing fails.
     */
    const val INBOX = "share-inbox"

    /**
     * Files a share intent, if it is one. Returns whether anything was filed.
     *
     * Called from two places, because Android delivers a share to two: a cold start
     * reaches the launch intent (`activity.intent`, handled from the plugin's `load`),
     * and a warm one reaches `onNewIntent` (`launchMode="singleTask"` means the
     * running activity is reused rather than a second one started).
     */
    fun handle(context: Context, intent: Intent?): Boolean {
        val uri = sharedUri(intent) ?: return false
        return file(context, uri, intent!!)
    }

    /**
     * The `ACTION_SEND` / `ACTION_VIEW` URI, if this intent carries one. A concrete
     * MIME type in the manifest's intent filter is what makes the OS offer this app
     * for a PDF and not for every file on the device (A313).
     */
    fun sharedUri(intent: Intent?): Uri? {
        if (intent == null) return null
        val action = intent.action
        if (action != Intent.ACTION_SEND && action != Intent.ACTION_VIEW) return null
        @Suppress("DEPRECATION")
        return if (action == Intent.ACTION_SEND) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        } else {
            intent.data
        }
    }

    /**
     * Reads the URI through the content resolver and writes both halves of the intake.
     *
     * **The URL-based read, never `stat`/`exists`** — those fail on a `content://`
     * URI, which is why this is Kotlin and not a `std::fs` call.
     */
    private fun file(context: Context, uri: Uri, intent: Intent): Boolean {
        val inbox = File(context.filesDir, INBOX).apply { mkdirs() }
        // The staged name is a fresh UUID, never the sender's string: a name from
        // another app joined onto a path is a path traversal waiting to happen.
        val id = UUID.randomUUID().toString()
        val name = displayName(context, uri) ?: "shared"
        val extension = name.substringAfterLast('.', "").ifEmpty { "bin" }
        val bytes = File(inbox, "$id.$extension")

        try {
            context.contentResolver.openInputStream(uri)?.use { input ->
                bytes.outputStream().use { output -> input.copyTo(output) }
            } ?: run {
                android.util.Log.w(TAG, "the shared URI could not be opened: $uri")
                return false
            }
        } catch (error: Throwable) {
            android.util.Log.w(TAG, "the shared file could not be copied", error)
            return false
        }

        // The sidecar is written only after the bytes land: a sidecar without bytes is
        // a row the SPA would show and then fail to upload, which is worse than no row.
        val sidecar = File(inbox, "$id.json")
        val mime = intent.type
        val json = buildString {
            append("{\"id\":").append(quote(id))
            append(",\"name\":").append(quote(name))
            append(",\"path\":").append(quote(bytes.absolutePath))
            append(",\"mime\":").append(mime?.let(::quote) ?: "null")
            append('}')
        }
        try {
            sidecar.writeText(json)
        } catch (error: Throwable) {
            bytes.delete()
            android.util.Log.w(TAG, "the sidecar could not be written", error)
            return false
        }

        android.util.Log.i(TAG, "shared file filed into the inbox: $id (${bytes.length()} bytes)")
        return true
    }

    /**
     * The name the *sending* app showed, which is what a user recognises. Read through
     * the resolver because a `content://` URI has no path to take a file name from; it
     * is metadata and is never joined onto the inbox path.
     */
    private fun displayName(context: Context, uri: Uri): String? {
        val cursor: Cursor = try {
            context.contentResolver.query(uri, null, null, null, null)
        } catch (error: Throwable) {
            android.util.Log.w(TAG, "the shared URI's name could not be queried", error)
            return null
        } ?: return uri.lastPathSegment?.substringAfterLast('/')

        return cursor.use {
            val index = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (index < 0 || !it.moveToFirst()) {
                uri.lastPathSegment?.substringAfterLast('/')
            } else {
                it.getString(index)
            }
        }
    }

    /** A JSON string literal. Hand-rolled because this file has no JSON dependency. */
    private fun quote(value: String): String {
        val out = StringBuilder(value.length + 2).append('"')
        for (character in value) {
            when (character) {
                '"' -> out.append("\\\"")
                '\\' -> out.append("\\\\")
                '\n' -> out.append("\\n")
                '\r' -> out.append("\\r")
                '\t' -> out.append("\\t")
                else -> if (character < ' ') {
                    out.append("\\u%04x".format(character.code))
                } else {
                    out.append(character)
                }
            }
        }
        return out.append('"').toString()
    }
}
