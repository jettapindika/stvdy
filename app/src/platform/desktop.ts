import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import { readFile } from '@tauri-apps/plugin-fs'
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'
import { openUrl } from '@tauri-apps/plugin-opener'
import { platform as osPlatform } from '@tauri-apps/plugin-os'

import type {
  DeepLinkIntent,
  Platform,
  PlatformKind,
  SyncApi,
  SyncReport,
  SyncStatus,
} from './types'
import type { SyncCredentials } from './index'
import { webPlatform } from './web'

/**
 * A native adapter: the `Platform` surface plus the one capability that is not
 * part of it.
 *
 * `configureSync` hands the shell's sync commands their credentials. It is
 * deliberately *not* a member of `Platform.sync`: A186 fixes that surface at
 * `{status, read, write, tombstone, syncNow, reset, onStatus}`, and a token is
 * transport configuration rather than sync state. `main.tsx` passes it to
 * `installPlatform` beside the adapter, which is the same seam the adapter itself
 * is installed through.
 */
export interface NativePlatform extends Platform {
  configureSync(credentials: SyncCredentials): Promise<void>
}

/** The wire shape of `sync_status`/`sync_now`, exactly as the shell serializes it. */
interface WireReport {
  state: SyncStatus
  code: string | null
  last_synced_at: string | null
  outbox: number
}

function reportOf(wire: WireReport): SyncReport {
  return {
    state: wire.state,
    code: wire.code,
    lastSyncedAt: wire.last_synced_at,
    outbox: wire.outbox,
  }
}

/**
 * The shell's sync, over IPC.
 *
 * Three things here are load-bearing and each is why this is not a thin `invoke`
 * wrapper:
 *
 * 1. **The credentials are pushed, never pulled.** The shell holds no token of
 *    its own; `configureSync` puts the current access token, the API origin and
 *    this device's id in its memory. That is also why the token can never reach
 *    the SQLite cache.
 * 2. **`stvdy://sync-auth-expired` triggers a refresh here**, using the `refresh`
 *    capability the SPA handed over: the shell has stopped the pass and is
 *    waiting, so the adapter refreshes, reconfigures and runs one pass. Without
 *    this a native app would sync once per launch and then go quiet the moment its
 *    short-lived access token expired.
 * 3. **`stvdy://sync-revoked` arrives as `state: 'revoked'` on the same channel as
 *    every other status.** The shell has already wiped the cache and the outbox
 *    (A186); the subscriber signs out. One channel means the control and the
 *    sign-out path cannot disagree about what happened.
 */
function tauriSync(): { api: SyncApi; configure: (credentials: SyncCredentials) => Promise<void> } {
  let credentials: SyncCredentials | null = null
  const handlers = new Set<(report: SyncReport) => void>()
  let listening = false

  function emit(report: SyncReport): void {
    for (const handler of handlers) handler(report)
  }

  /** Refreshes, reconfigures and runs one pass, after the shell lost its token. */
  async function reauthenticate(): Promise<void> {
    const current = credentials
    if (current === null) return
    const token = await current.refresh()
    // A refresh the API layer refused means the session is over; that path clears
    // the token and ends the session, so there is nothing to reconfigure with.
    if (token === null) return
    credentials = { ...current, accessToken: token }
    await invoke('sync_configure', {
      base_url: current.baseUrl,
      access_token: token,
      device_id: current.deviceId,
    })
    await invoke('sync_now')
  }

  async function listenOnce(): Promise<void> {
    if (listening) return
    listening = true
    await listen<WireReport>('stvdy://sync-status', (event) => {
      emit(reportOf(event.payload))
    })
    await listen('stvdy://sync-revoked', () => {
      emit({ state: 'revoked', code: null, lastSyncedAt: null, outbox: 0 })
    })
    await listen('stvdy://sync-auth-expired', () => {
      void reauthenticate()
    })
  }

  async function status(): Promise<SyncReport> {
    return reportOf(await invoke<WireReport>('sync_status'))
  }

  async function readCached<T>(entity: string): Promise<T[]>
  async function readCached<T>(entity: string, id: string): Promise<T | null>
  async function readCached<T>(entity: string, id?: string): Promise<T[] | T | null> {
    if (id === undefined) {
      // The shell's list form answers `[{id, row}, …]`; the envelope's id is
      // dropped because the cached row carries its own (the pull's allowlist
      // includes `id`), and a row the shell could not load is left out rather
      // than shown as an empty one.
      const items = await invoke<Array<{ id: string; row: T | null }>>('sync_read', {
        entity,
        id: null,
      })
      return items.flatMap((item) => (item.row === null ? [] : [item.row]))
    }
    return await invoke<T | null>('sync_read', { entity, id })
  }

  const api: SyncApi = {
    status,
    read: readCached,
    async write(entity, id, payload) {
      await invoke('sync_write', { entity, id, payload })
    },
    async tombstone(entity, id) {
      await invoke('sync_tombstone', { entity, id })
    },
    async syncNow() {
      return reportOf(await invoke<WireReport>('sync_now'))
    },
    async reset() {
      await invoke('sync_reset')
    },
    async onStatus(handler) {
      // The listeners are attached once, when the credentials first arrive: the
      // shell emits nothing before it is configured, so a subscription that
      // attached earlier would only be listening to a channel nothing writes to.
      await listenOnce()
      handlers.add(handler)
      return () => {
        handlers.delete(handler)
      }
    },
  }

  return {
    api,
    async configure(next) {
      credentials = next
      await invoke('sync_configure', {
        base_url: next.baseUrl,
        access_token: next.accessToken,
        device_id: next.deviceId,
      })
      await listenOnce()
    },
  }
}

