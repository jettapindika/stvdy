import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { DevicesScreen } from '@/features/devices/DevicesScreen'
import { formatInZone } from '@/lib/time'
import { installFakePlatform, jsonResponse, mockFetch, type RecordedCall } from '@/test/harness'
import { renderWithProviders } from '@/test/render'

/**
 * The Devices screen: what it lists, what it can do, and what it must never say.
 *
 * The three cases the plan names are the floor, and the fourth is roadmap gate 7f:
 * this screen is the one place a device count is discussed, so it is the place an
 * upgrade prompt would appear if one crept in. On iOS and Android that is a store
 * violation rather than a copy preference, so the assertion is a search of the
 * whole rendered screen for a price, a plan link, or the word "upgrade".
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

/** This device: it has pulled, so its `last_sync_at` is a real instant. */
const LAPTOP = {
  id: 'd1',
  name: "Jetta's MacBook",
  platform: 'macos',
  app_version: '0.1.0',
  last_seen_at: '2026-10-11T09:00:00Z',
  last_sync_at: '2026-10-11T08:00:00Z',
  created_at: '2026-10-01T08:00:00Z',
  revoked_at: null,
  current: true,
}

/**
 * Another device that signed in and never pulled. Its `last_seen_at` is recent —
 * which is exactly the trap: rendering *that* as "last synced" would claim a sync
 * that never happened (A180).
 */
const PHONE = {
  id: 'd2',
  name: 'Pixel 8',
  platform: 'android',
  app_version: null,
  last_seen_at: '2026-10-11T09:30:00Z',
  last_sync_at: null,
  created_at: '2026-10-02T08:00:00Z',
  revoked_at: null,
  current: false,
}

const notFound = () =>
  jsonResponse(404, {
    code: 'not_found',
    message: 'no such route',
    min_plan: null,
    limit: null,
    used: null,
  })

/**
 * Scripts the three routes this screen touches, and remembers whether the phone
 * has been signed out so the refetch answers the list the server would.
 */
function scriptRoutes(devices: unknown[] = [LAPTOP, PHONE]): RecordedCall[] {
  let revoked = false
  return mockFetch((url, init) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/devices' && (init.method ?? 'GET') === 'GET') {
      return jsonResponse(200, { devices: revoked ? [LAPTOP] : devices })
    }
    if (path === '/devices/d2' && init.method === 'DELETE') {
      revoked = true
      return jsonResponse(204)
    }
    return notFound()
  })
}

