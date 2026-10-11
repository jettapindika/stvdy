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

describe('form factor is defined once', () => {
  it('every adapter agrees with the canonical mapping', async () => {
    // Regression guard: web.ts used to return 'phone' for 'macos'/'windows',
    // contradicting formFactorFor and rendering desktops as phones.
    const { webPlatform } = await import('./web')
    const { desktopPlatform } = await import('./desktop')
    const { mobilePlatform } = await import('./mobile')

    for (const kind of ['macos', 'windows'] as const) {
      expect(desktopPlatform(kind).formFactor()).toBe('desktop')
      expect(webPlatform(kind).formFactor()).toBe('desktop')
    }
    for (const kind of ['ios', 'android'] as const) {
      expect(mobilePlatform(kind).formFactor()).toBe('phone')
      expect(webPlatform(kind).formFactor()).toBe('phone')
    }
    expect(webPlatform('web').formFactor()).toBe('web')
  })

  it('reserves tablet for a width, not a platform', async () => {
    const { formFactorFor, formFactorForWidth } = await import('./index')
    // No platform kind can ever be a tablet, which is why the variant must not
    // be reachable from formFactorFor.
    for (const kind of ['web', 'macos', 'windows', 'ios', 'android'] as const) {
      expect(formFactorFor(kind)).not.toBe('tablet')
    }
    expect(formFactorForWidth(1024)).toBe('desktop')
    expect(formFactorForWidth(800)).toBe('tablet')
    expect(formFactorForWidth(390)).toBe('phone')
  })
})
