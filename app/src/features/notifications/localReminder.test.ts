import { renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import en from '@/i18n/locales/en.json'
import id from '@/i18n/locales/id.json'
import i18n from '@/i18n'
import { readSeenReminderIds, useLocalNotifications } from '@/features/reminders/useLocalNotifications'
import { installPlatform, type PlatformKind } from '@/platform'
import { webPlatform } from '@/platform/web'
import { jsonResponse, mockFetch } from '@/test/harness'
import { setAccessToken } from '@/lib/tokens'

/**
 * The SPA's local-reminder scheduling path, and the one promise it must not make
 * (A318, A319).
 *
 * The device half of Task 7 — a token surviving a restart in the Keystore, a
 * scheduled notification firing while the app is closed — needs a physical device and,
 * for iOS, a signing identity. None exists here, so it is recorded as unverified in
 * the plan rather than asserted here. What *is* assertable on this machine is the half
 * the device would be testing the platform's half of:
 *
 *  1. **The instant reaches the platform as a `Date`**, which is `NotificationsApi`'s
 *     signature and the only thing the adapters can turn into a scheduled delivery.
 *     A string or a number here would be a reminder delivered immediately on one
 *     adapter and at the wrong time on another.
 *  2. **A refused schedule is not recorded as shown.** `schedule` is best-effort by
 *     contract; the failure that matters is the one that is silently marked as
 *     delivered, because the reminder then never comes back.
 *  3. **The copy claims no exact delivery, in either language.** A318's whole
 *     position is that inexact delivery is acceptable — Android falls back to
 *     `setAndAllowWhileIdle` and iOS scheduling is discretionary — so a string that
 *     promised the minute would be a promise the platform does not keep.
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
  title: 'Drink water',
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
 * A phone whose `schedule` is recorded, and which can be made to refuse.
 *
 * Installed through `installPlatform` rather than `installFakePlatform`, because the
 * notification API is the member under test and the fake platform's options
 * deliberately do not cover it. `runtime` stays `web` so the reads go through the REST
 * routes, which is what a mobile *webview* build does for the settings row it cannot
 * cache.
 */
function install(kind: PlatformKind, refuse = false) {
  const scheduled: Scheduled[] = []
  const base = webPlatform(kind)
  installPlatform({
    ...base,
    notifications: {
      ...base.notifications,
      async schedule(title: string, body: string, at: Date) {
        if (refuse) throw new Error('notification permission was refused')
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
    if (path === '/reminders') return jsonResponse(200, { reminders: [] })
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })

  return { scheduled }
}

function harness() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children)
  return { client, wrapper }
}

/** The instant every case plans from, so the fire time is a fixed value. */
const NOW = new Date('2026-10-12T00:00:00Z')

/**
 * Words that promise delivery at a named instant.
 *
 * `tepat` is the Indonesian "exactly" and is the word a translator would reach for;
 * `menit` is deliberately **not** here, because the shipped body says "in 60 minutes",
 * which is a lead time rather than a claim about when the OS delivers.
 */
const EXACTNESS = /exact|precisely|on the minute|\bsharp\b|tepat|persis/i

beforeEach(() => {
  setAccessToken('access-1')
  // The seen record is `localStorage` in this build and outlives a test in the same
  // jsdom environment, so a case that schedules a reminder would make the next case's
  // assertion pass for the wrong reason.
  window.localStorage.clear()
})

afterEach(async () => {
  vi.unstubAllGlobals()
  await i18n.changeLanguage('en')
})

describe('scheduling a local reminder', () => {
  it('hands the platform a Date, at the instant the plan computed', async () => {
    const { scheduled } = install('ios')
    const { wrapper } = harness()

    renderHook(() => useLocalNotifications(true, NOW), { wrapper })

    await waitFor(() => {
      expect(scheduled).toHaveLength(1)
    })
    // The member's signature is `(title, body, at: Date)`; the mobile adapter turns
    // this into `{ at: at.toISOString(), allowWhileIdle: true }` and the web adapter
    // ignores it and delivers now. A string here would satisfy neither.
    expect(scheduled[0]?.at).toBeInstanceOf(Date)
    // One hour before the due instant, which is `LEAD_TASK_MIN` — the same margin the
    // server's own plan uses, so the two are not two schedules for one subject.
    expect(scheduled[0]?.at.toISOString()).toBe('2026-10-16T15:59:00.000Z')
    expect(scheduled[0]?.body).toContain('Drink water')
    // The positive half of the case below: a reminder that *did* schedule is recorded
    // under the server's own dedupe key, so the refusal case is asserting a real
    // difference rather than an empty record either way.
    expect(await readSeenReminderIds()).toEqual(['task_due:t1:2026-10-16'])
  })

  it('does not record a reminder the platform refused to schedule', async () => {
    // Best-effort, and the direction of the failure is the point: a schedule that
    // threw is not a reminder that fired, so recording it as shown would suppress the
    // reminder for ever. The next mount tries again.
    install('ios', true)
    const { client, wrapper } = harness()

    renderHook(() => useLocalNotifications(true, NOW), { wrapper })

    // The read resolved, so this is not vacuous: the hook ran, planned the reminder
    // and called `schedule`.
    await waitFor(() => {
      expect(client.getQueryData(['tasks'])).toBeDefined()
    })
    expect(await readSeenReminderIds()).toEqual([])
  })

  it('promises no exact delivery, in either language', async () => {
    // The copy first, as the shipped resource: every string the reminder can show,
    // in both languages. This is the assertion the plan asks for, and it is made over
    // the strings rather than over a rendered screen because the same strings are what
    // the notification shows — there is no screen.
    for (const [language, resource] of [
      ['en', en.reminders],
      ['id', id.reminders],
    ] as const) {
      for (const [key, value] of Object.entries(resource.local)) {
        expect(value, `${language}: reminders.local.${key}`).not.toMatch(EXACTNESS)
      }
    }

    // And what was actually scheduled, in each language, so a string that reached the
    // platform through a path the resource check cannot see is covered too.
    for (const language of ['en', 'id'] as const) {
      await i18n.changeLanguage(language)
      window.localStorage.clear()
      const { scheduled } = install('ios')
      const { wrapper } = harness()

      renderHook(() => useLocalNotifications(true, NOW), { wrapper })
      await waitFor(() => {
        expect(scheduled).toHaveLength(1)
      })

      expect(scheduled[0]?.title, language).not.toMatch(EXACTNESS)
      expect(scheduled[0]?.body, language).not.toMatch(EXACTNESS)
    }
  })
})
