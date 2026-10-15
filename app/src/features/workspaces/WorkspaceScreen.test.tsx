import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'
import { setTimeout as sleep } from 'node:timers/promises'

import { WorkspaceScreen } from './WorkspaceScreen'
import type { Source, SourceStatus } from '@/api/queries/sources'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The sources panel: the list, the upload, and the polling that turns an upload
 * into a finished source.
 *
 * The polling test uses real time on purpose. `refetchInterval` is a timer the
 * query client owns, and a test that fakes the clock can prove the timer fires
 * without proving the query actually stops, which is the half of the behaviour
 * that matters (an endless poll is a request every second and a half forever).
 * Waiting three intervals of real time costs a few seconds and asserts the thing
 * the code is for.
 */

const WORKSPACE = {
  id: 'w1',
  name: 'Data Structures',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  source_count: 1,
  course_code: 'CS 401',
  color: 0,
  exam_date: '2026-10-26',
}

function source(over: Partial<Source> & { id: string }): Source {
  return {
    workspace_id: 'w1',
    kind: 'pdf',
    title: 'Lecture 3',
    url: 'https://objects.example/s1',
    file_size: 1024,
    mime: 'application/pdf',
    status: 'ready',
    error: null,
    created_at: '2026-10-11T08:00:00Z',
    ...over,
  }
}

let sources: Source[]
let statuses: Record<string, SourceStatus>

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
  sources = []
  statuses = {}
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function serve(extra?: (path: string, init: RequestInit) => Response | undefined) {
  return mockFetch((url, init) => {
    const path = new URL(url).pathname
    const handled = extra?.(path, init)
    if (handled !== undefined) return handled

    if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
    if (path === '/workspaces/w1/sources' && (init.method ?? 'GET') === 'GET') {
      return jsonResponse(200, { sources })
    }
    const statusMatch = /^\/sources\/([^/]+)\/status$/.exec(path)
    if (statusMatch?.[1] !== undefined) {
      const status = statuses[statusMatch[1]]
      if (status === undefined) return jsonResponse(404, { code: 'not_found' })
      return jsonResponse(200, status)
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

/**
 * Real waiting, because the poll interval is a timer the query client owns and a
 * faked clock would prove the timer fires without proving it ever stops.
 */
function delay(ms: number): Promise<void> {
  return sleep(ms)
}

function renderWorkspace() {
  return renderWithProviders(
    <Routes>
      <Route path="/workspaces/:workspaceId" element={<WorkspaceScreen />} />
    </Routes>,
    { route: '/workspaces/w1' },
  )
}

/**
 * The sources panel, as an element.
 *
 * Needed wherever a source's title is asserted on: a ready source is listed here
 * and again in the chat panel's picker, so a bare document-wide text query finds
 * two of them. Both are real, so the query is scoped rather than the markup
 * changed.
 */
async function sourcesPanel(): Promise<HTMLElement> {
  return await screen.findByRole('tabpanel', { name: 'Sources' })
}

describe('the workspace heading', () => {
  it('names the course, the source count and the exam date from the workspace row', async () => {
    serve()
    renderWorkspace()

    expect(await screen.findByRole('heading', { name: 'Data Structures' })).toBeInTheDocument()
    // One mono line of the facts the server holds, joined: the code, the count and
    // the exam date, in that order.
    expect(await screen.findByText('CS 401 · 1 source · exam 26 Oct 2026')).toBeInTheDocument()
    // And the way back to the list, which is the design's breadcrumb.
    expect(screen.getByRole('link', { name: 'Workspaces' })).toHaveAttribute(
      'href',
      '/workspaces',
    )
  })

  it('omits each fact the workspace does not have rather than filling it in', async () => {
    // `course_code` and `exam_date` are nullable, and a heading that printed a
    // placeholder would be stating something no row holds. The source count is
    // always real, so it is the one fact that survives.
    const bare = {
      id: 'w1',
      name: 'Data Structures',
      created_at: '2026-10-11T08:00:00Z',
      updated_at: '2026-10-11T08:00:00Z',
      source_count: 3,
      course_code: null,
      color: 2,
      exam_date: null,
    }
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: bare })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/workspaces/w1/messages') return jsonResponse(200, { messages: [] })
      if (path === '/workspaces/w1/decks') return jsonResponse(200, { decks: [] })
      if (path === '/workspaces/w1/reports') return jsonResponse(200, { reports: [] })
      return jsonResponse(404, { code: 'not_found', message: 'no such route', min_plan: null, limit: null, used: null })
    })
    renderWorkspace()

    expect(await screen.findByRole('heading', { name: 'Data Structures' })).toBeInTheDocument()
    expect(await screen.findByText('3 sources')).toBeInTheDocument()
    // Neither the code nor the date, anywhere on the screen: the heading line is the
    // only place either could appear.
    expect(screen.queryByText(/CS 401/)).not.toBeInTheDocument()
    expect(screen.queryByText(/26 Oct 2026/)).not.toBeInTheDocument()
  })
})

