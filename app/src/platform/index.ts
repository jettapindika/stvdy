import type { FormFactor, Platform, PlatformKind, Runtime } from './types'
import { webPlatform } from './web'

export type {
  DeepLinksApi,
  FilesApi,
  FormFactor,
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

export function formFactorFor(kind: PlatformKind): FormFactor {
  switch (kind) {
    case 'ios':
    case 'android':
      return 'phone'
    case 'macos':
    case 'windows':
      return 'desktop'
    case 'web':
      return 'web'
  }
}

/**
 * Viewport-based layout bucket, used for the tablet two-panel layout.
 *
 * Deliberately NOT derived from `PlatformKind`: a tablet is a width, not a
 * platform (an iPad and a small laptop window are the same problem), and
 * `formFactorFor` could never return `'tablet'`, which made that variant
 * unreachable and would have silently rendered tablets as desktop. Callers that
 * care about tablet pass their own width check.
 */
export function formFactorForWidth(width: number): FormFactor {
  if (width >= 1024) return 'desktop'
  if (width >= 768) return 'tablet'
  return 'phone'
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
