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

function setup(row: Row = ROW, available: string[] = []) {
  const cache = fakeCache({ user_settings: [{ id: 'u1', ...row }] })
  const fake = installFakePlatform('macos', { sync: { read: cache.read } })
  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/usage') return jsonResponse(200, USAGE)
    // `channels_available` is a **second** question from the plan's grant (A288):
    // what this deployment can deliver on. It defaults to empty here, which is what
    // the 404 fallback gives every other case in this file, and a case that needs a
    // channel to be choosable asks for it explicitly.
    if (path === '/settings') return jsonResponse(200, { channels_available: available })
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
    expect(await screen.findByText('Bring your own model key')).toBeInTheDocument()

    // And the list shrinks when a section arrives, which is the half that is easy
    // to forget: "Google Calendar and ICS arrive in Phase 10" was true until
    // Phase 10 shipped the calendar screen and `/calendar` stopped being a
    // `NotBuiltRoute`. Copy that outlives its phase is worse than no copy, because
    // it tells a student a working feature does not exist. Data export and deletion
    // was the same case until A355 shipped it: it is now a section of its own, so
    // the line that said it was not built must be gone rather than left standing.
    expect(screen.queryByText('Data export and deletion')).toBeNull()
    expect(screen.queryByText(/arrive in Phase 15/)).toBeNull()
    expect(screen.queryByText('Calendar connections')).toBeNull()
    expect(screen.queryByText(/arrive in Phase 10/)).toBeNull()
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

/**
 * The channel label as a 44px target, asserted as a source property.
 *
 * jsdom has no layout engine, so `getBoundingClientRect()` in this suite is all
 * zeros and no box can be measured here. What can be asserted is the class that
 * makes the box — the technique `components/layout/shellLayout.test.ts` uses for
 * the same reason — and **the numbers are the browser measurements**, taken in
 * Chromium at 390x844 in both themes.
 *
 * The defect, measured: the label was **65.9x22** (Phase 14 — a thumb aimed at the
 * channel name and landing a few pixels high hit the row `<div>` and toggled
 * nothing), so `min-h-11` was added; with the height fixed, `input#channel-email`
 * was still **35.3x44** at 100% text, because "Email" is a short word and the
 * label is only as wide as its own text. Both directions are asserted on **every**
 * label rather than the email row: the next short name has the same defect, and a
 * per-row fix is one that gets forgotten. The checkbox stays 13x13 — the label is
 * what a finger reaches.
 *
 * Measured after, 390px, both themes: Email **44x44** (100%) and 88x88 (200%),
 * Discord 50.9x44, Telegram 61.6x44, "In the app" 65.9x44, "Push notifications"
 * 118.1x44 — every label at least 44px in both directions.
 */
describe('the channel rows', () => {
  it('gives every channel label a 44px box in both directions, not just the short one', async () => {
    // `setup()` rather than relying on the platform a previous test installed. Without it
    // this test only passed when a sibling had run first and left its fake platform behind,
    // which is a test that passes for a reason unrelated to what it asserts.
    setup()
    const { container } = renderWithProviders(<SettingsScreen />, { route: '/settings' })
    await screen.findByLabelText('Email')

    const labels = [...container.querySelectorAll('label[for^="channel-"]')]
    const boxes = [...container.querySelectorAll('input[id^="channel-"]')]
    expect(labels).toHaveLength(boxes.length)
    expect(labels.length).toBeGreaterThanOrEqual(5)
    for (const label of labels) {
      expect(label.className, `${label.textContent ?? ''} must be a 44px target`).toMatch(
        /\bmin-h-11\b/,
      )
      expect(label.className, `${label.textContent ?? ''} must be a 44px target`).toMatch(
        /\bmin-w-11\b/,
      )
    }
  })
})

/**
 * A `/settings` response from a server that predates `reminder_channels`.
 *
 * **The version skew is a supported deployment, not a hypothetical.** The shell and
 * the server are versioned independently and released separately (A353) — a server
 * hotfix ships alone, and the desktop updater is the only thing that ever moves a
 * client — so a client newer than the server it talks to is normal. And the sibling
 * field on this same screen is already guarded for the same reason:
 * `channels_available` is read as `?? []` because a server without it must degrade
 * the notification section rather than the screen.
 *
 * The defect, observed against a stale server during Phase 12's gate 6 run:
 * `row.reminder_channels.includes(...)` threw `TypeError: Cannot read properties of
 * undefined (reading 'includes')` inside the render, and React Router's error
 * boundary caught it — **the whole Settings screen rendered nothing**, from one
 * absent metadata field. An absent list means "no channel is on", which is what the
 * server's own default would have said.
 */
describe('a settings row from an older server', () => {
  it('renders with no channel selected rather than taking the screen down', async () => {
    // The cast is the point: `SettingsRow` declares the field required because the
    // current server always sends it, and the case under test is precisely a server
    // that does not.
    const older = { ...ROW, reminder_channels: undefined } as unknown as Row
    setup(older)

    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    // The screen rendered its own content rather than the error boundary's.
    const inApp = await screen.findByLabelText('In the app')
    expect(inApp).not.toBeChecked()
    expect(screen.getByLabelText('Push notifications')).not.toBeChecked()
    // And the section the rows live in is present, which is what "the screen
    // rendered" means for a screen whose failure mode was rendering nothing at all.
    expect(screen.getByText('How Stvdy reaches you')).toBeInTheDocument()
  })

  it('writes the channel the student picks onto the absent list', async () => {
    const user = userEvent.setup()
    const older = { ...ROW, reminder_channels: undefined } as unknown as Row
    const { fake } = setup(older, ['web', 'push'])
    renderWithProviders(<SettingsScreen />, { route: '/settings' })

    // `web` rather than `email`: the free plan grants only web and push, and a
    // channel the plan does not grant is disabled by design (A288), so clicking it
    // would assert nothing about this defect.
    await user.click(await screen.findByLabelText('In the app'))

    const last = fake.sync.writes.at(-1)
    expect(last?.payload.reminder_channels).toEqual(['web'])
  })
})
