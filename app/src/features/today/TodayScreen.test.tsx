import { screen, within } from '@testing-library/react'
import userEvent, { type UserEvent } from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { fakeCache } from '@/test/syncStore'
import { setAccessToken } from '@/lib/tokens'
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
  // The access token lives in module memory, so it survives a test: without this
  // reset a test that signs in would leave every later test signed in too.
  setAccessToken(null)
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

  it('renders the streak empty state when nothing was logged, never a zero', async () => {
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

    // No streak is not a streak of zero: the card states the cause and where to
    // start. "0 day streak" would be the app using a number to say "you have
    // nothing", and a student who has not studied yet has lost nothing.
    expect(await screen.findByText('No study days yet')).toBeInTheDocument()
    expect(
      screen.getByText(/A day counts when you review a card, take a quiz, or log a session/),
    ).toBeInTheDocument()
    expect(screen.queryByText('0 day streak')).toBeNull()
  })

  it('keeps every control on screen while offline, and says what still works', async () => {
    // The offline state is a screen, not a hidden one. The band names the cause
    // and the two things that still work, and the review button — the one action
    // that genuinely works with no signal — stays exactly where it is. A screen
    // that hid its controls when the network dropped would take away the student's
    // only offline move.
    const cache = fakeCache({
      task: [TASK],
      event: [EVENT],
      flashcard: [DUE_CARD, { ...DUE_CARD, id: 'c2' }],
      workspace: [
        { id: 'w1', name: 'Biologi Sel', course_code: 'BIO 201', color: 0, deleted_at: null },
      ],
      source: [],
      deck: [{ id: 'd1', workspace_id: 'w1', title: 'Membran', deleted_at: null }],
      study_session: [],
      flashcard_review: [],
      quiz_attempt: [],
    })
    installFakePlatform('macos', {
      sync: {
        read: cache.read,
        status: async () => ({ state: 'offline', code: null, lastSyncedAt: null, outbox: 2 }),
      },
    })
    offlineFetch((path) => {
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, USAGE)
      return null
    })

    renderWithProviders(<TodayScreen />, { route: '/' })

    expect(await screen.findByText('Review 2 cards')).toBeInTheDocument()
    expect(screen.getByText(/You are offline/)).toBeInTheDocument()
    // The capture field is still rendered, and the one thing that needs the
    // network says so under its own control rather than being removed.
    expect(screen.getByLabelText('What do you have to do?')).toBeInTheDocument()
    expect(screen.getByText('AI actions need a connection.')).toBeInTheDocument()
    // The footer states where the day is saved, which is the fact an offline
    // student needs: on this device, waiting.
    expect(screen.getByText(/Saved on this device/)).toBeInTheDocument()
  })

  it('places the current time on the timeline and labels a deadline in calendar days', async () => {
    const DEADLINE = {
      ...EVENT,
      id: 'e2',
      kind: 'deadline',
      title: 'Laporan praktikum',
      // 17:00 on the 18th in Jakarta: two calendar days after the 16th.
      starts_at: '2026-10-18T17:00:00+07:00',
      ends_at: '2026-10-18T17:00:00+07:00',
      workspace_id: 'w1',
    }
    const cache = fakeCache({
      task: [],
      event: [EVENT, DEADLINE],
      flashcard: [],
      workspace: [
        { id: 'w1', name: 'Biologi Sel', course_code: 'BIO 201', color: 0, deleted_at: null },
      ],
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

    // The class starts at 09:00 and the view was opened at 08:00, so the indigo
    // rule is drawn above it — a rule pinned to the top of the list would be a
    // clock, not a position on the day.
    expect(await screen.findByText('Now')).toBeInTheDocument()
    expect(screen.getByText('Kuliah Kriptografi')).toBeInTheDocument()

    // The deadline is tagged with the course the student gave it, and its label is
    // a count of calendar days rather than of elapsed hours.
    expect(screen.getByText('Laporan praktikum')).toBeInTheDocument()
    expect(screen.getByText('BIO 201')).toBeInTheDocument()
    expect(screen.getByText('In 2 days')).toBeInTheDocument()
    // The due line is the student's own day and time, never the raw UTC instant.
    expect(screen.getByText(/Sun 18 Oct/)).toBeInTheDocument()
  })
})

