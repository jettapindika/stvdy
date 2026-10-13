import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CalendarScreen } from './CalendarScreen'
import { API_BASE_URL } from '@/api/client'
import { feedUrl, type Connection } from '@/api/queries/calendar'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { formatInZone, localDateKey } from '@/lib/time'
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
 *
 *  - **the panel states the API's own facts** — the connected account's address,
 *    its status word and its two sync preferences are what
 *    `GET /calendar/connections` answered, and the feed field holds the URL the
 *    returned token makes. A panel that invented any of them, or that filled an
 *    absent one with a plausible value, fails here.
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
 * The screen under test, with the clock and the zone its host supplies.
 *
 * `CalendarScreen` takes `now` and `timeZone` as props rather than reading either
 * itself — `PlannerScreen` captures the instant once, so the heading, the grid,
 * the `Today` cell and the current-time line cannot disagree about which day it
 * is. A test therefore has to pass both.
 *
 * `new Date()` is deliberate rather than a fixed clock: `serve` anchors the
 * fixture events to the **real** local today, so a hardcoded `now` would place
 * them outside the window the screen renders. The zone is the account's own,
 * which is what `GET /auth/me` answered.
 */
function renderCalendar() {
  return renderWithProviders(<CalendarScreen now={new Date()} timeZone={USER.timezone} />, {
    route: '/calendar',
  })
}

/**
 * The API as the screen reads it, with the plan switchable.
 *
 * `served` is what the account's own state is: the connections the panel renders
 * rows for, and the feed token it renders a field for. Both default to the empty
 * answer, so a test that is about the plan split does not have to state them.
 *
 * The screen takes its "today" from `new Date()` on mount, so the fixture events
 * are placed relative to the real clock rather than at fixed instants: a test
 * with a hardcoded June date would pass in June and render an empty grid in
 * December. That is the one place a screen test cannot be a pure function of a
 * fixed clock, and it is why the window maths lives in its own file.
 *
 * "Relative to the real clock" is anchored to the student's **local today**, not
 * to `Date.now() + n hours`: run after 23:00 the old form put the second event
 * across local midnight, and `placeEvents` places an event on every local day it
 * overlaps — deliberately, so a conference that began yesterday is not dropped —
 * so the grid rendered two buttons with one id and the query below found two
 * matches. `T12:00:00Z` is inside the same local date as `localDateKey(now)` for
 * any zone the app supports, and the pair of one-hour events ends long before the
 * day does.
 */
function serve(
  plan: { googleCalendar: boolean; googleAccountsMax: number | null },
  served: { connections?: Connection[]; feedToken?: string | null } = {},
) {
  const today = Date.parse(`${localDateKey(new Date(), USER.timezone)}T12:00:00Z`)
  const events = [OWN_EVENT, EXTERNAL_EVENT].map((event, index) => ({
    ...event,
    starts_at: new Date(today + index * 3600_000).toISOString(),
    ends_at: new Date(today + (index + 1) * 3600_000).toISOString(),
  }))

  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/events') return jsonResponse(200, { events })
    if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
    if (path === '/calendar/connections') {
      return jsonResponse(200, { connections: served.connections ?? [] })
    }
    if (path === '/calendar/ics-token') {
      return jsonResponse(200, { ics_feed_token: served.feedToken ?? null })
    }
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
    renderCalendar()

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
    renderCalendar()

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
    renderCalendar()

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
    renderCalendar()

    // The free matrix has the ICS feed and the import, so both are present and
    // neither is locked. A `LockedFeature` around either would refuse a feature
    // every plan has.
    expect(await screen.findByText('Your calendar feed')).toBeInTheDocument()
    expect(
      await screen.findByRole('button', { name: 'Create my feed link' }),
    ).toBeInTheDocument()
    expect(await screen.findByLabelText('ICS URL')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Add' })).toBeInTheDocument()

    // Google is the one gated thing on **this panel**, and the lock names the
    // plan that has it. The queries are scoped to the panel's own section rather
    // than to the whole screen: the planner's own locks are on the same page and
    // carry the same sentence, and an unscoped `getByText` finds all of them.
    const google = await screen.findByRole('region', { name: 'Google Calendar' })
    expect(within(google).getByText('Available on Pro')).toBeInTheDocument()
    expect(within(google).queryByRole('button', { name: 'Connect Google Calendar' })).toBeNull()
  })

  it('offers the Google connection on pro', async () => {
    serve({ googleCalendar: true, googleAccountsMax: 1 })
    renderCalendar()

    const google = await screen.findByRole('region', { name: 'Google Calendar' })
    expect(
      within(google).getByRole('button', { name: 'Connect Google Calendar' }),
    ).toBeInTheDocument()
    expect(within(google).queryByText('Available on Pro')).toBeNull()
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
    renderCalendar()

    expect(await screen.findByText('recurrence_truncated')).toBeInTheDocument()
    expect(screen.getByText('Needs attention')).toBeInTheDocument()
    // "Last synced" is the stored instant rendered, never a client-side guess.
    expect(screen.getByText(/Last synced/)).toBeInTheDocument()
  })
})

