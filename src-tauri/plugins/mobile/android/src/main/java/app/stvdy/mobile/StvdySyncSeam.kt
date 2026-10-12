package app.stvdy.mobile

/**
 * The Android worker's one hop into Rust (A315) — **and the place where it stops.**
 *
 * ## What the iOS half can do that this one cannot
 *
 * The iOS seam reaches Rust through `PluginManager`'s command bridge: Tauri's Swift
 * API lets a plugin build an `Invoke` by hand and dispatch it
 * (`ios/Sources/Scheduler.swift`). **Tauri's Android API is not symmetrical.** The
 * only Kotlin→Rust direction the generated code provides is
 * `PluginManager.handlePluginResponse` — a `private external fun` that exists to
 * *answer* a command Rust started — and the only JNI symbols
 * `tauri::android_binding!` exports are the activity lifecycle callbacks on
 * `MainActivity` (`onCreate`, `onResume`, `onNewIntent`, …). A `WorkManager` worker
 * has no activity and no in-flight command, so neither is reachable.
 *
 * A headless entry therefore has to be a new exported symbol:
 *
 * ```rust
 * #[unsafe(no_mangle)]
 * pub extern "C" fn Java_app_stvdy_mobile_StvdySyncSeam_backgroundSync(…) { … }
 * ```
 *
 * **and that is forbidden in this workspace, by measurement.** The lint block in
 * `src-tauri/Cargo.toml` — repeated verbatim in this crate's manifest — sets
 * `unsafe_code = "forbid"`, and rustc rejects the attribute under it:
 *
 * ```text
 * error: declaration of a `no_mangle` function
 *   = note: requested on the command line with `-F unsafe-code`
 * ```
 *
 * The lint is not weakened for this. That is the whole content of this file: it
 * names the missing symbol, and reports the deferral rather than pretending.
 *
 * ## What happens on a device meanwhile
 *
 * The `WorkManager` job still runs and still wakes the app's process. The pass it
 * requests is the one the app runs on its next foreground, through the same
 * `sync::sync_once` the iOS handler calls — which is A316's own position: a
 * background window is opportunistic, and the foreground sync is what makes a device
 * correct. The *schedule* is real; the *pass* is deferred. That is a degraded and
 * honest outcome, and the log line below is what makes it visible instead of silent.
 *
 * ## Why the symbol is not declared here
 *
 * An `external fun` with no Rust export to bind to is an `UnsatisfiedLinkError`
 * waiting for the first call, and a declaration nothing can satisfy reads like a
 * working seam. The symbol name lives in [MISSING_SYMBOL] as a string, where it
 * documents the requirement without lying about it.
 */
object StvdySyncSeam {

    /**
     * The symbol a build must export before Android background sync runs. Named in
     * full — `Java_` + package with dots as underscores + class + method — so the
     * missing half is unambiguous to whoever adds it.
     */
    const val MISSING_SYMBOL: String =
        "Java_app_stvdy_mobile_StvdySyncSeam_backgroundSync"

    private const val TAG = "StvdySyncSeam"

    /**
     * Whether this build has a headless Rust entry point for a background pass.
     *
     * `false`, and a constant rather than a runtime probe: the answer is a property
     * of the build, and the change that adds the symbol is the change that flips
     * this. A probe would only move the same fact somewhere less obvious.
     */
    const val AVAILABLE: Boolean = false

    /**
     * One background pass, or the honest report that there is none.
     *
     * Never throws. A worker that throws is retried by `WorkManager`, and retrying a
     * call that can never succeed is a battery drain dressed up as persistence.
     */
    fun backgroundSync(window: String): SyncOutcome {
        if (!AVAILABLE) {
            android.util.Log.i(
                TAG,
                "background sync ($window): no Rust entry point in this build " +
                    "($MISSING_SYMBOL); deferred to the next foreground sync",
            )
            return SyncOutcome.UNAVAILABLE
        }
        // Unreachable while `AVAILABLE` is false. Left as the shape the working
        // version takes, so the change that adds the symbol has one place to fill in
        // rather than a file to reconstruct.
        android.util.Log.i(TAG, "background sync ($window) requested")
        return SyncOutcome.UNAVAILABLE
    }
}

/** What a requested background pass did. */
enum class SyncOutcome {
    /** The pass ran. */
    DONE,

    /** The pass failed; the caller may retry. */
    FAILED,

    /**
     * No Rust entry point exists in this build, so the pass was deferred to the next
     * foreground sync. Deliberately not a retry: nothing changes until the symbol
     * exists, and `WorkManager` retrying for ever would be a battery drain.
     */
    UNAVAILABLE,
}