describe('quick capture', () => {
  /**
   * The cache and the two served reads every capture test needs.
   *
   * `overrides` exists for the cases where the answer is the point rather than the
   * scaffolding: the student's zone decides which day the due line names, and the
   * draft's instant decides it too, so a test that pins the pair has to say both.
   */
  function setup(granted: boolean, overrides: { timezone?: string; dueAt?: string } = {}) {
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
      if (path === '/auth/me') {
        return jsonResponse(200, { ...USER, timezone: overrides.timezone ?? USER.timezone })
      }
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
            due_at: overrides.dueAt ?? '2026-10-16T16:59:00Z',
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

  /** Types a line, runs the capture, and waits for the draft card. */
  async function openDraft(user: UserEvent) {
    const field = await screen.findByLabelText('What do you have to do?')
    await user.type(field, 'selesaikan tugas kripto hari Jumat')
    await user.click(screen.getByRole('button', { name: /Draft a task/ }))
    await screen.findByText('Suggested task')
    return field
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

  /**
   * The draft card's `Due` line names a day, not an instant.
   *
   * `2026-10-15T23:00Z` is **06:00 on the 16th in Jakarta** and 23:00 on the 15th
   * in UTC, so the pair below is what a formatter that ignores the zone cannot
   * pass: the raw instant renders the 15th, and every other date on this screen
   * renders the student's own day. Gate 6 exists for exactly this deadline — the
   * one a UTC implementation puts a day early.
   */
  describe('the due line is the student\u2019s day', () => {
    const DUE_AT = '2026-10-15T23:00:00Z'

    it('renders the 16th for a student in Jakarta', async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
      setup(true, { timezone: 'Asia/Jakarta', dueAt: DUE_AT })
      // Signed in, because the zone this asserts comes from the session's own
      // `timezone` and not from the default: an anonymous render would pass this
      // test whatever the student's zone said.
      setAccessToken('access-1')
      renderWithProviders(<TodayScreen />, { route: '/' })

      await openDraft(user)

      expect(screen.getByText('Due 16 Oct 2026')).toBeInTheDocument()
      // Not the instant, which is the same string a reader would have had to
      // parse before: the UTC day is the 15th.
      expect(screen.queryByText(/2026-10-15T23:00/)).toBeNull()
    })

    it('renders the 15th for the same instant in UTC', async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
      setup(true, { timezone: 'UTC', dueAt: DUE_AT })
      setAccessToken('access-1')
      renderWithProviders(<TodayScreen />, { route: '/' })

      await openDraft(user)

      // The zone decides, not the instant: this is the other half of the pair.
      expect(screen.getByText('Due 15 Oct 2026')).toBeInTheDocument()
    })
  })

  describe('dismissing the draft from the keyboard', () => {
    it('closes the card on Escape while the caret is in the composer', async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
      const { fake } = setup(true)
      renderWithProviders(<TodayScreen />, { route: '/' })

      const field = await openDraft(user)
      // The caret is in the composer, which is *outside* the card: a keydown
      // handler on the card's container would never see this one, which is why
      // the listener is on the window.
      await user.click(field)

      await user.keyboard('{Escape}')

      expect(screen.queryByText('Suggested task')).toBeNull()
      expect(document.activeElement).toBe(field)
      expect(fake.sync.writes).toEqual([])
    })

    it('returns the caret to the composer when Escape arrives from a card control', async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
      setup(true)
      renderWithProviders(<TodayScreen />, { route: '/' })

      const field = await openDraft(user)
      // Focus on the card's own button: this is the measurement the gate took.
      // Losing focus is half the defect — without the focus move the caret ends
      // at `document.body` and the next Tab starts at the top of the document.
      screen.getByRole('button', { name: 'Dismiss' }).focus()
      expect(document.activeElement).not.toBe(field)

      await user.keyboard('{Escape}')

      expect(screen.queryByText('Suggested task')).toBeNull()
      expect(document.activeElement).toBe(field)
    })

    it('leaves the composer alone on Escape when there is no draft', async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
      setup(true)
      renderWithProviders(<TodayScreen />, { route: '/' })

      const field = await screen.findByLabelText('What do you have to do?')
      await user.click(field)
      await user.keyboard('{Escape}')

      // The listener only exists while there is a draft, so a composer with
      // nothing to refuse keeps its caret.
      expect(document.activeElement).toBe(field)
      expect(screen.queryByText('Suggested task')).toBeNull()
    })
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
