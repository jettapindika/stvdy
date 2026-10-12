import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { readRefreshToken, setAccessToken } from '@/lib/tokens'

import { installFakePlatform, jsonResponse, mockFetch, type RecordedCall } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { fakeCache } from '@/test/syncStore'
import { SettingsScreen } from './SettingsScreen'

/**
 * The data section: export, and account deletion (A355).
 *
 * Four properties, each of which a plausible implementation gets wrong:
 *
 *  * **The consequences are stated before the control that acts on them.** The EULA (§ 5)
 *    promises the forfeiture of remaining prepaid time is said before confirmation, and a
 *    screen that only mentions it afterwards has broken the promise rather than the layout.
 *  * **The deletion is behind a type-to-confirm.** One tap is the wrong default for an
 *    irreversible action on a phone, so the final button must be unreachable until the
 *    student types their own address.
 *  * **The request is a DELETE to `/account`, not a client-side state change.** A screen that
 *    "deleted" the account by clearing the local session would leave the account alive on the
 *    server and the student believing otherwise.
 *  * **The export asks the server for each format**, because JSON, Markdown and ICS are three
 *    documents rather than one with a flag.
 */

const USER = {
  id: 'u1',
  email: 'rani@example.com',
  email_verified: true,
  email_verified_at: '2026-10-01T00:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-01T00:00:00Z',
}

