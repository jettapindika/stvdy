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

    expect(await screen.findByText('Lecture 3')).toBeInTheDocument()
    expect(screen.getByText('PDF')).toBeInTheDocument()
    expect(await screen.findByText('Ready')).toBeInTheDocument()
    expect(screen.getByText('7 chunks')).toBeInTheDocument()
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

  it('shows the readable ingestion error when a source fails', async () => {
    sources = [source({ id: 's1', title: 'Broken scan', status: 'failed', error: 'No text layer found' })]
    statuses = {
      s1: {
        id: 's1',
        status: 'failed',
        chunk_count: null,
        error: 'No text layer found, so nothing could be extracted',
        updated_at: '2026-10-11T08:00:00Z',
      },
    }
    serve()
    renderWorkspace()

    expect(await screen.findByText('Failed')).toBeInTheDocument()
    expect(
      await screen.findByText('No text layer found, so nothing could be extracted'),
    ).toBeInTheDocument()
  })

  it('uploads pasted text as multipart, with the fields the contract names', async () => {
    const sent: { form: FormData | null } = { form: null }
    const calls = serve((path, init) => {
      if (path === '/workspaces/w1/sources' && init.method === 'POST') {
        sent.form = init.body as FormData
        const created = source({ id: 's2', kind: 'text', title: 'Ohm law', status: 'pending' })
        sources = [...sources, created]
        return jsonResponse(201, { source: created })
      }
      return undefined
    })

    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByLabelText('Pasted text'))
    await user.type(screen.getByLabelText('Text to add'), 'V equals I times R')
    await user.type(screen.getByLabelText('Title'), 'Ohm law')
    await user.click(screen.getByRole('button', { name: 'Add source' }))

    await vi.waitFor(() => {
      expect(sent.form).not.toBeNull()
    })

    expect(sent.form?.get('kind')).toBe('text')
    expect(sent.form?.get('text')).toBe('V equals I times R')
    expect(sent.form?.get('title')).toBe('Ohm law')
    // No file part for a text source, and no content-type set by hand: the
    // multipart boundary is the browser's.
    expect(sent.form?.get('file')).toBeNull()
    const post = calls.find((call) => call.init.method === 'POST')
    expect(post?.init.headers).not.toHaveProperty('content-type')

    // The new source appears, with its real status from the server.
    expect(await screen.findByText('Ohm law')).toBeInTheDocument()
    expect(await screen.findByText('Waiting to start')).toBeInTheDocument()
  })

  it('will not submit an upload with nothing in it', async () => {
    serve()
    const user = userEvent.setup()
    renderWorkspace()

    const submit = await screen.findByRole('button', { name: 'Add source' })
    expect(submit).toBeDisabled()

    await user.click(screen.getByLabelText('Pasted text'))
    expect(screen.getByRole('button', { name: 'Add source' })).toBeDisabled()

    await user.type(screen.getByLabelText('Text to add'), '   ')
    expect(screen.getByRole('button', { name: 'Add source' })).toBeDisabled()
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

    const row = (await screen.findByText('Lecture 3')).closest('li')
    expect(row).not.toBeNull()
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'Remove' }))

    expect(screen.getByText('Remove this source?')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Remove it' }))

    expect(await screen.findByText('No sources yet')).toBeInTheDocument()
    expect(calls.some((call) => call.init.method === 'DELETE')).toBe(true)
  })
})
