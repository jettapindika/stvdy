import { useEffect } from 'react'

import { API_BASE_URL, ensureFreshSession } from '@/api/client'
import { clearSession, getAccessToken, readDeviceId, subscribeSession } from '@/lib/tokens'
import { configureSync, platform } from '@/platform'

/**
 * The glue between the session and the shell's sync engine.
 *
 * A186 fixes `Platform.sync` at seven members, and none of them can carry a token,
 * so the credentials are handed to the shell through `configureSync` — the same
 * kind of operation as installing the adapter. Three things happen here and each
 * has a reason it cannot live elsewhere:
 *
 * 1. **On boot, and again whenever the session changes**, the current access
 *    token, the API origin and this device's id go to the shell, and one pass runs.
 *    The token is short-lived and lives only in this app's memory, so the shell has
 *    to be re-told every time it is rotated — which is exactly what
 *    `subscribeSession` fires on. Without the first pass a fresh launch would show
 *    a status the device never computed.
 * 2. **`refresh` is handed over as a capability**, so the shell can ask for a new
 *    token when its own expires: the adapter calls it on
 *    `stvdy://sync-auth-expired`, reconfigures and runs a pass. The refresh itself
 *    is the API layer's single-flight one, which is why it is passed in rather than
 *    reimplemented.
 * 3. **`revoked` signs the account out.** The shell has already wiped the local
 *    cache and the outbox (A186); all that is left is to end the session —
 *    `clearSession` drops the access token from memory and the refresh token from
 *    the keychain, so a revoked device cannot restore a session it no longer has.
 *    Leaving the token behind would let the next launch present a credential the
 *    server has already refused.
 *
 * The web build has no engine (A14), so this is a no-op there: there is nothing to
 * configure, no pass to run, and no revocation to hear about.
 */
export function useSyncEngine(): void {
  useEffect(() => {
    if (platform.runtime() === 'web') return

    let cancelled = false

    async function handOverCredentials(): Promise<void> {
      const [accessToken, deviceId] = await Promise.all([getAccessToken(), readDeviceId()])
      // Nothing to configure before there is a session, and nothing after it ends:
      // the shell must never run a pass with a stale token, and a pass it cannot
      // authenticate would report `offline` over a working network.
      if (accessToken === null || deviceId === null) return
      await configureSync({
        baseUrl: API_BASE_URL,
        deviceId,
        accessToken,
        refresh: async () => {
          await ensureFreshSession()
          return getAccessToken()
        },
      })
      if (!cancelled) await platform.sync.syncNow()
    }

    void handOverCredentials()
    const unsubscribe = subscribeSession(() => {
      void handOverCredentials()
    })

    let offStatus: (() => void) | null = null
    void platform.sync
      .onStatus((report) => {
        if (report.state !== 'revoked') return
        // The session is over on the server's side; this is the app agreeing with
        // it. `clearSession` empties the token store and notifies the session
        // subscriber, so the gate in `App` sends the user to the sign-in screen.
        void clearSession()
      })
      .then((off) => {
        if (cancelled) off()
        else offStatus = off
      })
      .catch(() => {
        // A shell that cannot subscribe has no engine to report; the status
        // control already says it cannot read one.
      })

    return () => {
      cancelled = true
      unsubscribe()
      offStatus?.()
    }
  }, [])
}
