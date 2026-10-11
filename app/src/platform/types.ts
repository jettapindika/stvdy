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

export interface DeepLinksApi {
  /** The URL the app was launched with, if any. */
  current(): Promise<string | null>
  /** Subscribes to incoming links; resolves to an unsubscribe function. */
  onOpen(handler: (url: string) => void): Promise<() => void>
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
