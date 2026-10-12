import { act, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryRouter, Outlet, RouterProvider, useLocation } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  NotificationRouter,
  notificationPath,
  notificationRouteFor,
  notificationTarget,
} from './NotificationRouter'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { platform } from '@/platform'
import { setAccessToken } from '@/lib/tokens'

/**
 * Where a tapped notification lands (A320, A316).
 *
 * Two halves, and they fail differently:
 *
 *  1. **The mapping is pure and is asserted as a table.** The payload is the server's
 *     own `data` map (`reminders.rs` sends `reminder_id` and `kind`), so a route
 *     derived from different keys than the server writes is a tap that opens nothing —
 *     and nothing in a running app would report it.
 *  2. **The tap is buffered until there is a session.** A tap that *launched* the app
 *     is replayed by the plugin before `useSession` has exchanged the stored refresh
 *     token; navigating on arrival lands on `/signin` and spends the tap on a screen
 *     that redirects away. That window is the whole reason this component holds state
 *     instead of calling `navigate` from the subscription.
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

beforeEach(() => {
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the payload-to-route mapping', () => {
  it('reads the keys the server actually sends', () => {
    // `reminders.rs` builds the push `data` map as `reminder_id` + `kind`. The
    // mapping is asserted against those two names, so a client that read `id` or
    // `type` would be a tap that opens nothing.
    expect(notificationTarget({ reminder_id: 'r1', kind: 'task_due' })).toEqual({
      kind: 'reminder',
      subject: 'task',
    })
    expect(notificationTarget({ reminder_id: 'r1', kind: 'event_soon' })).toEqual({
      kind: 'reminder',
      subject: 'event',
    })
    // The server's third kind is day-granular and derived from the deck, so there is
    // no subject to open: the review queue is where it belongs.
    expect(notificationTarget({ reminder_id: 'r1', kind: 'cards_due' })).toEqual({
      kind: 'reminder',
      subject: 'review',
    })
  })

  it('prefers the most specific destination the payload names', () => {
    expect(notificationTarget({ workspace_id: 'w1', task_id: 't1' })).toEqual({
      kind: 'workspace',
      id: 'w1',
    })
    expect(notificationTarget({ task_id: 't1' })).toEqual({ kind: 'task' })
  })

  it('routes nothing for a tap that names no destination', () => {
    // Not a defensive branch: the plugin emits the same click event for a **local**
    // notification, which carries no payload at all. Guessing a destination would
    // move the student somewhere arbitrary for no reason.
    expect(notificationTarget(null)).toBeNull()
    expect(notificationTarget(undefined)).toBeNull()
    expect(notificationTarget({})).toBeNull()
    expect(notificationTarget({ messageId: 'm1', from: '123' })).toBeNull()
    // A blank value is not a destination either: `/workspaces/` is a route that
    // matches nothing, so an empty id would be a tap that lands on the 404.
    expect(notificationTarget({ workspace_id: '   ' })).toBeNull()
    expect(notificationRouteFor({ id: 1, data: null })).toBeNull()
  })

  it('resolves every target to a path the route table registers', () => {
    expect(notificationPath({ kind: 'workspace', id: 'w1' })).toBe('/workspaces/w1')
    expect(notificationPath({ kind: 'task' })).toBe('/tasks')
    expect(notificationPath({ kind: 'reminder', subject: 'task' })).toBe('/tasks')
    expect(notificationPath({ kind: 'reminder', subject: 'event' })).toBe('/calendar')
    expect(notificationPath({ kind: 'reminder', subject: 'review' })).toBe('/today')

    expect(notificationRouteFor({ id: 1, data: { workspace_id: 'w1' } })).toBe('/workspaces/w1')
  })
})

/** The path the router is on, so a navigation is observable rather than inferred. */
function Probe() {
  const location = useLocation()
  return <p data-testid="path">{location.pathname}</p>
}

/**
 * The route tree `router.tsx` builds, minus the screens.
 *
 * `NotificationRouter` renders nothing, so the outlet that shows a screen is the
 * sibling's — `DeepLinkHandler`'s in the real table, this stand-in's here. That is
 * the shape being asserted: the notification router is a side effect at the root,
 * not a layout.
 */
