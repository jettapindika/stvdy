import type { Platform, Runtime } from './types'
import { webPlatform } from './web'

export type {
  DeepLinkIntent,
  DeepLinksApi,
  FilesApi,
  NotificationsApi,
  Platform,
  PlatformKind,
  Runtime,
  SecureStore,
  SyncStatus,
} from './types'

export function detectRuntime(scope: Record<string, unknown>): Runtime {
  return '__TAURI_INTERNALS__' in scope ? 'tauri' : 'web'
}

export function isTauri(): boolean {
  return detectRuntime(globalThis as unknown as Record<string, unknown>) === 'tauri'
}

/**
 * The active adapter. The web implementation is the default; the Tauri shell
 * installs its own via `installPlatform` during startup, so no module outside
 * `platform/` ever imports a Tauri package (enforced by boundary.test.ts).
 */
let active: Platform = webPlatform()

export function installPlatform(next: Platform): void {
  active = next
}

export const platform: Platform = new Proxy({} as Platform, {
  get(_target, prop: string) {
    const value = active[prop as keyof Platform]
    return typeof value === 'function' ? value.bind(active) : value
  },
})
