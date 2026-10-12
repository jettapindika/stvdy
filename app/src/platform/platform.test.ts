import { describe, expect, it } from 'vitest'

import { detectRuntime, isTauri } from './index'
import { webPlatform } from './web'

describe('runtime detection', () => {
  it('detects the plain web build', () => {
    expect(detectRuntime({})).toBe('web')
    expect(detectRuntime({ __TAURI_INTERNALS__: {} })).toBe('tauri')
  })

  it('does not claim Tauri in the test environment', () => {
    expect(isTauri()).toBe(false)
  })
})

describe('platform identity', () => {
  it('reports the platform kind, which billing policy and update behaviour key off', () => {
    expect(webPlatform('web').kind()).toBe('web')
    expect(webPlatform('ios').kind()).toBe('ios')
  })

  it('does not expose a layout form factor', () => {
    // Layout is derived from viewport width (lib/useFormFactor.ts), not from the
    // platform. When it lived here, each adapter hardcoded its own answer and a
    // phone browser reported `web`, so the app rendered no navigation at all.
    expect('formFactor' in webPlatform('web')).toBe(false)
  })
})

describe('web adapter degradation', () => {
  it('throws rather than silently pretending to store a secret', async () => {
    const platform = webPlatform()
    // A silent no-op here would mean the user's session is never persisted and
    // the failure looks like a random logout later.
    await expect(platform.secureStore.set('refresh', 'x')).rejects.toThrow(/not available/)
  })

  it('reports no update path on the web', async () => {
    // `unsupported` rather than a rejection or `false`: the Updates panel is absent
    // in a browser tab, and a value is what lets it decide that without catching an
    // exception on mount (A336).
    expect(await webPlatform().checkForUpdate()).toEqual({ kind: 'unsupported' })
  })

  it('reports no update path on a phone either', async () => {
    // Mobile updates ship through TestFlight and Play; the Tauri updater declares
    // `ios: none, android: none` (A335).
    expect(await webPlatform('ios').checkForUpdate()).toEqual({ kind: 'unsupported' })
  })
})
