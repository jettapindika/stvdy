import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { screen } from '@testing-library/react'
import { Route, Routes } from 'react-router'

import { ChatScreen } from '@/features/chat/ChatScreen'
import { CheatsheetScreen } from '@/features/reports/CheatsheetScreen'
import { ReportScreen } from '@/features/reports/ReportScreen'
import { PlanUsageScreen } from '@/features/billing/PlanUsageScreen'
import { DevicesScreen } from '@/features/devices/DevicesScreen'
import { CalendarScreen } from '@/features/calendar/CalendarScreen'
import { StudyScreen } from '@/features/study/StudyScreen'
import { TodayScreen } from '@/features/today/TodayScreen'
import { WorkspaceListScreen } from '@/features/workspaces/WorkspaceListScreen'
import { WorkspaceScreen } from '@/features/workspaces/WorkspaceScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { expectNoA11yViolations } from '@/test/a11y'
import { setAccessToken } from '@/lib/tokens'

/**
 * The accessibility gate on the real screens, not just the gallery.
 *
 * The gallery test proves the component library is sound in isolation. It cannot
 * prove a *screen* is: the defects axe finds on a screen are the ones produced by
 * composition — a heading level skipped because a panel starts at `h3`, an input
 * whose label is a sibling instead of a `<label for>`, a region with no accessible
 * name. Those only exist once the pieces are assembled, so each screen is rendered
 * with its real data path and checked.
 *
 * axe is a floor and says nothing about whether a screen makes sense to a
 * screen-reader user; the keyboard and focus behaviour is asserted in the
 * per-screen test files, where it belongs.
 */

/**
 * Checks a screen, and fails if the screen rendered nothing.
 *
 * An empty container passes every accessibility rule, so a check without this
 * would go green the moment a screen stopped rendering — which is the failure mode
 * that looks least like a failure. The positive check is supplied by the caller
 * rather than assumed, because a panel rendered on its own has no heading: the
 * heading belongs to the screen that hosts it, so asserting one here would fail
 * for the wrong reason.
 */
async function expectAccessible(container: HTMLElement, rendered: () => void): Promise<void> {
  await expectNoA11yViolations(container)
  rendered()
}

/** The default positive check: the screen drew a heading and some prose. */
function heading(container: HTMLElement, what: string): void {
  const headings = container.querySelectorAll('h1, h2, h3')
  expect(headings.length, `${what} rendered no heading`).toBeGreaterThan(0)
  expect(container.textContent?.trim().length ?? 0, `${what} rendered no text`).toBeGreaterThan(20)
}

const WORKSPACE = {
  id: 'w1',
  name: 'Struktur Data',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  source_count: 1,
}

const SOURCE = {
  id: 's1',
  workspace_id: 'w1',
  kind: 'pdf',
  title: 'Bab 3 Graf',
  url: null,
  file_size: 2048,
  mime: 'application/pdf',
  status: 'ready',
  error: null,
  created_at: '2026-10-11T08:00:00Z',
}

const DECK = {
  id: 'd1',
  workspace_id: 'w1',
  title: 'Graf',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 10,
  card_count: 2,
  created_at: '2026-10-11T08:00:00Z',
}

/** A 404 for anything a screen asks for that this test does not script. */
const notFound = () =>
  jsonResponse(404, {
    code: 'not_found',
    message: 'no such route',
    min_plan: null,
    limit: null,
    used: null,
  })

/**
 * The signed-in user the two document screens read their plan from.
 *
 * `plan_id` decides which export formats exist, so a screen rendered without it
 * would show neither format and hide the very controls these checks are here to
 * look at.
 */
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

/**
 * The plan and usage body the calendar panel reads its plan switch from.
 *
 * A **free** account, which is the state the panel's split matters in: free has
 * the ICS feed and the import, and not the Google connection. The calendar test
 * above asserts that split on the rendered panel; this fixture is what makes the
 * axe run see the panel as a free user does.
 */
