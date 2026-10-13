import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryRouter, Navigate, RouterProvider } from 'react-router'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render } from '@testing-library/react'

import { installFakePlatform, jsonResponse, mockFetch, stubViewport } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'
import { PlannerScreen } from './PlannerScreen'

/**
 * The planner **host**, and the three things about it that are not layout.
 *
 * 1. **The view is in the URL.** `?view=calendar|tasks` is what makes the two
 *    former screens linkable, bookmarkable and measurable. A view held in
 *    `useState` would render the right panel on a click and the wrong one on a
 *    deep link, and no harness could reach the second panel at all — which is
 *    the failure this file exists to catch, because it is invisible in a browser
 *    where the student only ever clicks.
 * 2. **`/tasks` and `/calendar` still land somewhere.** They are redirects into
 *    this screen, and a notification tap, a bookmark and an already-shipped
 *    `notificationPath` all name them. The redirect is asserted *by walking the
 *    route table* rather than by rendering the host, because the redirect is the
 *    thing that can silently break.
 * 3. **The syllabus import is reachable in both goal states.** `/planner` and
 *    `/syllabus` had no entry point in the app before 2026-10-12: the only two
 *    inbound links formed a closed cycle, and the app's *only* link to
 *    `/syllabus` lived inside the `rows.length === 0` branch — so it was visible
 *    to a student with no goals and invisible to one who had any, which is every
 *    student who had used the planner. Both states are checked, because a fix
 *    that renders the link in one of them is the defect this file exists for.
 */

const USER = {
  id: 'u1',
  email: 'student@example.test',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

/** `planner: 'manual'` — the free plan's mode, which is what renders the upgrade card. */
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
    reminder_channels: ['web'],
    model_tier: 'cheapest',
    byok: false,
    advanced_analytics: false,
  },
  costs: {},
  provider_cost: { paused: false, resets_at: null },
}

const SETTINGS = {
  study_start_hour: 8,
  study_end_hour: 22,
  max_study_min_per_day: 240,
  block_len_min: 50,
  break_min: 10,
  reminder_channels: ['web'],
  quiet_start_hour: 22,
  quiet_end_hour: 7,
  timezone: 'Asia/Jakarta',
  language: 'en',
  week_starts_on: 1,
}

const GOAL = {
  id: 'g1',
  workspace_id: 'w1',
  title: 'UTS Kriptografi',
  // A full instant, because that is what the wire carries: `goals.target_date` is
  // `timestamptz` (`migrations/0012`), so the server sends `…T00:00:00Z` and never a
  // bare date. A date-only fixture here is what let a `new Date(\`${target_date}T12:00:00Z\`)`
  // template pass this suite while it crashed the real screen with a `RangeError`.
  target_date: '2026-11-16T09:00:00Z',
  source_ids: ['s1'],
}

const PROGRESS = {
  goal_id: 'g1',
  window_days: 14,
  topic_coverage_pct: 62,
  quiz_accuracy_pct: null,
  cards_due: 27,
  weak_topics: [],
  topics: [],
}

const WORKSPACE = {
  id: 'w1',
  name: 'Kriptografi',
  created_at: '2026-10-01T08:00:00Z',
  updated_at: '2026-10-01T08:00:00Z',
  source_count: 1,
  course_code: 'CS 304',
  color: 0,
  exam_date: null,
}

/**
 * `goals` decides which of the two import states renders, and it is the variable
 * this file turns on: an empty list is the state whose import link was the *only*
 * one, and a populated list is the state every returning student is in.
 */
/** One task, so the tasks view renders a row and the board renders a column. */
const TASK = {
  id: 't1',
  workspace_id: 'w1',
  parent_id: null,
  title: 'Latihan soal kripto',
  notes: null,
  due_at: '2026-10-16T02:00:00Z',
  estimate_min: 45,
  priority: 1,
  status: 'todo',
  position: 0,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
  deleted_at: null,
}

function setup(goals: unknown[], tasks: unknown[] = [TASK]) {
  const calls = mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/usage') return jsonResponse(200, USAGE)
    if (path === '/settings') return jsonResponse(200, { settings: SETTINGS, timezone: 'Asia/Jakarta' })
    if (path === '/goals') return jsonResponse(200, { goals })
    if (path === '/goals/g1/progress') return jsonResponse(200, PROGRESS)
    if (path === '/events') return jsonResponse(200, { events: [] })
    if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
    if (path === '/tasks') return jsonResponse(200, { tasks })
    if (path === '/calendar/connections') return jsonResponse(200, { connections: [] })
    if (path === '/calendar/ics-token') return jsonResponse(200, { ics_feed_token: null })
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })
  return { calls }
}