/**
 * Tauri adapter for macOS and Windows.
 *
 * Secrets go through the Rust keychain commands rather than any JS storage: only
 * the refresh token is persisted (the access token stays in memory), and the
 * store is the OS keychain/keystore, never a file.
 */
export function desktopPlatform(kind: PlatformKind): NativePlatform {
  const base = webPlatform(kind)
  const sync = tauriSync()

  return {
    ...base,
    runtime: () => 'tauri',
    sync: sync.api,
    configureSync: sync.configure,

    secureStore: {
      set: async (key, value) => {
        await invoke('secret_set', { key, value })
      },
      get: (key) => invoke<string | null>('secret_get', { key }),
      delete: async (key) => {
        await invoke('secret_delete', { key })
      },
    },

    notifications: {
      async requestPermission() {
        if (await isPermissionGranted()) return true
        return (await requestPermission()) === 'granted'
      },
      async notify(title, body) {
        if (await isPermissionGranted()) sendNotification({ title, body })
      },
      // Desktop cannot schedule (the plugin ignores it), so deliver now. Server
      // push and in-app delivery are the real desktop reminder channels.
      async schedule(title, body) {
        if (await isPermissionGranted()) sendNotification({ title, body })
      },
    },

    files: {
      async pick(options) {
        const selected = await open({
          multiple: options?.multiple ?? false,
          filters: options?.mimeTypes?.map((mime) => ({ name: mime, extensions: [] })),
        })
        if (selected === null) return null
        return Array.isArray(selected) ? selected : [selected]
      },
      read: (path) => readFile(path),
    },

    deepLinks: {
      // The shell classifies the URL and hands over `{action, payload}`; the
      // SPA never parses a raw URL. `deep_link_current` is the cold-start pull
      // (macOS/iOS deliver the launch URL there), the event covers a running
      // instance.
      async current() {
        const intents = await invoke<DeepLinkIntent[]>('deep_link_current')
        return intents[0] ?? null
      },
      async onOpen(handler) {
        return listen<DeepLinkIntent[]>('stvdy://deep-link-intent', (event) => {
          event.payload.forEach((intent) => handler(intent))
        })
      },
    },

    // The device-local key-value store, over the shell's own SQLite file (A206).
    // The value crosses the IPC boundary as a JSON **string** rather than a
    // structured payload, because `local_state.value` is `text` and a value that
    // round-tripped through two different serialisers is how a stored timer comes
    // back with a field renamed.
    //
    // `delete` is `local_state_set` with a null value rather than a third
    // command: the row is removed instead of holding the text "null", which
    // `value text not null` would otherwise happily store and which would then
    // read back as a timer that exists and is broken.
    localState: {
      async get<T>(key: string): Promise<T | null> {
        const raw = await invoke<string | null>('local_state_get', { key })
        if (raw === null) return null
        try {
          return JSON.parse(raw) as T
        } catch {
          // A value this build cannot read is an absent one. The timer's honest
          // answer is then "not running", which is the same state a fresh install
          // is in — never a crash on mount.
          console.warn(`stvdy: local state "${key}" is not JSON; treating it as absent`)
          return null
        }
      },
      async set(key, value) {
        await invoke('local_state_set', { key, value: JSON.stringify(value) })
      },
      async delete(key) {
        await invoke('local_state_set', { key, value: null })
      },
    },

    openExternal: (url) => openUrl(url),

    async checkForUpdate() {
      // The updater plugin is not registered yet: it needs a signing keypair and
      // a release endpoint, which arrive in Phase 15 (A35). Returning false
      // rather than calling a command that does not exist keeps the same
      // no-fabricated-capability rule the Rust side follows.
      return false
    },
  }
}

/** Exposed so the shell can branch on the OS without importing plugin-os elsewhere. */
export function currentOsPlatform(): PlatformKind {
  const name = osPlatform()
  switch (name) {
    case 'macos':
    case 'windows':
    case 'ios':
    case 'android':
      return name
    default:
      return 'web'
  }
}