/**
 * The connection the server reports for a signed-in Google account, as
 * `GET /calendar/connections` answers it (A249).
 *
 * The instant is fixed rather than relative to the clock: the account block
 * states a stored timestamp, not a countdown, so nothing about it should move
 * with the run.
 */
const SYNCED_AT = '2026-10-12T03:30:00Z'

const GOOGLE_CONNECTION: Connection = {
  id: 'c-google',
  provider: 'google',
  account_email: 'rani.study@gmail.com',
  status: 'connected',
  error: null,
  last_synced_at: SYNCED_AT,
  study_calendar_id: null,
}

describe('the connected Google account', () => {
  it('is stated as the API reports it, with its preferences as marks rather than controls', async () => {
    serve({ googleCalendar: true, googleAccountsMax: 1 }, { connections: [GOOGLE_CONNECTION] })
    renderCalendar()

    const card = await screen.findByRole('region', { name: 'Google Calendar' })

    // The address is `account_email`, and the state is the server's own `status`
    // word with the tone repeating it rather than carrying it alone.
    expect(await within(card).findByText('rani.study@gmail.com')).toBeInTheDocument()
    expect(within(card).getByText('Connected').className).toContain('chip-ready')
    // "Last synced" is the stored instant in the account's own zone.
    expect(
      within(card).getByText(
        formatInZone(new Date(SYNCED_AT), 'Asia/Jakarta', {
          dateStyle: 'medium',
          timeStyle: 'short',
        }),
      ),
    ).toBeInTheDocument()

    // Two preferences, both of them the server's facts: a connected account
    // imports its events, and `study_calendar_id: null` says no Study Plan
    // calendar is receiving blocks yet. Neither is a control, because no route
    // writes either one.
    const marks = within(card).getAllByRole('checkbox')
    expect(marks).toHaveLength(2)
    expect(marks[0]).toBeChecked()
    expect(marks[1]).not.toBeChecked()
    expect(marks[0]).toBeDisabled()
    expect(marks[1]).toBeDisabled()
    // The mark's name is the preference it belongs to, not the word beside it:
    // "On, checkbox, checked, unavailable" tells a screen-reader user nothing
    // about what is on. The title's text is a new key, so the name itself is not
    // asserted here — the reference to it is.
    expect(marks[0]?.getAttribute('aria-labelledby')).not.toBeNull()
  })

  it('shows the Study Plan calendar as receiving blocks once the server reports one', async () => {
    serve(
      { googleCalendar: true, googleAccountsMax: 1 },
      { connections: [{ ...GOOGLE_CONNECTION, study_calendar_id: 'sp-9' }] },
    )
    renderCalendar()

    const card = await screen.findByRole('region', { name: 'Google Calendar' })
    const marks = await within(card).findAllByRole('checkbox')
    expect(marks[1]).toBeChecked()
  })

  it('states the status of each account when there is more than one', async () => {
    serve(
      { googleCalendar: true, googleAccountsMax: 2 },
      {
        connections: [
          GOOGLE_CONNECTION,
          {
            ...GOOGLE_CONNECTION,
            id: 'c-work',
            account_email: 'rani@work.example',
            status: 'revoked',
          },
        ],
      },
    )
    renderCalendar()

    const card = await screen.findByRole('region', { name: 'Google Calendar' })
    expect(await within(card).findByText('rani@work.example')).toBeInTheDocument()
    // One chip per account: a single chip in the card's heading row would report
    // one account's health as all of them.
    expect(within(card).getAllByText('Connected')).toHaveLength(1)
    expect(within(card).getAllByText('Access revoked')).toHaveLength(1)
  })
})

describe('the calendar feed link', () => {
  it('is a labelled, read-only field holding the token the API returned', async () => {
    serve({ googleCalendar: false, googleAccountsMax: 0 }, { feedToken: 'tok' })
    renderCalendar()

    const card = await screen.findByRole('region', { name: 'Your calendar feed' })
    // The subscription URL is the API's token, not a value this build can make
    // up, and the field is read-only: Regenerate rotates the link and nothing
    // else does.
    const field = await within(card).findByRole('textbox')
    expect(field).toHaveValue(feedUrl(API_BASE_URL, 'tok'))
    expect(field).toHaveAttribute('readonly')
  })
})
