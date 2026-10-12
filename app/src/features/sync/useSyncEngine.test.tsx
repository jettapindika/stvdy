import { render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useSyncEngine } from '@/features/sync/useSyncEngine'
import { getAccessToken, readRefreshToken, saveSession, setAccessToken } from '@/lib/tokens'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'

/**
 * The engine's glue: the token handshake, the first pass, and the sign-out.
 *
 * These are the three things `Platform.sync`'s seven members cannot carry (A186),
 * so they are exactly the three that could be missing without anything failing to
 * compile. Each case below is a consumer-visible defect if it regresses:
 *
 * - without the handshake the shell runs no pass at all, and the status control
 *   sits on "Syncing" for ever on a native build;
 * - without the refresh capability a device syncs once per launch and then goes
 *   quiet the moment its short-lived access token expires;
 * - without the sign-out a revoked device keeps a shell around requests that all
 *   401, and keeps a token the server has already refused.
 */

function Probe() {
  useSyncEngine()
  return null
}

beforeEach(() => {
  setAccessToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the sync engine glue', () => {
  it('hands the shell its credentials once there is a session, and runs a pass', async () => {
    const { sync } = installFakePlatform('macos')
    mockFetch(() => jsonResponse(200, {}))
    setAccessToken('access-1')
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    render(<Probe />)

    await waitFor(() => {
      expect(sync.configured).toHaveLength(1)
    })
    // The three values the shell cannot obtain itself, and the API origin the
    // requests go to.
    expect(sync.configured[0]).toMatchObject({
      deviceId: 'device-1',
      accessToken: 'access-1',
    })
    expect(sync.configured[0]?.baseUrl).toMatch(/^https?:\/\//)
    // A device that has just launched shows a status it computed, not one it never
    // asked for.
    await waitFor(() => {
      expect(sync.syncNows).toBe(1)
    })
  })

  it('does nothing at all on the web build, which has no local database', async () => {
    // A14: there is no cache and no outbox in a browser, so there is nothing to
    // configure and no pass to run. A handshake here would be this hook claiming a
    // capability the platform does not have.
    const { sync } = installFakePlatform('web')
    mockFetch(() => jsonResponse(200, {}))
    setAccessToken('access-1')
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    render(<Probe />)

    await Promise.resolve()
    expect(sync.configured).toEqual([])
    expect(sync.syncNows).toBe(0)
  })

  it('waits for a session rather than configuring with nothing', async () => {
    const { sync } = installFakePlatform('macos')
    mockFetch(() => jsonResponse(200, {}))

    render(<Probe />)

    // No access token and no device id: the shell must not be handed a pass it
    // cannot authenticate, which would report `offline` over a working network.
    await Promise.resolve()
    expect(sync.configured).toEqual([])
  })

  it('ends the session when the shell reports the device revoked', async () => {
    const { sync } = installFakePlatform('macos')
    mockFetch(() => jsonResponse(200, {}))
    setAccessToken('access-1')
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    render(<Probe />)
    await waitFor(() => {
      expect(sync.subscribers).toBeGreaterThan(0)
    })

    // The shell has already wiped the cache and the outbox by this point (A186).
    sync.emit({ state: 'revoked', code: null, lastSyncedAt: null, outbox: 0 })

    // Both tokens go: the access token from memory, the refresh token from the
    // keychain. A revoked device that kept its refresh token would try to restore
    // a session the server has already refused, and would be signed in again on the
    // next launch.
    await waitFor(() => {
      expect(getAccessToken()).toBeNull()
    })
    await expect(readRefreshToken()).resolves.toBeNull()
  })
})
