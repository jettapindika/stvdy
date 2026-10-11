import { invoke } from '@tauri-apps/api/core'
import { onOpenUrl } from '@tauri-apps/plugin-deep-link'
import { open } from '@tauri-apps/plugin-dialog'
import { readFile } from '@tauri-apps/plugin-fs'
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'
import { openUrl } from '@tauri-apps/plugin-opener'
import { platform as osPlatform } from '@tauri-apps/plugin-os'
import { check } from '@tauri-apps/plugin-updater'

import { formFactorFor } from './index'
import type { Platform, PlatformKind } from './types'
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
    formFactor: () => formFactorFor(kind),

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
      async current() {
        const urls = await invoke<string[] | null>('plugin:deep-link|get_current')
        return urls?.[0] ?? null
      },
      async onOpen(handler) {
        return onOpenUrl((urls) => urls.forEach(handler))
      },
    },

    openExternal: (url) => openUrl(url),

    async checkForUpdate() {
      const update = await check()
      return update !== null
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
