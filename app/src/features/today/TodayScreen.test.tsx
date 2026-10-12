import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { fakeCache } from '@/test/syncStore'
import { TodayScreen } from './TodayScreen'

/**
 * The Today view, with the network down.
 *
 * This is the roadmap's offline half as a test: `fetch` fails for everything the
 * screen might reach for, the rows come from a local store, and the screen still
 * answers "what is due, what is on today, and what should I study now". A Today
 * view that quietly read a server route would fail here rather than on a train.
 *
 * The other two things asserted are the honesty rules: the number in "N due" is
 * the due-card count and not a phrase, and a capture draft is a draft — nothing is
 * written until the student confirms (gate 7).
 */

const NOW = new Date('2026-10-16T01:00:00Z') // 08:00 in Jakarta

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

const USAGE = {
  plan: 'pro',
  name: 'Pro',
  subscription: { status: 'active', period_end: '2026-11-11T00:00:00Z' },
  credits: { monthly: 500, purchased: 0, total: 500 },
  storage_bytes: 0,
  limits: {},
  features: {
    export_formats: ['md'],
    task_ai_breakdown: true,
    quick_capture: true,
    google_calendar: true,
    planner: 'manual',
    agent_actions: true,
    reminder_channels: ['web', 'push'],
    model_tier: 'cheapest',
    byok: false,
    advanced_analytics: false,
  },
  costs: {},
  provider_cost: { paused: false, resets_at: null },
}

/** A task due at 07:00+07:00 on the 16th: today in Jakarta, yesterday in UTC. */
const TASK = {
  id: 't1',
  workspace_id: null,
  parent_id: null,
  title: 'Kerjakan bab kriptografi',
  notes: null,
  due_at: '2026-10-16T07:00:00+07:00',
  estimate_min: 60,
  priority: 2,
  status: 'todo',
  position: 0,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
  deleted_at: null,
}

const EVENT = {
  id: 'e1',
  workspace_id: null,
  kind: 'class',
  title: 'Kuliah Kriptografi',
  starts_at: '2026-10-16T09:00:00+07:00',
  ends_at: '2026-10-16T11:00:00+07:00',
  source: 'app',
  task_id: null,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
}

const DUE_CARD = {
  id: 'c1',
  deck_id: 'd1',
  due_at: '2026-10-16T08:00:00+07:00',
  deleted_at: null,
}

/** A transport that is down, which is the condition the screen has to survive. */
function offlineFetch(allowed: (path: string) => Response | null) {
  return mockFetch((url) => {
    const path = new URL(url).pathname
    const answer = allowed(path)
    if (answer !== null) return answer
    throw new TypeError('Failed to fetch')
  })
}

beforeEach(() => {
  // `shouldAdvanceTime` matters: the clock is pinned so the local day is fixed,
  // but testing-library's `findBy` polls on timers and a frozen clock would make
  // every `await` in this file hang rather than fail.
  vi.useFakeTimers({ shouldAdvanceTime: true })
  vi.setSystemTime(NOW)
  window.localStorage.clear()
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('the Today view offline', () => {
  it('renders today from the local store while the network is down', async () => {
    const cache = fakeCache({
      task: [TASK],
      event: [EVENT],
      flashcard: [DUE_CARD],
      workspace: [{ id: 'w1', name: 'Kripto', updated_at: '2026-10-10T00:00:00Z', deleted_at: null }],
      source: [],
      deck: [],
      study_session: [],
      flashcard_review: [],
      quiz_attempt: [],
    })
    installFakePlatform('macos', {
      sync: {
        read: cache.read,
        // The shell's own report: this device has a queue it cannot deliver.
        status: async () => ({
          state: 'offline',
          code: null,
          lastSyncedAt: null,
          outbox: 0,
        }),
      },
    })
    // Only the two reads that need the network are served; everything else fails,
    // which is what proves the screen is not depending on them.
    offlineFetch((path) => {
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, USAGE)
      return null
    })

    renderWithProviders(<TodayScreen />, { route: '/' })

    // The task is on today's timeline, in Jakarta: a UTC bucketing would put it
    // on the 15th and show nothing here.
    expect(await screen.findByText('Kerjakan bab kriptografi')).toBeInTheDocument()
    expect(screen.getByText('Kuliah Kriptografi')).toBeInTheDocument()
    // The reason, in words, from the locale the app already carries.
    expect(screen.getByText(/You are offline/)).toBeInTheDocument()
    expect(
      screen.getByText(/Your tasks, events, and reviews still work/),
    ).toBeInTheDocument()
  })

  it('shows the due count it was given, as a number', async () => {
    const cache = fakeCache({
      task: [],
      event: [],
      flashcard: [DUE_CARD, { ...DUE_CARD, id: 'c2' }],
      workspace: [],
      source: [],
      deck: [],
      study_session: [],
      flashcard_review: [],
      quiz_attempt: [],
    })
    installFakePlatform('macos', { sync: { read: cache.read } })
    offlineFetch((path) => {
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, USAGE)
      return null
    })

    renderWithProviders(<TodayScreen />, { route: '/' })

    // The count is the number of cards in the queue, and it is rendered as that
    // number — not as "some cards are due", which would be a sentence pretending
    // to be a fact.
    expect(await screen.findByText('Review 2 cards')).toBeInTheDocument()
  })

  it('shows zero minutes and no streak when nothing was logged, rather than a placeholder', async () => {
    const cache = fakeCache({
      task: [],
      event: [],
      flashcard: [],
      workspace: [],
      source: [],
      deck: [],
      study_session: [],
      flashcard_review: [],
      quiz_attempt: [],
    })
    installFakePlatform('macos', { sync: { read: cache.read } })
    offlineFetch((path) => {
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, USAGE)
      return null
    })

    renderWithProviders(<TodayScreen />, { route: '/' })

    expect(await screen.findByText('0 minutes logged today')).toBeInTheDocument()
    expect(screen.getByText('0 day streak')).toBeInTheDocument()
  })
})

