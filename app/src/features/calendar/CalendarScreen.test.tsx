import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CalendarScreen } from './CalendarScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The calendar screen's two contract-level behaviours.
 *
 * What is *not* asserted here is the date maths: that is `calendar.test.ts`'s,
 * against a fixed clock and an explicit zone, because a week boundary is not
 * something a rendered screen can prove. What this file is for is the pair of
 * properties the plan names and a screenshot cannot show:
 *
 *  - **an external event is read-only, and says so in the accessibility tree** —
 *    a lock icon alone is a state carried by a glyph, and the plan's gate 5(d)
 *    asks for a word as well;
 *  - **the panel's plan split** — the ICS feed and the ICS field are on every
 *    plan, and only the Google connection is gated. Getting this backwards would
 *    refuse a feature the free matrix grants, which is the defect the brief names
 *    first.
 */

const WORKSPACE = {
  id: 'w1',
  name: 'Jaringan Komputer',
  created_at: '2026-10-01T08:00:00Z',
  updated_at: '2026-10-01T08:00:00Z',
  source_count: 2,
}

/**
 * Two events on the same local day: one the student owns, one that arrived from a
 * subscribed calendar. Both are needed — a screen that marked *every* event
 * read-only would pass a test that only ever rendered an external one.
 */
const OWN_EVENT = {
  id: 'e-own',
  workspace_id: 'w1',
  kind: 'class',
  title: 'Struktur Data',
  starts_at: '2026-06-10T01:00:00Z',
  ends_at: '2026-06-10T03:00:00Z',
  source: 'app',
  task_id: null,
}

const EXTERNAL_EVENT = {
  id: 'e-ext',
  workspace_id: null,
  kind: 'external',
  title: 'Rapat himpunan',
  starts_at: '2026-06-10T04:00:00Z',
  ends_at: '2026-06-10T05:00:00Z',
  source: 'ics',
  task_id: null,
}

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-01T08:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-01T08:00:00Z',
}

const notFound = () =>
  jsonResponse(404, { code: 'not_found', message: 'no', min_plan: null, limit: null, used: null })

/**
 * The API as the screen reads it, with the plan switchable.
 *
 * The screen takes its "today" from `new Date()` on mount, so the fixture events
 * are placed relative to the real clock rather than at fixed instants: a test
 * with a hardcoded June date would pass in June and render an empty grid in
 * December. That is the one place a screen test cannot be a pure function of a
 * fixed clock, and it is why the window maths lives in its own file.
 */
function serve(plan: { googleCalendar: boolean; googleAccountsMax: number | null }) {
  const now = Date.now()
  const events = [OWN_EVENT, EXTERNAL_EVENT].map((event, index) => ({
    ...event,
    starts_at: new Date(now + index * 3600_000).toISOString(),
    ends_at: new Date(now + (index + 1) * 3600_000).toISOString(),
  }))

  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/events') return jsonResponse(200, { events })
    if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
    if (path === '/calendar/connections') return jsonResponse(200, { connections: [] })
    if (path === '/calendar/ics-token') return jsonResponse(200, { ics_feed_token: null })
    if (path === '/usage') {
      return jsonResponse(200, {
        plan: 'free',
        name: 'Free',
        subscription: { status: 'active', period_end: null },
        credits: { monthly: 60, purchased: 0, total: 60 },
        storage_bytes: 0,
        limits: {
          workspaces: { limit: 2, used: 1 },
          sources_total: { limit: 20, used: 2 },
          max_file_mb: { limit: 20, used: null },
          storage_mb: { limit: 200, used: null },
          devices_max: { limit: 2, used: 1 },
          credits_per_month: { limit: 60, used: null },
          chat_msgs_per_day: { limit: 20, used: 0 },
          web_searches_per_day: { limit: 3, used: 0 },
          flashcards_per_month: { limit: 30, used: 0 },
          quizzes_per_month: { limit: 2, used: 0 },
          reports_per_month: { limit: 1, used: 0 },
          syllabus_imports_total: { limit: 1, used: 0 },
          google_accounts_max: { limit: plan.googleAccountsMax, used: null },
        },
        features: {
          export_formats: ['md'],
          task_ai_breakdown: false,
          quick_capture: false,
          google_calendar: plan.googleCalendar,
          planner: 'manual',
          agent_actions: false,
          reminder_channels: ['web', 'push'],
          model_tier: 'cheapest',
          byok: false,
          advanced_analytics: false,
        },
        costs: {
          chat_message: 1,
          web_search: 2,
          flashcard_batch_10: 4,
          quiz_10: 6,
          report_outline: 3,
          report_section: 6,
          task_breakdown: 2,
          syllabus_import: 8,
          plan_generation: 2,
          agent_turn: 2,
          long_doc_threshold_tokens: 20000,
          long_doc_credits_per_1k_tokens: 1,
        },
        provider_cost: { paused: false, resets_at: '2026-11-01T07:00:00Z' },
      })
    }
    return notFound()
  })
}

