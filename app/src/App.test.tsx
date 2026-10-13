import { screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'

import { App } from './App'
import { installFakePlatform, jsonResponse, mockFetch, stubViewport } from './test/harness'
import { renderWithProviders } from './test/render'
import { saveSession, setAccessToken } from './lib/tokens'

/**
 * The gate in front of the signed-in shell.
 *
 * This is the app's first frame, and it has three possible answers: restore the
 * session, refuse it, or wait. Getting the wait wrong is the visible bug (a flash
 * of the sign-in screen on every cold start); getting the refusal wrong is the
 * dangerous one (a shell rendered around requests that all fail).
 */

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-11T09:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken(null)
  stubViewport(1280)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function renderApp() {
  return renderWithProviders(
    <Routes>
      <Route path="/" element={<App />}>
        <Route index element={<p>Today content</p>} />
      </Route>
      <Route path="/signin" element={<p>Sign in screen</p>} />
    </Routes>,
  )
}

describe('the session gate', () => {
  it('shows the sign-in screen when there is nothing stored', async () => {
    const calls = mockFetch(() => jsonResponse(200, {}))

    renderApp()

    expect(await screen.findByText('Sign in screen')).toBeInTheDocument()
    expect(screen.queryByText('Today content')).not.toBeInTheDocument()
    // Nothing to restore means nothing is asked of the server.
    expect(calls).toHaveLength(0)
  })

  it('restores a stored session and renders the shell', async () => {
    mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-1',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:00:00Z',
        })
      }
      return jsonResponse(200, USER)
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    renderApp()

    expect(await screen.findByText('Today content')).toBeInTheDocument()
    // The shell, not just the outlet: navigation is the thing a half-gated app
    // loses first.
    expect(screen.getByRole('navigation', { name: 'Main' })).toBeInTheDocument()
    expect(screen.queryByText('Sign in screen')).not.toBeInTheDocument()
  })

  it('falls back to the sign-in screen when the stored session is refused', async () => {
    mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(401, {
          code: 'unauthorized',
          message: 'refresh token revoked',
          min_plan: null,
          limit: null,
          used: null,
        })
      }
      return jsonResponse(200, USER)
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    renderApp()

    expect(await screen.findByText('Sign in screen')).toBeInTheDocument()
    expect(screen.queryByText('Today content')).not.toBeInTheDocument()
  })
})

/**
 * The second gate (A359): an unverified session reaches its own account surface and nothing
 * else.
 *
 * The server refuses the workspace to an unverified account, so this is not the enforcement —
 * it is what keeps the refusal from being the whole interface. Twenty screens of
 * `403 email_not_verified` is a worse answer than one screen carrying the remedy, and the
 * account routes are deliberately still open so a student whose mail never arrives can leave.
 */
describe('the verification gate', () => {
  const UNVERIFIED = { ...USER, email_verified: false, email_verified_at: null }

  function renderAppWithSettings() {
    return renderWithProviders(
      <Routes>
        <Route path="/" element={<App />}>
          <Route index element={<p>Today content</p>} />
          <Route path="settings" element={<p>Settings content</p>} />
        </Route>
        <Route path="/signin" element={<p>Sign in screen</p>} />
        <Route path="/verify" element={<p>Verify screen</p>} />
      </Routes>,
      { route: '/' },
    )
  }

  it('holds an unverified session at the verify screen instead of the shell', async () => {
    mockFetch(() => jsonResponse(200, UNVERIFIED))
    setAccessToken('access-1')

    renderAppWithSettings()

    expect(await screen.findByText('Verify screen')).toBeInTheDocument()
    expect(screen.queryByText('Today content')).not.toBeInTheDocument()
  })

  it('lets an unverified session reach its own account surface', async () => {
    // The way out: a student who mistyped their address has to be able to reach the device
    // list, the data export and the deletion. Refusing those is the trap this asserts against.
    mockFetch(() => jsonResponse(200, UNVERIFIED))
    setAccessToken('access-1')

    renderWithProviders(
      <Routes>
        <Route path="/" element={<App />}>
          <Route path="settings" element={<p>Settings content</p>} />
        </Route>
        <Route path="/verify" element={<p>Verify screen</p>} />
      </Routes>,
      { route: '/settings' },
    )

    expect(await screen.findByText('Settings content')).toBeInTheDocument()
    expect(screen.queryByText('Verify screen')).not.toBeInTheDocument()
  })

  it('renders the shell for a verified session', async () => {
    mockFetch(() => jsonResponse(200, USER))
    setAccessToken('access-1')

    renderAppWithSettings()

    expect(await screen.findByText('Today content')).toBeInTheDocument()
    expect(screen.queryByText('Verify screen')).not.toBeInTheDocument()
  })
})
