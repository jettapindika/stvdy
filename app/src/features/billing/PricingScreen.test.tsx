import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { PricingScreen } from './PricingScreen'
import { formatAmount } from './checkout'
import i18n from '@/i18n'
import { setAccessToken } from '@/lib/tokens'
import { PLANS, PLANS_WITHOUT_PRICES, PRO_MONTHLY, serveBilling } from '@/test/billing'
import { installFakePlatform } from '@/test/harness'
import { renderWithProviders } from '@/test/render'

/**
 * The pricing page, against a scripted `GET /plans`.
 *
 * The load-bearing assertion is the honesty one, and it is written so it can fail:
 * the fixture's `pro` monthly price (`12345`) is **not** the value in
 * `config/plans.toml` (`10000`). So a screen that rendered the client's own
 * reading of the matrix — `features/billing/plans.ts`, which the app legitimately
 * has and uses in the export menu — would print `10,000` here and fail, while a
 * screen that renders the payload passes. That is the same property the bundle
 * check asserts from the other end: one number, and the server computed it.
 *
 * The other half is the platform rule. On a phone the server omits every price
 * (A305) and the client's policy forbids the surface, so the page must state the
 * fact and offer nothing: no amount, no button, no link. A fallback price here
 * would be the store-policy violation the whole `billing_policy.toml` exists to
 * prevent, which is why `PLANS_WITHOUT_PRICES` has the fields absent rather than
 * zeroed.
 */

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(async () => {
  vi.unstubAllGlobals()
  await i18n.changeLanguage('en')
})

/** The card for one plan, found by its heading. */
function card(plan: string): HTMLElement {
  const heading = screen.getByRole('heading', { name: plan, level: 2 })
  const found = heading.closest('li')
  if (found === null) throw new Error(`no card for ${plan}`)
  return found
}

/** Every rupiah amount the screen renders, as the formatted strings they are. */
function amounts(container: HTMLElement): string[] {
  return (container.textContent ?? '').match(/(?:Rp|IDR)\u00a0[\d.,]+/g) ?? []
}

