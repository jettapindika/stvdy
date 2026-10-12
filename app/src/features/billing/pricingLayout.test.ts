import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The price line, pinned against the defect the desktop leg of the measurement
 * found in it.
 *
 * These are **source** assertions, not rendered ones, and the reason is the harness:
 * jsdom has no layout engine, so `scrollWidth` and `getBoundingClientRect()` are all
 * zeros in this suite and no overflow can be measured here. What can be asserted is
 * the property that makes the overflow impossible — the technique
 * `features/phoneLayout.test.ts`, `tokens/invariants.test.ts` and
 * `features/calendar/layout.test.ts` already use. **The numbers are the browser's**,
 * taken by `scripts/measure-mobile.mjs` with `VIEWPORT_WIDTH=1365 VIEWPORT_HEIGHT=900
 * VIEWPORT_DPR=2 SCALES=200`: on both themes `p.text-h2 "IDR 125,000 per year"`
 * measured `scrollWidth` 226 against a `clientWidth` of 181, and `"IDR 50,000 per
 * year"` 211 against the same 181.
 *
 * The defect is *not* the wrapping rule the workspace panels needed, and the
 * difference is the point of this file. `formatAmount` joins the currency to the
 * number with a **no-break space** — `IDR\u00a0125,000` — which is right at 100%,
 * because the currency and its number are one price and must not drift apart. It is
 * also exactly what forbids the break a 200% reader needs, because `--text-h2` is
 * `1.25rem` and renders at 40px, so the pair needs 226px of a card whose content box
 * is 181px. The fix adds the break opportunity back after the currency and leaves the
 * digits whole.
 *
 * `overflow-wrap: anywhere` — the right rule for the workspace panels — is the wrong
 * one here, and the second assertion below pins that: it would "fix" the overflow by
 * splitting the number, and `IDR 125,0` / `00` is a price a student misreads, which
 * is worse than the clip it replaced. The type scale is untouched too, which the
 * third assertion pins: the reader chose 200%, and shrinking `--text-h2` to make the
 * string fit would defeat the setting.
 */

/**
 * The price paragraph's body, from its `{price !== null && !free && (` guard to the
 * `</p>` that closes it, with comments stripped.
 *
 * The comments matter: the price block's own comment quotes `overflow-wrap: anywhere`
 * and `break-words` to say why neither is used, so a scan of the raw text would fail
 * on the explanation of the decision. A range rather than a single `className`,
 * because what has to hold is the *content* of the price line — a break opportunity
 * and no break-anywhere rule — and the element's own classes say nothing about either.
 */
const PRICE_BLOCK = (source: string): string => {
  const stripped = source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\{\/\*[\s\S]*?\*\/\}/g, '')
    .replace(/\/\/[^\n]*/g, '')
  const block = /\{price !== null && !free && \(([\s\S]*?)<\/p>\s*\)\}/.exec(stripped)?.[1]
  if (block === undefined) throw new Error('the price paragraph must exist')
  return block
}

const PRICING = readFileSync(join(__dirname, 'PricingScreen.tsx'), 'utf8')

describe('the price line survives 200% text', () => {
  it('lets the amount break after the currency', () => {
    // `formatAmount`'s no-break space is what forbids the break, and it is right at
    // 100%: without a break opportunity after it, the whole `IDR\u00a0125,000` token
    // is one unbreakable run of glyphs 226px wide in a 181px box. `<wbr>` adds the
    // opportunity back without touching the string, so the currency moves to its own
    // line and `textContent` still carries the same `IDR\u00a0…` the money-pattern
    // assertions in `PricingScreen.test.tsx` match on.
    expect(PRICE_BLOCK(PRICING)).toContain('<wbr />')
  })

  it('keeps the digits whole, and keeps the type scale', () => {
    const block = PRICE_BLOCK(PRICING)
    // Not `anywhere` and not `break-word`: both would break inside `125,000`, and a
    // price split across lines mid-number is worse than the clip it replaces. The
    // workspace panels take `anywhere` for exactly the opposite reason — a long
    // Indonesian word has no break opportunity at all, and there is no unit inside it
    // that a break would falsify.
    expect(block).not.toContain('wrap-anywhere')
    expect(block).not.toContain('break-words')
    expect(block).not.toContain('break-all')
    expect(block).not.toContain('overflow-wrap')
    // The spec's scale, not a smaller one: `--text-h2` is what makes the string 226px
    // at 200% text, and fitting it by shrinking the type would defeat the setting.
    expect(block).toContain('text-h2')
  })
})

describe('the checks detect the pattern they exist for', () => {
  // A rule that cannot fail is worse than no rule, so the negative is pinned too:
  // these are the pre-fix form's own strings, and each fails the property its
  // assertion above checks.
  it('reads the price block, and fails on the pre-fix form', () => {
    expect(PRICE_BLOCK(PRICING)).toBeDefined()
    // Pre-fix: the formatted amount rendered straight into the paragraph, with no
    // break opportunity anywhere in it.
    expect('<p className="text-h2">{formatAmount(price, i18n.language)}').not.toContain('<wbr />')
    // And the rule that would have "fixed" it by splitting the number is still
    // detectable, so the assertion above is not passing on any string.
    expect('<p className="text-h2 wrap-anywhere">').toContain('wrap-anywhere')
  })
})
