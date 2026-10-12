import { renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  SEEN_KEY,
  markRemindersSeen,
  readSeenReminderIds,
  useLocalNotifications,
} from './useLocalNotifications'
import { installPlatform, platform, type PlatformKind } from '@/platform'
import { webPlatform } from '@/platform/web'
import { jsonResponse, mockFetch } from '@/test/harness'
import { setAccessToken } from '@/lib/tokens'

/**
 * The local schedule's wiring: the platform gate, the `seen` record, and the
 * cross-device suppression.
 *
 * Three assertions, and each is a defect class rather than a detail:
 *
 *  1. **On web and desktop the hook schedules nothing.** `Platform.notifications`
 *     says it: web and desktop cannot schedule and deliver immediately, so an
 *     ungated mount is a burst of notifications on every page load — one per
 *     upcoming task and event, fired at load time rather than at the time the
 *     reminder is about. Asserted on both non-mobile kinds.
 *  2. **A scheduled reminder is recorded**, so the next mount does not show it
 *     again. The record is the mechanism A285's fallback names.
 *  3. **A reminder the server already delivered for this device is not scheduled**,
 *     which is the cross-device half: the push and the local notification are the
 *     same reminder, and showing both is the double-notification the dedupe exists
 *     to prevent.
 */

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'pro',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

const SETTINGS_ROW = {
  study_start_hour: 8,
  study_end_hour: 22,
  max_study_min_per_day: 240,
  block_len_min: 50,
  break_min: 10,
  reminder_channels: ['web', 'push'],
  quiet_start_hour: null,
  quiet_end_hour: null,
  language: 'en',
  theme: 'system',
  updated_at: '2026-10-11T08:00:00Z',
}

const TASK = {
  id: 't1',
  workspace_id: null,
  parent_id: null,
  title: 'Selesaikan tugas kriptografi',
  notes: null,
  due_at: '2026-10-16T16:59:00Z',
  estimate_min: null,
  priority: 0,
  status: 'todo',
  position: 0,
  created_at: '2026-10-11T08:00:00Z',
  deleted_at: null,
}

interface Scheduled {
  title: string
  body: string
  at: Date
}

/**
 * A platform of the given kind whose `schedule` is recorded.
 *
 * Installed through `installPlatform` rather than `installFakePlatform`, because
 * the notification API is the one member under test and the fake platform's
 * options deliberately do not cover it. `runtime` stays `web` so the reads go
 * through the REST routes, which is what a mobile *webview* build would do for the
 * settings row it cannot cache.
 */
function install(
  kind: PlatformKind,
  reminders: { kind: string; subject_id: string | null; local_date: string }[] = [],
) {
  const scheduled: Scheduled[] = []
  const base = webPlatform(kind)
  installPlatform({
    ...base,
    // Inside `notifications`, not as a top-level key: `schedule` is a member of the
    // notification API, and a top-level `schedule` would be a second, unreachable
    // function that the hook never calls.
    notifications: {
      ...base.notifications,
      async schedule(title: string, body: string, at: Date) {
        scheduled.push({ title, body, at })
      },
    },
  })

  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/settings') {
      return jsonResponse(200, {
        settings: SETTINGS_ROW,
        timezone: USER.timezone,
        channels_available: ['web', 'push', 'email'],
      })
    }
    if (path === '/tasks') return jsonResponse(200, { tasks: [TASK] })
    if (path === '/events') return jsonResponse(200, { events: [] })
    if (path === '/reminders') return jsonResponse(200, { reminders })
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })

  return { scheduled }
}

function harness() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  // `createElement` rather than JSX so this stays a `.ts` file, which is the name
  // the plan gives it: the hook is a plain module and its test needs no markup.
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children)
  return { client, wrapper }
}

/** The instant every case plans from, so the ids and fire times are fixed. */
const NOW = new Date('2026-10-12T00:00:00Z')

