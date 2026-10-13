import { describe, expect, it } from 'vitest'
import { screen } from '@testing-library/react'

import { installFakePlatform } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { StyleguideScreen } from './StyleguideScreen'
import { expectNoA11yViolations } from '@/test/a11y'

/**
 * The design system's spec page renders every specimen, so it is the cheapest
 * place to catch an unnamed control: a button added without an accessible name
 * shows up here once, rather than on the four screens that use it.
 */
describe('design system spec page accessibility', () => {
  it('has no axe violations', async () => {
    installFakePlatform('macos')
    const { container } = renderWithProviders(<StyleguideScreen />, { route: '/styleguide' })

    await expectNoA11yViolations(container)
    // A page that rendered nothing would pass every check above. The heading is
    // the one thing the screen is guaranteed to have.
    expect(screen.getAllByRole('heading').length).toBeGreaterThan(0)
  })

  it('outlines the page as one title, sections and specimens', async () => {
    installFakePlatform('macos')
    renderWithProviders(<StyleguideScreen />, { route: '/styleguide' })

    const levels = screen.getAllByRole('heading').map((heading) => Number(heading.tagName.slice(1)))

    // Exactly one `h1`, and it is the page title. Two would flatten the document
    // outline a screen-reader user navigates by.
    expect(levels.filter((level) => level === 1)).toHaveLength(1)
    expect(levels[0]).toBe(1)

    // No level is skipped: a specimen is an `h3` inside a section's `h2`, never a
    // fourth level under a heading that was never rendered.
    let previous = levels[0] ?? 1
    for (const level of levels) {
      expect(level - previous).toBeLessThanOrEqual(1)
      previous = level
    }
  })
})
