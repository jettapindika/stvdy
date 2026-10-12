import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { logSession } from '@/api/queries/study'
import { platform } from '@/platform'

/**
 * The focus timer.
 *
 * A Pomodoro-style timer whose only job is to log real minutes to
 * `study_sessions`, across a restart, **once**. Everything below follows from
 * those three words.
 *
 * **Across a restart.** The timer's running state is device-local — an unfinished
 * session is not a fact about the student's study, so it is not a synced row
 * (A206). It lives in `Platform.localState` under one key, and what it holds is
 * the timer's identity as well as its start instant.
 *
 * **Once.** The session id is minted at *start* and persisted with the timer, so
 * the flush after a restart is the **same** append-only id. The server's
 * `on conflict (id) do nothing` then makes a second flush of that id a no-op,
 * which is the mechanical defence against the double-count this file exists to
 * avoid: study minutes are the one number this app asks the student to trust, and
 * a fresh id per resume would pass every other test here while quietly doubling
 * it.
 *
 * **Real minutes.** The elapsed time is *derived* from `startedAt` and the
 * current clock on every tick, never accumulated, so a throttled background tab
 * cannot drift. And the number written is the elapsed time — never the configured
 * block length. A timer stopped after three minutes records three.
 *
 * The bound is A207's: a timer found running after four hours is not resumed, it
 * is put in front of the student to Save or Discard. Auto-resuming an overnight
 * timer would log a plausible-looking false session into the streak and the
 * Today counts, which are numbers the honesty gate says must trace to something
 * real.
 */

/** The one key the app writes to local state. The web adapter prefixes it. */
export const TIMER_KEY = 'focus_timer'

/** A timer running longer than this is not resumed: the screen offers Save or Discard. */
export const TIMER_STALE_AFTER_MIN = 240

/** The longest session the server accepts (`stvdy_core::study::check_session`). */
export const MAX_SESSION_MIN = 1440

/** The timer as it is persisted, which is the whole of what a restart resumes. */
export interface RunningTimer {
  /** Minted at start and persisted with the timer, so a restart resumes the same
   *  append-only session id and a double flush is a no-op (A206). */
  id: string
  startedAt: string
  taskId: string | null
  workspaceId: string | null
}

export type TimerState =
  | { kind: 'idle' }
  | { kind: 'running'; timer: RunningTimer; elapsedMin: number; elapsedSec: number }
  | { kind: 'stale'; timer: RunningTimer; elapsedMin: number; elapsedSec: number }

export interface FocusTimer {
  state: TimerState
  /** Starts a timer, optionally linked to the task being worked on. */
  start(options?: { taskId?: string | null; workspaceId?: string | null }): Promise<void>
  /** Logs the elapsed time and stops. Writes nothing if the timer is stale. */
  stop(): Promise<void>
  /** Stops without logging anything. */
  discard(): Promise<void>
  /** Logs `durationMin` for a stale timer the student decided to keep. */
  saveStale(durationMin: number): Promise<void>
}

/** Whole seconds since `startedAt`, floored, and never negative. */
export function elapsedSeconds(startedAt: string, now: Date): number {
  const ms = now.getTime() - Date.parse(startedAt)
  return ms <= 0 ? 0 : Math.floor(ms / 1000)
}

/**
 * The minutes a session may record.
 *
 * Rounded to the nearest minute, then held inside `1..=1440` — the server's own
 * bound, so a write is never refused for a length the app itself chose. The
 * floor of one is deliberate and disclosed: a stop at twenty seconds records one
 * minute, because `check_session` refuses a zero-length session and a timer that
 * answered "too short" would be a stop button that silently does nothing.
 */
export function sessionMinutes(minutes: number): number {
  return Math.min(Math.max(Math.round(minutes), 1), MAX_SESSION_MIN)
}