beforeEach(() => {
  installFakePlatform('macos')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the devices screen', () => {
  it('lists devices with their platform and last-synced time', async () => {
    scriptRoutes()

    renderWithProviders(<DevicesScreen />)

    expect(await screen.findByText("Jetta's MacBook")).toBeInTheDocument()
    expect(screen.getByText('Pixel 8')).toBeInTheDocument()
    expect(screen.getByText('macOS')).toBeInTheDocument()
    expect(screen.getByText('Android')).toBeInTheDocument()

    // "Last synced" is the stored `last_sync_at`, rendered in the user's own zone.
    // The design splits the label from the instant — the label in the reading face,
    // the instant in mono, because the instant is the evidence — so the assertion is
    // on the row's two halves rather than on one concatenated string. The label's
    // own words ("Last synced") are a **new key**, which renders as its own key text
    // until the locale files carry it, so they are asserted nowhere; the split
    // itself is asserted by class. Scoped to the laptop's row: both rows carry a
    // label, and an unscoped lookup would pass on either one.
    const expected = formatInZone(new Date(LAPTOP.last_sync_at), USER.timezone, {
      dateStyle: 'medium',
      timeStyle: 'short',
    })
    const laptop = screen.getByText("Jetta's MacBook").closest('li')
    if (!laptop) throw new Error('the laptop must have its own row')
    const inLaptop = within(laptop)
    expect(laptop.querySelector('.text-label')).not.toBeNull()
    expect(inLaptop.getByText(expected).className).toContain('meta')

    // A device that has never pulled says so. It must not borrow `last_seen_at`,
    // which would read as "synced just now" for a device that never synced.
    expect(screen.getByText('Not synced yet')).toBeInTheDocument()
    const seen = formatInZone(new Date(PHONE.last_seen_at), USER.timezone, {
      dateStyle: 'medium',
      timeStyle: 'short',
    })
    expect(screen.queryByText(new RegExp(seen))).not.toBeInTheDocument()

    // Which row is this device is the server's answer, not a guess from the name.
    expect(screen.getByText('This device')).toBeInTheDocument()
  })

  it('revokes a device after confirmation and removes it from the list', async () => {
    const calls = scriptRoutes()
    const user = userEvent.setup()

    renderWithProviders(<DevicesScreen />)
    await screen.findByText('Pixel 8')

    const row = screen.getByText('Pixel 8').closest('li')
    expect(row).not.toBeNull()
    const inRow = within(row as HTMLElement)

    // Nothing is sent until the confirmation is answered: a one-click revoke of a
    // device the user is not holding is how a laptop gets signed out by accident.
    // The row's control is found by its tone rather than by its label: the design
    // words it "Sign out device", which is a new key, and until the locale files
    // carry it `t()` renders the key itself — so a name-based lookup here would
    // assert on the merge rather than on the behaviour. `.btn-danger` is the row's
    // one destructive control, and the button inside the confirmation keeps the bare
    // verb ("Sign out"), which is what the second click below asserts.
    const signOut = inRow
      .getAllByRole('button')
      .find((button) => button.classList.contains('btn-danger'))
    if (!signOut) throw new Error('the device row must offer a sign-out control')
    await user.click(signOut)
    expect(calls.some((call) => call.init.method === 'DELETE')).toBe(false)
    expect(
      inRow.getByText("Sign out Pixel 8? Its local copy is deleted the next time it connects."),
    ).toBeInTheDocument()

    await user.click(inRow.getByRole('button', { name: 'Sign out' }))

    expect(
      calls.filter((call) => call.url.endsWith('/devices/d2') && call.init.method === 'DELETE'),
    ).toHaveLength(1)
    // The list is the server's answer after the revoke, not a row deleted locally.
    expect(await screen.findByText("Jetta's MacBook")).toBeInTheDocument()
    expect(screen.queryByText('Pixel 8')).not.toBeInTheDocument()
  })

  it('offers "Reset local data and resync" and says what it deletes', async () => {
    const { sync } = installFakePlatform('macos')
    scriptRoutes()
    const user = userEvent.setup()

    renderWithProviders(<DevicesScreen />)
    await screen.findByText("Jetta's MacBook")

    // The sentence names the half a student would not guess: the local copy *and*
    // the changes it has not sent yet. A reset that silently dropped the outbox
    // would destroy work, so the copy has to say so before the button is pressed.
    expect(screen.getByText(/has not sent yet/)).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Reset and resync' }))
    expect(sync.resets).toBe(0)
    expect(screen.getByText(/anything not yet sent/)).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Delete and resync' }))

    // The shell's wipe, not a screen-level cache clear: only it can delete the
    // database and the outbox together (A186).
    expect(sync.resets).toBe(1)
    expect(await screen.findByText('Local data was deleted and resynced.')).toBeInTheDocument()
  })

  it('offers no price and no upgrade anywhere (gate 7f)', async () => {
    scriptRoutes()

    const { container } = renderWithProviders(<DevicesScreen />)
    await screen.findByText("Jetta's MacBook")

    // A device over the plan's `devices_max` is over it; the remedy is to sign one
    // out. The plan screen is where a plan is discussed, and on a phone build a
    // route to a purchase is a store violation rather than a copy preference.
    expect(screen.queryByRole('link', { name: /plan/i })).not.toBeInTheDocument()
    expect(screen.queryByRole('link', { name: /upgrade/i })).not.toBeInTheDocument()
    expect(container.textContent ?? '').not.toMatch(/upgrade/i)
    expect(container.textContent ?? '').not.toMatch(/(Rp|IDR|\$|USD)\s?\d/)
  })
})
