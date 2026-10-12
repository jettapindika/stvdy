package app.stvdy.mobile

/**
 * The scheduler's constants, in one place.
 *
 * They are also written into Rust (`src/scheduler.rs`) and into the iOS half
 * (`ios/Sources/Scheduler.swift`), and a mismatch between the three is a job that
 * never fires and never says so — so `tests/scheduler.rs` asserts the names are
 * spelled the same in all of them rather than trusting three copies to agree.
 */
object StvdySyncWork {
    /** The `WorkManager` unique work name; `app.stvdy.sync` in `scheduler.rs`. */
    const val NAME = "app.stvdy.sync"

    /** `WorkManager`'s floor. Below it, `enqueueUniquePeriodicWork` throws. */
    const val MIN_INTERVAL_MINUTES = 15L
}
