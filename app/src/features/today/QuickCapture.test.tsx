import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The capture card's strings must be breakable.
 *
 * This is a **source** assertion rather than a rendered one, for the reason
 * `shellLayout.test.ts` gives for the same class of defect: jsdom has no layout
 * engine, so nothing in this suite can measure a box. What can be asserted is the
 * property that makes the overflow impossible, and the property is a class on each
 * element that renders a string the app did not author.
 *
 * Measured in headless Chromium at 390x844 with the root font at **32px** (200%
 * text scale), against the built bundle, with the draft card open:
 *
 *   - **Before the fix**, the card rendered the raw ISO instant and
 *     `document.scrollingElement.scrollWidth` was **424** against `clientWidth`
 *     **390** — 34px of horizontal page overflow, and the offending box was
 *     `TodayScreen`'s own root (measured 424.4px wide). Dismissing the card gave
 *     `390 === 390`, so the card was the entire cause.
 *   - **After the fix**, the same measurement with the same draft is `390 === 390`:
 *     a formatted date has spaces, so the date alone no longer overflows.
 *   - That is not the whole hazard, and this is why the assertion exists anyway.
 *     With the date fixed and a 55-character unbreakable title from the model
 *     (`pertanggungjawabankeuangandanadministrasiumumperusahaan`), `scrollWidth`
 *     went to **1051**; a long warning string took it to **1160**. `min-w-0
 *     break-words` — the pair this app uses where a flex *row* has to shrink — left
 *     the first at **1051**, because `overflow-wrap: break-word` does not lower a
 *     box's min-content size and the card is a flex *column*. `overflow-wrap:
 *     anywhere` brings both back to **390**, and that is what `wrap-anywhere`
 *     compiles to.
 *
 * So the rule below is about every string in the card, not about the one that was
 * measured: the title, the date, the estimate and each warning is text the model
 * chose, and the next long one is the same defect.
 */

const source = readFileSync(join(__dirname, 'QuickCapture.tsx'), 'utf8')

describe('the capture draft card', () => {
  it('renders the due line through the same zone helper the screen uses, not as an instant', () => {
    // The pre-fix expression, exactly, so this cannot pass by accident on a rename.
    expect(source).not.toContain('new Date(draft.due_at).toISOString()')
    // And the fix's own form: the same helper, with the same option, that
    // `TodayScreen` uses four lines away for every other date on this screen.
    expect(source).toMatch(
      /formatInZone\(new Date\(draft\.due_at\), timeZone, \{ dateStyle: 'medium' \}\)/,
    )
  })

  it('takes the zone from the session, which is where the screen takes it from', () => {
    // A second timezone source would be a second answer to "which day is it": the
    // screen and the card have to name the same day for the same instant.
    expect(source).toContain("from '@/lib/time'")
    expect(source).toMatch(/useSession\(\)/)
    expect(source).toMatch(/session\.status === 'signed-in' \? session\.user\.timezone : DEFAULT_TIMEZONE/)
    expect(source).toContain('DEFAULT_TIMEZONE')
  })

  it('lets every string in the card break inside a word', () => {
    // Every `<li>` in this component renders a string the model wrote, so every one
    // of them needs the token-breaking utility. A new long string added to this card
    // as a plain `<li>` fails here rather than at 200% text on a phone.
    const items = source.match(/<li\b[^>]*>/g) ?? []
    expect(items.length).toBeGreaterThan(0)
    const unbreakable = items.filter((tag) => !tag.includes('wrap-anywhere'))
    expect(unbreakable).toEqual([])
  })
})
