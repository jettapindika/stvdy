import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, render, waitFor } from '@testing-library/react'
import { createElement } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { settingsKeys } from '@/api/queries/settings'
import { ThemeSync } from '@/features/settings/ThemeSync'

import { COLORS, COURSES } from '@/tokens/tokens'
import { applyAppearance, initializeAppearance } from './appearance'

const ROOT_STYLE_NAMES = [
  ...Object.keys(COLORS.light),
  ...Object.keys(COURSES.light).flatMap((name) => [`course-${name}`, `course-${name}-tint`]),
]

let systemPalette: HTMLStyleElement
let queryClient: QueryClient | undefined

function clearRoot() {
  document.documentElement.removeAttribute('data-theme')
  document.documentElement.style.cssText = ''
}

describe('appearance preference precedence', () => {
  beforeEach(() => {
    clearRoot()
    localStorage.clear()
    // jsdom does not evaluate OS media queries; this is the active dark stylesheet palette.
    systemPalette = document.createElement('style')
    systemPalette.textContent = `:root { ${[
      ...Object.entries(COLORS.dark).map(([name, value]) => `--${name}: ${value};`),
      ...Object.entries(COURSES.dark).flatMap(([name, value]) => [
        `--course-${name}: ${value.marker};`,
        `--course-${name}-tint: ${value.tint};`,
      ]),
    ].join(' ')} }`
    document.head.append(systemPalette)
  })

  afterEach(() => {
    cleanup()
    queryClient?.clear()
    queryClient = undefined
    clearRoot()
    systemPalette.remove()
    vi.restoreAllMocks()
  })

  it('keeps explicit light values when the OS palette is dark', () => {
    expect(getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()).toBe(
      COLORS.dark.bg,
    )
    applyAppearance('light')

    expect(document.documentElement.dataset.theme).toBe('light')
    expect(document.documentElement.style.colorScheme).toBe('light')
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.light.bg)
    expect(document.documentElement.style.getPropertyValue('--course-indigo')).toBe(
      COURSES.light.indigo!.marker,
    )
    expect(document.documentElement.style.getPropertyValue('--course-indigo-tint')).toBe(
      COURSES.light.indigo!.tint,
    )
    expect(getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()).toBe(
      COLORS.light.bg,
    )
  })

  it('applies explicit dark values and color scheme', () => {
    systemPalette.textContent = `:root { --surface: ${COLORS.light.surface}; }`
    applyAppearance('dark')

    expect(document.documentElement.dataset.theme).toBe('dark')
    expect(document.documentElement.style.colorScheme).toBe('dark')
    expect(document.documentElement.style.getPropertyValue('--surface')).toBe(COLORS.dark.surface)
    expect(document.documentElement.style.getPropertyValue('--course-rose')).toBe(
      COURSES.dark.rose!.marker,
    )
    expect(document.documentElement.style.getPropertyValue('--course-rose-tint')).toBe(
      COURSES.dark.rose!.tint,
    )
    expect(getComputedStyle(document.documentElement).getPropertyValue('--surface').trim()).toBe(
      COLORS.dark.surface,
    )
  })

  it('removes explicit color and course overrides when returning to system', () => {
    applyAppearance('dark')
    applyAppearance('system')

    expect(document.documentElement.dataset.theme).toBe('system')
    expect(document.documentElement.style.colorScheme).toBe('')
    for (const name of ROOT_STYLE_NAMES) {
      expect(document.documentElement.style.getPropertyValue(`--${name}`)).toBe('')
    }
    expect(getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()).toBe(
      COLORS.dark.bg,
    )
    systemPalette.textContent = `:root { --bg: ${COLORS.light.bg}; }`
    expect(getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()).toBe(
      COLORS.light.bg,
    )
  })

  it('lets the cached account preference win over boot and on a same-value refresh', async () => {
    localStorage.setItem('stvdy.theme', 'light')
    initializeAppearance()
    queryClient = new QueryClient({ defaultOptions: { queries: { enabled: false } } })
    const client = queryClient
    client.setQueryData(settingsKeys.all, { settings: { theme: 'dark' } }, { updatedAt: 100 })
    render(createElement(QueryClientProvider, { client }, createElement(ThemeSync)))

    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.dark.bg)
    })
    expect(localStorage.getItem('stvdy.theme')).toBe('dark')

    applyAppearance('light')
    act(() => {
      client.setQueryData(settingsKeys.all, { settings: { theme: 'dark' } }, { updatedAt: 200 })
    })
    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.dark.bg)
    })
  })

  it('does not invent an account preference when the row is missing or invalid', async () => {
    applyAppearance('light')
    queryClient = new QueryClient({ defaultOptions: { queries: { enabled: false } } })
    const client = queryClient
    render(createElement(QueryClientProvider, { client }, createElement(ThemeSync)))

    expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.light.bg)
    await act(async () => {
      client.setQueryData(settingsKeys.all, { settings: { theme: 'sepia' } })
    })
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.light.bg)
    expect(localStorage.getItem('stvdy.theme')).toBe('light')
  })

  it.each(['system', 'light', 'dark'] as const)('boots from persisted %s', (theme) => {
    localStorage.setItem('stvdy.theme', theme)

    initializeAppearance()

    expect(document.documentElement.dataset.theme).toBe(theme)
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe(
      theme === 'system' ? '' : COLORS[theme].bg,
    )
  })

  it('falls back to system for corrupt or blocked storage without corrupting state', () => {
    applyAppearance('dark')
    localStorage.setItem('stvdy.theme', 'solarized')
    initializeAppearance()
    expect(document.documentElement.dataset.theme).toBe('system')
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe('')

    clearRoot()
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('blocked', 'SecurityError')
    })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('blocked', 'SecurityError')
    })

    expect(() => initializeAppearance()).not.toThrow()
    expect(() => applyAppearance('light')).not.toThrow()
    expect(document.documentElement.dataset.theme).toBe('light')
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.light.bg)
  })

  it('keeps preference transitions usable when access to localStorage itself is denied', () => {
    vi.spyOn(window, 'localStorage', 'get').mockImplementation(() => {
      throw new DOMException('blocked', 'SecurityError')
    })

    expect(() => initializeAppearance()).not.toThrow()
    expect(document.documentElement.dataset.theme).toBe('system')
    expect(() => applyAppearance('dark')).not.toThrow()
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe(COLORS.dark.bg)
    expect(() => applyAppearance('system')).not.toThrow()
    expect(document.documentElement.style.getPropertyValue('--bg')).toBe('')
  })
})