describe('the pricing page', () => {
  it("renders the server's prices, not the client's reading of the matrix", async () => {
    serveBilling()
    const { container } = renderWithProviders(<PricingScreen />)

    expect(await screen.findByRole('heading', { name: 'Pro', level: 2 })).toBeInTheDocument()

    // Yearly is the default, so the card shows the payload's yearly price.
    expect(within(card('Pro')).getByText(/98,765/)).toBeInTheDocument()
    expect(amounts(container)).toContain(formatAmount(98765, 'en'))
    // The matrix's own monthly number — which the app *does* carry, in
    // `plans.ts` — must never appear as a price, because the file is not what the
    // student is charged. This is the assertion that fails if the screen renders
    // the file. It is scoped to the money pattern rather than the whole text: the
    // comparison table legitimately carries `10000` as power's storage ceiling.
    expect(PLANS.plans[1]?.price_idr_monthly).toBe(PRO_MONTHLY)
    expect(PRO_MONTHLY).not.toBe(10000)
    expect(amounts(container)).not.toContain(formatAmount(10000, 'en'))

    // The monthly price is a click away, from the same payload.
    screen.getByRole('button', { name: 'Yearly' }).click()
    expect(await within(card('Pro')).findByText(/12,345/)).toBeInTheDocument()
    expect(amounts(container)).toContain(formatAmount(PRO_MONTHLY, 'en'))
    expect(amounts(container)).not.toContain(formatAmount(10000, 'en'))
  })

  it('defaults to yearly and shows the period the price is for', async () => {
    serveBilling()
    renderWithProviders(<PricingScreen />)

    // Yearly is the default the spec names, and the toggle says which period the
    // numbers on the cards are for — a price with no period is a price the student
    // has to guess the unit of.
    const toggle = await screen.findByRole('button', { name: 'Yearly' })
    expect(toggle).toHaveAttribute('aria-pressed', 'true')
    expect(within(card('Pro')).getByText(/98,765/)).toBeInTheDocument()
    expect(within(card('Pro')).getByText('per year')).toBeInTheDocument()

    // Flipping it shows the monthly price from the same payload.
    toggle.click()
    expect(await screen.findByRole('button', { name: 'Monthly' })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
    expect(within(card('Pro')).getByText('per month')).toBeInTheDocument()
  })

  it('offers a button on a plan that can be bought and none on the one in effect', async () => {
    serveBilling()
    renderWithProviders(<PricingScreen />)

    // The fixture's account is on `free`, which is also the plan with a zero price:
    // nothing to buy, so no button rather than a disabled one.
    expect(await screen.findByRole('button', { name: 'Choose Pro' })).toBeInTheDocument()
    expect(within(card('Free')).queryByRole('button')).toBeNull()
  })

  it('shows the student banner only to a verified student', async () => {
    serveBilling({ plans: PLANS })
    const first = renderWithProviders(<PricingScreen />)
    // The fixture's account is not verified, so no discount is claimed at all —
    // a banner promising 50% off to an account the checkout would charge full
    // price is the defect this pins.
    await screen.findByRole('heading', { name: 'Pro', level: 2 })
    expect(screen.queryByText(/student address is verified/)).toBeNull()
    first.client.clear()
    first.container.remove()

    serveBilling({ plans: { ...PLANS, student_verified: true } })
    renderWithProviders(<PricingScreen />)
    expect(
      await screen.findByText('Your student address is verified, so every plan is 50% off.'),
    ).toBeInTheDocument()
    // The verified student sees the server's computed student price, not a
    // percentage the app applied to the list price: the yearly figure here is
    // `student_price_idr_yearly`, which is neither the list price nor a
    // subtraction this screen performed.
    expect(within(card('Pro')).getByText(/49,382/)).toBeInTheDocument()
    expect(within(card('Pro')).queryByText(/98,765/)).toBeNull()
  })

  it('sells nothing on a platform whose policy forbids it', async () => {
    installFakePlatform('ios')
    serveBilling({ plans: PLANS_WITHOUT_PRICES })
    const { container } = renderWithProviders(<PricingScreen />)

    // The honest sentence, and nothing else: no amount, no button, no link, and
    // no route to a checkout.
    expect(await screen.findByText(/does not sell subscriptions/)).toBeInTheDocument()
    expect(amounts(container)).toEqual([])
    expect(container.querySelectorAll('button, a')).toHaveLength(0)
    expect(container.textContent).not.toMatch(/billing\/checkout|\bbuy\b|\bcheckout\b/i)
  })

  it('renders no price when the payload omits one, even on a platform that may show them', async () => {
    // The policy and the payload can disagree — a patched client, a stale
    // `show_prices` — and the payload is the authority: `purchase_allowed: false`
    // is the server's own reading, so nothing is offered.
    serveBilling({ plans: { ...PLANS_WITHOUT_PRICES, show_prices: true } })
    const { container } = renderWithProviders(<PricingScreen />)

    await screen.findByRole('heading', { name: 'Pro', level: 2 })
    expect(amounts(container)).toEqual([])
    expect(screen.queryByRole('button', { name: 'Choose Pro' })).toBeNull()
  })

  it('compares the plans on the matrix limits, including the uncapped ones', async () => {
    serveBilling()
    renderWithProviders(<PricingScreen />)

    await screen.findByRole('heading', { name: 'Pro', level: 2 })
    // `sources_total` is a number on both plans, and `workspaces` is
    // `"unlimited"` on pro — the two shapes `config/plans.toml` genuinely has, and
    // the word is what an uncapped axis says rather than a zero.
    const row = screen.getByRole('rowheader', { name: 'Sources' }).closest('tr')
    if (row === null) throw new Error('no sources row')
    expect(within(row).getByText('20')).toBeInTheDocument()
    expect(within(row).getByText('150')).toBeInTheDocument()

    const workspaces = screen.getByRole('rowheader', { name: 'Workspaces' }).closest('tr')
    if (workspaces === null) throw new Error('no workspaces row')
    expect(within(workspaces).getAllByText('Unlimited')).toHaveLength(2)
  })
})