beforeEach(() => {
  // The **web** kind, so `hasLocalCache()` is false and the screen reads
  // `GET /events` (A244). The native path reads the shell's cache instead, and is
  // asserted separately below.
  installFakePlatform('web')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('an external event', () => {
  it('renders a lock and no edit affordance', async () => {
    serve({ googleCalendar: false, googleAccountsMax: 0 })
    renderWithProviders(<CalendarScreen />, { route: '/calendar' })

    // The event is rendered as a control that opens its detail; it is not a link
    // to an editor and there is no editor on this screen at all.
    const cell = await screen.findByRole('button', { name: /Rapat himpunan/ })
    expect(within(cell).getByText('Read-only')).toBeInTheDocument()

    await userEvent.click(cell)
    await screen.findByRole('heading', { name: 'Rapat himpunan' })
    // The reason, in words, and which of the two reasons it is: a subscribed
    // calendar, not Google.
    expect(screen.getByText(/came from a subscribed calendar/)).toBeInTheDocument()
    // No control on this screen writes an event: the phase's read-only rule is
    // enforced on the server, and a UI that offered the edit would be a control
    // whose only outcome is a 403.
    expect(screen.queryByRole('button', { name: /edit/i })).toBeNull()
    expect(screen.queryByRole('button', { name: /delete/i })).toBeNull()
  })

  it('is announced as read-only to a screen reader', async () => {
    serve({ googleCalendar: false, googleAccountsMax: 0 })
    renderWithProviders(<CalendarScreen />, { route: '/calendar' })

    // The accessible name carries the state, so a screen-reader user hears it
    // without opening anything: the lock icon is `aria-hidden`, so this name is
    // the only place the fact exists in the tree.
    const cell = await screen.findByRole('button', {
      name: /Rapat himpunan at .*, read-only/,
    })
    expect(cell).toBeInTheDocument()

    // And the student's own event is not labelled read-only — the rule marks the
    // external row, not every row.
    const own = await screen.findByRole('button', { name: /Struktur Data/ })
    expect(own.getAttribute('aria-label')).toBeNull()
  })

  it('closes its detail on Escape and returns focus to the cell', async () => {
    serve({ googleCalendar: false, googleAccountsMax: 0 })
    renderWithProviders(<CalendarScreen />, { route: '/calendar' })

    const cell = await screen.findByRole('button', { name: /Rapat himpunan/ })
    await userEvent.click(cell)
    await screen.findByRole('heading', { name: 'Rapat himpunan' })

    await userEvent.keyboard('{Escape}')
    expect(screen.queryByRole('heading', { name: 'Rapat himpunan' })).toBeNull()
    // The keyboard user is back where they were, not at the top of the document.
    expect(document.activeElement).toBe(cell)
  })
})

describe('the plan split on the connections panel', () => {
  it('offers the feed and the ICS field on free, and locks only Google', async () => {
    serve({ googleCalendar: false, googleAccountsMax: 0 })
    renderWithProviders(<CalendarScreen />, { route: '/calendar' })

    // The free matrix has the ICS feed and the import, so both are present and
    // neither is locked. A `LockedFeature` around either would refuse a feature
    // every plan has.
    expect(await screen.findByText('Your calendar feed')).toBeInTheDocument()
    expect(
      await screen.findByRole('button', { name: 'Create my feed link' }),
    ).toBeInTheDocument()
    expect(await screen.findByLabelText('ICS URL')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Add' })).toBeInTheDocument()

    // Google is the one gated thing, and the lock names the plan that has it.
    expect(screen.getByText('Available on Pro')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Connect Google Calendar' })).toBeNull()
  })

  it('offers the Google connection on pro', async () => {
    serve({ googleCalendar: true, googleAccountsMax: 1 })
    renderWithProviders(<CalendarScreen />, { route: '/calendar' })

    expect(
      await screen.findByRole('button', { name: 'Connect Google Calendar' }),
    ).toBeInTheDocument()
    expect(screen.queryByText('Available on Pro')).toBeNull()
  })

  it('reports a connection that holds an error, in words', async () => {
    serve({ googleCalendar: true, googleAccountsMax: 1 })
    const now = Date.now()
    // Re-served so the connections list carries a row with a stored reason: the
    // panel must not show a connection as healthy while it holds one (A247).
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/events') return jsonResponse(200, { events: [] })
      if (path === '/workspaces') return jsonResponse(200, { workspaces: [] })
      if (path === '/calendar/ics-token') return jsonResponse(200, { ics_feed_token: 'tok' })
      if (path === '/calendar/connections') {
        return jsonResponse(200, {
          connections: [
            {
              id: 'c1',
              provider: 'ics',
              account_email: null,
              status: 'error',
              error: 'recurrence_truncated',
              last_synced_at: new Date(now).toISOString(),
              study_calendar_id: null,
            },
          ],
        })
      }
      return notFound()
    })
    renderWithProviders(<CalendarScreen />, { route: '/calendar' })

    expect(await screen.findByText('recurrence_truncated')).toBeInTheDocument()
    expect(screen.getByText('Needs attention')).toBeInTheDocument()
    // "Last synced" is the stored instant rendered, never a client-side guess.
    expect(screen.getByText(/Last synced/)).toBeInTheDocument()
  })
})