const ROW = {
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

const USAGE = {
  plan: 'free',
  name: 'Free',
  subscription: { status: 'active', period_end: '2026-11-11T00:00:00Z' },
  credits: { monthly: 60, purchased: 0, total: 60 },
  storage_bytes: 0,
  limits: {},
  features: {
    export_formats: ['md'],
    task_ai_breakdown: false,
    quick_capture: false,
    google_calendar: false,
    planner: 'manual',
    agent_actions: false,
    reminder_channels: ['web', 'push'],
    model_tier: 'cheapest',
    byok: false,
    advanced_analytics: false,
  },
  costs: {},
  provider_cost: { paused: false, resets_at: null },
}

function setup() {
  const cache = fakeCache({ user_settings: [{ id: 'u1', ...ROW }] })
  const fake = installFakePlatform('macos', { sync: { read: cache.read } })
  const calls = mockFetch((url) => {
    const parsed = new URL(url)
    if (parsed.pathname === '/auth/me') return jsonResponse(200, USER)
    if (parsed.pathname === '/usage') return jsonResponse(200, USAGE)
    if (parsed.pathname === '/account' && parsed.search === '') {
      return jsonResponse(200, {
        deleted_at: '2026-10-12T00:00:00Z',
        purges_at: '2026-10-19T00:00:00Z',
        forfeited_period_end: null,
        sessions_revoked: 2,
        jobs_cancelled: 1,
        already_deleted: false,
      })
    }
    if (parsed.pathname === '/account/export') {
      // A response carrying bytes rather than JSON, which is what the export answers.
      return {
        ok: true,
        status: 200,
        blob: async () => new Blob(['{"account":{}}'], { type: 'application/json' }),
      } as Response
    }
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })
  return { fake, calls }
}

beforeEach(() => {
  setAccessToken('access-1')
  // Assigned onto the real `URL` rather than stubbed as a whole object: the test's own
  // `new URL(...)` calls and the client's both need the constructor, and a spread of a
  // class copies neither its statics nor its prototype. Typed to the DOM's own signatures
  // rather than left as `vi.fn()`'s generic mock, so the assignment is checkable.
  URL.createObjectURL = vi.fn<(obj: Blob | MediaSource) => string>(() => 'blob:fake')
  URL.revokeObjectURL = vi.fn<(obj: string) => void>()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the data section', () => {
  it('offers all three documents the spec names, each as its own request', async () => {
    const user = userEvent.setup()
    const { calls } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    // JSON, Markdown and ICS are three documents, not one with a flag: the spec names
    // them because they answer different questions, and offering only one would be
    // choosing for the student.
    expect(await screen.findByRole('button', { name: 'Export JSON' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Export Markdown' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Export calendar (ICS)' })).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Export calendar (ICS)' }))

    const request = await vi.waitFor(() => {
      const found = calls.find((call) => new URL(call.url).pathname === '/account/export')
      expect(found).toBeDefined()
      return found as RecordedCall
    })
    expect(new URL(request.url).searchParams.get('format')).toBe('ics')
  })

  it('states the consequences before the control that acts on them', async () => {
    setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    // The EULA § 5 promise: the forfeiture is said *before* confirmation. The assertion
    // is on the copy being present before the confirm field is opened, because a screen
    // that only mentioned it afterwards would have broken the promise.
    expect(
      await screen.findByText(/Any remaining prepaid time is forfeited\./),
    ).toBeInTheDocument()
    expect(
      screen.getByText(/signs you out on every device immediately, cancels anything still running/),
    ).toBeInTheDocument()
    expect(screen.queryByLabelText(/Type rani@example.com to confirm/)).toBeNull()
  })

  it('requires the student to type their own address before the deletion is reachable', async () => {
    const user = userEvent.setup()
    const { calls } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    const start = await screen.findByRole('button', { name: 'Delete my account' })
    await user.click(start)

    const confirm = screen.getByRole('button', { name: 'Delete my account permanently' })
    // One tap is the wrong default for an irreversible action on a phone: the button is
    // unreachable until the address matches.
    expect(confirm).toBeDisabled()

    const field = screen.getByLabelText(/Type rani@example.com to confirm/)
    await user.type(field, 'rani@exampl')
    expect(confirm).toBeDisabled()
    expect(calls.find((call) => new URL(call.url).pathname === '/account')).toBeUndefined()

    await user.type(field, 'e.com')
    expect(confirm).toBeEnabled()

    await user.click(confirm)
    const request = await vi.waitFor(() => {
      const found = calls.find((call) => new URL(call.url).pathname === '/account')
      expect(found).toBeDefined()
      return found as RecordedCall
    })
    // The request is a DELETE to the server's route, not a local sign-out wearing the
    // word "delete": an account cleared only on this device is still alive.
    expect(request.init.method).toBe('DELETE')
  })

  it('cancels the confirmation and forgets what was typed', async () => {
    const user = userEvent.setup()
    const { calls } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    await user.click(await screen.findByRole('button', { name: 'Delete my account' }))
    await user.type(screen.getByLabelText(/Type rani@example.com to confirm/), 'rani@example.com')
    await user.click(screen.getByRole('button', { name: 'Cancel' }))

    // Back to the first step, with the field gone and nothing sent.
    expect(screen.getByRole('button', { name: 'Delete my account' })).toBeInTheDocument()
    expect(screen.queryByLabelText(/Type rani@example.com to confirm/)).toBeNull()
    expect(calls.find((call) => new URL(call.url).pathname === '/account')).toBeUndefined()

    // Re-opening it starts empty: a stale value that already matched would make the
    // second confirmation a single tap.
    await user.click(screen.getByRole('button', { name: 'Delete my account' }))
    expect(screen.getByLabelText(/Type rani@example.com to confirm/)).toHaveValue('')
  })

  it('signs this device out, because the server already did', async () => {
    const user = userEvent.setup()
    setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    await user.click(await screen.findByRole('button', { name: 'Delete my account' }))
    await user.type(
      screen.getByLabelText(/Type rani@example.com to confirm/),
      'rani@example.com',
    )
    await user.click(screen.getByRole('button', { name: 'Delete my account permanently' }))

    // The request revokes every session family including this one, so the very next request
    // from here would be a 401. The local session is cleared on success so the app moves to
    // `signed-out` deliberately — asserted through the token store, which is the fact
    // `useSession` reads, rather than through a screen the router has already left.
    await vi.waitFor(async () => {
      expect(await readRefreshToken()).toBeNull()
    })
  })

  it('does not offer the export as a plan feature', async () => {
    // `ExportFormat` gates the *report* and *cheatsheet* documents; the right to take your
    // own data out is on every plan, and the free plan's `export_formats: ['md']` must not
    // reach this section. A locked export button would be a plan decision the spec does not
    // have, made on the screen where a student is trying to leave.
    setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    const section = (await screen.findByRole('heading', { name: 'Your data' })).closest('section')
    expect(section).not.toBeNull()
    const buttons = within(section as HTMLElement).getAllByRole('button')
    for (const button of buttons) {
      expect(button).toBeEnabled()
    }
    expect(within(section as HTMLElement).queryByText(/Available on/)).toBeNull()
  })
})
