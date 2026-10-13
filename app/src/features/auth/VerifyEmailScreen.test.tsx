import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { VerifyEmailScreen } from './VerifyEmailScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { saveSession, setAccessToken } from '@/lib/tokens'

/**
 * The verification page.
 *
 * The token is single use, so the one behaviour that must not regress is *how
 * many times* it is spent. Everything else here is about the three answers a user
 * can meet: it worked, the link is dead, or the page was opened without a link at
 * all (which is where signup lands).
 */

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: false,
  email_verified_at: null,
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function verifyRequests(calls: { url: string; init: RequestInit }[]) {
  return calls.filter((call) => call.url.endsWith('/auth/verify-email'))
}

describe('the verification page', () => {
  it('spends the token from the URL once and reports success', async () => {
    const calls = mockFetch(() => jsonResponse(200, { verified: true }))

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify?token=abc123' })

    expect(await screen.findByText('Email verified')).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent(
      'Your email address is verified. Sign in to continue.',
    )

    const requests = verifyRequests(calls)
    expect(requests).toHaveLength(1)
    expect(JSON.parse(String(requests[0]?.init.body))).toEqual({ token: 'abc123' })
    // A public route: no bearer token, so a stale access token cannot make the
    // server attribute the verification to the wrong session.
    expect(requests[0]?.init.headers).not.toHaveProperty('authorization')
  })

  it('explains a dead link and offers a way forward', async () => {
    mockFetch(() =>
      jsonResponse(400, {
        code: 'validation',
        message: 'verification token expired',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify?token=stale' })

    expect(await screen.findByText('This link no longer works')).toBeInTheDocument()
    expect(screen.getByText(/single use and they expire/)).toBeInTheDocument()
    // Signed out, so the next action is to sign in and ask for another one.
    expect(screen.getByRole('link', { name: 'Sign in to request a new link' })).toBeInTheDocument()
  })

  it('asks the user to open the email when there is no token at all', async () => {
    mockFetch(() => jsonResponse(200, {}))

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify' })

    expect(await screen.findByText('Check your email')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Sign in' })).toBeInTheDocument()
  })

  it('lets a signed-in user request a new link', async () => {
    const calls = mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-1',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:00:00Z',
        })
      }
      if (url.endsWith('/auth/me')) return jsonResponse(200, USER)
      if (url.endsWith('/auth/resend-verification')) return jsonResponse(204)
      return jsonResponse(400, {
        code: 'validation',
        message: 'verification token expired',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    const user = userEvent.setup()

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify?token=stale' })

    await user.click(await screen.findByRole('button', { name: 'Send a new link' }))

    expect(await screen.findByText(/A new link is on its way/)).toBeInTheDocument()
    expect(calls.filter((call) => call.url.endsWith('/auth/resend-verification'))).toHaveLength(1)
  })

  it('does not offer the workspace while the session still says unverified', async () => {
    // The shell's gate reads `email_verified` from the session (A359), and the token is spent
    // a moment before `/auth/me` reports the new value. Offering the workspace link during that
    // window bounces the student straight back here — with `?token=` still in the URL and the
    // token already consumed, so the screen would tell them their link no longer works, right
    // after it worked. The wait is what prevents that, and the assertion is on the *absence*:
    // with the wait removed the link renders here and this fails.
    mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-1',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:00:00Z',
        })
      }
      // The session never catches up in this test, which is the window being pinned.
      if (url.endsWith('/auth/me')) return jsonResponse(200, USER)
      if (url.endsWith('/auth/verify-email')) return jsonResponse(200, { verified: true })
      return jsonResponse(204)
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify?token=fresh' })

    expect(await screen.findByText('Email verified')).toBeInTheDocument()
    expect(screen.getByText('Finishing your sign-in')).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Go to your workspaces' })).not.toBeInTheDocument()
    expect(screen.queryByText('This link no longer works')).not.toBeInTheDocument()
  })

  it('offers the workspace once the session reports the address verified', async () => {
    // The other side of the same branch, so the wait cannot be satisfied by never showing the
    // link at all.
    mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-1',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:00:00Z',
        })
      }
      if (url.endsWith('/auth/me')) {
        return jsonResponse(200, { ...USER, email_verified: true })
      }
      if (url.endsWith('/auth/verify-email')) return jsonResponse(200, { verified: true })
      return jsonResponse(204)
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify?token=fresh' })

    expect(await screen.findByRole('link', { name: 'Go to your workspaces' })).toBeInTheDocument()
    expect(screen.queryByText('Finishing your sign-in')).not.toBeInTheDocument()
  })

  it('offers an unverified signed-in user a way out, and no way into the workspace', async () => {
    // A359 closes the workspace until the address is confirmed, so the invitation to it is
    // gone. What replaces it is the two things a student whose mail never arrives actually
    // needs: another link, and a way to stop being signed in as an account they cannot use —
    // a mistyped address would otherwise be permanent.
    const calls = mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-1',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:00:00Z',
        })
      }
      if (url.endsWith('/auth/me')) return jsonResponse(200, USER)
      if (url.endsWith('/auth/logout')) return jsonResponse(204)
      return jsonResponse(204)
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    const user = userEvent.setup()

    renderWithProviders(<VerifyEmailScreen />, { route: '/verify' })

    expect(await screen.findByText('Check your email')).toBeInTheDocument()
    // Waited for rather than asserted directly: this text is the signed-in half of the view,
    // and the session is still being restored when the title first paints.
    expect(
      await screen.findByText(/Your workspace opens as soon as this address is confirmed/),
    ).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Go to your workspaces' })).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Use a different address' }))

    // The refresh token is revoked server-side rather than only dropped locally: the whole
    // point of this control is that the session is over. It is the *rotated* token
    // (`refresh-2`), because restoring the session exchanged the stored one — the value the
    // server would still accept is the only one worth revoking.
    await vi.waitFor(() => {
      expect(calls.filter((call) => call.url.endsWith('/auth/logout'))).toHaveLength(1)
    })
    const logout = calls.find((call) => call.url.endsWith('/auth/logout'))
    expect(JSON.parse(String(logout?.init.body))).toEqual({ refresh_token: 'refresh-2' })
  })
})