const USAGE_BODY = {
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
    google_accounts_max: { limit: 0, used: null },
  },
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
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('screen accessibility', () => {
  it('Today has no axe violations', async () => {
    mockFetch(() => notFound())
    const { container } = renderWithProviders(<TodayScreen />, { route: '/' })
    await expectAccessible(container, () => heading(container, 'Today'))
  })

  it('the workspace list has no axe violations', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
      return notFound()
    })
    const { container } = renderWithProviders(<WorkspaceListScreen />, { route: '/workspaces' })
    await expectAccessible(container, () => heading(container, 'the workspace list'))
  })

  it('the workspace, on its sources panel, has no axe violations', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [SOURCE] })
      if (path === '/workspaces/w1/messages') return jsonResponse(200, { messages: [] })
      if (path === '/workspaces/w1/decks') return jsonResponse(200, { decks: [DECK] })
      // The Studio's third sub-tab mounts with the other two, so the workspace
      // screen requests the report list whether or not it is the visible panel.
      if (path === '/workspaces/w1/reports') return jsonResponse(200, { reports: [] })
      return notFound()
    })
    // The workspace screen takes its id from the route, so the route is the prop.
    const { container } = renderWithProviders(<WorkspaceScreen />, { route: '/workspaces/w1' })
    await expectAccessible(container, () => heading(container, 'the workspace'))
  })

  it('the chat composer has no axe violations', async () => {
    // The composer is the screen the spec makes the most accessibility demands of:
    // two labelled toggles, a described hint, and the honesty line.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [SOURCE] })
      if (path === '/workspaces/w1/messages') return jsonResponse(200, { messages: [] })
      return notFound()
    })
    const { container } = renderWithProviders(<ChatScreen workspaceId="w1" />, {
      route: '/workspaces/w1',
    })
    await expectAccessible(container, () => {
      // The panel has no heading of its own; its landmark is the composer, which
      // the spec requires to be labelled and described.
      const composer = container.querySelector('textarea#chat-message')
      expect(composer, 'the composer is missing').not.toBeNull()
      expect(composer?.getAttribute('aria-describedby')).toBe('chat-message-hint')
      expect(container.textContent).toContain('AI can make mistakes. Check the citations.')
    })
  })

  it('the flashcard review has no axe violations', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') {
        return jsonResponse(200, { deck: { ...DECK, cards: [] } })
      }
      if (path === '/decks/d1/due') {
        return jsonResponse(200, {
          cards: [
            {
              id: 'c1',
              deck_id: 'd1',
              front: 'Apa itu graf?',
              back: 'Kumpulan simpul dan sisi.',
              chunk_id: null,
              starred: false,
              flagged: false,
              ease: 2.5,
              interval_days: 0,
              due_at: '2026-10-11T08:00:00Z',
              created_at: '2026-10-11T08:00:00Z',
            },
          ],
          next_due_at: null,
        })
      }
      return notFound()
    })
    const { container } = renderWithProviders(<StudyScreen />, { route: '/study/d1' })
    await expectAccessible(container, () => heading(container, 'the flashcard review'))
  })

  it('the report screen has no axe violations', async () => {
    // The outline editor is the composition most likely to break a rule: a list of
    // inputs whose labels are generated, a caveat that must be readable without its
    // tone, and buttons whose names repeat per row.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, { ...USER, plan_id: 'pro' })
      if (path === '/reports/r1') {
        return jsonResponse(200, {
          report: {
            id: 'r1',
            workspace_id: 'w1',
            title: 'Jaringan Komputer',
            status: 'draft',
            error: null,
            outline: {
              title: 'Jaringan Komputer',
              sections: [
                { heading: 'Pendahuluan', purpose: 'Ruang lingkup.', labels: [1], gaps: false },
                { heading: 'Keamanan', purpose: 'WPA3.', labels: [], gaps: true },
              ],
            },
            body_md: null,
            sections: [
              { heading: 'Pendahuluan', body_md: null, generating: false, error: null },
              { heading: 'Keamanan', body_md: null, generating: false, error: null },
            ],
            created_at: '2026-10-11T08:00:00Z',
            updated_at: '2026-10-11T08:00:00Z',
          },
        })
      }
      return notFound()
    })
    // The screen takes its id from the route, so the route is what renders it:
    // `useParams` needs a real match, not just a location.
    const { container } = renderWithProviders(
      <Routes>
        <Route path="/reports/:reportId" element={<ReportScreen />} />
      </Routes>,
      { route: '/reports/r1' },
    )
    // The screen needs its report before it renders anything: an empty container
    // passes every rule, so the check waits for the content it is checking.
    await screen.findByRole('heading', { name: 'Jaringan Komputer' })
    await expectAccessible(container, () => heading(container, 'the report screen'))
  })

  it('the cheatsheet preview has no axe violations', async () => {
    // axe cannot evaluate an `iframe` in jsdom ("Respondable target must be a frame
    // in the current window"), so the frame is left out of this run and its own two
    // properties — that it is named, and that its sandbox is empty — are asserted
    // directly in `CheatsheetScreen.test.tsx`, where the frame's contents can be
    // driven. What this check covers is the rest of the screen: the headings, the
    // action buttons, the locked-format block, and the labels on all of them.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, { ...USER, plan_id: 'pro' })
      if (path === '/cheatsheets/c1') {
        return jsonResponse(200, {
          cheatsheet: {
            id: 'c1',
            workspace_id: 'w1',
            title: 'Jaringan Komputer',
            scope: 'UTS Jaringan Komputer',
            exam_date: '2026-10-20',
            depth: 'normal',
            source_ids: ['s1'],
            status: 'ready',
            error: null,
            template_version: 'v1',
            page_count: 6,
            created_at: '2026-10-11T08:00:00Z',
            updated_at: '2026-10-11T08:00:00Z',
          },
        })
      }
      // Deliberately unanswered: no frame is created, and the screen's own error
      // state for the preview is what renders in its place.
      return notFound()
    })
    const { container } = renderWithProviders(
      <Routes>
        <Route path="/cheatsheets/:cheatsheetId" element={<CheatsheetScreen />} />
      </Routes>,
      { route: '/cheatsheets/c1' },
    )
    await screen.findByRole('heading', { name: 'Jaringan Komputer' })
    await expectAccessible(container, () => heading(container, 'the cheatsheet preview'))
  })

  it('the plan and usage screen has no axe violations', async () => {
    // The composition most likely to break a rule here is the pair of definition
    // lists: a `<dt>`/`<dd>` pair inside a `<div>` is valid, but a label that is a
    // `<span>` instead would leave the value without a name, and the two lists
    // must not be mistaken for one another. The locked format block is the third:
    // it is a link on this platform, so it needs an accessible name of its own.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/usage') {
        return jsonResponse(200, {
          plan: 'free',
          name: 'Free',
          subscription: { status: 'active', period_end: '2026-11-01T07:00:00Z' },
          credits: { monthly: 60, purchased: 12, total: 72 },
          storage_bytes: 1048576,
          limits: {
            workspaces: { limit: 2, used: 2 },
            sources_total: { limit: 20, used: 3 },
            max_file_mb: { limit: 20, used: null },
            storage_mb: { limit: 200, used: null },
            devices_max: { limit: 2, used: 1 },
            credits_per_month: { limit: 60, used: null },
            chat_msgs_per_day: { limit: 20, used: 4 },
            web_searches_per_day: { limit: 3, used: 3 },
            flashcards_per_month: { limit: 30, used: 12 },
            quizzes_per_month: { limit: 2, used: 0 },
            reports_per_month: { limit: 1, used: 1 },
            syllabus_imports_total: { limit: 1, used: 0 },
            google_accounts_max: { limit: 0, used: null },
          },
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
          provider_cost: { paused: true, resets_at: '2026-11-01T07:00:00Z' },
        })
      }
      return notFound()
    })
    const { container } = renderWithProviders(<PlanUsageScreen />, { route: '/billing' })
    await screen.findByRole('heading', { name: 'Plan and usage' })
    await expectAccessible(container, () => heading(container, 'the plan and usage screen'))
  })

  it('the devices screen has no axe violations', async () => {
    // The composition here is a list of rows that each carry a status word, two
    // buttons and a confirmation that replaces the row's own action. The rename
    // field is the one that can lose its name — a label wrapping an input is fine,
    // but the `<span>` beside it has to be the label rather than a sibling — and
    // the reset block's confirmation is a second `role="alert"`-capable region.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/devices') {
        return jsonResponse(200, {
          devices: [
            {
              id: 'd1',
              name: "Jetta's MacBook",
              platform: 'macos',
              app_version: '0.1.0',
              last_seen_at: '2026-10-11T09:00:00Z',
              last_sync_at: '2026-10-11T08:00:00Z',
              created_at: '2026-10-01T08:00:00Z',
              revoked_at: null,
              current: true,
            },
            {
              id: 'd2',
              name: 'Pixel 8',
              platform: 'android',
              app_version: null,
              last_seen_at: '2026-10-11T09:30:00Z',
              last_sync_at: null,
              created_at: '2026-10-02T08:00:00Z',
              revoked_at: null,
              current: false,
            },
          ],
        })
      }
      return notFound()
    })
    const { container } = renderWithProviders(<DevicesScreen />, { route: '/devices' })
    await screen.findByRole('heading', { name: 'Devices and sync' })
    await expectAccessible(container, () => heading(container, 'the devices screen'))
  })

  it('the calendar screen has no axe violations', async () => {
    // The composition most likely to break a rule here is the grid: a table whose
    // cells are controls, and a panel of labelled fields beside it. The grid is a
    // table rather than positioned divs precisely so that a keyboard can enter it
    // and a screen reader can read it (A244), and axe is what checks the roles and
    // the names that decision produces.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/events') {
        return jsonResponse(200, {
          events: [
            {
              id: 'e1',
              workspace_id: null,
              kind: 'class',
              title: 'UAS Jaringan Komputer',
              starts_at: '2026-10-11T09:00:00Z',
              ends_at: '2026-10-11T11:00:00Z',
              source: 'app',
              task_id: null,
            },
            {
              id: 'e2',
              workspace_id: null,
              kind: 'external',
              title: 'Rapat himpunan',
              starts_at: '2026-10-11T12:00:00Z',
              ends_at: '2026-10-11T13:00:00Z',
              source: 'ics',
              task_id: null,
            },
          ],
        })
      }
      if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
      if (path === '/calendar/connections') return jsonResponse(200, { connections: [] })
      if (path === '/calendar/ics-token') return jsonResponse(200, { ics_feed_token: null })
      if (path === '/usage') return jsonResponse(200, USAGE_BODY)
      return notFound()
    })
    const { container } = renderWithProviders(<CalendarScreen />, { route: '/calendar' })
    await screen.findByRole('heading', { name: 'Calendar' })
    await expectAccessible(container, () => heading(container, 'the calendar screen'))
  })
})
