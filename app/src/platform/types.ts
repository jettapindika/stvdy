export type PlatformKind = 'web' | 'macos' | 'windows' | 'ios' | 'android'
export type Runtime = 'web' | 'tauri'
export type SyncStatus = 'synced' | 'syncing' | 'offline' | 'error'

export interface SecureStore {
  /** Stores a secret in the OS keychain/keystore. Web throws. */
  set(key: string, value: string): Promise<void>
  get(key: string): Promise<string | null>
  delete(key: string): Promise<void>
}

export interface NotificationsApi {
  /** Requests permission and returns whether notifications may be shown. */
  requestPermission(): Promise<boolean>
  notify(title: string, body: string): Promise<void>
  /**
   * Schedules a local notification. Mobile honours `at` and fires while the app
   * is closed; desktop and web cannot schedule, so they deliver immediately. * callers must treat scheduling as best-effort and not assume parity.
   */
  schedule(title: string, body: string, at: Date): Promise<void>
}

export interface FilesApi {
  /** Opens a picker and returns local paths, or null when cancelled. */
  pick(options?: { multiple?: boolean; mimeTypes?: string[] }): Promise<string[] | null>
  /** Reads a picked file as bytes. */
  read(path: string): Promise<Uint8Array>
}

/**
 * A classified incoming URL, as the shell delivers it. The SPA never sees a raw
 * URL string: `auth_callback` carries the OAuth `code`/`state` to exchange,
 * `calendar` carries a calendar-return's query parameters, and `ignored` says a
 * URL was not actionable (with a coarse reason safe to log).
 */
export type DeepLinkIntent =
  | { action: 'auth_callback'; payload: { code: string; state: string } }
  // The Google login return: an opaque single-use ticket, never tokens, because
  // a deep link lands in browser history and the OS URL handler (A48).
  | { action: 'auth_ticket'; payload: { ticket: string } }
  | { action: 'calendar'; payload: { params: Record<string, string> } }
  | { action: 'ignored'; payload: { reason: string } }

export interface DeepLinksApi {
  /** The classified URL the app was launched with, if any. */
  current(): Promise<DeepLinkIntent | null>
  /** Subscribes to incoming links; resolves to an unsubscribe function. */
  onOpen(handler: (intent: DeepLinkIntent) => void): Promise<() => void>
}

export interface Platform {
  kind(): PlatformKind
  runtime(): Runtime
  secureStore: SecureStore
  notifications: NotificationsApi
  files: FilesApi
  deepLinks: DeepLinksApi
  openExternal(url: string): Promise<void>
  /** Haptics are a no-op off mobile. */
  haptic(kind: 'light' | 'medium' | 'success'): Promise<void>
  /** Desktop only; resolves false elsewhere. */
  checkForUpdate(): Promise<boolean>
}
