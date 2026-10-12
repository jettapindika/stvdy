import { screen, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { Shell } from './Shell'
import { jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'

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
 *
 * The shell now carries the credit meter, which reads `GET /usage`, so the render
 * goes through `renderWithProviders` and the route is answered. The answer is a
 * 404: what these tests are about is navigation, and the meter's own failure state
 * is asserted where it belongs.
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

/** A `GET /usage` body, of which the meter reads `credits.total`. */
const USAGE = {
  plan: 'free',
  name: 'Free',
  subscription: { status: 'active', period_end: null },
  credits: { monthly: 60, purchased: 12, total: 72 },
  storage_bytes: 0,
  limits: {},
  features: { export_formats: ['md'] },
  costs: {},
  provider_cost: { paused: false, resets_at: '2026-11-01T07:00:00Z' },
}

/**
 * Renders the shell with `GET /usage` answered, or with a 404.
 *
 * The shell now carries the credit meter, and the meter reads that route, so the
 * shell's render needs a query client and an answer. A 404 is the default: what
 * most of these tests are about is navigation, and the meter's own failure state
 * is asserted where it belongs.
 */
function renderShell(usage?: unknown) {
  mockFetch((url) => {
    if (new URL(url).pathname === '/usage' && usage !== undefined) {
      return jsonResponse(200, usage)
    }
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
  return renderWithProviders(
    <Shell>
      <p>content</p>
    </Shell>,
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

  it('carries the credit meter in the sidebar, with the balance the server sent', async () => {
    mockViewport(1280)
    renderShell(USAGE)

    // The one place the balance is formatted, and it is a link: the number and
    // the screen that explains it are one `Tab` apart (R-32).
    const meter = await screen.findByRole('link', { name: '72 credits left' })
    expect(meter).toHaveAttribute('href', '/billing')
  })

  it('shows no balance when the usage route did not answer', async () => {
    mockViewport(1280)
    renderShell()

    // A number the shell does not have is worse than none: the plan screen
    // carries the failure and its remedy, and this says only that it does not
    // know. A zero here would read as a spent account.
    expect(await screen.findByRole('link', { name: 'Credits unavailable' })).toBeInTheDocument()
    expect(screen.queryByText('0 credits left')).not.toBeInTheDocument()
  })

  it('does not put the meter in the phone tab bar', () => {
    mockViewport(390)
    renderShell(USAGE)

    // The tab bar is five destinations and no number. The meter belongs to the
    // sidebar, and a sixth item would cost every other item its 44px target.
    const bar = screen.getByRole('navigation', { name: 'Main' })
    expect(within(bar).queryByRole('link', { name: /credit/i })).not.toBeInTheDocument()
    expect(within(bar).getAllByRole('link')).toHaveLength(5)
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

  /**
   * The sync status control, in both form factors (A189).
   *
   * Devices is reached from this control rather than from a sixth nav item,
   * because the sidebar and the tab bar are both built from the same five labels.
   * Two consequences are asserted here and both would be silent regressions:
   * the control has to exist on *both* form factors, or one of them loses its only
   * route to the device list; and it must not be one of the tab bar's links, or the
   * five labels lose the widths they were measured at (5 x 78px on a 390px phone).
   */
  it('carries the sync status control in the sidebar, on desktop', () => {
    mockViewport(1280)
    renderShell()

    expect(screen.getByRole('link', { name: /Devices and sync/ })).toHaveAttribute(
      'href',
      '/devices',
    )
  })

  it('carries the sync status control on a phone without making it a sixth tab', () => {
    mockViewport(390)
    renderShell()

    // Reachable, and outside the nav so the five labels keep their widths.
    expect(screen.getByRole('link', { name: /Devices and sync/ })).toHaveAttribute(
      'href',
      '/devices',
    )
    const bar = screen.getByRole('navigation', { name: 'Main' })
    expect(within(bar).getAllByRole('link')).toHaveLength(5)
    expect(within(bar).queryByRole('link', { name: /Devices and sync/ })).not.toBeInTheDocument()
  })

  /**
   * The tab bar must be able to scroll sideways, and that is a 200%-text-scale
   * defect rather than a style choice.
   *
   * Each item is `flex: 1` with the UA's `min-width: auto`, so it cannot shrink
   * below its label. The five labels need 390px at 100% type (measured: 5 x 78px)
   * but ~435px at 200%, and the bar is `position: fixed` — so with `overflow:
   * visible` the fifth item, "More" (Settings and Billing), was painted 45px past
   * the right edge of the viewport with nothing to scroll it into view. It was
   * unreachable, and only for the reader who had enlarged their type.
   *
   * jsdom does not lay out, so this asserts the declaration that fixes it. Without
   * `overflow-x-auto` the bar overflows again; `overscroll-x-contain` keeps a
   * horizontal swipe on the bar from triggering the browser's back gesture.
   */
  it('lets the tab bar scroll sideways, so no item can be unreachable', () => {
    mockViewport(390)
    renderShell()

    const bar = screen.getByRole('navigation', { name: 'Main' })
    expect(bar.className).toContain('overflow-x-auto')
    expect(bar.className).toContain('overscroll-x-contain')
  })
})