describe('quick capture', () => {
  /** The cache and the two served reads every capture test needs. */
  function setup(granted: boolean) {
    const cache = fakeCache({
      task: [],
      event: [],
      flashcard: [],
      workspace: [],
      source: [],
      deck: [],
      study_session: [],
      flashcard_review: [],
      quiz_attempt: [],
    })
    const fake = installFakePlatform('macos', { sync: { read: cache.read } })
    const calls = mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') {
        return jsonResponse(200, {
          ...USAGE,
          features: { ...USAGE.features, quick_capture: granted },
        })
      }
      if (path === '/tasks/quick-capture') {
        return jsonResponse(200, {
          draft: {
            title: 'Selesaikan tugas kripto',
            notes: null,
            due_at: '2026-10-16T16:59:00Z',
            priority: 2,
            estimate_min: 60,
            warnings: ['due_at: could not read "friday-ish"'],
          },
          credits_spent: 2,
          credits_remaining: 498,
          prompt_version: 'quick_capture.v1',
        })
      }
      return jsonResponse(404, { code: 'not_found', message: 'no' })
    })
    return { fake, calls }
  }

  it('shows the credit cost before the capture runs and saves nothing until confirmed', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    const { fake, calls } = setup(true)
    renderWithProviders(<TodayScreen />, { route: '/' })

    const field = await screen.findByLabelText('What do you have to do?')
    const runButton = screen.getByRole('button', { name: /Draft a task/ })
    // Gate 7(d): the price is on the control, before the action runs, and it is
    // the matrix's number rather than a literal.
    expect(runButton).toHaveTextContent('2 credits')

    await user.type(field, 'selesaikan tugas kripto hari Jumat')
    await user.click(runButton)

    // Gate 7(e): the draft is labelled a draft, and the model's warning about the
    // date is shown as text rather than swallowed (A194).
    expect(await screen.findByText('Suggested task')).toBeInTheDocument()
    expect(screen.getByText('Nothing is added until you confirm.')).toBeInTheDocument()
    expect(screen.getByText(/could not read/)).toBeInTheDocument()
    expect(screen.getByText('Selesaikan tugas kripto')).toBeInTheDocument()

    // The rule whose failure is invisible until a student finds a task they did
    // not create: the capture wrote no row at all.
    expect(fake.sync.writes).toEqual([])
    expect(calls.filter((call) => call.init.method === 'POST')).toHaveLength(1)

    await user.click(screen.getByRole('button', { name: 'Confirm' }))

    // Exactly one write, with the draft's own fields and an id minted at confirm.
    expect(fake.sync.writes).toHaveLength(1)
    const written = fake.sync.writes[0]
    expect(written?.entity).toBe('task')
    expect(written?.payload.title).toBe('Selesaikan tugas kripto')
    expect(written?.payload.due_at).toBe('2026-10-16T16:59:00Z')
    expect(written?.payload.estimate_min).toBe(60)
    expect(written?.payload.priority).toBe(2)
    // The draft is gone once it is a task, so the same capture cannot be saved twice.
    expect(screen.queryByText('Suggested task')).toBeNull()
  })

  it('dismissing a draft writes nothing', async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    const { fake } = setup(true)
    renderWithProviders(<TodayScreen />, { route: '/' })

    const field = await screen.findByLabelText('What do you have to do?')
    await user.type(field, 'selesaikan tugas kripto hari Jumat')
    await user.click(screen.getByRole('button', { name: /Draft a task/ }))
    await screen.findByText('Suggested task')

    await user.click(screen.getByRole('button', { name: 'Dismiss' }))

    expect(fake.sync.writes).toEqual([])
    expect(screen.queryByText('Suggested task')).toBeNull()
  })

  it('locks quick capture on a plan that does not grant it', async () => {
    const { fake } = setup(false)
    renderWithProviders(<TodayScreen />, { route: '/' })

    // The locked card, with the plan that has it, and no capture field at all: a
    // button that would 403 is not offered.
    expect(await screen.findByText('Available on Pro')).toBeInTheDocument()
    expect(screen.queryByLabelText('What do you have to do?')).toBeNull()
    expect(fake.sync.writes).toEqual([])
  })

  it('names the locked feature from the matrix rather than from a literal', async () => {
    // The same assertion, from the other side: the plan named is the one
    // `config/plans.toml` grants `quick_capture` on. If the grant moved, this is
    // the test that would move with it.
    setup(false)
    renderWithProviders(<TodayScreen />, { route: '/' })
    const locked = await screen.findByText('Available on Pro')
    expect(within(locked.closest('.card') ?? document.body).getByText('Quick capture')).toBeInTheDocument()
  })
})