describe('the workspace panels', () => {
  it('orders the phone strip and the desktop columns differently, as the design draws them', async () => {
    // The two orders are not the same list, and that is the whole point of the test:
    // the segmented control reads `Chat | Sources | Studio` in every mobile frame,
    // while the desktop is a source rail, then the chat, then the reference panel.
    // One array for both puts the chat in the left-hand column of a 1440px window.
    serve()
    renderWorkspace()

    const strip = await screen.findByRole('tablist', { name: 'Workspace panels' })
    const tabs = within(strip).getAllByRole('tab')
    expect(tabs.map((tab) => tab.textContent)).toEqual(['Chat', 'Sources', 'Studio'])

    // The tabpanels are the desktop grid's columns, so their DOM order is the
    // left-to-right order at `lg`. The Studio sub-panels are tabpanels too, hence the
    // id prefix rather than every `role="tabpanel"` on the screen.
    const panels = [...document.querySelectorAll('[role="tabpanel"]')].filter((panel) =>
      /^panel-/.test(panel.id),
    )
    expect(panels.map((panel) => panel.id)).toEqual([
      'panel-sources',
      'panel-chat',
      'panel-studio',
    ])
  })

  it('offers the source to open only when it is ready and has somewhere to go', async () => {
    // The design's `Open PDF · page 12` action. A source that is still being indexed
    // has no page for a viewer to open, and one that was uploaded has no URL at all —
    // only the two fetched kinds carry one — so the action is rendered from those two
    // facts rather than from the kind alone.
    sources = [
      source({ id: 's1', title: 'Ready and linked', status: 'ready', url: 'https://example.test/a' }),
      source({ id: 's2', title: 'Still processing', status: 'processing', url: 'https://example.test/b' }),
      source({ id: 's3', title: 'Uploaded, no URL', status: 'ready', url: null }),
    ]
    serve()
    renderWorkspace()

    const panel = await sourcesPanel()
    await within(panel).findByText('Ready and linked')
    const rows = within(panel).getAllByRole('listitem')
    const openIn = (title: string) => {
      const row = rows.find((item) => item.textContent?.includes(title))
      expect(row, `no row for ${title}`).toBeDefined()
      return within(row as HTMLElement).queryByRole('link', { name: 'Open source' })
    }

    expect(openIn('Ready and linked')).toHaveAttribute('href', 'https://example.test/a')
    expect(openIn('Still processing')).toBeNull()
    expect(openIn('Uploaded, no URL')).toBeNull()
  })
})

