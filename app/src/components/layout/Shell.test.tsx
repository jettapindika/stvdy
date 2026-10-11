import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { Shell } from './Shell'

/**
 * The shell is the one place where a mistake removes navigation entirely, and it
 * happened once: the layout was chosen from `platform.formFactor()`, which
 * returned `web` on a phone browser, so the sidebar was hidden by CSS while the
 * tab bar never rendered and the app had no way to navigate.
 *
 * Testing the form-factor helper alone did not catch that, because the helper was
 * correct and the shell simply did not use it. These tests render the shell at a
 * given width and assert what the user can actually see, which is the level the
 * bug lived at.
 */

function mockViewport(width: number) {
  Object.defineProperty(window, 'innerWidth', { writable: true, configurable: true, value: width })

  // jsdom has no matchMedia. The hook subscribes to it, so it must exist and must
  // report the same answer the width implies.
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

function renderShell() {
  return render(
    <MemoryRouter>
      <Shell>
        <p>content</p>
      </Shell>
    </MemoryRouter>,
  )
}

beforeEach(() => {
  vi.unstubAllGlobals()
})

describe('the shell always offers navigation', () => {
  it('renders the tab bar on a phone, which is the bug that shipped', () => {
    mockViewport(390)
    renderShell()

    // Exactly one nav, so neither zero (no navigation, the bug) nor two.
    expect(screen.getAllByRole('navigation', { name: 'Main' })).toHaveLength(1)

    // The tab bar is identified by its short labels rather than by a CSS class or
    // a computed style: jsdom does not process Tailwind, so `position: fixed` is
    // invisible here, and a class assertion would pin the implementation.
    for (const label of ['Today', 'Work', 'Calendar', 'Tasks', 'More']) {
      expect(screen.getByRole('link', { name: label })).toBeInTheDocument()
    }
    // The sidebar's long labels must be absent, which is what proves this is the
    // phone shell rather than the sidebar.
    expect(screen.queryByRole('link', { name: 'Workspaces' })).not.toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Settings' })).not.toBeInTheDocument()
  })

  it('renders the sidebar on desktop and no tab bar', () => {
    mockViewport(1280)
    renderShell()

    const navs = screen.getAllByRole('navigation', { name: 'Main' })
    expect(navs).toHaveLength(1)
    for (const label of ['Today', 'Workspaces', 'Calendar', 'Tasks', 'Settings']) {
      expect(screen.getByRole('link', { name: label })).toBeInTheDocument()
    }
  })

  it('renders the sidebar on a tablet too, since only phones get the tab bar', () => {
    mockViewport(800)
    renderShell()

    expect(screen.getAllByRole('navigation', { name: 'Main' })).toHaveLength(1)
    expect(screen.getByRole('link', { name: 'Workspaces' })).toBeInTheDocument()
  })

  it('exposes a skip link and a main landmark', () => {
    // R-32: a keyboard user must be able to skip the nav, which needs both the
    // link and something to skip to.
    mockViewport(1280)
    renderShell()

    expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute('href', '#main')
    expect(screen.getByRole('main')).toHaveAttribute('id', 'main')
  })
})
