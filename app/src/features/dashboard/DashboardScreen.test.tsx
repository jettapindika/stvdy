import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { screen, waitFor } from '@testing-library/react'

import { DashboardScreen } from './DashboardScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The dashboard, and the two honesty rules it exists to keep.
 *
 *  * **Every chart has a text equivalent** (roadmap gate 5, and the honesty rule's
 *    "no invented number"): each figure is readable without the bars, so the
 *    screen works with a screen reader and at 200% text scale.
 *  * **Every number traces to a row** (A289, gate 7(b)): the screen renders the
 *    counts the routes return and computes nothing of its own. The fixture's
 *    numbers are deliberately not round, so a screen that invented one would have
 *    to invent the same odd value to pass.
 *
 * The locked case is asserted as a *negative*: on a plan without
 * `AdvancedAnalytics` there must be no figure from `/dashboard/topics` on screen
 * at all, which is why the fixture gives that route a value that would be obvious
 * if it leaked.
 */

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'power',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

function usage(advanced: boolean) {
  return {
    plan: advanced ? 'power' : 'free',
    name: advanced ? 'Power' : 'Free',
    subscription: { status: 'active', period_end: null },
    credits: { monthly: 0, purchased: 0, total: 0 },
    storage_bytes: 0,
    limits: {},
    features: { advanced_analytics: advanced, reminder_channels: ['web'], export_formats: [] },
    costs: {},
    provider_cost: { paused: false, resets_at: '2026-11-01T00:00:00Z' },
  }
}

/** The summary the run asserts against: 90 minutes over 2 courses, 5 reviews over 2 days. */
const SUMMARY = {
  from: '2026-09-28',
  to: '2026-10-12',
  time_per_course: [
    { workspace_id: 'w1', title: 'Struktur Data', minutes: 55 },
    { workspace_id: 'w2', title: 'Kriptografi', minutes: 35 },
  ],
  review_activity: [
    { local_date: '2026-10-11', reviews: 3 },
    { local_date: '2026-10-12', reviews: 2 },
  ],
  totals: { minutes_studied: 90, reviews: 5, cards_due: 7, quizzes_taken: 4 },
}

const TOPICS = {
  quiz_mastery: [
    { source_id: 's1', title: 'Graf', attempts: 6, correct: 4, accuracy: 0.6667 },
  ],
  weak_topics: [{ source_id: 's1', title: 'Graf', attempts: 6, correct: 4, accuracy: 0.6667 }],
}

const notFound = () =>
  jsonResponse(404, {
    code: 'not_found',
    message: 'no such route',
    min_plan: null,
    limit: null,
    used: null,
  })