describe('the sources panel', () => {
  it('lists the sources with their real status and chunk count', async () => {
    sources = [source({ id: 's1', title: 'Lecture 3' })]
    statuses = {
      s1: {
        id: 's1',
        status: 'ready',
        chunk_count: 7,
        error: null,
        updated_at: '2026-10-11T08:00:00Z',
      },
    }
    serve()
    renderWorkspace()

    // Scoped to the sources panel: a ready source's title also appears in the
    // chat panel's source picker, which is the same fact shown twice on purpose.
    const panel = await sourcesPanel()
    expect(await within(panel).findByText('Lecture 3')).toBeInTheDocument()
    expect(within(panel).getByText('PDF')).toBeInTheDocument()
    expect(await within(panel).findByText('Ready')).toBeInTheDocument()
    expect(within(panel).getByText('7 chunks')).toBeInTheDocument()
  })

  it('polls a processing source until it is ready, then stops', async () => {
    sources = [source({ id: 's1', status: 'processing' })]
    let statusCalls = 0
    const calls = serve((path) => {
      if (path === '/sources/s1/status') {
        statusCalls += 1
        // Still working on the first answer, finished on the second.
        return jsonResponse(200, {
          id: 's1',
          status: statusCalls === 1 ? 'processing' : 'ready',
          chunk_count: statusCalls === 1 ? null : 12,
          error: null,
        })
      }
      return undefined
    })

    renderWorkspace()

    expect(await screen.findByText('Processing')).toBeInTheDocument()

    // The poll fires, the source becomes ready, and the list is refetched with
    // the server's new answer.
    await vi.waitFor(
      () => {
        expect(screen.getByText('Ready')).toBeInTheDocument()
      },
      { timeout: 4000 },
    )
    expect(screen.getByText('12 chunks')).toBeInTheDocument()

    // And then it stops: a settled source is not asked again.
    const settledCalls = statusCalls
    await delay(3200)
    expect(statusCalls).toBe(settledCalls)
    expect(calls.filter((call) => call.url.endsWith('/sources/s1/status'))).toHaveLength(
      settledCalls,
    )
  }, 15000)

  it('shows the localized ingestion error when a source fails', async () => {
    sources = [source({ id: 's1', title: 'Broken scan', status: 'failed', error: 'no extractable text: the document may be a scan' })]
    statuses = {
      s1: {
        id: 's1',
        status: 'failed',
        chunk_count: null,
        error: 'no extractable text: the document may be a scan',
        updated_at: '2026-10-11T08:00:00Z',
      },
    }
    serve()
    renderWorkspace()

    expect(await screen.findByText('Failed')).toBeInTheDocument()
    expect(
      await screen.findByText('This PDF looks scanned. Text recognition is not available yet.'),
    ).toBeInTheDocument()
  })

  it('infers pptx kind from the file extension when adding a source', async () => {
    const sent: { form: FormData | null } = { form: null }
    serve((path, init) => {
      if (path === '/workspaces/w1/sources' && init.method === 'POST') {
        sent.form = init.body as FormData
        const created = source({ id: 's2', kind: 'pptx', title: 'Slides', status: 'pending' })
        sources = [...sources, created]
        return jsonResponse(201, { source: created })
      }
      return undefined
    })

    const user = userEvent.setup()
    renderWorkspace()

    const panel = await sourcesPanel()

    // The hidden file input is labelled by the same text as the button.
    const fileInput = within(panel).getByLabelText('+ Add Source') as HTMLInputElement
    const file = new File(['slides'], 'lecture.pptx', { type: 'application/vnd.openxmlformats-officedocument.presentationml.presentation' })
    await user.upload(fileInput, file)

    await vi.waitFor(() => {
      expect(sent.form).not.toBeNull()
    })
    expect(sent.form?.get('kind')).toBe('pptx')
  })



  it('removes a source only after the confirmation', async () => {
    sources = [source({ id: 's1', title: 'Lecture 3' })]
    statuses = {
      s1: {
        id: 's1',
        status: 'ready',
        chunk_count: 7,
        error: null,
        updated_at: '2026-10-11T08:00:00Z',
      },
    }
    const calls = serve((path, init) => {
      if (path === '/sources/s1' && init.method === 'DELETE') {
        sources = []
        return jsonResponse(204)
      }
      return undefined
    })

    const user = userEvent.setup()
    renderWorkspace()

    const panel = await sourcesPanel()
    const row = (await within(panel).findByText('Lecture 3')).closest('li')
    expect(row).not.toBeNull()
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'Remove' }))

    expect(within(panel).getByText('Remove this source?')).toBeInTheDocument()
    await user.click(within(panel).getByRole('button', { name: 'Remove it' }))

    expect(await within(panel).findByText('No sources yet')).toBeInTheDocument()
    expect(calls.some((call) => call.init.method === 'DELETE')).toBe(true)
  })
})
