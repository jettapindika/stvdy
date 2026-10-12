import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { platform } from '@/platform'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import {
  MAX_SESSION_MIN,
  TIMER_KEY,
  TIMER_STALE_AFTER_MIN,
  sessionMinutes,
  useFocusTimer,
  type RunningTimer,
  type TimerState,
} from './useFocusTimer'

/**
 * The focus timer, and the one number it must not get wrong.
 *
 * A session is the student's study minutes, and the timer is the only thing in
 * the app that measures them. Two failures matter more than any other here, and
 * both look like success:
 *
 *   - **a resume that mints a new id**, which logs the same sitting twice;
 *   - **a duration taken from the configured block length**, which records fifty
 *     minutes for a three-minute stop.
 *
 * The first is closed by minting the id at start and persisting it with the
 * timer, so the flush after a restart is the same append-only id (A206). Every
 * test below is written against that: the remount case asserts the *id*, not the
 * count, because a fresh id per resume would pass a count-based assertion.
 */

const START = new Date('2026-10-12T09:00:00Z')

/** The persisted marker, as the app reads it back. */
function marker(): Promise<RunningTimer | null> {
  return platform.localState.get<RunningTimer>(TIMER_KEY)
}

/**
 * The timer's measured state, with the idle case refused.
 *
 * A union read twice would let a test assert `elapsedMin` on a state that has no
 * such field; this makes the missing case a failure rather than a silently
 * undefined comparison.
 */
function measured(state: TimerState): { kind: 'running' | 'stale'; elapsedMin: number } {
  if (state.kind === 'idle') throw new Error('expected a running or stale timer')
  return state
}

/** Mounts the hook and lets its mount-time read of the marker settle. */
async function mountTimer() {
  const view = renderHook(() => useFocusTimer())
  await act(async () => {})
  return view
}