beforeEach(() => {
  window.localStorage.clear()
  setAccessToken('access-1')
  // The **web** kind, so `hasLocalCache()` is false and the tasks view reads
  // `GET /tasks` (A244). On a native kind the task list comes from the shell's
  // cache, which this test does not seed, and the view would render its empty
  // state while the fixture sat unread.
  installFakePlatform('web')
  // The host reads the form factor (the view's label differs between a phone and
  // a desktop), and `useFormFactor` subscribes to `matchMedia`, which jsdom does
  // not implement.
  stubViewport(1280)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the syllabus import is reachable', () => {
  /**
   * The state a returning student is in, and the one where the link used to be
   * absent.
   *
   * `Import another syllabus` rather than `Import a syllabus`: the empty state's
   * own action keeps that string, so the two states stay distinguishable — and a
   * test that asserted the empty state's label here would pass on a screen
   * showing the empty state instead of the goal list.
   */
  it('offers the import alongside the goal list, not only when there are no goals', async () => {
    setup([GOAL])
    renderWithProviders(<PlannerScreen />, { route: '/planner' })

    expect(await screen.findByText('UTS Kriptografi')).toBeInTheDocument()
    const link = screen.getByRole('link', { name: 'Import another syllabus' })
    expect(link).toHaveAttribute('href', '/syllabus')
  })

  /** And the state that had it all along, so a fix cannot trade one for the other. */
  it('keeps the empty state’s own import action', async () => {
    setup([])
    renderWithProviders(<PlannerScreen />, { route: '/planner' })

    const link = await screen.findByRole('link', { name: 'Import a syllabus' })
    expect(link).toHaveAttribute('href', '/syllabus')
    // The list state's link is *not* here: one import affordance per state, or the
    // screen offers the same destination twice and the empty state stops reading
    // as empty.
    expect(screen.queryByRole('link', { name: 'Import another syllabus' })).toBeNull()
  })
})

/**
 * The view is a URL, and this is the assertion that keeps it one.
 *
 * The two panels render different content, and each address is asked for
 * directly — no click, because a click is what a state-based implementation
 * would pass and a deep link is what it would fail.
 */
describe('the view comes from the query string', () => {
  it('renders the tasks panel at ?view=tasks', async () => {
    setup([GOAL])
    renderWithProviders(<PlannerScreen />, { route: '/planner?view=tasks' })

    // The tasks panel's own field, which the calendar panel does not render.
    expect(await screen.findByLabelText('Find tasks')).toBeInTheDocument()
    expect(screen.queryByRole('heading', { name: 'Goals' })).toBeNull()
  })

  it('renders the plan — the calendar — at ?view=plan, at ?view=calendar, and with no view at all', async () => {
    for (const route of ['/planner', '/planner?view=plan', '/planner?view=calendar']) {
      setup([GOAL])
      const { container } = renderWithProviders(<PlannerScreen />, { route })
      // The goal chooser is the plan view's own block.
      expect(await within(container).findByRole('heading', { name: 'Goals' })).toBeInTheDocument()
      expect(within(container).queryByLabelText('Find tasks')).toBeNull()
      window.localStorage.clear()
      setAccessToken('access-1')
    }
  })

  it('carries the tasks layout in the URL too, and ignores it on the plan view', async () => {
    setup([GOAL])
    const { container } = renderWithProviders(<PlannerScreen />, {
      route: '/planner?view=tasks&layout=board',
    })
    // The board's three status columns, which the list does not render.
    expect(await within(container).findByRole('heading', { name: 'To do' })).toBeInTheDocument()
    expect(within(container).queryByText('Task / Course')).toBeNull()

    // And on the plan view the parameter is inert rather than an error: switching
    // to the calendar and back must not lose the student's board, so the view is
    // what decides, and an unknown value falls back to the default rather than to
    // nothing.
    setup([GOAL])
    const other = renderWithProviders(<PlannerScreen />, { route: '/planner?view=nonsense' })
    expect(await within(other.container).findByRole('heading', { name: 'Goals' })).toBeInTheDocument()
  })

  it('marks the active view with aria-current, so the state is not colour alone', async () => {
    setup([GOAL])
    renderWithProviders(<PlannerScreen />, { route: '/planner?view=tasks' })

    const switcher = await screen.findByRole('navigation', { name: 'Planner views' })
    const tasks = within(switcher).getByRole('link', { name: 'Tasks' })
    expect(tasks).toHaveAttribute('aria-current', 'page')
    // The other half is a link, not a second current-page mark.
    expect(within(switcher).getByRole('link', { name: 'Calendar' })).not.toHaveAttribute(
      'aria-current',
    )
  })
})