const ROUTES = [
  {
    element: (
      <>
        <NotificationRouter />
        <Probe />
        <Outlet />
      </>
    ),
    children: [
      { index: true, element: <p>home</p> },
      { path: 'tasks', element: <p>tasks</p> },
      { path: 'calendar', element: <p>calendar</p> },
      { path: 'workspaces/:workspaceId', element: <p>workspace</p> },
    ],
  },
]

function renderRoot(routes: Parameters<typeof createMemoryRouter>[0]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const router = createMemoryRouter(routes, { initialEntries: ['/'] })
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
  return router
}

describe('a tap arriving at a running app', () => {
  it('navigates straight to the screen the payload names', async () => {
    const fake = installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    mockFetch(() => jsonResponse(200, USER))

    renderRoot(ROUTES)
    await waitFor(() => {
      expect(fake.push?.notificationSubscribers).toBe(1)
    })

    fake.push?.emitNotification({ id: 1, data: { reminder_id: 'r1', kind: 'event_soon' } })

    expect(await screen.findByText('calendar')).toBeInTheDocument()
    expect(screen.getByTestId('path').textContent).toBe('/calendar')
  })

  it('leaves the student where they are when the tap names no destination', async () => {
    const fake = installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    mockFetch(() => jsonResponse(200, USER))

    renderRoot(ROUTES)
    await waitFor(() => {
      expect(fake.push?.notificationSubscribers).toBe(1)
    })

    // A local notification's tap, which has no payload.
    fake.push?.emitNotification({ id: 2, data: null })

    expect(await screen.findByText('home')).toBeInTheDocument()
    expect(screen.getByTestId('path').textContent).toBe('/')
  })
})

describe('a tap that launched the app', () => {
  it('is spent when the session arrives, not on arrival', async () => {
    // The cold-start window, and the reason this component holds state: the plugin
    // replays the tap as soon as a listener registers, which is *before*
    // `useSession` has exchanged the stored refresh token. Navigating then would land
    // on `/signin` and spend the tap on a screen that redirects away.
    const fake = installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })

    // The holder is an object rather than a `let` because the executor's assignment
    // is not control flow TypeScript can see: a captured `let` narrows to `never` at
    // the read below, which is a compile error rather than a runtime one.
    const deferred: { resolve: ((response: Response) => void) | null } = { resolve: null }
    mockFetch(
      () =>
        new Promise<Response>((resolve) => {
          deferred.resolve = resolve
        }),
    )

    renderRoot(ROUTES)
    await waitFor(() => {
      expect(fake.push?.notificationSubscribers).toBe(1)
    })

    // Wrapped in `act` so the tap's state update and the effect that would spend it
    // are flushed before the assertion. Without that, this case passes against a
    // router that navigates immediately: the navigation is an effect, and a
    // microtask is not enough for React to have run it — which is exactly the
    // vacuous green this assertion exists to avoid.
    await act(async () => {
      fake.push?.emitNotification({ id: 1, data: { workspace_id: 'w1' } })
    })

    // Still loading: nothing has moved, and the tap is held rather than dropped.
    expect(screen.getByTestId('path').textContent).toBe('/')

    deferred.resolve?.(jsonResponse(200, USER))

    expect(await screen.findByText('workspace')).toBeInTheDocument()
    expect(screen.getByTestId('path').textContent).toBe('/workspaces/w1')
  })

  it('is not spent while the app is signed out', async () => {
    // No session at all: the app is on its way to `/signin`, and a tap spent there
    // would be a tap the student never sees the result of.
    const fake = installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    setAccessToken(null)
    mockFetch(() => jsonResponse(401, { code: 'unauthorized', message: 'no' }))

    renderRoot(ROUTES)
    await waitFor(() => {
      expect(fake.push?.notificationSubscribers).toBe(1)
    })

    await act(async () => {
      fake.push?.emitNotification({ id: 1, data: { task_id: 't1' } })
    })

    expect(screen.getByTestId('path').textContent).toBe('/')
  })
})

describe('a build with no push capability', () => {
  it('renders and subscribes to nothing', async () => {
    // Every browser and every laptop. There is no channel to subscribe to, and this
    // is the path that would otherwise ship broken for every non-phone platform.
    installFakePlatform('web')
    mockFetch(() => jsonResponse(200, USER))

    renderRoot(ROUTES)

    expect(platform.push).toBeUndefined()
    expect(await screen.findByText('home')).toBeInTheDocument()
    expect(screen.getByTestId('path').textContent).toBe('/')
  })
})