beforeEach(() => {
  window.localStorage.clear()
  vi.useFakeTimers()
  vi.setSystemTime(START)
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('the focus timer', () => {
  it('resumes the same session id after a remount', async () => {
    const { sync } = installFakePlatform('macos')

    const first = await mountTimer()
    await act(async () => {
      await first.result.current.start({ taskId: 't1', workspaceId: 'w1' })
    })
    const started = await marker()
    expect(started).not.toBeNull()

    // The app is killed and comes back twenty minutes later. The marker is all
    // that survives, and the id inside it is the session's identity.
    vi.setSystemTime(new Date('2026-10-12T09:20:00Z'))
    first.unmount()

    const second = await mountTimer()
    expect(second.result.current.state.kind).toBe('running')
    await act(async () => {
      await second.result.current.stop()
    })

    expect(sync.writes).toHaveLength(1)
    // The assertion this file exists for: the id written after the restart is the
    // id the first mount minted. A resume that minted a fresh one would pass a
    // "one write" assertion and log the sitting twice on the server.
    expect(sync.writes[0]?.id).toBe(started?.id)
    expect(sync.writes[0]?.entity).toBe('study_session')
    expect(sync.writes[0]?.payload.duration_min).toBe(20)
    expect(sync.writes[0]?.payload.task_id).toBe('t1')
    expect(sync.writes[0]?.payload.workspace_id).toBe('w1')
    // The start instant is the first mount's, not the resume's: the twenty
    // minutes spent away are study, and the four hours spent asleep are not.
    expect(sync.writes[0]?.payload.started_at).toBe(START.toISOString())
    expect(await marker()).toBeNull()
  })

  it('stopping writes one session with the elapsed minutes', async () => {
    const { sync } = installFakePlatform('macos')
    const { result } = await mountTimer()

    await act(async () => {
      await result.current.start()
    })
    vi.setSystemTime(new Date('2026-10-12T09:12:00Z'))
    await act(async () => {
      vi.advanceTimersByTime(1000)
    })
    expect(measured(result.current.state).elapsedMin).toBe(12)

    await act(async () => {
      await result.current.stop()
    })

    expect(sync.writes).toHaveLength(1)
    expect(sync.writes[0]?.payload.duration_min).toBe(12)
    expect(result.current.state.kind).toBe('idle')
    expect(await marker()).toBeNull()
  })

  it('a double stop writes once', async () => {
    const { sync } = installFakePlatform('macos')
    const { result } = await mountTimer()

    await act(async () => {
      await result.current.start()
    })
    vi.setSystemTime(new Date('2026-10-12T09:12:00Z'))

    // Two taps in the same tick, which is the way a student actually causes this:
    // both handlers run before either flush has cleared the marker.
    await act(async () => {
      await Promise.all([result.current.stop(), result.current.stop()])
    })

    expect(sync.writes).toHaveLength(1)
    // And the second stop of the same timer, after the first completed, is a
    // no-op for the same reason: there is no marker left to flush.
    await act(async () => {
      await result.current.stop()
    })
    expect(sync.writes).toHaveLength(1)
  })

  it('gives each session its own id', async () => {
    const { sync } = installFakePlatform('macos')
    const { result } = await mountTimer()

    for (const minute of [12, 13]) {
      await act(async () => {
        await result.current.start()
      })
      vi.setSystemTime(new Date(`2026-10-12T09:${minute}:00Z`))
      await act(async () => {
        await result.current.stop()
      })
    }

    expect(sync.writes).toHaveLength(2)
    expect(sync.writes[0]?.id).not.toBe(sync.writes[1]?.id)
  })

  it('a timer older than four hours is stale rather than resumed', async () => {
    const { sync } = installFakePlatform('macos')
    // Five hours: an overnight timer the student never stopped.
    await platform.localState.set(TIMER_KEY, {
      id: 'overnight',
      startedAt: new Date(START.getTime() - 5 * 60 * 60 * 1000).toISOString(),
      taskId: null,
      workspaceId: null,
    })

    const { result } = await mountTimer()

    expect(result.current.state.kind).toBe('stale')
    expect(measured(result.current.state).elapsedMin).toBe(300)
    // And stopping it does not log it: auto-resuming an overnight timer would put
    // a plausible-looking five hours into the streak and the Today counts (A207).
    await act(async () => {
      await result.current.stop()
    })
    expect(sync.writes).toHaveLength(0)
    expect(await marker()).not.toBeNull()
  })

  it('becomes stale while it is running, rather than stopping silently', async () => {
    const { sync } = installFakePlatform('macos')
    const { result } = await mountTimer()

    await act(async () => {
      await result.current.start()
    })
    vi.setSystemTime(new Date(START.getTime() + (TIMER_STALE_AFTER_MIN + 1) * 60 * 1000))
    await act(async () => {
      vi.advanceTimersByTime(1000)
    })

    expect(result.current.state.kind).toBe('stale')
    await act(async () => {
      await result.current.stop()
    })
    expect(sync.writes).toHaveLength(0)
  })

  it('discarding a stale timer writes no session', async () => {
    const { sync } = installFakePlatform('macos')
    await platform.localState.set(TIMER_KEY, {
      id: 'overnight',
      startedAt: new Date(START.getTime() - 5 * 60 * 60 * 1000).toISOString(),
      taskId: null,
      workspaceId: null,
    })

    const { result } = await mountTimer()
    await act(async () => {
      await result.current.discard()
    })

    expect(sync.writes).toHaveLength(0)
    expect(await marker()).toBeNull()
    expect(result.current.state.kind).toBe('idle')
  })

  it('saves a stale timer with the minutes the student left', async () => {
    const { sync } = installFakePlatform('macos')
    await platform.localState.set(TIMER_KEY, {
      id: 'overnight',
      startedAt: new Date(START.getTime() - 5 * 60 * 60 * 1000).toISOString(),
      taskId: null,
      workspaceId: null,
    })

    const { result } = await mountTimer()
    await act(async () => {
      await result.current.saveStale(45)
    })

    // The edited number, not the measured one: the student is the one who knows
    // how much of that window was study.
    expect(sync.writes).toHaveLength(1)
    expect(sync.writes[0]?.payload.duration_min).toBe(45)
    expect(sync.writes[0]?.id).toBe('overnight')
  })

  it('records the elapsed time, never the configured block length', async () => {
    // The configured block length is 50 minutes in the defaults, and the failure
    // this pins is a timer that reports it instead of what elapsed: a student who
    // stops after three minutes must not be told they studied fifty.
    const { sync } = installFakePlatform('macos')
    const { result } = await mountTimer()

    await act(async () => {
      await result.current.start()
    })
    vi.setSystemTime(new Date('2026-10-12T09:03:00Z'))
    await act(async () => {
      await result.current.stop()
    })

    expect(sync.writes[0]?.payload.duration_min).toBe(3)
  })

  it('records one minute for a stop under a minute rather than zero', async () => {
    // `check_session` refuses a zero-length session, so a floor of zero would be
    // a Stop button that silently does nothing. The rounding is disclosed rather
    // than hidden: one minute is the minute the student studied in.
    const { sync } = installFakePlatform('macos')
    const { result } = await mountTimer()

    await act(async () => {
      await result.current.start()
    })
    vi.setSystemTime(new Date('2026-10-12T09:00:20Z'))
    await act(async () => {
      await result.current.stop()
    })

    expect(sync.writes[0]?.payload.duration_min).toBe(1)
  })

  it('posts the session to the REST route on a build with no cache', async () => {
    // A14/A183: the web build has no outbox, so `sync.write` refuses. The timer
    // must reach the same route with the same body rather than failing at stop.
    const { sync } = installFakePlatform('web')
    const calls = mockFetch(() => jsonResponse(201, { applied: true }))
    const { result } = await mountTimer()

    await act(async () => {
      await result.current.start({ taskId: 't1' })
    })
    vi.setSystemTime(new Date('2026-10-12T09:12:00Z'))
    await act(async () => {
      await result.current.stop()
    })

    expect(sync.writes).toHaveLength(0)
    expect(calls).toHaveLength(1)
    expect(new URL(calls[0]?.url ?? '').pathname).toBe('/sessions')
    expect(calls[0]?.init.method).toBe('POST')
    const body = JSON.parse(String(calls[0]?.init.body)) as Record<string, unknown>
    expect(body.duration_min).toBe(12)
    expect(body.task_id).toBe('t1')
    // `client_ts` is the field the server's bound reads (A204), and it is on the
    // body here because this route takes it from the body. On the push path the
    // mutation carries it instead, which is why the payload above omits it.
    expect(typeof body.client_ts).toBe('string')
  })
})

describe('the session length', () => {
  it('holds the server\u2019s bound, so a write is never refused for a length the app chose', () => {
    expect(sessionMinutes(12)).toBe(12)
    expect(sessionMinutes(0)).toBe(1)
    expect(sessionMinutes(-3)).toBe(1)
    expect(sessionMinutes(0.4)).toBe(1)
    expect(sessionMinutes(12.6)).toBe(13)
    expect(sessionMinutes(MAX_SESSION_MIN + 100)).toBe(MAX_SESSION_MIN)
  })
})
