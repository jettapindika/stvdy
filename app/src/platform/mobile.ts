import { addPluginListener, invoke } from '@tauri-apps/api/core'
import { impactFeedback, notificationFeedback } from '@tauri-apps/plugin-haptics'
import { openUrl } from '@tauri-apps/plugin-opener'

import { desktopPlatform, type NativePlatform } from './desktop'
import type { NotificationTap, PlatformKind, PushToken } from './types'

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
      // The mobile half of A310's platform split, and the shape differs from the desktop's
      // because the plugin does: these are `invoke` calls to
      // `tauri-plugin-notifications` (Choochmeque), whose `Schedule::At` names the instant
      // `date` where the official plugin's JS wrapper took `at`. The fork's npm wrapper is
      // not published for this version, which is why the calls are raw — the click listener
      // below was already written this way.
      async requestPermission() {
        if ((await invoke<boolean | null>('plugin:notifications|is_permission_granted')) === true) {
          return true
        }
        return (await invoke<string>('plugin:notifications|request_permission')) === 'granted'
      },
      async notify(title, body) {
        if ((await invoke<boolean | null>('plugin:notifications|is_permission_granted')) !== true) {
          return
        }
        await invoke('plugin:notifications|notify', { options: { title, body } })
      },
      async schedule(title, body, at) {
        if ((await invoke<boolean | null>('plugin:notifications|is_permission_granted')) !== true) {
          return
        }
        // Local scheduling survives app restarts and works offline, which is why
        // the client schedules reminders from synced data rather than waiting
        // for the server.
        //
        // `date`, not `at` — the two crates are forks, so the field names are close enough
        // to be a silent no-op if they are mixed up. `adapter.test.ts` asserts the key.
        await invoke('plugin:notifications|notify', {
          options: {
            title,
            body,
            schedule: { date: at.toISOString(), allowWhileIdle: true },
          },
        })
      },
    },

    openExternal: (url) => openUrl(url),

    /**
     * Push, which is the one capability that exists here and nowhere else (A329).
     *
     * Three things about the shape, each of which is a decision rather than a
     * transcription:
     *
     * 1. **`token()` is the shell's command, not the plugin's.** The shell is where
     *    the provider is decided (`push::provider_for`), where the token is checked
     *    non-blank before it can become a round trip, and where the token is kept out
     *    of the log. Going to the plugin directly would be a second provider mapping
     *    and a second validation.
     * 2. **`onToken` is the plugin's event, because a rotation is the plugin's to
     *    report.** APNs/FCM mint a new token and tell the plugin; the shell has no
     *    event for it, and the SPA has no other way to hear it. The callback hands
     *    over the token string only — the caller re-reads the pair through `token()`
     *    so there is still exactly one provider mapping.
     * 3. **`onNotification` drains the plugin's cold-start buffer.** A tap that
     *    launched the app arrives before any listener exists, and the plugin holds it
     *    (`pendingNotificationClick`) until something says it is listening. This is
     *    the notification half of the shell's `deep_link_current`: without the
     *    activation below, a tap that opened the app is the tap that does nothing.
     */
    push: {
      async token() {
        return await invoke<PushToken>('push_token')
      },

      async onToken(handler) {
        // `addPluginListener` is the wrapper for the `register_listener` command the
        // plugin implements natively on mobile; its event names are the plugin's
        // own (`push-token`, `notificationClicked`), not ours.
        const listener = await addPluginListener<{ token: string }>(
          'notifications',
          'push-token',
          (payload) => handler(payload.token),
        )
        return () => {
          void listener.unregister()
        }
      },

      async onNotification(handler) {
        const listener = await addPluginListener<NotificationTap>(
          'notifications',
          'notificationClicked',
          handler,
        )
        // Activating *after* the listener is registered is the order that matters:
        // the plugin replays a held tap the moment this flips true, so flipping it
        // first would hand the tap to a channel nothing is reading.
        await invoke('plugin:notifications|set_click_listener_active', { active: true })
        return () => {
          // Back to holding, not to dropping. The plugin only *buffers* a tap while
          // no listener claims the event, so leaving this true after the last
          // subscriber left would throw the next tap away instead of keeping it for
          // whoever mounts next.
          void invoke('plugin:notifications|set_click_listener_active', { active: false })
          void listener.unregister()
        }
      },
    },

    async haptic(hapticKind) {
      if (hapticKind === 'success') {
        await notificationFeedback('success')
      } else {
        await impactFeedback(hapticKind)
      }
    },

    async checkForUpdate() {
      // Mobile updates ship through TestFlight and Play; the Tauri updater declares
      // `ios: none, android: none` (A335), so there is no endpoint to ask.
      return { kind: 'unsupported' } as const
    },
    async installUpdate() {
      throw new Error('installing an update is not available on mobile')
    },
  }
}
