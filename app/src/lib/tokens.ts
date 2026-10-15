import { platform } from '../platform'

/**
 * The session store: where the two tokens live and who gets told when they move.
 *
 * The split is deliberate and comes from the Phase 1 contract. The **refresh
 * token** is long-lived, so it goes through `platform.secureStore` (the OS
 * keychain on native). The **access token** is short-lived and never touches
 * storage: it lives in this module's memory only, never `localStorage`, never the
 * keychain. A token that is never written cannot be read out of a browser
 * profile or a backup.
 *
 * The device id is stored beside the refresh token because the refresh call
 * needs both, and losing one without the other would leave a session that can be
 * neither used nor cleaned up.
 *
 * `secureStore` is not available everywhere: the web adapter has no keychain and
 * throws on write (see `platform/web.ts`). On web, session persistence is
 * handled by `HttpOnly` cookies set by the server on auth responses. The cookie
 * carries the refresh token and device id, so the browser sends them
 * automatically on `/auth/refresh` and no script can read them. The `secureStore`
 * on web remains a no-op — the cookie path bypasses it entirely.
 *
 * Subscribers exist so a 401 anywhere in the app can end the session once, and
 * every screen watching the session finds out. That is why this is a store
 * rather than a set of functions: "signed out" is a fact several components need
 * at the same time.
 */

const REFRESH_TOKEN_KEY = 'stvdy.refresh_token'
const DEVICE_ID_KEY = 'stvdy.device_id'

let accessToken: string | null = null
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

/** Subscribes to every change to the stored session. Returns an unsubscribe. */
export function subscribeSession(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function getAccessToken(): string | null {
  return accessToken
}

export function setAccessToken(token: string | null): void {
  if (accessToken === token) return
  accessToken = token
  emit()
}

async function storeGet(key: string): Promise<string | null> {
  try {
    return await platform.secureStore.get(key)
  } catch {
    // A store that cannot be read is indistinguishable from an empty one: there
    // is no session to restore either way.
    return null
  }
}

async function storeSet(key: string, value: string): Promise<void> {
  try {
    await platform.secureStore.set(key, value)
  } catch {
    // No keychain (the web build). The `secureStore` on web is a deliberate
    // no-op: session persistence is handled by the HttpOnly cookie the server
    // sets on login/signup/refresh. The caller's in-memory session still works,
    // so this is not surfaced as a failure.
  }
}

async function storeDelete(key: string): Promise<void> {
  try {
    await platform.secureStore.delete(key)
  } catch {
    // Nothing was stored, so there is nothing to delete.
  }
}

export function readRefreshToken(): Promise<string | null> {
  return storeGet(REFRESH_TOKEN_KEY)
}

export function readDeviceId(): Promise<string | null> {
  return storeGet(DEVICE_ID_KEY)
}

/**
 * Persists a rotated session. Called after signup, login, and every refresh,
 * because the server rotates the refresh token on each use and the old one stops
 * working.
 */
export async function saveSession(session: {
  refreshToken: string
  deviceId: string
}): Promise<void> {
  await storeSet(REFRESH_TOKEN_KEY, session.refreshToken)
  await storeSet(DEVICE_ID_KEY, session.deviceId)
}

/**
 * Ends the session: the access token is dropped from memory and the refresh
 * token and device id are removed from the store, so nothing can restore it.
 */
export async function clearSession(): Promise<void> {
  setAccessToken(null)
  await storeDelete(REFRESH_TOKEN_KEY)
  await storeDelete(DEVICE_ID_KEY)
  emit()
}
