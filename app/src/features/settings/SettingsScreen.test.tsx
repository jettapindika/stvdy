import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { setAccessToken } from '@/lib/tokens'

import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { fakeCache } from '@/test/syncStore'
import { SettingsScreen } from './SettingsScreen'

/**
 * Settings: the window that wraps midnight, the channel the plan does not grant,
 * and the sections another phase owns.
 *
 * The quiet-hours case is the one a plausible implementation gets wrong: `22 → 7`
 * is the normal window, and a reader that tested `start < end` would call it
 * invalid. The channel case is the honesty rule — a locked channel is shown with
 * the plan that has it and cannot be chosen, rather than being offered and refused
 * with a 403 after the fact.
 */

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

/** The row's shape, with the two quiet ends nullable as the column is. */
interface Row {
  study_start_hour: number
  study_end_hour: number
  max_study_min_per_day: number
  block_len_min: number
  break_min: number
  reminder_channels: string[]
  quiet_start_hour: number | null
  quiet_end_hour: number | null
  language: string
  theme: string
  updated_at: string
}

const ROW: Row = {
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

function setup(row: Row = ROW) {
  const cache = fakeCache({ user_settings: [{ id: 'u1', ...row }] })
  const fake = installFakePlatform('macos', { sync: { read: cache.read } })
  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/usage') return jsonResponse(200, USAGE)
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })
  return { fake }
}

beforeEach(() => {
  // A session is what makes `useSession` read `/auth/me` rather than answer
  // `signed-out`: without an access token the bootstrap tries a refresh and gives
  // up, and a screen rendered for no account has no user id to save under (A200).
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the settings form', () => {
  it('says which window a wrapping pair means, rather than calling it invalid', async () => {
    // 22 → 7 is the normal case, not an error: the reader tests membership, never
    // `start < end` (A202), and the form says which window it means rather than
    // leaving the student to work out whether the pair is allowed.
    setup({ ...ROW, quiet_start_hour: 22, quiet_end_hour: 7 })
    renderWithProviders(<SettingsScreen />, { route: '/settings' })
    expect(await screen.findByText('Quiet from 22:00 to 7:00 the next day.')).toBeInTheDocument()
  })

  it('saves the wrapping window as a pair', async () => {
    const user = userEvent.setup()
    const { fake } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    const from = await screen.findByLabelText('From')
    await user.clear(from)
    await user.type(from, '22')
    const to = screen.getByLabelText('To')
    await user.clear(to)
    await user.type(to, '7')

    const last = fake.sync.writes.at(-1)
    expect(last?.entity).toBe('user_settings')
    // A200: the row's primary key *is* the user id, so the mutation's id is that.
    expect(last?.id).toBe('u1')
    // Both ends are always written: the migration's own check constraint refuses
    // a window with one end, so a partial pair would be a write the server rejects.
    expect(last?.payload.quiet_start_hour).toBe(22)
    expect(last?.payload.quiet_end_hour).toBe(7)
  })

  it('names a same-day window as the same day', async () => {
    setup({ ...ROW, quiet_start_hour: 13, quiet_end_hour: 15 })
    renderWithProviders(<SettingsScreen />, { route: '/settings' })
    expect(await screen.findByText('Quiet from 13:00 to 15:00.')).toBeInTheDocument()
  })

  it('locks a channel the plan does not grant and names the plan that does', async () => {
    const user = userEvent.setup()
    const { fake } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    const telegram = await screen.findByLabelText('Telegram')
    // The plan that first has it, read from `config/plans.toml`: free has web and
    // push, and telegram arrives on Power. Two channels are locked on this plan,
    // so the assertion is scoped to the row rather than to the whole screen.
    const row = telegram.closest('div')
    expect(row).not.toBeNull()
    expect(within(row as HTMLElement).getByText('Available on Power')).toBeInTheDocument()
    expect(telegram).toBeDisabled()

    // Choosing it is impossible rather than a 403 after the fact.
    await user.click(telegram)
    expect(telegram).not.toBeChecked()
    expect(fake.sync.writes).toEqual([])
  })

  it('offers the local reset and calls it', async () => {
    const user = userEvent.setup()
    const { fake } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    const reset = await screen.findByRole('button', { name: 'Reset local data and resync' })
    // The copy says what it deletes, because a control that silently removes
    // queued work is one a student cannot consent to.
    expect(
      screen.getByText(/deletes everything cached on this device, including changes you have not synced yet/),
    ).toBeInTheDocument()

    await user.click(reset)
    expect(fake.sync.resets).toBe(1)
  })

  it('names the sections other phases own rather than omitting them', async () => {
    setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    // Each names the phase that brings it, so a student looking for a control
    // finds the reason it is not there instead of nothing at all.
    expect(await screen.findByText('Calendar connections')).toBeInTheDocument()
    expect(screen.getByText('Google Calendar and ICS arrive in Phase 10.')).toBeInTheDocument()
    expect(screen.getByText('Data export and deletion')).toBeInTheDocument()
    expect(screen.getByText('Bring your own model key')).toBeInTheDocument()
  })

  it('links to the plan screen, which is a phone build\'s only route to it', async () => {
    // The credit meter lives in the sidebar, which a phone does not render, and
    // both `LockedFeature` and the plan-limit notice carry no link on iOS and
    // Android (A143). So this link is what makes Phase 14's read-only plan screen
    // reachable on a phone at all — without it the screen exists and no one can
    // open it. The link is on every platform, because the destination is the plan
    // and not a purchase.
    setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    expect(await screen.findByRole('link', { name: 'Open plan and usage' })).toHaveAttribute(
      'href',
      '/billing',
    )
  })

  it('saves a language choice to the synced row, so the other device uses it', async () => {
    const user = userEvent.setup()
    const { fake } = setup()
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    const language = await screen.findByLabelText('Interface and generated text')
    await user.selectOptions(language, 'id')

    const last = fake.sync.writes.at(-1)
    expect(last?.payload.language).toBe('id')
  })
})
