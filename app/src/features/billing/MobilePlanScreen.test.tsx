import { screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { MobilePlanScreen } from './MobilePlanScreen'
import i18n from '@/i18n'
import { setAccessToken } from '@/lib/tokens'
import { formatInZone } from '@/lib/time'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { PERIOD_END, USAGE, USER, serveUsage } from '@/test/usage'

/**
 * The phone's plan screen, and the two absences it exists for.
 *
 * A store review rejects an app that offers an off-store purchase path, so on
 * iOS and Android `/billing` may show the plan, the credits and the usage and
 * **nothing that sells**: no price, no upgrade button, no link, no route to a
 * checkout. These tests assert that absence where a reviewer sees it — in the
 * rendered screen — and they assert it for *every* locked surface the screen can
 * reach rather than for the one that happens to be on screen: both export formats
 * when neither is included, and the plan-limit error path as well as the happy
 * path.
 *
 * Which element `/billing` renders on which platform is a property of the route
 * table, and it is asserted there (`src/router.test.tsx`).
 *
 * What no test here can prove is the property the roadmap's exit gate names,
 * because the rendered screen is not what ships. `scripts/check-no-purchase-ui.mjs`
 * reads the shipped bytes and fails on a price, the checkout route or a purchase
 * call; this file is the other half, and neither replaces the other.
 */

beforeEach(() => {
  installFakePlatform('ios')
  setAccessToken('access-1')
})

afterEach(async () => {
  vi.unstubAllGlobals()
  await i18n.changeLanguage('en')
})

/** Every control that could take the user somewhere. */
function controls(container: HTMLElement): Element[] {
  return [...container.querySelectorAll('a, button, [role="button"], [role="link"]')]
}

describe('the phone plan screen', () => {
  it('renders the plan and the computed usage, and no purchase control', async () => {
    serveUsage()
    const { container } = renderWithProviders(<MobilePlanScreen />)

    // The plan the account is on and the numbers the server computed.
    expect(await screen.findByText('Free')).toBeInTheDocument()
    expect(screen.getByText('2 of 2')).toBeInTheDocument()
    expect(screen.getByText('1,048,576 bytes of 200 MB')).toBeInTheDocument()
    // Computed rather than written out: the app formats the instant in the
    // account's own zone, and a literal here would pin the runner's locale. The
    // renewal row renders it as a `.meta` span; the credits card's reset sentence
    // carries the same instant inside a sentence, so this exact-text match is the
    // renewal row alone.
    const periodEnd = formatInZone(new Date(PERIOD_END), USER.timezone, { dateStyle: 'medium' })
    expect(screen.getByText(periodEnd)).toBeInTheDocument()

    // The one string this screen adds: the honest statement of the store rule.
    expect(
      screen.getByText(
        'This app does not sell subscriptions. Your plan and credits sync from your account.',
      ),
    ).toBeInTheDocument()

    // And the absence. A price, a way to reach a checkout, or any control at all
    // would be the store violation, so the assertions are on the whole subtree.
    expect(screen.queryByRole('link', { name: /upgrade/i })).toBeNull()
    expect(screen.queryByText(/Rp/)).toBeNull()
    expect(container.textContent).not.toMatch(/\bupgrade\b|\bcheckout\b|\bbuy\b|\bprice\b/i)
    expect(controls(container)).toEqual([])
  })

  it('states the fact for every locked export format, not only the one on screen', async () => {
    // The free plan includes Markdown; with none included, every format the app
    // can export is a locked row. A screen that rendered a CTA for one of them
    // would fail here, which a single-format assertion could not catch.
    serveUsage({ ...USAGE, features: { ...USAGE.features, export_formats: [] } })
    const { container } = renderWithProviders(<MobilePlanScreen />)

    const locked = await screen.findAllByText('Not available on your plan')
    expect(locked).toHaveLength(2)
    expect(screen.queryByText(/Available on/)).toBeNull()
    expect(screen.queryByText(/Upgrade to/)).toBeNull()
    expect(controls(container)).toEqual([])
  })

  it('offers no route to a checkout in its error states either', async () => {
    // The plan-limit error is the other locked surface this screen can reach, and
    // it is where a web/desktop build links to `/billing` (States.tsx). A phone
    // must not, and the assertion is on the same absence as the happy path.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(401, { code: 'unauthorized', message: 'no' })
      return jsonResponse(403, {
        code: 'plan_required',
        message: 'Your plan does not include PDF export.',
        min_plan: 'Pro',
        limit: null,
        used: null,
      })
    })
    const { container } = renderWithProviders(<MobilePlanScreen />)

    expect(await screen.findByText('Not on your plan')).toBeInTheDocument()
    expect(controls(container)).toEqual([])
  })

  it('reads the statement in Indonesian', async () => {
    serveUsage()
    await i18n.changeLanguage('id')
    renderWithProviders(<MobilePlanScreen />)

    expect(
      await screen.findByText(
        'Aplikasi ini tidak menjual langganan. Paket dan kredit Anda disinkronkan dari akun Anda.',
      ),
    ).toBeInTheDocument()
    expect(screen.getByText('Paket dan pemakaian')).toBeInTheDocument()
  })
})
