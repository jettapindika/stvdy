import { describe, expect, it, vi } from 'vitest'

const { listenMock, invokeMock } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  invokeMock: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}))

import { desktopPlatform } from './desktop'
import { mobilePlatform } from './mobile'

/**
 * The shell classifies a URL and hands the adapter `{action, payload}`. These
 * tests pin that contract from the SPA side: the command result and the emitted
 * event both surface as a classified intent, never a raw URL string.
 */
describe('deep-link adapter', () => {
  it('pulls the cold-start intent from the shell command', async () => {
    invokeMock.mockResolvedValueOnce([
      { action: 'auth_callback', payload: { code: 'c', state: 's' } },
    ])

    const intent = await desktopPlatform('macos').deepLinks.current()

    expect(invokeMock).toHaveBeenCalledWith('deep_link_current')
    expect(intent).toEqual({ action: 'auth_callback', payload: { code: 'c', state: 's' } })
  })

  it('returns null when the app was not launched with a link', async () => {
    invokeMock.mockResolvedValueOnce([])
    await expect(desktopPlatform('macos').deepLinks.current()).resolves.toBeNull()
  })

  it('forwards each intent from the shell event to the handler', async () => {
    const handler = vi.fn()
    const unlisten = vi.fn()
    listenMock.mockResolvedValueOnce(unlisten)

    const platform = desktopPlatform('macos')
    const off = await platform.deepLinks.onOpen(handler)

    expect(off).toBe(unlisten)
    const [eventName, callback] = listenMock.mock.calls[0] as [
      string,
      (event: { payload: unknown[] }) => void,
    ]
    expect(eventName).toBe('stvdy://deep-link-intent')

    callback({
      payload: [
        { action: 'calendar', payload: { params: { provider: 'google' } } },
        { action: 'ignored', payload: { reason: 'not_stvdy_scheme' } },
      ],
    })

    expect(handler).toHaveBeenCalledTimes(2)
    expect(handler).toHaveBeenNthCalledWith(1, {
      action: 'calendar',
      payload: { params: { provider: 'google' } },
    })
    expect(handler).toHaveBeenNthCalledWith(2, {
      action: 'ignored',
      payload: { reason: 'not_stvdy_scheme' },
    })
  })
})

/**
 * The keychain leg: the adapter must route the refresh token through the Rust
 * `secret_*` commands (which own the OS keychain), never JS storage. A silent
 * no-op here is how a session is lost without an error.
 */
describe('secure store adapter', () => {
  it('routes set/get/delete through the shell keychain commands', async () => {
    invokeMock.mockResolvedValueOnce(undefined)
    invokeMock.mockResolvedValueOnce('refresh-value')
    invokeMock.mockResolvedValueOnce(undefined)

    const platform = desktopPlatform('macos')
    await platform.secureStore.set('stvdy.refresh_token', 'refresh-value')
    await expect(platform.secureStore.get('stvdy.refresh_token')).resolves.toBe('refresh-value')
    await platform.secureStore.delete('stvdy.refresh_token')

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'secret_set', {
      key: 'stvdy.refresh_token',
      value: 'refresh-value',
    })
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'secret_get', { key: 'stvdy.refresh_token' })
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'secret_delete', { key: 'stvdy.refresh_token' })
  })

  it('gives mobile the same classified deep-link and keychain path as desktop', async () => {
    invokeMock.mockResolvedValueOnce([])
    await expect(mobilePlatform('ios').deepLinks.current()).resolves.toBeNull()
    expect(invokeMock).toHaveBeenCalledWith('deep_link_current')

    invokeMock.mockResolvedValueOnce(undefined)
    await mobilePlatform('ios').secureStore.set('k', 'v')
    expect(invokeMock).toHaveBeenCalledWith('secret_set', { key: 'k', value: 'v' })
  })
})
