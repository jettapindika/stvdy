import { invoke } from '@tauri-apps/api/core'
import { onOpenUrl } from '@tauri-apps/plugin-deep-link'
import { impactFeedback, notificationFeedback } from '@tauri-apps/plugin-haptics'
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'
import { openUrl } from '@tauri-apps/plugin-opener'

import { desktopPlatform } from './desktop'
import type { Platform, PlatformKind } from './types'

/**
 * Tauri adapter for iOS and Android.
 *
 * Differs from desktop in the two places that matter: local notifications can be
 * *scheduled* and fire while the app is closed, and haptics exist. Updates ship
 * through the stores, so there is no updater path here.
 */
export function mobilePlatform(kind: PlatformKind): Platform {
  const base = desktopPlatform(kind)

  return {
    ...base,
    formFactor: () => 'phone',

    notifications: {
      async requestPermission() {
        if (await isPermissionGranted()) return true
        return (await requestPermission()) === 'granted'
      },
      async notify(title, body) {
        if (await isPermissionGranted()) sendNotification({ title, body })
      },
      async schedule(title, body, at) {
        if (!(await isPermissionGranted())) return
        // Local scheduling survives app restarts and works offline, which is why
        // the client schedules reminders from synced data rather than waiting
        // for the server.
        await invoke('plugin:notification|notify', {
          options: {
            title,
            body,
            schedule: { at: at.toISOString(), allowWhileIdle: true },
          },
        })
      },
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

    async haptic(hapticKind) {
      if (hapticKind === 'success') {
        await notificationFeedback('success')
      } else {
        await impactFeedback(hapticKind)
      }
    },

    async checkForUpdate() {
      // Mobile updates ship through TestFlight / Play.
      return false
    },
  }
}
