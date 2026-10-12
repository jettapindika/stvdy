import { renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { pushTokenPath, registerPush, usePushRegistration, watchPushToken } from './registerPush'
import { installFakePlatform, jsonResponse, mockFetch, type RecordedCall } from '@/test/harness'
import { platform } from '@/platform'
import { saveSession, setAccessToken } from '@/lib/tokens'

/**
 * The push registration path (A329).
 *
 * Four properties, and the fourth is the one that would otherwise ship broken for
 * every platform that is not a phone:
 *
 *  1. **The `PUT` carries the shell's pair verbatim.** The provider is derived by the
 *     shell (`push::provider_for`) and the route is the one
 *     `crates/server/src/devices.rs` mounts, so the body and the path are both
 *     asserted rather than assumed.
 *  2. **A rotation re-registers.** The OS re-issues a token after a restore or a
 *     reinstall, and the old one stops delivering; the plugin's event is the only
 *     thing that hears it, so a registration that ran once at login is a device that
 *     silently stops receiving reminders.
 *  3. **The capability is absent on the web build and on desktop, and the caller
 *     degrades rather than throwing.** `Platform.push` is optional on purpose, and
 *     this is the only path a browser can prove.
 *  4. **Nothing is registered before there is a session**, because the device id is
 *     written by login and a `PUT` before it could only 401.
 */

/** The body of the one call to a path, parsed from the JSON the client sent. */
function bodyOf(calls: RecordedCall[], path: string): unknown {
  const call = calls.find((candidate) => new URL(candidate.url).pathname === path)
  if (call === undefined) throw new Error(`no request to ${path}`)
  return JSON.parse(String(call.init.body))
}

function putsTo(calls: RecordedCall[], path: string): RecordedCall[] {
  return calls.filter(
    (call) => new URL(call.url).pathname === path && call.init.method === 'PUT',
  )
}

beforeEach(() => {
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('registerPush', () => {
  it('PUTs the shell’s provider and token to the device row', async () => {
    installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    const calls = mockFetch(() => jsonResponse(204))

    await expect(registerPush('d1')).resolves.toEqual({ status: 'registered', provider: 'apns' })

    expect(putsTo(calls, '/devices/d1/push-token')).toHaveLength(1)
    // The body is the contract the server validates: a provider outside
    // `PUSH_PROVIDERS` is a 422 and a blank token is a 422, so both halves are
    // asserted as the server will read them.
    expect(bodyOf(calls, '/devices/d1/push-token')).toEqual({ provider: 'apns', token: 'tok-1' })
  })

  it('carries the android provider through unchanged, because the shell decides it', async () => {
    // The SPA does not map a platform to a provider; it sends what the shell said.
    // A second mapping here is how the two would disagree about `fcm`.
    installFakePlatform('android', { push: { provider: 'fcm', token: 'tok-2' } })
    const calls = mockFetch(() => jsonResponse(204))

    await registerPush('d2')

    expect(bodyOf(calls, '/devices/d2/push-token')).toEqual({ provider: 'fcm', token: 'tok-2' })
  })

  it('spells the route the server mounts, in one place', () => {
    // `crates/server/src/devices.rs` mounts `put(put_push_token)` at this path and the
    // shell's `push::token_path` builds the same string; a typo here is a 404 that
    // reads like a server problem.
    expect(pushTokenPath('d1')).toBe('/devices/d1/push-token')
  })

  it('degrades rather than throwing where the platform has no push capability', async () => {
    // The web build, and every desktop. There is no APNs/FCM token to mint and no
    // provider to name, so `Platform.push` is absent — and the honest answer is that
    // there is nothing to register, not an exception for a platform behaving
    // correctly. A caller that had to catch one would end up rendering an error.
    installFakePlatform('web')
    const calls = mockFetch(() => jsonResponse(204))

    expect(platform.push).toBeUndefined()
    await expect(registerPush('d1')).resolves.toEqual({ status: 'unavailable' })
    expect(calls).toEqual([])

    installFakePlatform('macos')
    expect(platform.push).toBeUndefined()
    await expect(registerPush('d1')).resolves.toEqual({ status: 'unavailable' })
    expect(calls).toEqual([])
  })

  it('surfaces a refused registration rather than reporting success', async () => {
    // A refused token costs the push channel, not the session — but it must not read
    // as registered, or the device would sit in `push_tokens` having never delivered.
    // The refusal is `devices.rs`'s own: a provider outside `PUSH_PROVIDERS` is a
    // `AppError::Validation`, which is a 400 with the `validation` code.
    installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    mockFetch(() =>
      jsonResponse(400, { code: 'validation', message: 'provider must be "apns" or "fcm"' }),
    )

    await expect(registerPush('d1')).rejects.toMatchObject({
      status: 400,
      code: 'validation',
    })
  })
})

describe('a token the OS re-issued', () => {
  it('is re-registered, with the new token and the same provider', async () => {
    const fake = installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    const calls = mockFetch(() => jsonResponse(204))

    const off = await watchPushToken('d1')
    fake.push?.emitToken('tok-2')

    await waitFor(() => {
      expect(putsTo(calls, '/devices/d1/push-token')).toHaveLength(1)
    })
    expect(bodyOf(calls, '/devices/d1/push-token')).toEqual({ provider: 'apns', token: 'tok-2' })

    off()
    expect(fake.push?.tokenSubscribers).toBe(0)
  })

  it('has no channel to subscribe to where there is no push capability', async () => {
    const fake = installFakePlatform('web')

    const off = await watchPushToken('d1')

    expect(fake.push).toBeNull()
    // A no-op unsubscribe rather than a throw: the caller's cleanup must not be the
    // thing that breaks on the platform with nothing to clean up.
    expect(() => {
      off()
    }).not.toThrow()
  })
})

describe('usePushRegistration', () => {
  it('registers this device once there is a session', async () => {
    installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'd1' })
    const calls = mockFetch(() => jsonResponse(204))

    renderHook(() => usePushRegistration(true))

    await waitFor(() => {
      expect(putsTo(calls, '/devices/d1/push-token')).toHaveLength(1)
    })
    // The device id is the one login wrote, not one invented here: a second source
    // for it would register the token against a row the server does not have.
    expect(bodyOf(calls, '/devices/d1/push-token')).toEqual({ provider: 'apns', token: 'tok-1' })
  })

  it('registers nothing before there is a session', async () => {
    installFakePlatform('ios', { push: { provider: 'apns', token: 'tok-1' } })
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'd1' })
    const calls = mockFetch(() => jsonResponse(204))

    renderHook(() => usePushRegistration(false))

    // Nothing to wait for, so the assertion is on a settled tick: the hook's effect
    // returned before it started anything.
    await Promise.resolve()
    expect(putsTo(calls, '/devices/d1/push-token')).toEqual([])
  })

  it('does nothing at all on a platform with no push capability', async () => {
    installFakePlatform('macos')
    await saveSession({ refreshToken: 'refresh-1', deviceId: 'd1' })
    const calls = mockFetch(() => jsonResponse(204))

    renderHook(() => usePushRegistration(true))

    await Promise.resolve()
    expect(calls).toEqual([])
  })
})
