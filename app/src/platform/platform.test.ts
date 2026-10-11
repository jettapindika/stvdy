import { describe, expect, it } from 'vitest'

import { detectRuntime, formFactorFor, isTauri } from './index'
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

describe('form factor', () => {
  it('maps platforms to a layout form factor', () => {
    expect(formFactorFor('ios')).toBe('phone')
    expect(formFactorFor('android')).toBe('phone')
    expect(formFactorFor('macos')).toBe('desktop')
    expect(formFactorFor('windows')).toBe('desktop')
    expect(formFactorFor('web')).toBe('web')
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
    expect(await webPlatform().checkForUpdate()).toBe(false)
  })
})
