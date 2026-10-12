import { useEffect } from 'react'

import { api } from '@/api/client'
import { readDeviceId } from '@/lib/tokens'
import { platform } from '@/platform'

/**
 * The push token, registered against the device row login created (A329).
 *
 * **The client's whole job is to obtain a token and `PUT` it.** The route
 * (`PUT /devices/{id}/push-token`, `crates/server/src/devices.rs`) already exists and
 * upserts on `device_id`; it refuses a provider outside `PUSH_PROVIDERS` and a blank
 * token. `platform.push.token()` is the shell's `push_token` command, which asks the
 * OS, checks the token is non-blank before it can become a round trip, and derives the
 * provider from the platform rather than from the token.
 *
 * **Why the token is re-registered on every rotation.** APNs and FCM re-issue a token
 * — a restore from a backup, a reinstall, an OS update — and the old one stops
 * delivering. The plugin reports the new one on its own event, which nothing else in
 * the app hears, so a registration that ran once at login is a device that silently
 * stops receiving reminders after its first restore. `watchPushToken` is that half.
 */

/** What a registration attempt did. */
export type PushRegistration =
  | { status: 'registered'; provider: string }
  /**
   * This platform has no push capability at all. Not a failure: the web build and
   * desktop have no APNs/FCM token to mint and no provider to name, and
   * `Platform.push` is absent there on purpose.
   */
  | { status: 'unavailable' }

/** The route a token is registered against, spelled once (A329). */
export function pushTokenPath(deviceId: string): string {
  return `/devices/${deviceId}/push-token`
}

/**
 * Registers this device's token with the server.
 *
 * Degrades rather than throws where the capability is absent, which is the whole
 * reason `Platform.push` is optional: on the web build and on desktop there is no
 * token to register, and a caller that had to catch an exception to learn that would
 * end up rendering an error for a platform behaving correctly.
 *
 * A refusal from the server is deliberately **not** swallowed here — it is the
 * caller's to decide about — but it is not fatal to the app either: a push token that
 * failed to register costs the push channel, not the session.
 */
export async function registerPush(deviceId: string): Promise<PushRegistration> {
  const push = platform.push
  if (push === undefined) return { status: 'unavailable' }

  const { provider, token } = await push.token()
  await api.put(pushTokenPath(deviceId), { provider, token })
  return { status: 'registered', provider }
}

/**
 * Re-registers on every token the OS re-issues; resolves to an unsubscribe.
 *
 * The event carries the token string and nothing else, so the pair is re-read through
 * `registerPush` rather than assembled here: one provider mapping (the shell's), one
 * validation, one `PUT`.
 *
 * A rotation that fails to register is warned about rather than thrown: there is no
 * screen this failure belongs to, and the token is still valid until it is rotated
 * again — the next launch re-registers from `usePushRegistration` regardless.
 */
export async function watchPushToken(deviceId: string): Promise<() => void> {
  const push = platform.push
  if (push === undefined) return () => {}

  return await push.onToken(() => {
    void registerPush(deviceId).catch((error: unknown) => {
      console.warn('stvdy: a rotated push token could not be registered', error)
    })
  })
}

/**
 * Registers this device once there is a session, and keeps it registered.
 *
 * `signedIn` is the caller's statement that there is an account: the device id is
 * written by login (`saveSession`), so before sign-in there is no row to register
 * against and the `PUT` could only 401.
 *
 * Mounted from `App` beside `useLocalNotifications`, for the same reason: this is a
 * property of *this device* and not of any one route.
 */
export function usePushRegistration(signedIn: boolean): void {
  useEffect(() => {
    if (!signedIn) return

    let cancelled = false
    let off: (() => void) | null = null

    void (async () => {
      const deviceId = await readDeviceId()
      // A session with no stored device id is one that cannot be registered against;
      // it is also a state the app should not be in, so it says so rather than
      // PUTting to `/devices/null/push-token`.
      if (deviceId === null) {
        console.warn('stvdy: a push token cannot be registered without a device id')
        return
      }

      try {
        await registerPush(deviceId)
      } catch (error) {
        // A refused or unreachable registration is not a reason to lose the
        // subscription below: the token may rotate later and succeed, and the next
        // launch tries again.
        console.warn('stvdy: the push token could not be registered', error)
      }

      try {
        const stop = await watchPushToken(deviceId)
        if (cancelled) stop()
        else off = stop
      } catch (error) {
        // A shell whose notification plugin cannot register a listener has no
        // rotation event to hear. The registration above still stands, and this is
        // not worth failing a mount over.
        console.warn('stvdy: the push token channel is unavailable', error)
      }
    })()

    return () => {
      cancelled = true
      off?.()
    }
  }, [signedIn])
}
