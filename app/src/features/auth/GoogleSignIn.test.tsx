import { render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryRouter, RouterProvider } from 'react-router'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'

import { DeepLinkHandler } from './DeepLinkHandler'
import { GoogleCallbackRoute } from './GoogleCallbackScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { setAccessToken } from '@/lib/tokens'
import type { FakeDeepLinks, FakeSecureStore } from '@/test/harness'

/**
 * Google sign-in, delivered as a deep link.
 *
 * The server's OAuth callback answers with a single-use ticket and never with
 * tokens, so the SPA has to spend it exactly once and store the session the way
 * the password path does. Two things matter here and neither is visible from a
 * screen test: the ticket is exchanged (the defect this covers is that nothing
 * called `/auth/google/exchange` at all, so a Google sign-in left the user signed
 * out), and an exchange failure is shown rather than swallowed into a blank
 * screen.
 */

const SESSION = {
  user: {
    id: 'u1',
    email: 'student@example.com',
    email_verified: true,
    email_verified_at: '2026-10-11T09:00:00Z',
    plan_id: 'free',
    timezone: 'Asia/Jakarta',
    student_verified: false,
    created_at: '2026-10-11T08:00:00Z',
  },
  access_token: 'access-1',
  refresh_token: 'refresh-1',
  expires_at: '2026-10-11T10:00:00Z',
  device_id: 'device-1',
}

let store: FakeSecureStore & { deepLinks: FakeDeepLinks }

beforeEach(() => {
  setAccessToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function renderApp(route = '/') {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const router = createMemoryRouter(
    [
      {
        element: <DeepLinkHandler />,
        children: [
          { path: '/', element: <p>Home content</p> },
          { path: '/workspaces', element: <p>Workspace list</p> },
          { path: '/auth/callback', element: <GoogleCallbackRoute /> },
        ],
      },
    ],
    { initialEntries: [route] },
  )
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
}

describe('a Google sign-in delivered as a deep link', () => {
  it('exchanges the ticket once and lands signed in', async () => {
    store = installFakePlatform('web', { deepLinks: true })
    const calls = mockFetch((url) => {
      if (url.endsWith('/auth/google/exchange')) return jsonResponse(200, SESSION)
      return jsonResponse(200, {})
    })

    renderApp()
    // The subscription is registered in an effect, so the intent is delivered
    // after the handler exists.
    await vi.waitFor(() => {
      expect(store.deepLinks.subscribers).toBeGreaterThan(0)
    })
    store.deepLinks.emit({ action: 'auth_ticket', payload: { ticket: 'ticket-1' } })

    expect(await screen.findByText('Workspace list')).toBeInTheDocument()

    const exchanges = calls.filter((call) => call.url.endsWith('/auth/google/exchange'))
    expect(exchanges).toHaveLength(1)
    expect(JSON.parse(String(exchanges[0]?.init.body))).toMatchObject({
      ticket: 'ticket-1',
      device: { platform: 'web' },
    })
    // The session is stored exactly as the password path stores it.
    expect(store.values.get('stvdy.refresh_token')).toBe('refresh-1')
    expect(store.values.get('stvdy.device_id')).toBe('device-1')
  })

  it('shows an error when the exchange is refused, not a blank screen', async () => {
    store = installFakePlatform('web', { deepLinks: true })
    mockFetch(() =>
      jsonResponse(401, {
        code: 'unauthorized',
        message: 'ticket expired',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )

    renderApp()
    await vi.waitFor(() => {
      expect(store.deepLinks.subscribers).toBeGreaterThan(0)
    })
    store.deepLinks.emit({ action: 'auth_ticket', payload: { ticket: 'stale' } })

    expect(await screen.findByText('That link no longer works')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Back to sign in' })).toBeInTheDocument()
    expect(store.values.size).toBe(0)
  })

  it('completes a cold start where the shell launched with the ticket', async () => {
    store = installFakePlatform('web', {
      deepLinks: true,
      current: { action: 'auth_ticket', payload: { ticket: 'launch-ticket' } },
    })
    const calls = mockFetch((url) => {
      if (url.endsWith('/auth/google/exchange')) return jsonResponse(200, SESSION)
      return jsonResponse(200, {})
    })

    renderApp()

    expect(await screen.findByText('Workspace list')).toBeInTheDocument()
    expect(calls.filter((call) => call.url.endsWith('/auth/google/exchange'))).toHaveLength(1)
  })

  it('spends the ticket the web build returns to /auth/callback with', async () => {
    store = installFakePlatform('web')
    const calls = mockFetch((url) => {
      if (url.endsWith('/auth/google/exchange')) return jsonResponse(200, SESSION)
      return jsonResponse(200, {})
    })

    renderApp('/auth/callback?ticket=web-ticket')

    expect(await screen.findByText('Workspace list')).toBeInTheDocument()
    const exchanges = calls.filter((call) => call.url.endsWith('/auth/google/exchange'))
    expect(exchanges).toHaveLength(1)
    expect(JSON.parse(String(exchanges[0]?.init.body))).toMatchObject({ ticket: 'web-ticket' })
  })

  it('explains a callback URL with no ticket instead of hanging on a loader', async () => {
    store = installFakePlatform('web')
    mockFetch(() => jsonResponse(200, {}))

    renderApp('/auth/callback')

    expect(await screen.findByText('This sign-in link is incomplete')).toBeInTheDocument()
  })
})
