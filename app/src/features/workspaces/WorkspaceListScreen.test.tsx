import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { WorkspaceListScreen } from './WorkspaceListScreen'
import type { Workspace } from '@/api/queries/workspaces'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The workspace list against a scripted server.
 *
 * The mock is a tiny in-memory version of the routes this screen uses, rather
 * than a per-test assertion on a fetch stub, because the behaviour worth testing
 * is a sequence: the screen reads, the user acts, the list is refetched and shows
 * the server's answer. A stub that returns one canned response cannot express
 * that.
 *
 * The limit case is here rather than in a component test of the notice, because
 * the thing that can go wrong is the *wiring*: a prompt that renders correctly
 * and is never shown when a limit is hit is the defect the spec's "inline upgrade
 * prompt exactly where a limit is hit" is about.
 *
 * **The create form is behind the header's one primary action.** The redesign's
 * page header carries a single `+ New workspace` button and the form is what it
 * opens, so every test that creates one clicks it first. That is the interaction,
 * not an obstacle: the alternative — a form permanently open above the list —
 * costs the first screenful to a control most sessions do not use.
 */

function workspace(over: Partial<Workspace> & { id: string; name: string }): Workspace {
  return {
    created_at: '2026-10-11T08:00:00Z',
    updated_at: '2026-10-11T08:00:00Z',
    source_count: 0,
    // The course fields the redesign added. Defaulted here rather than in every
    // call site: a workspace with no code and no exam date is the ordinary case,
    // and a test that has to state them is a test stating the wrong thing.
    course_code: null,
    color: 0,
    exam_date: null,
    ...over,
  }
}

