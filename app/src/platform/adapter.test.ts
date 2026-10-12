import { describe, expect, it, vi } from 'vitest'

const { listenMock, invokeMock, pluginListenerMock } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  invokeMock: vi.fn(),
  pluginListenerMock: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
  addPluginListener: (...args: unknown[]) => pluginListenerMock(...args),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}))

import { desktopPlatform } from './desktop'
import { mobilePlatform } from './mobile'
import { webPlatform } from './web'

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

/**
 * Push, which exists on the phones and nowhere else (A329).
 *
 * Two halves. The **capability** half is that the member is absent off mobile, which
 * is the shape `Platform.push` is optional for; the **forwarding** half is that the
 * shell's and the plugin's events reach a subscriber through this adapter rather than
 * being reimplemented per caller.
 */
describe('the push adapter', () => {
  /** A plugin listener that records the callback it was handed. */
  function scriptedListener() {
    let delivered: ((payload: never) => void) | null = null
    const unregister = vi.fn().mockResolvedValue(undefined)
    pluginListenerMock.mockImplementationOnce(
      async (_plugin: string, _event: string, callback: (payload: never) => void) => {
        delivered = callback
        return { unregister }
      },
    )
    return {
      unregister,
      deliver(payload: unknown) {
        if (delivered === null) throw new Error('no listener was registered')
        ;(delivered as unknown as (value: unknown) => void)(payload)
      },
    }
  }

  it('is absent on the web build and on desktop', () => {
    // Not a stub that resolves, and not a throw: a browser tab and a laptop have no
    // APNs/FCM token to mint and no provider to name. The absence is the answer, and
    // the caller says what it does instead — which is why this member is the one
    // `Platform` declares as optional.
    expect(webPlatform().push).toBeUndefined()
    expect(desktopPlatform('macos').push).toBeUndefined()
    expect(desktopPlatform('windows').push).toBeUndefined()
  })

  it('asks the shell for the token, not the plugin', async () => {
    invokeMock.mockResolvedValueOnce({ provider: 'apns', token: 'tok-1' })

    await expect(mobilePlatform('ios').push?.token()).resolves.toEqual({
      provider: 'apns',
      token: 'tok-1',
    })
    // The shell's command is where the provider is derived and the token is
    // validated, so this is the one call that may mint a provider.
    expect(invokeMock).toHaveBeenCalledWith('push_token')
  })

  it('forwards the shell’s token event to the subscriber', async () => {
    const listener = scriptedListener()
    const handler = vi.fn()

    const off = await mobilePlatform('ios').push?.onToken(handler)

    // The plugin's own event names, because a rotation is the plugin's to report:
    // APNs/FCM re-issue a token and nothing else in the app hears it.
    expect(pluginListenerMock).toHaveBeenCalledWith(
      'notifications',
      'push-token',
      expect.any(Function),
    )

    listener.deliver({ token: 'tok-2' })

    // The token string only. The provider mapping stays the shell's, and the caller
    // re-reads the pair through `token()` — one mapping, one `PUT`.
    expect(handler).toHaveBeenCalledWith('tok-2')

    off?.()
    expect(listener.unregister).toHaveBeenCalledTimes(1)
  })

  it('activates the click listener so a cold-start tap is replayed', async () => {
    // A tap that launched the app arrives before anything is listening. The plugin
    // holds it and replays it the moment the listener says it is active — which is
    // the notification half of the shell's `deep_link_current`, and the reason the
    // activation has to happen *after* the listener is registered.
    const listener = scriptedListener()
    invokeMock.mockResolvedValue(undefined)
    const handler = vi.fn()

    const off = await mobilePlatform('ios').push?.onNotification(handler)

    expect(pluginListenerMock).toHaveBeenCalledWith(
      'notifications',
      'notificationClicked',
      expect.any(Function),
    )
    expect(invokeMock).toHaveBeenCalledWith('plugin:notifications|set_click_listener_active', {
      active: true,
    })

    listener.deliver({ id: 3, data: { reminder_id: 'r1', kind: 'task_due' } })
    expect(handler).toHaveBeenCalledWith({ id: 3, data: { reminder_id: 'r1', kind: 'task_due' } })

    // Back to holding rather than dropping: the plugin only buffers a tap while no
    // listener claims the event, so leaving it active would throw the next tap away.
    invokeMock.mockClear()
    off?.()
    expect(invokeMock).toHaveBeenCalledWith('plugin:notifications|set_click_listener_active', {
      active: false,
    })
    expect(listener.unregister).toHaveBeenCalledTimes(1)
  })
})

/**
 * The local-notification path, and the one field that would fail silently.
 *
 * One plugin serves every platform now (A310 as revised), so the payload shape is the
 * SPA's half of a contract with `tauri-plugin-notifications`' Rust deserializer. The
 * plugin is a fork of the official one, and the official one's JS wrapper named the
 * instant `at` while the fork's `Schedule::At` names it `date` — a mix-up that produces
 * no error, just a notification that never fires. So the assertion is on the key.
 */
describe('the local-notification adapter', () => {
  it('schedules a phone reminder with the field the plugin deserializes', async () => {
    invokeMock.mockResolvedValue(true)
    const at = new Date('2026-10-16T06:00:00.000Z')

    await mobilePlatform('ios').notifications.schedule('Due', 'Hand in the essay', at)

    expect(invokeMock).toHaveBeenCalledWith('plugin:notifications|notify', {
      options: {
        title: 'Due',
        body: 'Hand in the essay',
        schedule: { date: at.toISOString(), allowWhileIdle: true },
      },
    })
  })

  it('delivers a desktop reminder now, because notify-rust cannot schedule one', async () => {
    invokeMock.mockResolvedValue(true)

    await desktopPlatform('macos').notifications.schedule('Due', 'Hand in the essay', new Date())

    expect(invokeMock).toHaveBeenCalledWith('plugin:notifications|notify', {
      options: { title: 'Due', body: 'Hand in the essay' },
    })
  })

  it('sends nothing when permission was refused', async () => {
    invokeMock.mockResolvedValue(false)
    invokeMock.mockClear()

    await mobilePlatform('android').notifications.schedule('Due', 'body', new Date())
    await desktopPlatform('macos').notifications.notify('Due', 'body')

    expect(invokeMock).not.toHaveBeenCalledWith('plugin:notifications|notify', expect.anything())
  })
})
