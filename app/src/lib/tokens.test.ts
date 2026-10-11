import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  clearSession,
  getAccessToken,
  readDeviceId,
  readRefreshToken,
  saveSession,
  setAccessToken,
  subscribeSession,
} from './tokens'
import { installFailingSecureStore, installFakePlatform } from '../test/harness'
import type { FakeSecureStore } from '../test/harness'

/**
 * Where the two tokens live.
 *
 * The rule this file protects is the one from the Phase 1 contract: the refresh
 * token goes to the secure store, and the access token goes nowhere but memory.
 * A regression here would not break a test of the UI, and it would not break
 * anything visible at all: it would quietly put a credential somewhere a backup
 * or another script can read it.
 */

let store: FakeSecureStore

beforeEach(() => {
  store = installFakePlatform()
  setAccessToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the session store', () => {
  it('round trips the refresh token and the device id', async () => {
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    await expect(readRefreshToken()).resolves.toBe('refresh-1')
    await expect(readDeviceId()).resolves.toBe('device-1')
  })

  it('keeps the access token out of the store entirely', () => {
    setAccessToken('access-1')

    expect(getAccessToken()).toBe('access-1')
    // Nothing was written: the access token is short lived and there is nowhere
    // safe to keep it, so it is not kept.
    expect([...store.values.keys()]).toEqual([])
  })

  it('removes both stored values when the session ends', async () => {
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    setAccessToken('access-1')

    await clearSession()

    await expect(readRefreshToken()).resolves.toBeNull()
    await expect(readDeviceId()).resolves.toBeNull()
    expect(getAccessToken()).toBeNull()
    expect(store.deleted).toEqual(['stvdy.refresh_token', 'stvdy.device_id'])
  })

  it('tells subscribers that the session changed', async () => {
    let notifications = 0
    const unsubscribe = subscribeSession(() => {
      notifications += 1
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    setAccessToken('access-1')
    await clearSession()

    // This is what turns a refused refresh into a signed-out screen: the token
    // store changes, `useSession` hears it, and the shell stops rendering.
    expect(notifications).toBeGreaterThan(0)

    unsubscribe()
    const afterUnsubscribe = notifications
    await clearSession()
    expect(notifications).toBe(afterUnsubscribe)
  })

  it('survives a platform with no keychain at all', async () => {
    // The web adapter throws on write, because a browser has no keychain. A
    // session still works in the tab; it simply cannot be restored later, and
    // that degradation must not surface as a crash on the way in or out.
    installFailingSecureStore('web')

    await expect(
      saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' }),
    ).resolves.toBeUndefined()
    await expect(readRefreshToken()).resolves.toBeNull()
    await expect(clearSession()).resolves.toBeUndefined()
  })
})
