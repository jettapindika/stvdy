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
