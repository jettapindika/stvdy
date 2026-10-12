import { describe, expect, it } from 'vitest'
import { screen } from '@testing-library/react'

import { installFakePlatform } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { LockedFeature } from './LockedFeature'

/**
 * The mobile rule, at the surface the user actually sees.
 *
 * A locked feature on a phone may state the fact and nothing more: no price, no
 * upgrade button, no link, no wording that points at a purchase elsewhere. That
 * is what an app-store reviewer looks for, and it is why the platform variant is
 * tested rather than assumed from the config file's own test.
 */
describe('LockedFeature', () => {
  it('offers an upgrade on desktop', () => {
    installFakePlatform('macos')
    renderWithProviders(<LockedFeature whatKey="planner.rebalance" minPlan="Power" />)

    expect(screen.getByText('Rebalance my week')).toBeInTheDocument()
    expect(screen.getByText('Available on Power')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Upgrade to Power' })).toHaveAttribute(
      'href',
      '/billing',
    )
  })

  /**
   * The upgrade link is a real target, and that is a defect this file now pins.
   *
   * Phase 7's accessibility gate measured it in Chromium at 390px and it was
   * **93x19**: a `link` is text-height, and the one control on a card whose whole
   * purpose is to send the user to the plans screen was 19px tall against the
   * 44px phone target the gate requires. The fix is the shape `States.tsx`'s plan
   * link already uses — `min-h-11` (2.75rem = 44px) plus `inline-flex
   * items-center`, which is what makes the box 44px tall rather than the line box
   * of an inline element.
   *
   * **jsdom has no layout engine**, so `getBoundingClientRect()` here is zeros and
   * a measured height cannot be asserted from this harness — the same limit
   * `Shell.test.tsx`'s tab-bar case and `ReportScreen.test.tsx`'s row case record.
   * The assertion is therefore on the classes that decide the height, and the
   * number itself is measured in Chromium against the built bundle (44px at both
   * 390px and 1280px; recorded in the Phase 7 plan's gate notes). Removing any of
   * the three utilities puts the link back under the target: `min-h-11` alone on
   * an inline element still paints the line box, and `inline-flex items-center`
   * alone has no height to centre in.
   */
  it('gives the upgrade link a 44px phone target, not a text-height one', () => {
    installFakePlatform('macos')
    renderWithProviders(<LockedFeature whatKey="planner.rebalance" minPlan="Power" />)

    const link = screen.getByRole('link', { name: 'Upgrade to Power' })
    expect(link.className).toContain('min-h-11')
    expect(link.className).toContain('inline-flex')
    expect(link.className).toContain('items-center')
  })

  it('states the fact without a call to action on iOS', () => {
    installFakePlatform('ios')
    renderWithProviders(<LockedFeature whatKey="planner.rebalance" minPlan="Power" />)

    expect(screen.getByText('Rebalance my week')).toBeInTheDocument()
    expect(screen.getByText('Not available on your plan')).toBeInTheDocument()
    // The three things a store review rejects, asserted as absent.
    expect(screen.queryByRole('link')).not.toBeInTheDocument()
    expect(screen.queryByRole('button')).not.toBeInTheDocument()
    expect(screen.queryByText(/Power|Rp|upgrade/i)).not.toBeInTheDocument()
  })

  it('states the fact without a call to action on Android', () => {
    installFakePlatform('android')
    renderWithProviders(<LockedFeature
        whatKey="planner.rebalance"
        minPlan="Pro"
        reasonKey="planner.rebalanceReason"
      />)

    expect(
      screen.getByText('Moves and splits study blocks when a week is overloaded.'),
    ).toBeInTheDocument()
    expect(screen.getByText('Not available on your plan')).toBeInTheDocument()
    expect(screen.queryByRole('link')).not.toBeInTheDocument()
  })
})
