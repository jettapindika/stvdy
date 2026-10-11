import { vi } from 'vitest'

import { installPlatform, type PlatformKind, type SecureStore } from '@/platform'
import { webPlatform } from '@/platform/web'

/**
 * Test doubles for the two things the API layer talks to: the platform's secure
 * store and `fetch`.
 *
 * The secure store is faked through `installPlatform`, which is the platform
 * adapter's own seam for exactly this (the Tauri shell installs its own adapter
 * the same way). Faking it here rather than mocking the module means the tests
 * exercise the real `tokens.ts` and the real client against a store that behaves
 * like a keychain, including the part that matters: what is in it afterwards.
 *
 * `fetch` is scripted per test rather than globally stubbed, because almost every
 * test here is about *which* requests were made and in what order, and that is
 * easier to read from a list of calls than from assertions spread across mocks.
 */

export interface FakeSecureStore {
  values: Map<string, string>
  deleted: string[]
}

export function installFakePlatform(kind: PlatformKind = 'web'): FakeSecureStore {
  const values = new Map<string, string>()
  const deleted: string[] = []

  const secureStore: SecureStore = {
    async set(key, value) {
      values.set(key, value)
    },
    async get(key) {
      return values.get(key) ?? null
    },
    async delete(key) {
      deleted.push(key)
      values.delete(key)
    },
  }

  installPlatform({ ...webPlatform(kind), secureStore })
  return { values, deleted }
}

/**
 * A platform with no usable keychain, which is what the web adapter really is:
 * `set` and `delete` throw, and `get` answers null. The app has to keep working
 * in the tab and simply be unable to restore the session later.
 */
export function installFailingSecureStore(kind: PlatformKind = 'web'): void {
  installPlatform({
    ...webPlatform(kind),
    secureStore: {
      async set() {
        throw new Error('secure storage is not available in the web build')
      },
      async get() {
        return null
      },
      async delete() {
        throw new Error('secure storage is not available in the web build')
      },
    },
  })
}

/**
 * A response shaped like the ones `fetch` returns, built from a status and a JSON
 * body. Only the three members the client reads are implemented, so a test never
 * depends on a global `Response` that jsdom does not provide.
 */
export function jsonResponse(status: number, body?: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (body === undefined ? '' : JSON.stringify(body)),
  } as Response
}

export interface RecordedCall {
  url: string
  init: RequestInit
}

export function mockFetch(
  handler: (url: string, init: RequestInit) => Response | Promise<Response>,
): RecordedCall[] {
  const calls: RecordedCall[] = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString()
      const request = init ?? {}
      calls.push({ url, init: request })
      return handler(url, request)
    }),
  )
  return calls
}

/** The `authorization` header a request carried, if any. */
export function bearerOf(call: RecordedCall): string | null {
  const headers = call.init.headers as Record<string, string> | undefined
  return headers?.authorization ?? null
}

/**
 * jsdom has no `matchMedia`, and the shell's form-factor hook subscribes to it.
 * Without this the whole app throws on render rather than reporting a layout.
 */
export function stubViewport(width: number): void {
  Object.defineProperty(window, 'innerWidth', { writable: true, configurable: true, value: width })
  vi.stubGlobal('matchMedia', (query: string) => {
    const min = Number(/min-width:\s*(\d+)px/.exec(query)?.[1] ?? 0)
    return {
      matches: width >= min,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      onchange: null,
      dispatchEvent: () => false,
    } as unknown as MediaQueryList
  })
}
