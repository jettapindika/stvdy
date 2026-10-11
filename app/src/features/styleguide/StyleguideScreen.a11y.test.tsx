import { describe, expect, it } from 'vitest'
import { screen } from '@testing-library/react'

import { installFakePlatform } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { StyleguideScreen } from './StyleguideScreen'
import { expectNoA11yViolations } from '@/test/a11y'

/**
 * The component gallery is where every component and state is shown, so it is the
 * cheapest place to catch an unnamed control: a button added without an
 * accessible name shows up here once, rather than on the four screens that use it.
 */
describe('component gallery accessibility', () => {
  it('has no axe violations', async () => {
    installFakePlatform('macos')
    const { container } = renderWithProviders(<StyleguideScreen />, { route: '/styleguide' })

    await expectNoA11yViolations(container)
    // A gallery that rendered nothing would pass every check above. The heading
    // is the one thing the screen is guaranteed to have.
    expect(screen.getAllByRole('heading').length).toBeGreaterThan(0)
  })
})