export function useFocusTimer(): FocusTimer {
  const [active, setActive] = useState<RunningTimer | null>(null)
  const [now, setNow] = useState(() => new Date())
  // One flush at a time. Two taps on Stop inside the same tick would otherwise
  // both read the marker before either cleared it and both enqueue the same
  // session: the server dedupes by id so the minutes stay right, but the outbox
  // would carry a mutation that can only ever be a duplicate. The ref is the
  // guard this device can make; the id is the guard the server makes.
  const flushing = useRef(false)

  // The persisted marker is the authority on mount, not React state: it is what
  // survived the restart. A read that finds nothing leaves the timer idle, which
  // is also what a stored value this build cannot parse resolves to.
  useEffect(() => {
    let live = true
    void platform.localState.get<RunningTimer>(TIMER_KEY).then((stored) => {
      if (live && stored !== null) setActive(stored)
    })
    return () => {
      live = false
    }
  }, [])

  // The tick exists to move the display, and it is derived from `startedAt`
  // rather than accumulated — every tick recomputes the elapsed time from the
  // start instant, so a throttled or suspended tab cannot drift.
  useEffect(() => {
    if (active === null) return
    const handle = setInterval(() => setNow(new Date()), 1000)
    return () => clearInterval(handle)
  }, [active])

  // `stale` is derived rather than stored, so the rule is the same whether the
  // timer was found running at mount or crossed the bound while it was on screen.
  const state: TimerState = useMemo(() => {
    if (active === null) return { kind: 'idle' }
    const elapsedSec = elapsedSeconds(active.startedAt, now)
    const elapsedMin = sessionMinutes(elapsedSec / 60)
    return elapsedSec > TIMER_STALE_AFTER_MIN * 60
      ? { kind: 'stale', timer: active, elapsedMin, elapsedSec }
      : { kind: 'running', timer: active, elapsedMin, elapsedSec }
  }, [active, now])

  const start = useCallback(
    async (options: { taskId?: string | null; workspaceId?: string | null } = {}) => {
      const timer: RunningTimer = {
        // Minted **at start** (A206). This is the line the double-count turns on:
        // minting here rather than at stop is what makes a resume flush the same
        // append-only id instead of a second session.
        id: crypto.randomUUID(),
        startedAt: new Date().toISOString(),
        taskId: options.taskId ?? null,
        workspaceId: options.workspaceId ?? null,
      }
      await platform.localState.set(TIMER_KEY, timer)
      setNow(new Date())
      setActive(timer)
    },
    [],
  )

  /**
   * Logs `durationMin` for `stored` and clears the marker.
   *
   * Write, then clear. The order matters: a crash in between leaves the marker in
   * place, so the next launch resumes the same id and the replay is a no-op —
   * whereas clearing first and crashing would lose the session with nothing on
   * screen to say so.
   */
  const commit = useCallback(async (stored: RunningTimer, durationMin: number) => {
    if (flushing.current) return
    flushing.current = true
    try {
      await logSession({
        id: stored.id,
        startedAt: stored.startedAt,
        durationMin,
        taskId: stored.taskId,
        workspaceId: stored.workspaceId,
      })
      await platform.localState.delete(TIMER_KEY)
      setActive(null)
    } finally {
      flushing.current = false
    }
  }, [])

  const stop = useCallback(async () => {
    // The marker is read rather than taken from `state`, because `state` is a
    // render-time snapshot: a second stop arriving before React has re-rendered
    // would otherwise flush the same timer twice. A marker that is already gone
    // means this timer has been logged, so there is nothing to do.
    const stored = await platform.localState.get<RunningTimer>(TIMER_KEY)
    if (stored === null) return

    const elapsedSec = elapsedSeconds(stored.startedAt, new Date())
    if (elapsedSec > TIMER_STALE_AFTER_MIN * 60) {
      // Not stopped silently. A timer past the bound goes through the screen's
      // Save or Discard (A207); logging four hours of "study" here would be the
      // false session the bound exists to prevent.
      setActive(stored)
      return
    }

    await commit(stored, sessionMinutes(elapsedSec / 60))
  }, [commit])

  const discard = useCallback(async () => {
    // Discarding writes nothing at all: it is the student saying this was not
    // study, and a "discarded" session in the minutes is exactly the invented
    // number the honesty gate forbids.
    await platform.localState.delete(TIMER_KEY)
    setActive(null)
  }, [])

  const saveStale = useCallback(
    async (durationMin: number) => {
      const stored = await platform.localState.get<RunningTimer>(TIMER_KEY)
      if (stored === null) return
      await commit(stored, sessionMinutes(durationMin))
    },
    [commit],
  )

  return { state, start, stop, discard, saveStale }
}