let workspaces: Workspace[] = []
let refuseWithLimit = false
let refuseWithLocked = false
/** The account-wide due-card count the review band reads. `null` means "not served". */
let cardsDue: number | null = 12

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
  refuseWithLimit = false
  refuseWithLocked = false
  cardsDue = 12
  workspaces = [
    workspace({ id: 'w1', name: 'Data Structures', source_count: 2 }),
    workspace({ id: 'w2', name: 'Linear Algebra', source_count: 1 }),
  ]
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function serve() {
  return mockFetch((url, init) => {
    const method = init.method ?? 'GET'
    const path = new URL(url).pathname

    if (path === '/workspaces' && method === 'GET') return jsonResponse(200, { workspaces })

    if (path === '/dashboard' && method === 'GET' && cardsDue !== null) {
      return jsonResponse(200, {
        from: '2026-10-12',
        to: '2026-10-12',
        time_per_course: [],
        review_activity: [],
        totals: {
          minutes_studied: 0,
          reviews: 0,
          cards_due: cardsDue,
          quizzes_taken: 0,
        },
      })
    }

    if (path === '/workspaces' && method === 'POST') {
      if (refuseWithLocked) {
        return jsonResponse(403, {
          code: 'feature_locked',
          message: 'workspaces are not available on this device',
          min_plan: null,
          limit: null,
          used: null,
        })
      }
      if (refuseWithLimit) {
        return jsonResponse(422, {
          code: 'limit_reached',
          message: 'workspaces limit reached',
          min_plan: 'pro',
          limit: 2,
          used: 2,
        })
      }
      const body = JSON.parse(String(init.body)) as {
        name: string
        course_code: string | null
        exam_date: string | null
      }
      const created = workspace({
        id: 'w3',
        name: body.name,
        course_code: body.course_code,
        exam_date: body.exam_date,
      })
      workspaces = [...workspaces, created]
      return jsonResponse(201, { workspace: created })
    }

    const match = /^\/workspaces\/([^/]+)$/.exec(path)
    if (match?.[1] !== undefined) {
      const id = match[1]
      if (method === 'PATCH') {
        const body = JSON.parse(String(init.body)) as {
          name: string
          course_code: string | null
          exam_date: string | null
        }
        const renamed = workspaces.find((entry) => entry.id === id)
        if (renamed === undefined) return jsonResponse(404, { code: 'not_found' })
        const next = { ...renamed, ...body }
        workspaces = workspaces.map((entry) => (entry.id === id ? next : entry))
        return jsonResponse(200, { workspace: next })
      }
      if (method === 'DELETE') {
        workspaces = workspaces.filter((entry) => entry.id !== id)
        return jsonResponse(204)
      }
    }

    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
}

describe('the workspace list', () => {
  it('shows the workspaces the server returned, with their real source counts', async () => {
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByRole('link', { name: 'Data Structures' })).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Linear Algebra' })).toBeInTheDocument()
    expect(screen.getByText('2 sources')).toBeInTheDocument()
    // Singular, because the count is one. A count that reads "1 sources" is the
    // kind of thing that makes a real number look like placeholder data.
    expect(screen.getByText('1 source')).toBeInTheDocument()
    expect(screen.queryByText('No workspaces yet')).not.toBeInTheDocument()
  })

  it('shows the course code and the exam date only when the workspace has them', async () => {
    // The honesty rule at the card: `course_code` and `exam_date` are nullable, and
    // a card that printed a dash or a placeholder would be stating a fact no row
    // holds. One workspace has both, the other neither.
    workspaces = [
      workspace({
        id: 'w1',
        name: 'Cell biology',
        course_code: 'BIO 201',
        exam_date: '2026-11-05',
      }),
      workspace({ id: 'w2', name: 'Statistics' }),
    ]
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByText('BIO 201')).toBeInTheDocument()
    // Formatted from the date the student set, at UTC midnight so the reader's zone
    // cannot shift it a day.
    expect(screen.getByText('Next exam · 05 Nov 2026')).toBeInTheDocument()

    const statistics = (await screen.findByRole('link', { name: 'Statistics' })).closest('li')
    expect(statistics).not.toBeNull()
    expect(within(statistics as HTMLElement).queryByText(/Next exam/)).not.toBeInTheDocument()
    // And no code line at all, rather than an empty one.
    expect(within(statistics as HTMLElement).queryByText('BIO 201')).not.toBeInTheDocument()
  })

  it('says what a workspace is for when there are none', async () => {
    workspaces = []
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByText('No workspaces yet')).toBeInTheDocument()
    // The empty state names the cause and the next action, not just the absence.
    expect(screen.getByText(/Create one per course/)).toBeInTheDocument()
  })

  it('creates a workspace with its course code and exam date', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    // The create form is behind the header's one primary action.
    await user.click(screen.getByRole('button', { name: 'New workspace' }))
    await user.type(await screen.findByLabelText('New workspace'), 'Algorithms')
    await user.type(screen.getByLabelText('Course code'), 'CS 401')
    await user.type(screen.getByLabelText('Next exam'), '2026-11-05')
    await user.click(screen.getByRole('button', { name: 'Create' }))

    expect(await screen.findByRole('link', { name: 'Algorithms' })).toBeInTheDocument()
    expect(await screen.findByText('CS 401')).toBeInTheDocument()

    const posted = calls.find((call) => call.init.method === 'POST')
    expect(posted).toBeDefined()
    expect(JSON.parse(String(posted?.init.body))).toEqual({
      name: 'Algorithms',
      course_code: 'CS 401',
      exam_date: '2026-11-05',
    })
  })

  it('clears the course fields by sending null, not by omitting them', async () => {
    // `PATCH` distinguishes an absent field ("leave it alone") from an explicit
    // `null` ("clear it"), so a form that omitted an emptied field could never
    // remove a code or a date once it had one. The body is the contract here.
    workspaces = [
      workspace({
        id: 'w1',
        name: 'Cell biology',
        course_code: 'BIO 201',
        exam_date: '2026-11-05',
      }),
    ]
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    const card = (await screen.findByRole('link', { name: 'Cell biology' })).closest('li')
    await user.click(within(card as HTMLElement).getByRole('button', { name: 'More' }))
    await user.click(within(card as HTMLElement).getByRole('button', { name: 'Rename' }))
    await user.clear(screen.getByLabelText('Course code'))
    await user.clear(screen.getByLabelText('Next exam'))
    await user.click(screen.getByRole('button', { name: 'Save' }))

    await vi.waitFor(() => {
      expect(calls.some((call) => call.init.method === 'PATCH')).toBe(true)
    })
    const patched = calls.find((call) => call.init.method === 'PATCH')
    expect(JSON.parse(String(patched?.init.body))).toEqual({
      name: 'Cell biology',
      course_code: null,
      exam_date: null,
    })
    // And the card stops claiming them.
    await vi.waitFor(() => {
      expect(screen.queryByText('Next exam · 05 Nov 2026')).not.toBeInTheDocument()
    })
  })

  it('shows the plan limit, with the numbers, when the server refuses the create', async () => {
    refuseWithLimit = true
    serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    // The create form is behind the header's one primary action.
    await user.click(screen.getByRole('button', { name: 'New workspace' }))
    await user.type(await screen.findByLabelText('New workspace'), 'Third course')
    await user.click(screen.getByRole('button', { name: 'Create' }))

    // The prompt names the ceiling and the plan that lifts it, at the control
    // that hit it. "Upgrade to continue" would tell the user nothing.
    expect(await screen.findByText('Plan limit reached')).toBeInTheDocument()
    expect(screen.getByText(/The Pro plan has reached its limit for workspaces\./)).toBeInTheDocument()
    expect(screen.getByText(/The limit is 2 and you have 2\./)).toBeInTheDocument()
    expect(screen.getByText(/Pro lifts this limit\./)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'See plans' })).toBeInTheDocument()
  })

  it('renames a workspace in place', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    const row = (await screen.findByRole('link', { name: 'Data Structures' })).closest('li')
    expect(row).not.toBeNull()
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'More' }))
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'Rename' }))

    const field = screen.getByLabelText('Workspace name')
    await user.clear(field)
    await user.type(field, 'Algorithms and data structures')
    await user.click(screen.getByRole('button', { name: 'Save' }))

    expect(
      await screen.findByRole('link', { name: 'Algorithms and data structures' }),
    ).toBeInTheDocument()
    const patched = calls.find((call) => call.init.method === 'PATCH')
    expect(patched?.url.endsWith('/workspaces/w1')).toBe(true)
  })

  it('sorts by name when the student asks for it', async () => {
    // The sort is real: the two names are in the opposite order to their
    // `updated_at`, so an implementation that ignored the control would leave the
    // cards where they were and pass a weaker assertion.
    workspaces = [
      workspace({ id: 'w1', name: 'Statistics', updated_at: '2026-10-11T09:00:00Z' }),
      workspace({ id: 'w2', name: 'Cell biology', updated_at: '2026-10-11T08:00:00Z' }),
    ]
    serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    await screen.findByRole('link', { name: 'Statistics' })
    const cardNames = (): (string | null)[] =>
      screen
        .getAllByRole('listitem')
        .map((item) => item.querySelector('h3')?.textContent ?? null)
    expect(cardNames()).toEqual(['Statistics', 'Cell biology'])

    await user.selectOptions(screen.getByLabelText('Sort'), 'name')
    expect(cardNames()).toEqual(['Cell biology', 'Statistics'])
  })

  it('shows the account due count in the review band', async () => {
    cardsDue = 44
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByText('44 flashcards to revisit')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Review due cards' })).toBeInTheDocument()
  })

  it('says nothing is due rather than offering an empty queue', async () => {
    // Zero is a real answer and reads as one; the action that would open an empty
    // queue is not offered.
    cardsDue = 0
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByText('Nothing is due today')).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Review due cards' })).not.toBeInTheDocument()
  })

  it('omits the review band when the due count cannot be read', async () => {
    // The band's one figure is the reason it exists, so a failed read renders no
    // band at all rather than a heading over nothing. The workspaces below are
    // complete without it.
    cardsDue = null
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByRole('link', { name: 'Data Structures' })).toBeInTheDocument()
    expect(screen.queryByText(/flashcards to revisit/)).not.toBeInTheDocument()
    expect(screen.queryByText('Nothing is due today')).not.toBeInTheDocument()
  })

  it('asks before deleting, and says what else goes', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    const row = (await screen.findByRole('link', { name: 'Data Structures' })).closest('li')
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'More' }))
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'Delete' }))

    expect(screen.getByText('Delete Data Structures?')).toBeInTheDocument()
    expect(screen.getByText('2 sources go with it. This cannot be undone.')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Delete workspace' }))

    expect(await screen.findByRole('link', { name: 'Linear Algebra' })).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Data Structures' })).not.toBeInTheDocument()
    expect(calls.some((call) => call.init.method === 'DELETE')).toBe(true)
  })

  it('leaves the workspace alone when the confirmation is declined', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    const row = (await screen.findByRole('link', { name: 'Data Structures' })).closest('li')
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'More' }))
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'Delete' }))
    await user.click(screen.getByRole('button', { name: 'Keep it' }))

    expect(screen.getByRole('link', { name: 'Data Structures' })).toBeInTheDocument()
    expect(calls.some((call) => call.init.method === 'DELETE')).toBe(false)
  })

  it('offers no upgrade route when the server says a feature is locked', async () => {
    // `feature_locked` and `limit_reached` are not the same refusal (A143): the
    // first means "locked, and no upgrade may be offered to this client", which
    // also covers a device that is over its plan's device count. A "See plans"
    // link there would send the user to a page that cannot fix their problem, so
    // the notice states the fact and stops.
    refuseWithLocked = true
    serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    // The create form is behind the header's one primary action.
    await user.click(screen.getByRole('button', { name: 'New workspace' }))
    await user.type(await screen.findByLabelText('New workspace'), 'Third course')
    await user.click(screen.getByRole('button', { name: 'Create' }))

    expect(
      await screen.findByText('Not available on this plan or device'),
    ).toBeInTheDocument()
    expect(screen.getByText(/not available on your current plan or device/)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'See plans' })).not.toBeInTheDocument()
  })
})
