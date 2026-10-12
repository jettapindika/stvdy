import type {
  DeepLinksApi,
  FilesApi,
  LocalStateApi,
  NotificationsApi,
  Platform,
  PlatformKind,
  SecureStore,
  SyncApi,
} from './types'

const unsupported = (what: string): never => {
  throw new Error(`${what} is not available in the web build`)
}

/**
 * The web build is online-first and has no OS integration: no secure storage
 * (there is no keychain in a browser), no local database, no scheduling. Every
 * method that cannot work either throws loudly or degrades explicitly, so a
 * caller never silently believes a secret was stored.
 */
const secureStore: SecureStore = {
  set: async () => unsupported('secure storage'),
  get: async () => null,
  delete: async () => unsupported('secure storage'),
}

const notifications: NotificationsApi = {
  async requestPermission() {
    if (typeof Notification === 'undefined') return false
    return (await Notification.requestPermission()) === 'granted'
  },
  async notify(title, body) {
    if (typeof Notification === 'undefined') return
    if (Notification.permission === 'granted') new Notification(title, { body })
  },
  // The web build cannot schedule; it fires immediately so a reminder is not
  // silently dropped.
  async schedule(title, body) {
    await notifications.notify(title, body)
  },
}

const files: FilesApi = {
  async pick() {
    return unsupported('file picking')
  },
  async read() {
    return unsupported('file reading')
  },
}

const deepLinks: DeepLinksApi = {
  async current() {
    return null
  },
  async onOpen() {
    return () => {}
  },
}

/**
 * The web build's reads: there is no cache, so a list is empty and a row is
 * absent. Both are honest answers rather than throws — "this device has no
 * cached copy" is a fact a screen can render, and the caller's remedy (the REST
 * route) is the same one `write` names.
 *
 * Declared as two overloads because the `SyncApi` member has two: a list read
 * omits the id and a row read supplies one.
 */
async function readNothing<T>(entity: string): Promise<T[]>
async function readNothing<T>(entity: string, id: string): Promise<T | null>
async function readNothing(_entity: string, id?: string): Promise<unknown> {
  return id === undefined ? [] : null
}

/**
 * The web build's sync, which is online-only (A14).
 *
 * There is no local database and no outbox in a browser, so there is nothing to
 * queue a write into. The two write paths therefore **refuse** — and they refuse
 * by name, because the caller's remedy is the REST route that already exists for
 * every one of these entities. A `write` that resolved while dropping the payload
 * would be the worst possible answer: the screen would mark the row saved, the
 * next pull would overwrite it with the server's older copy, and nothing would
 * ever have failed.
 *
 * The read side is the other half of the same fact. `status()` reports `synced`
 * with `outbox: 0` — not because work was flushed, but because this build never
 * holds work: the number is a fact about the platform, and it is the one number
 * that keeps the control from claiming a pending write it cannot have. `reset()`
 * deletes nothing, because there is no local data to delete; it is a no-op rather
 * than a throw so a caller wiring "reset and resync" for both form factors does
 * not have to branch. `onStatus` never fires: the state cannot change without a
 * pass, and a pass only exists where the outbox does.
 */
const sync: SyncApi = {
  async status() {
    return { state: 'synced', code: null, lastSyncedAt: null, outbox: 0 }
  },
  read: readNothing,
  async write(entity) {
    // The message names the entity and the REST route, because that is what the
    // caller has to do instead; "not supported" alone leaves nowhere to go.
    return unsupported(`an offline write of ${entity} (use the REST route for this entity)`)
  },
  async tombstone(entity) {
    return unsupported(`an offline delete of ${entity} (use the REST route for this entity)`)
  },
  async syncNow() {
    return { state: 'synced', code: null, lastSyncedAt: null, outbox: 0 }
  },
  async reset() {
    // Nothing is stored locally on the web, so there is nothing to wipe.
  },
  async onStatus() {
    return () => {}
  },
}

/**
 * The web build's device-local state, over `localStorage`.
 *
 * This is **not** a lie about offline support. The cache is absent on the web
 * (A14), and that stays true; this is a key-value store for one device-local
 * fact, and the focus timer's running state is the only key the app writes. A
 * browser tab that is reloaded mid-session resumes its timer for the same reason
 * a desktop app does: the student did not stop it.
 *
 * Every key is prefixed, so this shares the origin's storage without colliding
 * with anything else that uses it. A stored value this build cannot parse is
 * treated as absent and named on the console rather than thrown: the timer's
 * answer to "was I running?" is then an honest "no", and a parse failure is still
 * visible to whoever is looking.
 */
const LOCAL_STATE_PREFIX = 'stvdy.'

const localState: LocalStateApi = {
  async get<T>(key: string): Promise<T | null> {
    const raw = window.localStorage.getItem(`${LOCAL_STATE_PREFIX}${key}`)
    if (raw === null) return null
    try {
      return JSON.parse(raw) as T
    } catch {
      console.warn(`stvdy: local state "${key}" is not JSON; treating it as absent`)
      return null
    }
  },
  async set(key, value) {
    window.localStorage.setItem(`${LOCAL_STATE_PREFIX}${key}`, JSON.stringify(value))
  },
  async delete(key) {
    window.localStorage.removeItem(`${LOCAL_STATE_PREFIX}${key}`)
  },
}

export function webPlatform(kind: PlatformKind = 'web'): Platform {
  return {
    kind: () => kind,
    runtime: () => 'web',
    secureStore,
    notifications,
    files,
    deepLinks,
    sync,
    localState,
    async openExternal(url) {
      window.open(url, '_blank', 'noopener,noreferrer')
    },
    async haptic() {
      // No-op: haptics only exist on mobile.
    },
    async checkForUpdate() {
      return false
    },
  }
}
