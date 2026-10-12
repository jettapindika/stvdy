import { invoke } from '@tauri-apps/api/core'
import { impactFeedback, notificationFeedback } from '@tauri-apps/plugin-haptics'
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification'
import { openUrl } from '@tauri-apps/plugin-opener'

import { desktopPlatform, type NativePlatform } from './desktop'
import type { PlatformKind } from './types'

/**
 * Tauri adapter for iOS and Android.
 *
 * Differs from desktop in the two places that matter: local notifications can be
 * *scheduled* and fire while the app is closed, and haptics exist. Updates ship
 * through the stores, so there is no updater path here.
 *
 * Everything else — the keychain, the deep-link classification, and the whole
 * sync surface including `configureSync` — is the desktop implementation, spread
 * rather than restated. The phone and the laptop run the same engine over the same
 * SQLite cache; a second copy here is how the two would drift, and the spec's
 * "one account, every device" is exactly the property that a drift would break.
 */
export function mobilePlatform(kind: PlatformKind): NativePlatform {
  const base = desktopPlatform(kind)

  return {
    ...base,

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
