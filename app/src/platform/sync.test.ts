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
import { webPlatform } from './web'

/**
 * The two halves of `Platform.sync`: the shell's, and the web build's.
 *
 * The web half is the one that differs, and it is the one that could lie. A
 * browser has no SQLite and no outbox (A14), so there is nowhere for an offline
 * write to go; a `write` that resolved while dropping the payload would mark a row
 * saved, and the next pull would quietly overwrite it with the server's older
 * copy. The refusal is therefore the contract, and it names the REST route because
 * that is the caller's actual remedy.
 *
 * The shell half is tested at the boundary it owns: the IPC command names, the
 * snake_case payloads the Rust commands take, and the three events. The mapping
 * from `last_synced_at` to `lastSyncedAt` matters because a status control that
 * read the wrong key would render "Not synced yet" over a device that had just
 * synced — a lie with no error attached to it.
 */

describe('the web build has no local database', () => {
  it('reports an empty outbox and no stored timestamp', async () => {
    const sync = webPlatform().sync

    // `outbox: 0` is a fact about the platform rather than a flushed queue, and it
    // is what keeps the status control from ever claiming a pending write here.
    await expect(sync.status()).resolves.toEqual({
      state: 'synced',
      code: null,
      lastSyncedAt: null,
      outbox: 0,
    })
    await expect(sync.syncNow()).resolves.toEqual({
      state: 'synced',
      code: null,
      lastSyncedAt: null,
      outbox: 0,
    })
  })

  it('reads no cached row and lists none', async () => {
    const sync = webPlatform().sync

    // An empty list and an absent row are honest answers — "this device has no
    // cached copy" — and the caller's remedy is the REST route either way.
    await expect(sync.read('task')).resolves.toEqual([])
    await expect(sync.read('task', 't1')).resolves.toBeNull()
  })

  it('refuses an offline write rather than queueing one it cannot deliver', async () => {
    const sync = webPlatform().sync

    // The message names the entity and points at the REST route, because a caller
    // that is refused needs to know what to do instead.
    await expect(sync.write('task', 't1', { title: 'Bab 4' })).rejects.toThrow(/task/)
    await expect(sync.write('task', 't1', { title: 'Bab 4' })).rejects.toThrow(/REST route/)
    await expect(sync.tombstone('task', 't1')).rejects.toThrow(/REST route/)
  })

  it('has nothing to reset and no status to report', async () => {
    const sync = webPlatform().sync

    // A no-op rather than a throw: "reset and resync" is wired once for both form
    // factors, and there is no local data here for it to delete.
    await expect(sync.reset()).resolves.toBeUndefined()
    await expect(sync.onStatus(() => {})).resolves.toBeTypeOf('function')
  })
})

