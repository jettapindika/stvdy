import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The phone shell's band must be in the flow, not painted over the content.
 *
 * This is a **source** assertion rather than a rendered one, and the reason is the
 * harness: jsdom has no layout engine, so `getBoundingClientRect()` in this suite is
 * all zeros and no height or overlap can be measured here. What can be asserted is
 * the property that makes the overlap impossible, which is the same technique
 * `tokens/invariants.test.ts` uses for the design-system invariants.
 *
 * The defect this exists for, found by the Phase 8 accessibility gate at 390x844
 * with 200% text scale:
 *
 *   - the tab bar's band was `fixed inset-x-0 bottom-0`, so it was painted over the
 *     content rather than laid out beside it;
 *   - `main` therefore reserved the band's height with
 *     `padding-bottom: calc(2.75rem + 1px + 3rem + env(safe-area-inset-bottom))`;
 *   - the `3rem` was meant to cover the sync chip's row, whose measured height is
 *     **73px at 100% text and 176px at 200%** — the chip wraps, because `max-w-64`
 *     caps it at 256px inside a 390px viewport;
 *   - so the last line box of the Devices screen's reset sentence sat **32.4px
 *     behind the opaque band**, and `document.elementFromPoint` at that point
 *     returned the chip. At 100% the same screen cleared by 22.8px only because the
 *     content happened to be short.
 *
 * A constant cannot be right here: the band grows with the text scale and with the
 * string, and a longer Indonesian sentence is the next surprise. The fix is
 * structural — the shell root is a column on a phone and the band is a flow sibling
 * below `main` — so what is asserted below is the structure, not a number. Every
 * assertion fails against the pre-fix form.
 */

const appRoot = join(__dirname, '..', '..', '..')
const stylesheet = readFileSync(join(appRoot, 'src', 'styles.css'), 'utf8')
const shell = readFileSync(join(appRoot, 'src', 'components', 'layout', 'Shell.tsx'), 'utf8')
const tabBar = readFileSync(join(appRoot, 'src', 'components', 'layout', 'TabBar.tsx'), 'utf8')

/**
 * The stylesheet without its comments.
 *
 * Comments matter here: the fix's own comment quotes the expression it removed, so a
 * scan of the raw file would fail on the explanation of the defect. What is asserted
 * is what the browser applies.
 */
const css = stylesheet.replace(/\/\*[\s\S]*?\*\//g, '')

/** The body of a CSS rule, by its exact selector, or `null` when there is none. */
function ruleBody(selector: string): string | null {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const match = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`).exec(css)
  return match?.[1] ?? null
}

describe('the phone shell lays the band out instead of reserving room for it', () => {
  it('makes the shell root a column on a phone', () => {
    const body = ruleBody('.shell-phone')
    expect(body, '.shell-phone must exist').not.toBeNull()
    expect(body).toMatch(/flex-direction:\s*column/)
  })

  it('applies that class from the same decision that renders the tab bar', () => {
    // `isPhone` is what renders the band; if the column came from anywhere else the
    // two could disagree, which is how a band ends up overlaying a row-based shell.
    expect(shell).toMatch(/isPhone \? ' shell-phone' : ''/)
    expect(shell).toMatch(/isPhone && <TabBar \/>/)
  })

  it('reserves no hardcoded height for the band', () => {
    // The exact pre-fix expression, so this cannot pass by accident on a rename.
    expect(css).not.toContain('2.75rem + 1px + 3rem')
    // And the general form: a `padding-bottom` on the phone shell that is arithmetic
    // over literals is the same bug with different numbers. The safe-area inset is
    // the band nav's own `pb-safe-bottom`, so the shell needs no bottom padding.
    const main = ruleBody('.shell-main')
    expect(main ?? '').not.toMatch(/padding-bottom/)
    expect(ruleBody('.shell-main-phone')).toBeNull()
  })

  it('puts the band in the flow, at the bottom, rather than fixing it to the viewport', () => {
    const root = /<div className="([^"]*)"/.exec(tabBar)?.[1]
    expect(root, 'the tab bar root div must carry its classes inline').toBeTruthy()
    expect(root).toContain('md:hidden')
    expect(root).toContain('sticky')
    expect(root).toContain('bottom-0')
    expect(root).not.toContain('fixed')
    // `inset-x-0` was there to stretch a fixed box across the viewport; a flow item
    // in a column is already full width, and leaving it in would be a leftover.
    expect(root).not.toContain('inset-x-0')
  })

  it('keeps the band opaque, so content scrolling under it stays hidden', () => {
    // Sticky still overlaps content mid-scroll (that is what sticky is for), so both
    // of the band's rows must paint a background.
    const backgrounds = tabBar.match(/bg-surface/g) ?? []
    expect(backgrounds.length).toBeGreaterThanOrEqual(2)
  })

  it('keeps the tab bar a horizontal scroll container, which is what makes the fifth item reachable', () => {
    // Found by measurement in Phase 6 and load-bearing at 200% text scale: five
    // labels need ~435px on a 390px phone. A sixth item is what A189 refused, so the
    // fix for the overlap must not have come from shrinking or removing anything.
    expect(tabBar).toContain('overflow-x-auto')
    const bar = tabBar.match(/<nav[\s\S]*?className="([^"]*)"/)?.[1] ?? ''
    expect(bar).toContain('overflow-x-auto')
  })

  it('keeps the 44px touch target on the tab items', () => {
    const item = ruleBody('.nav-item-tab')
    expect(item).toMatch(/min-height:\s*2\.75rem/)
  })
})
