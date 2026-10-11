import type {
  DeepLinksApi,
  FilesApi,
  NotificationsApi,
  Platform,
  PlatformKind,
  SecureStore,
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

export function webPlatform(kind: PlatformKind = 'web'): Platform {
  return {
    kind: () => kind,
    runtime: () => 'web',
    formFactor: () => (kind === 'web' ? 'web' : 'phone'),
    secureStore,
    notifications,
    files,
    deepLinks,
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
