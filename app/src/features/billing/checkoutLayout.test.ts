import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The checkout screen at 200% text on a phone, pinned against the defect gate 5
 * measured in it.
 *
 * These are **source** assertions, not rendered ones, and the reason is the harness:
 * jsdom has no layout engine, so `scrollWidth`, `scrollHeight` and
 * `getBoundingClientRect()` are all zeros in this suite and no clip can be measured
 * here. What can be asserted is the property that makes the clip impossible — the
 * same technique `features/billing/pricingLayout.test.ts`, `features/phoneLayout.test.ts`
 * and `features/calendar/layout.test.ts` already use. **The numbers are the
 * browser's**, taken by `scripts/measure-mobile.mjs` at its defaults (390×844, DPR 3,
 * both themes, 100% and 200%): before this fix, `checkout-waiting` reported three
 * clipped elements at 200% in both themes —
 *
 *  - `div.flex.items-baseline.justify-between` (`"Price before discountIDR 12,345"`),
 *    scroll 290 against a client width of 244;
 *  - `p.text-xs.text-subtle` (`"Order stvdy_01j0checkoutexample"`), scroll 316
 *    against the same 244;
 *  - `p.text-lg.font-medium` (`"Waiting for confirmation"`), scroll 219 against 200;
 *
 * and the other five checkout surfaces reported the first two. 22 findings in all.
 *
 * **Two mechanisms fix them, and the difference is the point of this file.** The
 * card takes `overflow-wrap: anywhere` on one container, because the rule inherits
 * and because `anywhere` — not `break-word` — is what lowers a box's **min-content**
 * size. `break-word` lowers only the *content* width, and min-content is what these
 * boxes were failing on: a flex row cannot shrink its items below their min-content,
 * so the label's longest word (114px) plus the gap plus the value's unbreakable
 * token (160px) stayed 290px wide inside a 244px box no matter what the content
 * width said. That is the rule the workspace panels take, for the same reason.
 *
 * A money figure must **not** take it. `formatAmount` joins the currency to the
 * digits with a no-break space, and `IDR 12,3` / `45` is a price a student
 * misreads — worse than the clip it replaced. So the value's own box takes
 * `break-normal` back and a `<wbr />` supplies the one break opportunity the figure
 * needs, after the currency. That is `PricingScreen`'s technique for the same
 * figure at 200% text, and the two fixes look alike and are not alike: an order id
 * may be cut anywhere (it is an identifier), a price may only be cut at its
 * currency (it is a figure).
 */

/** The source, with every comment form removed. */
const strip = (source: string): string =>
  source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\{\/\*[\s\S]*?\*\/\}/g, '')
    .replace(/\/\/[^\n]*/g, '')

const CHECKOUT = strip(readFileSync(join(__dirname, 'CheckoutScreen.tsx'), 'utf8'))

/**
 * The screen's root container's class list.
 *
 * Anchored backwards from the loading skeleton, because the component has two
 * `flex flex-col gap-6 px-5 py-6` divs — the platform gate's early return is the
 * other, and it renders a `PageHeader` too. Matching on the class string alone
 * would read the gate's branch and pass on it; the nearest `<div className="…"`
 * *before* the skeleton is the container that renders the order summary.
 */
const ROOT_CLASSES = (source: string): string => {
  const anchor = source.indexOf('{payment.isPending &&')
  if (anchor < 0) throw new Error('the checkout screen root container must exist')
  const before = source.slice(0, anchor)
  const start = before.lastIndexOf('<div className="')
  if (start < 0) throw new Error('the checkout screen root container must exist')
  const from = start + '<div className="'.length
  const end = before.indexOf('"', from)
  return before.slice(from, end)
}

/** The order-summary `<dl>`'s body, which is where the two clipped rows live. */
const ORDER_BODY = (source: string): string => {
  const found = /<dl className="[^"]*">([\s\S]*?)<\/dl>/.exec(source)?.[1]
  if (found === undefined) throw new Error('the order summary list must exist')
  return found
}

/** The order-summary `<dl>`'s own class list. */
const ORDER_CLASSES = (source: string): string => {
  const found = /<dl className="([^"]*)">/.exec(source)?.[1]
  if (found === undefined) throw new Error('the order summary list must exist')
  return found
}

/** The `LineValue` component's body, which is where the money break opportunity lives. */
const LINE_VALUE = (source: string): string => {
  const found = /function LineValue\(([\s\S]*?)\n\}/.exec(source)?.[1]
  if (found === undefined) throw new Error('the line value component must exist')
  return found
}