describe('the shell adapter', () => {
  /** Attaches the listeners `configureSync` registers and returns their callbacks. */
  function scriptShell() {
    const listeners = new Map<string, (event: { payload: unknown }) => void>()
    listenMock.mockImplementation((name: string, callback: (event: { payload: unknown }) => void) => {
      listeners.set(name, callback)
      return Promise.resolve(() => {})
    })
    invokeMock.mockResolvedValue(undefined)
    return listeners
  }

  async function configuredPlatform() {
    const listeners = scriptShell()
    const platform = desktopPlatform('macos')
    const reports: unknown[] = []
    await platform.configureSync({
      baseUrl: 'https://api.stvdy.test',
      deviceId: 'd1',
      accessToken: 'access-1',
      refresh: vi.fn(async () => 'access-2'),
    })
    await platform.sync.onStatus((report) => reports.push(report))
    return { platform, listeners, reports }
  }

  it('hands the shell its credentials in the command it expects', async () => {
    const { platform } = await configuredPlatform()

    expect(invokeMock).toHaveBeenCalledWith('sync_configure', {
      base_url: 'https://api.stvdy.test',
      access_token: 'access-1',
      device_id: 'd1',
    })
    // The seven members A186 fixes, and the token rides in configuration rather
    // than in any of them.
    expect(Object.keys(platform.sync).sort()).toEqual([
      'onStatus',
      'read',
      'reset',
      'status',
      'syncNow',
      'tombstone',
      'write',
    ])
  })

  it('maps the shell status event onto the report the control reads', async () => {
    const { listeners, reports } = await configuredPlatform()

    listeners.get('stvdy://sync-status')?.({
      payload: { state: 'needs_attention', code: 'read_only_entity', last_synced_at: '2026-10-11T08:00:00Z', outbox: 2 },
    })

    expect(reports).toEqual([
      {
        state: 'needs_attention',
        code: 'read_only_entity',
        lastSyncedAt: '2026-10-11T08:00:00Z',
        outbox: 2,
      },
    ])
  })

  it('reports a revoked device on the same channel, so one subscriber signs out', async () => {
    const { listeners, reports } = await configuredPlatform()

    // The shell has already wiped the cache and the outbox by the time this
    // arrives (A186); the SPA's only remaining job is to end the session.
    listeners.get('stvdy://sync-revoked')?.({ payload: null })

    expect(reports).toEqual([{ state: 'revoked', code: null, lastSyncedAt: null, outbox: 0 }])
  })

  it('refreshes, reconfigures and runs a pass when the shell loses its token', async () => {
    const listeners = scriptShell()
    const refresh = vi.fn(async () => 'access-2')
    const platform = desktopPlatform('macos')
    await platform.configureSync({
      baseUrl: 'https://api.stvdy.test',
      deviceId: 'd1',
      accessToken: 'access-1',
      refresh,
    })
    invokeMock.mockClear()

    listeners.get('stvdy://sync-auth-expired')?.({ payload: null })
    // The handler is async and fire-and-forget from the event's point of view.
    await vi.waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('sync_now')
    })

    // The refresh is the API layer's, handed over as a capability: the shell has
    // no way to obtain a token itself, and a stale one would 401 every pass.
    expect(refresh).toHaveBeenCalled()
    expect(invokeMock).toHaveBeenCalledWith('sync_configure', {
      base_url: 'https://api.stvdy.test',
      access_token: 'access-2',
      device_id: 'd1',
    })
  })

  it('unwraps the shell’s cached rows and drops one it could not load', async () => {
    const { platform } = await configuredPlatform()
    invokeMock.mockResolvedValueOnce([
      { id: 't1', row: { id: 't1', title: 'Bab 4' } },
      { id: 't2', row: null },
    ])

    await expect(platform.sync.read('task')).resolves.toEqual([{ id: 't1', title: 'Bab 4' }])
    expect(invokeMock).toHaveBeenCalledWith('sync_read', { entity: 'task', id: null })

    invokeMock.mockResolvedValueOnce({ id: 't1', title: 'Bab 4' })
    await expect(platform.sync.read('task', 't1')).resolves.toEqual({ id: 't1', title: 'Bab 4' })
    expect(invokeMock).toHaveBeenCalledWith('sync_read', { entity: 'task', id: 't1' })
  })

  it('sends a local write, a tombstone and a reset to the shell', async () => {
    const { platform } = await configuredPlatform()
    invokeMock.mockClear()

    await platform.sync.write('task', 't1', { title: 'Bab 4' })
    await platform.sync.tombstone('task', 't1')
    await platform.sync.reset()

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'sync_write', {
      entity: 'task',
      id: 't1',
      payload: { title: 'Bab 4' },
    })
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'sync_tombstone', { entity: 'task', id: 't1' })
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'sync_reset')
  })

  it('gives the phone the same sync surface as the laptop', async () => {
    // The mobile adapter spreads the desktop one rather than restating it, so the
    // two cannot drift: the same engine, the same cache, the same commands.
    const phone = mobilePlatform('ios')
    expect(Object.keys(phone.sync).sort()).toEqual(Object.keys(desktopPlatform('macos').sync).sort())
    expect(typeof phone.configureSync).toBe('function')
  })
})
