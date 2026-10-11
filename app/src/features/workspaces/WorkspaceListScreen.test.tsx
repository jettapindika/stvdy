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
 * The mock is a tiny in-memory version of the four routes this screen uses,
 * rather than a per-test assertion on a fetch stub, because the behaviour worth
 * testing is a sequence: the screen reads, the user acts, the list is refetched
 * and shows the server's answer. A stub that returns one canned response cannot
 * express that.
 *
 * The limit case is here rather than in a component test of the notice, because
 * the thing that can go wrong is the *wiring*: a prompt that renders correctly
 * and is never shown when a limit is hit is the defect the spec's "inline upgrade
 * prompt exactly where a limit is hit" is about.
 */

function workspace(over: Partial<Workspace> & { id: string; name: string }): Workspace {
  return {
    created_at: '2026-10-11T08:00:00Z',
    updated_at: '2026-10-11T08:00:00Z',
    source_count: 0,
    ...over,
  }
}

let workspaces: Workspace[] = []
let refuseWithLimit = false

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
  refuseWithLimit = false
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

    if (path === '/workspaces' && method === 'POST') {
      if (refuseWithLimit) {
        return jsonResponse(422, {
          code: 'limit_reached',
          message: 'workspaces limit reached',
          min_plan: 'pro',
          limit: 2,
          used: 2,
        })
      }
      const body = JSON.parse(String(init.body)) as { name: string }
      const created = workspace({ id: 'w3', name: body.name })
      workspaces = [...workspaces, created]
      return jsonResponse(201, { workspace: created })
    }

    const match = /^\/workspaces\/([^/]+)$/.exec(path)
    if (match?.[1] !== undefined) {
      const id = match[1]
      if (method === 'PATCH') {
        const body = JSON.parse(String(init.body)) as { name: string }
        const renamed = workspaces.find((entry) => entry.id === id)
        if (renamed === undefined) return jsonResponse(404, { code: 'not_found' })
        workspaces = workspaces.map((entry) =>
          entry.id === id ? { ...entry, name: body.name } : entry,
        )
        return jsonResponse(200, { workspace: { ...renamed, name: body.name } })
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

  it('says what a workspace is for when there are none', async () => {
    workspaces = []
    serve()
    renderWithProviders(<WorkspaceListScreen />)

    expect(await screen.findByText('No workspaces yet')).toBeInTheDocument()
    // The empty state names the cause and the next action, not just the absence.
    expect(screen.getByText(/Create one per course/)).toBeInTheDocument()
  })

  it('creates a workspace and shows it in the refetched list', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    await user.type(await screen.findByLabelText('New workspace'), 'Algorithms')
    await user.click(screen.getByRole('button', { name: 'Create' }))

    expect(await screen.findByRole('link', { name: 'Algorithms' })).toBeInTheDocument()

    const posted = calls.find((call) => call.init.method === 'POST')
    expect(posted).toBeDefined()
    expect(JSON.parse(String(posted?.init.body))).toEqual({ name: 'Algorithms' })
  })

  it('shows the plan limit, with the numbers, when the server refuses the create', async () => {
    refuseWithLimit = true
    serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

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

  it('asks before deleting, and says what else goes', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderWithProviders(<WorkspaceListScreen />)

    const row = (await screen.findByRole('link', { name: 'Data Structures' })).closest('li')
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
    await user.click(within(row as HTMLElement).getByRole('button', { name: 'Delete' }))
    await user.click(screen.getByRole('button', { name: 'Keep it' }))

    expect(screen.getByRole('link', { name: 'Data Structures' })).toBeInTheDocument()
    expect(calls.some((call) => call.init.method === 'DELETE')).toBe(false)
  })
})