describe('the checkout screen survives 200% text on a phone', () => {
  it('wraps the screen in one inheriting overflow-wrap rule', () => {
    const root = ROOT_CLASSES(CHECKOUT)
    // `anywhere`, not `break-words`: the order id is one unbreakable token 316px
    // wide in a 244px box, and `break-word` would leave its min-content at 316 and
    // the box overflowing — which is the clip, not the fix.
    expect(root).toContain('wrap-anywhere')
    // One container, not a copy on every paragraph: the rule inherits, so the status
    // title (rendered by `CheckoutStatus`) is reached by the same declaration, and a
    // second copy in another file would be the same rule drifting apart.
    expect(root).not.toContain('break-all')
  })

  it('keeps the fix to wrapping, and leaves the type scale and the spacing alone', () => {
    const root = ROOT_CLASSES(CHECKOUT)
    // The reader chose 200%; shrinking the type to make the string fit would defeat
    // the setting, and moving the padding would be fitting the text by changing the
    // page rather than by letting it wrap. The padding is the idiom's `px-5 py-8`
    // (the screen took `py-6` before the redesign), and the width is the modal's own
    // `max-w-2xl` — the checkout is a route, so it says its own width.
    expect(root).toContain('px-5')
    expect(root).toContain('py-8')
    expect(root).toContain('max-w-2xl')
    // `text-label`, not `text-sm`: the app's scale replaced the Tailwind defaults,
    // and `text-sm` is 14px — between two named steps, and the same 14/20 as
    // `text-label`. The assertion is that the list keeps a real body step, so it
    // names the step the scale actually has.
    expect(ORDER_CLASSES(CHECKOUT)).toContain('text-label')
    expect(ORDER_BODY(CHECKOUT)).toContain('meta')
  })

  it('breaks a money figure after the currency and never inside the digits', () => {
    const order = ORDER_BODY(CHECKOUT)
    // The value's own box restores the default the container's `anywhere` took
    // away, which is what brings the digits' min-content width back under the box.
    expect(order).toContain("'meta break-normal'")
    // And the figure itself is not break-anywhere: the rule that would "fix" the
    // overflow by splitting the number is detectable here, and absent.
    expect(order).not.toContain('wrap-anywhere')
    expect(order).not.toContain('break-all')
    const value = LINE_VALUE(CHECKOUT)
    // The break opportunity the figure does need: `formatAmount`'s no-break space is
    // where the currency ends, and `<wbr />` after it is the one place a price may
    // be split. `textContent` still carries the same `IDR\u00a0…` the money
    // assertions in `CheckoutScreen.test.tsx` match on.
    expect(value).toContain("split('\\u00a0')")
    expect(value).toContain("<wbr />")
    expect(value).toContain("'\\u00a0'")
    // An id may be cut anywhere and a figure may not, so the component that splits
    // the money is the one that also declines the value when there is no currency in
    // it — a bare count or a percentage keeps the container's `anywhere`.
    expect(value).toContain('digits === undefined')
  })
})

describe('the checks detect the pattern they exist for', () => {
  // A rule that cannot fail is worse than no rule, so each check is run against the
  // **pre-fix form** as well — the real file with the fix taken back out, one
  // mechanism at a time. These are the forms the browser measured: the container
  // with only its layout classes, the `<dd>` with nothing but `meta`, and a value
  // rendered with no break opportunity in it.
  it('reads the real file, and fails on each pre-fix form', () => {
    expect(ROOT_CLASSES(CHECKOUT)).toBeDefined()
    expect(ORDER_BODY(CHECKOUT)).toBeDefined()
    expect(LINE_VALUE(CHECKOUT)).toBeDefined()

    const withoutContainerRule = CHECKOUT.replace(' wrap-anywhere', '')
    expect(withoutContainerRule).not.toBe(CHECKOUT)
    expect(ROOT_CLASSES(withoutContainerRule)).not.toContain('wrap-anywhere')

    const withoutBreakNormal = CHECKOUT.replace("'meta break-normal'", "'meta'")
    expect(withoutBreakNormal).not.toBe(CHECKOUT)
    expect(ORDER_BODY(withoutBreakNormal)).not.toContain('break-normal')

    const withoutBreakOpportunity = CHECKOUT.replace('<wbr />', '')
    expect(withoutBreakOpportunity).not.toBe(CHECKOUT)
    expect(LINE_VALUE(withoutBreakOpportunity)).not.toContain('<wbr />')

    // And the extractors really are reading the file rather than returning a
    // constant, so none of the three can pass on any source at all.
    expect(ROOT_CLASSES(CHECKOUT)).not.toBe(ROOT_CLASSES(withoutContainerRule))
  })
})
