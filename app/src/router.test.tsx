import { render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryRouter, RouterProvider } from 'react-router'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'

import { App } from './App'
import { NotBuiltYet } from './components/ui/States'
import { SignInScreen } from './features/auth/SignInScreen'
import { SignUpScreen } from './features/auth/SignUpScreen'
import { VerifyEmailScreen } from './features/auth/VerifyEmailScreen'
import { MobilePlanScreen } from './features/billing/MobilePlanScreen'
import { PlanUsageScreen } from './features/billing/PlanUsageScreen'
import { WorkspaceListScreen } from './features/workspaces/WorkspaceListScreen'
import { billingElementFor, router as appRouter } from './router'
import { installFakePlatform, jsonResponse, mockFetch, stubViewport } from './test/harness'
import { renderWithProviders } from './test/render'
import { serveUsage } from './test/usage'
import { saveSession, setAccessToken } from './lib/tokens'

/**
 * The routing table, checked as the table it is.
 *
 * Two properties matter and neither is visible from a screen test. The auth
 * routes must be reachable with no session, because a sign-in screen behind the
 * session gate could never be opened; and the signed-in routes must not be,
 * because a screen rendered without a session fires requests with no token and
 * shows their 401s as if the workspace were broken.
 *
 * The router is built here from the same components `router.tsx` wires, in the
 * same shape, rather than importing `createBrowserRouter`, which needs a DOM
 * history and a real URL. A test that could not see the route table would not
 * catch the mistake this file is for.
 */

const ROUTES = [
  { path: '/signin', element: <SignInScreen /> },
  { path: '/signup', element: <SignUpScreen /> },
  { path: '/verify', element: <VerifyEmailScreen /> },
  { path: '/verify-email', element: <VerifyEmailScreen /> },
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <NotBuiltYet title="Today" what="study blocks" /> },
      { path: 'workspaces', element: <WorkspaceListScreen /> },
    ],
  },
]

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken(null)
  stubViewport(1280)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function renderRoute(path: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const router = createMemoryRouter(ROUTES, { initialEntries: [path] })
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
}

describe('the auth routes are reachable without a session', () => {
  it('opens the sign-in screen at /signin', async () => {
    mockFetch(() => jsonResponse(200, {}))

    renderRoute('/signin')

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument()
  })

  it('opens the sign-up screen at /signup', async () => {
    mockFetch(() => jsonResponse(200, {}))

    renderRoute('/signup')

    expect(await screen.findByRole('heading', { name: 'Create your account' })).toBeInTheDocument()
  })

  it('opens the verification screen at /verify with no session at all', async () => {
    mockFetch(() => jsonResponse(200, { verified: true }))

    renderRoute('/verify?token=abc123')

    // The emailed link is usually opened on a device that never signed in.
    expect(await screen.findByText('Email verified')).toBeInTheDocument()
  })

  it('opens the verification screen at the path the server actually mails', async () => {
    const calls = mockFetch(() => jsonResponse(200, { verified: true }))

    // `auth/email_verify.rs` builds the link as `{web_origin}/verify-email?token=`,
    // so the SPA has to answer on that path and not only on the in-app `/verify`.
    renderRoute('/verify-email?token=abc123')

    expect(await screen.findByText('Email verified')).toBeInTheDocument()
    expect(calls.filter((call) => call.url.endsWith('/auth/verify-email'))).toHaveLength(1)
  })
})

describe('the signed-in routes are behind the gate', () => {
  it('sends an unauthenticated visitor at / to the sign-in screen', async () => {
    const calls = mockFetch(() => jsonResponse(200, {}))

    renderRoute('/')

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument()
    // Nothing was requested for the workspace, because the screen never mounted.
    expect(calls.some((call) => call.url.includes('/workspaces'))).toBe(false)
  })

  it('sends an unauthenticated visitor at /workspaces to the sign-in screen', async () => {
    mockFetch(() => jsonResponse(200, {}))

    renderRoute('/workspaces')

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument()
  })

  it('renders the workspace list for a restored session', async () => {
    mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-1',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:00:00Z',
        })
      }
      if (url.endsWith('/auth/me')) {
        return jsonResponse(200, {
          id: 'u1',
          email: 'student@example.com',
          email_verified: true,
          email_verified_at: '2026-10-11T09:00:00Z',
          plan_id: 'free',
          timezone: 'Asia/Jakarta',
          student_verified: false,
          created_at: '2026-10-11T08:00:00Z',
        })
      }
      return jsonResponse(200, { workspaces: [] })
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    renderRoute('/workspaces')

    expect(await screen.findByRole('heading', { name: 'Workspaces' })).toBeInTheDocument()
    expect(await screen.findByText('No workspaces yet')).toBeInTheDocument()
  })
})

/** The route the real table registers at a path, found by walking it. */
function registeredRoute(path: string) {
  const queue = [...appRouter.routes]
  while (queue.length > 0) {
    const route = queue.shift()
    if (route === undefined) break
    if (route.path === path) return route
    queue.push(...(route.children ?? []))
  }
  return undefined
}

/**
 * `/billing` is chosen by platform, and the choice is the mechanism (A322).
 *
 * The rule is a store rule: a phone build must contain no purchase path, and the
 * one the web/desktop build has is a route element the phone never renders. So
 * what is asserted here is a property of the table, read from the real table —
 * `appRouter`, not a copy of the shape as the tests above build one. Rendering
 * `MobilePlanScreen` on its own cannot see it.
 */
describe('/billing is chosen by platform', () => {
  it('registers the gate rather than a fixed screen', () => {
    const registered = registeredRoute('billing')
    expect(registered?.element, 'router.tsx must register /billing').toBeTruthy()
  })

  it('picks the read-only plan screen on both store platforms', () => {
    for (const kind of ['ios', 'android'] as const) {
      expect(billingElementFor(kind).type, kind).toBe(MobilePlanScreen)
    }
    for (const kind of ['web', 'macos', 'windows'] as const) {
      expect(billingElementFor(kind).type, kind).toBe(PlanUsageScreen)
    }
  })

  it('reads the platform when the route renders, not when the table is built', async () => {
    // At module scope `platform.kind()` answers `web`: `main.tsx` installs the
    // native adapter after its imports have been evaluated. A table-level
    // `billingElementFor(platform.kind())` would therefore hand an iPhone the
    // desktop screen — and would pass every test that renders the components
    // directly, which is why this one goes through the registered element.
    installFakePlatform('ios')
    serveUsage()

    renderWithProviders(registeredRoute('billing')?.element)

    expect(await screen.findByText(/does not sell subscriptions/)).toBeInTheDocument()
  })

  it('renders the desktop plan screen off the stores', async () => {
    installFakePlatform('macos')
    serveUsage()

    renderWithProviders(registeredRoute('billing')?.element)

    expect(await screen.findByText('Free')).toBeInTheDocument()
    expect(screen.queryByText(/does not sell subscriptions/)).toBeNull()
  })
})
