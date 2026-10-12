import { screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SyncStatus } from '@/features/sync/SyncStatus'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'

/**
 * The honesty rules of the status control, at the level the user reads them
 * (roadmap gate 7, A187).
 *
 * These are the cases the plan names, and each one is a real defect if it fails.
 * A green "Synced" over a pending write is the single most misleading thing a sync
 * UI can say: the student stops worrying about the write that is still sitting in
 * the outbox, closes the laptop, and loses it. So the assertion is not "the
 * component renders the report" — it is that a report claiming `synced` with a
 * non-empty outbox still cannot produce the word "Synced".
 *
 * The render goes through the query client and the router because the control is a
 * link (it is the only route to `/devices`), and through `installFakePlatform`
 * because the status arrives from the shell rather than from an HTTP route.
 */

const STORED = '2026-10-11T08:00:00Z'

/** A 404 for anything a screen asks for that this test does not script. */
const notFound = () =>
  jsonResponse(404, {
    code: 'not_found',
    message: 'no such route',
    min_plan: null,
    limit: null,
    used: null,
  })

beforeEach(() => {
  mockFetch(() => notFound())
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('the sync status control', () => {
  it('never says Synced while the outbox has a pending write', async () => {
    const { sync } = installFakePlatform('macos')
    // The shell claiming `synced` with three queued writes is exactly the report
    // this rule exists for: it is what an older build, or a bug in the status
    // derivation, would send.
    sync.set({ state: 'synced', code: null, lastSyncedAt: STORED, outbox: 3 })

    renderWithProviders(<SyncStatus />)

    const word = await screen.findByRole('status')
    expect(word).toHaveTextContent('Syncing')
    expect(word).not.toHaveTextContent('Synced')
    // And the count is on screen, so "Syncing" is not a word with nothing behind
    // it: the student can see how much is waiting.
    expect(await screen.findByText('3 changes waiting to sync')).toBeInTheDocument()
  })

  it('shows the server code for a rejected mutation and marks the row not saved', async () => {
    const { sync } = installFakePlatform('macos')
    sync.set({ state: 'needs_attention', code: 'read_only_entity', lastSyncedAt: STORED, outbox: 0 })

    renderWithProviders(<SyncStatus />)

    expect(await screen.findByRole('status')).toHaveTextContent('Needs attention')
    // The server's own code, not a paraphrase of it: "read_only_entity" is the
    // stable half of the refusal and the half a support request can carry.
    const refusal = await screen.findByText(/Not saved/)
    expect(refusal).toHaveTextContent('read_only_entity')
  })

  it('renders Last synced from the stored timestamp, never from a fresh clock read', async () => {
    // The clock is moved well past the stored instant. A control that read the
    // clock at render time would show the *current* date here; the only way this
    // assertion passes is by rendering what the shell stored.
    vi.useFakeTimers({ shouldAdvanceTime: true })
    vi.setSystemTime(new Date('2026-12-01T23:30:00Z'))

    const { sync } = installFakePlatform('macos')
    sync.set({ state: 'synced', code: null, lastSyncedAt: STORED, outbox: 0 })

    renderWithProviders(<SyncStatus />)

    const expected = formatInZone(new Date(STORED), DEFAULT_TIMEZONE, {
      dateStyle: 'medium',
      timeStyle: 'short',
    })
    expect(await screen.findByText(`Last synced ${expected}`)).toBeInTheDocument()
    expect(screen.queryByText(/1 Dec 2026/)).not.toBeInTheDocument()
  })

  it('is reachable and operable by keyboard alone, with a visible focus ring', async () => {
    const { sync } = installFakePlatform('macos')
    sync.set({ state: 'synced', code: null, lastSyncedAt: STORED, outbox: 0 })

    renderWithProviders(<SyncStatus />)

    // A link, so `Tab` reaches it and `Enter` activates it — and its destination is
    // the Devices screen, which is how Devices is reached at all (A189).
    const control = await screen.findByRole('link', { name: /Devices and sync/ })
    expect(control).toHaveAttribute('href', '/devices')
    expect(control.className).toContain('min-h-11')
    // jsdom does not process Tailwind, so the ring is asserted as the declaration
    // it is made of — the same approach the tab bar's overflow check takes.
    expect(control.className).toContain('focus:outline-ring')

    control.focus()
    expect(control).toHaveFocus()
  })

  it('never claims a state it could not read', async () => {
    installFakePlatform('macos', {
      sync: {
        async status() {
          throw new Error('the shell did not answer')
        },
      },
    })

    renderWithProviders(<SyncStatus />)

    // An unreadable status is its own sentence. Rendering "Synced" here would be
    // the app asserting work it never verified, and rendering nothing would leave
    // a student with no idea whether their notes are safe.
    expect(await screen.findByRole('status')).toHaveTextContent('Sync status unavailable')
    expect(screen.queryByText('Synced')).not.toBeInTheDocument()
  })

  it('says the web build keeps nothing on the device, rather than naming a time', async () => {
    // The online-only platform (A14): there is no local database and no outbox, so
    // there is no pass that could have produced a timestamp. The control says that
    // instead of inventing one.
    installFakePlatform('web')

    renderWithProviders(<SyncStatus />)

    expect(await screen.findByRole('status')).toHaveTextContent('Synced')
    expect(await screen.findByText('This build keeps nothing on this device')).toBeInTheDocument()
    expect(screen.queryByText(/Last synced/)).not.toBeInTheDocument()
  })
})