beforeEach(() => {
  setAccessToken('access-1')
  // The web adapter's local state is `window.localStorage`, which outlives a test
  // in the same jsdom environment. The seen record is exactly the kind of state
  // that would leak: a case that schedules a reminder would leave the key behind
  // and make the next case's "this device has already shown it" assertion pass for
  // the wrong reason.
  window.localStorage.clear()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the platform gate', () => {
  it('schedules nothing on the web build, because web cannot schedule', async () => {
    const { scheduled } = install('web')
    const { client, wrapper } = harness()
    renderHook(() => useLocalNotifications(true, NOW), { wrapper })

    // The read resolved, so this is not vacuous: the hook ran and decided against
    // scheduling. On the web, `schedule` delivers *immediately*, so the ungated
    // version of this hook notifies on every page load.
    await waitFor(() => {
      expect(client.getQueryData(['tasks'])).toBeDefined()
    })
    expect(scheduled).toEqual([])
  })

  it('schedules nothing on the desktop build, for the same reason', async () => {
    // The desktop shell's own native notification is the shell's (A5/A32), so the
    // gate is the platform kind rather than a capability probe.
    const { scheduled } = install('macos')
    const { client, wrapper } = harness()
    renderHook(() => useLocalNotifications(true, NOW), { wrapper })
    await waitFor(() => {
      expect(client.getQueryData(['tasks'])).toBeDefined()
    })
    expect(scheduled).toEqual([])
  })
})

describe('on a phone', () => {
  it('schedules the task’s reminder and records the key it used', async () => {
    const { scheduled } = install('ios')
    const { client, wrapper } = harness()
    renderHook(() => useLocalNotifications(true, NOW), { wrapper })

    await waitFor(() => {
      expect(scheduled).toHaveLength(1)
    })

    // The key is the server's format, derived from the subject and the local date
    // and never from the computed instant (A284).
    expect(await readSeenReminderIds()).toEqual(['task_due:t1:2026-10-16'])
    // One hour before the due instant, which is `LEAD_TASK_MIN`.
    expect(scheduled[0]?.at.toISOString()).toBe('2026-10-16T15:59:00.000Z')
    // The notification names the subject: "a task is due" without saying which one
    // is not a reminder.
    expect(scheduled[0]?.body).toContain('Selesaikan tugas kriptografi')
    expect(client.getQueryData(['reminders', 'feed'])).toBeDefined()
  })

  it('does not schedule a key the server already delivered to this device', async () => {
    // The cross-device half (A285): the push carries `reminder_id`, and the feed
    // row for this device is the same reminder. Showing both is the double
    // notification the dedupe exists to prevent, so the feed's own key is unioned
    // into the seen list.
    const { scheduled } = install('ios', [
      { kind: 'task_due', subject_id: 't1', local_date: '2026-10-16' },
    ])
    const { client, wrapper } = harness()
    renderHook(() => useLocalNotifications(true, NOW), { wrapper })

    await waitFor(() => {
      expect(client.getQueryData(['reminders', 'feed'])).toBeDefined()
    })
    await waitFor(() => {
      expect(client.getQueryData(['tasks'])).toBeDefined()
    })
    expect(scheduled).toEqual([])
  })

  it('does not schedule a key this device already showed on an earlier mount', async () => {
    const { scheduled } = install('ios')
    await markRemindersSeen(['task_due:t1:2026-10-16'])
    const { client, wrapper } = harness()
    renderHook(() => useLocalNotifications(true, NOW), { wrapper })

    await waitFor(() => {
      expect(client.getQueryData(['tasks'])).toBeDefined()
    })
    expect(scheduled).toEqual([])
  })
})

describe('the seen record', () => {
  it('appends rather than replaces, so an earlier key is not forgotten', async () => {
    install('ios')
    expect(await readSeenReminderIds()).toEqual([])
    await markRemindersSeen(['task_due:t1:2026-10-16'])
    await markRemindersSeen(['event_soon:e1:2026-10-14'])
    // A device that replaced the list would re-notify about everything it had
    // already said.
    expect(await readSeenReminderIds()).toEqual([
      'task_due:t1:2026-10-16',
      'event_soon:e1:2026-10-14',
    ])
  })

  it('treats a stored value this build cannot read as an empty record', async () => {
    // The honest direction: "nothing recorded" shows the reminder, which is better
    // than suppressing one because a value failed to parse.
    install('ios')
    await markRemindersSeen(['task_due:t1:2026-10-16'])
    await platform.localState.set(SEEN_KEY, 'not an array')
    expect(await readSeenReminderIds()).toEqual([])
  })

  it('writes nothing for an empty list, so a mount with nothing due is a no-op', async () => {
    install('ios')
    await markRemindersSeen([])
    expect(await platform.localState.get(SEEN_KEY)).toBeNull()
  })
})
