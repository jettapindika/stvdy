import type { Platform, Runtime } from './types'
import { webPlatform } from './web'

export type {
  DeepLinkIntent,
  DeepLinksApi,
  FilesApi,
  LocalStateApi,
  NotificationTap,
  NotificationsApi,
  Platform,
  PlatformKind,
  PushApi,
  PushToken,
  Runtime,
  SecureStore,
  SyncApi,
  SyncReport,
  SyncStatus,
  UpdateProgress,
  UpdateStatus,
} from './types'

export function detectRuntime(scope: Record<string, unknown>): Runtime {
  return '__TAURI_INTERNALS__' in scope ? 'tauri' : 'web'
}

export function isTauri(): boolean {
  return detectRuntime(globalThis as unknown as Record<string, unknown>) === 'tauri'
}

/**
 * What the shell's sync commands need to run a pass (A186).
 *
 * The SPA owns the session, so the shell is handed these rather than reading them
 * itself: the access token is short-lived and lives in this app's memory, and the
 * shell must never read a keychain for it. `baseUrl` is the API origin and
 * `deviceId` is this device's own row, which the push route checks against the
 * authenticated device.
 *
 * `refresh` is a capability rather than a value, because the shell needs a *new*
 * token the moment its own expires: on `stvdy://sync-auth-expired` it calls this,
 * which is the API layer's single-flight refresh, and it is handed the fresh token
 * to configure with. The alternative — the shell reaching into the token store —
 * is the layering inversion this exists to avoid.
 */
export interface SyncCredentials {
  baseUrl: string
  deviceId: string
  accessToken: string
  refresh(): Promise<string | null>
}

/**
 * The native adapters' configure path, which is not part of `Platform.sync`.
 *
 * A186 fixes that surface at seven members, and none of them can carry a token;
 * this is the shell's transport being handed its credentials, which is the same
 * kind of operation as `installPlatform` rather than a second sync API. The web
 * build has none, and `configureSync` is a no-op there.
 */
export interface PlatformCapabilities {
  configureSync?(credentials: SyncCredentials): Promise<void>
}

/**
 * The active adapter. The web implementation is the default; the Tauri shell
 * installs its own via `installPlatform` during startup, so no module outside
 * `platform/` ever imports a Tauri package (enforced by boundary.test.ts).
 */
let active: Platform = webPlatform()
let activeConfigureSync: ((credentials: SyncCredentials) => Promise<void>) | null = null

export function installPlatform(next: Platform, capabilities?: PlatformCapabilities): void {
  active = next
  activeConfigureSync = capabilities?.configureSync ?? null
}

/** Hands the shell's sync commands their credentials; a no-op off the shell. */
export async function configureSync(credentials: SyncCredentials): Promise<void> {
  await activeConfigureSync?.(credentials)
}

export const platform: Platform = new Proxy({} as Platform, {
  get(_target, prop: string) {
    const value = active[prop as keyof Platform]
    return typeof value === 'function' ? value.bind(active) : value
  },
})