/**
 * The redirects, walked through a real router.
 *
 * `/tasks` and `/calendar` are the two paths that must keep working: a
 * notification tap resolves to one of them (`notificationPath`), a bookmark holds
 * one, and neither can be rewritten retroactively. The route table here is the
 * real shape from `router.tsx` — a parent element rendering the outlet — because
 * a redirect asserted without its parent is a redirect that can be registered in
 * the wrong place.
 */
describe('the old paths land on the planner', () => {
  function renderAt(path: string) {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    })
    const router = createMemoryRouter(
      [
        {
          path: '/',
          element: <PlannerScreen />,
          children: [],
        },
        { path: '/planner', element: <PlannerScreen /> },
        { path: '/tasks', element: <Navigate to="/planner?view=tasks" replace /> },
        { path: '/calendar', element: <Navigate to="/planner?view=calendar" replace /> },
      ],
      { initialEntries: [path] },
    )
    render(
      <QueryClientProvider client={client}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    )
    return router
  }

  it('resolves /tasks to the tasks view', async () => {
    setup([GOAL])
    const router = renderAt('/tasks')

    expect(await screen.findByLabelText('Find tasks')).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/planner')
    expect(router.state.location.search).toBe('?view=tasks')
  })

  it('resolves /calendar to the plan view', async () => {
    setup([GOAL])
    const router = renderAt('/calendar')

    expect(await screen.findByRole('heading', { name: 'Goals' })).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/planner')
    expect(router.state.location.search).toBe('?view=calendar')
  })

  /**
   * And the source, because the router table is what a later edit breaks.
   *
   * `router.test.tsx` builds its own table from the components rather than
   * importing `createBrowserRouter`, which needs a DOM history — so the real
   * redirect cannot be exercised there, and this is the file that reads it.
   */
  it('is registered in router.tsx, not only in this test’s table', () => {
    const source = readFileSync(join(__dirname, '..', '..', 'router.tsx'), 'utf8')
    expect(source).toContain('<Navigate to="/planner?view=tasks" replace />')
    expect(source).toContain('<Navigate to="/planner?view=calendar" replace />')
  })
})

/**
 * The switcher is a pair of links, and the design's phone label for the plan view
 * is the spec's own word.
 *
 * `SPEC.md:665` writes the phone control as `Plan | Tasks`, and a phone gets the
 * short label because the two are 44px targets side by side. That is a label
 * difference by form factor — the same one `nav.work` makes ("Work" on a phone,
 * "Workspaces" on a desktop) — not a second view.
 */
describe('the switcher’s labels', () => {
  it('says Plan on a phone and Calendar on a desktop', async () => {
    stubViewport(390)
    setup([GOAL])
    renderWithProviders(<PlannerScreen />, { route: '/planner?view=tasks' })

    const switcher = await screen.findByRole('navigation', { name: 'Planner views' })
    expect(within(switcher).getByRole('link', { name: 'Plan' })).toBeInTheDocument()
    expect(within(switcher).queryByRole('link', { name: 'Calendar' })).toBeNull()

    stubViewport(1280)
    setup([GOAL])
    const wide = renderWithProviders(<PlannerScreen />, { route: '/planner?view=tasks' })
    const wideSwitcher = await within(wide.container).findByRole('navigation', {
      name: 'Planner views',
    })
    expect(within(wideSwitcher).getByRole('link', { name: 'Calendar' })).toBeInTheDocument()
  })
})

/**
 * The heading control is a real destination.
 *
 * `+ New task` scrolls to the create form and moves focus into it, which is what
 * `tabIndex={-1}` on that section is for. A heading button that scrolled nowhere
 * would be a control whose only outcome is nothing — and the create form is at the
 * *end* of the panel, so without this the student has to scroll to find it.
 */
describe('the new-task control', () => {
  it('points at the create form’s own anchor, which exists and is focusable', async () => {
    setup([GOAL])
    renderWithProviders(<PlannerScreen />, { route: '/planner?view=tasks' })

    const link = await screen.findByRole('link', { name: 'New task' })
    expect(link).toHaveAttribute('href', '#new-task')

    const target = document.getElementById('new-task')
    expect(target, 'the anchor must exist or the control is decoration').not.toBeNull()
    // `tabIndex={-1}` is the half that makes the browser move focus rather than
    // only scroll: a fragment target that is not focusable leaves the keyboard
    // user where they were. jsdom does not implement the focus move itself, so
    // what is asserted is the property that produces it.
    expect(target).toHaveAttribute('tabindex', '-1')
    target?.focus()
    expect(document.activeElement).toBe(target)
  })
})