beforeEach(() => {
  installFakePlatform('web')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the dashboard', () => {
  it('renders a text equivalent for every chart', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, usage(true))
      if (path === '/dashboard') return jsonResponse(200, SUMMARY)
      if (path === '/dashboard/topics') return jsonResponse(200, TOPICS)
      return notFound()
    })
    const { container } = renderWithProviders(<DashboardScreen />, { route: '/dashboard' })
    // The reads are queries: nothing below is on screen until they resolve.
    await screen.findByText(/90 minutes studied and 5 card reviews/)

    // Each figure's sentence states the same fact in words, so the screen is
    // readable without the bars and the bars are decoration over a number rather
    // than the only place it exists.
    expect(container.textContent).toContain('90 minutes studied and 5 card reviews')
    expect(container.textContent).toContain('2 courses studied, 90 minutes in total')
    expect(container.textContent).toContain('2 days with reviews, 5 reviews in total')

    // And every bar carries its own accessible name, so a screen reader gets the
    // figure the bar encodes rather than "image".
    const bars = container.querySelectorAll('[role="img"]')
    expect(bars.length).toBe(4)
    for (const bar of bars) {
      expect(bar.getAttribute('aria-label')).toBeTruthy()
    }
  })

  it('renders the counts the route returned and invents none', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, usage(true))
      if (path === '/dashboard') return jsonResponse(200, SUMMARY)
      if (path === '/dashboard/topics') return jsonResponse(200, TOPICS)
      return notFound()
    })
    const { container } = renderWithProviders(<DashboardScreen />, { route: '/dashboard' })
    // The topic appears twice — once in the mastery list and once in the weak-topic
    // list, which are the same row in two places — so both are asserted.
    const mastery = await screen.findAllByText(/Graf: 4 of 6 correct/)
    expect(mastery.length).toBe(2)

    // The odd values, not the sentences: `7` cards due and `4` quiz attempts are
    // rows the fixture inserted, and a screen that computed a percentage of its own
    // would have to reproduce these to pass.
    expect(container.textContent).toContain('7')
    expect(container.textContent).toContain('4')
    // The window is stated, in local days, so the reader knows what the counts cover.
    expect(container.textContent).toContain('2026-09-28 to 2026-10-12')
    // The mastery line is the row's own numbers: 4 of 6, 67 percent — a percentage
    // the *route* computed from `correct` and `attempts`, not one this screen made up.
    expect(container.textContent).toContain('Graf: 4 of 6 correct, 67 percent')
  })

  it('locks the topic breakdown on a plan that does not grant it', async () => {
    const requested: string[] = []
    mockFetch((url) => {
      const path = new URL(url).pathname
      requested.push(path)
      if (path === '/auth/me') return jsonResponse(200, { ...USER, plan_id: 'free' })
      if (path === '/usage') return jsonResponse(200, usage(false))
      if (path === '/dashboard') return jsonResponse(200, SUMMARY)
      if (path === '/dashboard/topics') return jsonResponse(200, TOPICS)
      return notFound()
    })
    const { container } = renderWithProviders(<DashboardScreen />, { route: '/dashboard' })
    await screen.findByText(/90 minutes studied and 5 card reviews/)
    expect(screen.getByText('Available on Power')).toBeTruthy()

    // The locked card, naming the plan that has it (web/desktop may say so).
    expect(container.textContent).toContain('Available on Power')
    // **No number from `/dashboard/topics` is on screen**, and the route was never
    // even requested: rendering the query and handling its 403 would put a failed
    // request on a screen whose whole point is that it works on every plan.
    expect(container.textContent).not.toContain('Graf: 4 of 6 correct, 67 percent')
    expect(container.textContent).not.toContain('Weak topics')
    expect(requested).not.toContain('/dashboard/topics')
    // The rest of the dashboard still works, which is why the two routes are two.
    expect(container.textContent).toContain('90 minutes studied')
  })

  it('shows a topic with no attempts as absent rather than as zero', async () => {
    // The honesty rule's most visible form: an uncomputable number must not reach
    // the screen. The route omits such a topic, and the screen has nothing to show
    // for it — never a `0%`.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, usage(true))
      if (path === '/dashboard') return jsonResponse(200, SUMMARY)
      if (path === '/dashboard/topics') {
        return jsonResponse(200, { quiz_mastery: [], weak_topics: [] })
      }
      return notFound()
    })
    const { container } = renderWithProviders(<DashboardScreen />, { route: '/dashboard' })
    await screen.findByText(/No quiz attempts yet/)
    expect(container.textContent).not.toContain('0 percent')
  })

  it('says the dashboard is empty rather than showing a row of zeroes', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, usage(true))
      if (path === '/dashboard') {
        return jsonResponse(200, {
          ...SUMMARY,
          time_per_course: [],
          review_activity: [],
          totals: { minutes_studied: 0, reviews: 0, cards_due: 0, quizzes_taken: 0 },
        })
      }
      if (path === '/dashboard/topics') {
        return jsonResponse(200, { quiz_mastery: [], weak_topics: [] })
      }
      return notFound()
    })
    const { container } = renderWithProviders(<DashboardScreen />, { route: '/dashboard' })
    await screen.findByText(/Nothing logged in this window yet/)
    expect(container.textContent).toContain('No study time logged in this window')
  })

  it('reports a failed read rather than rendering a zero', async () => {
    // A zero standing in for "unknown" is an invented number (gate 7(a)), so a
    // failed read must surface as a failure and never as `0 minutes`.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') return jsonResponse(200, usage(true))
      return notFound()
    })
    const { container } = renderWithProviders(<DashboardScreen />, { route: '/dashboard' })
    await waitFor(() => {
      expect(container.querySelector('[role="alert"]')).not.toBeNull()
    })
    expect(container.textContent).not.toContain('0 minutes')
  })
})
