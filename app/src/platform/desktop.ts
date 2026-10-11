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

import type { DeepLinkIntent, Platform, PlatformKind } from './types'
import { webPlatform } from './web'

/**
 * Tauri adapter for macOS and Windows.
 *
 * Secrets go through the Rust keychain commands rather than any JS storage: only
 * the refresh token is persisted (the access token stays in memory), and the
 * store is the OS keychain/keystore, never a file.
 */
export function desktopPlatform(kind: PlatformKind): Platform {
  const base = webPlatform(kind)

  return {
    ...base,
    runtime: () => 'tauri',

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
