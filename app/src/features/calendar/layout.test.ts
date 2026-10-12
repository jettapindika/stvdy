import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The calendar screen has no minimum width that grows with the reader's text size.
 *
 * This is a **source** assertion, not a rendered one, and the reason is the
 * harness: jsdom has no layout engine, so `getBoundingClientRect()` and
 * `scrollWidth` here are all zeros and no overflow can be measured in this suite.
 * What can be asserted is the property that makes the overflow impossible, which is
 * the technique `components/layout/shellLayout.test.ts` and
 * `tokens/invariants.test.ts` already use. **The number is the browser
 * measurement**, taken by `scripts/measure-mobile.mjs` at 390x844 with 200% text;
 * this file is what stops the defect coming back without a browser.
 *
 * The defect, found by Phase 10's accessibility gate:
 *
 *   - `ConnectionsPanel`'s ICS URL label carried `min-w-60`, i.e.
 *     `min-width: 15rem` — 240px at a 16px root and **480px at a 32px root**;
 *   - at 200% text on a 390px phone the field alone was wider than the viewport,
 *     so the page scrolled sideways: `scrollWidth` **626** against `clientWidth`
 *     **390**, in both themes, and **no overflow at all at 100%**;
 *   - it is the seventh defect of this class across Phases 6-14 and the first whose
 *     mechanism is a rem-based *minimum* rather than a fixed box, which is why the
 *     rule below is about the whole family and not about `min-w-60`.
 *
 * `min-w-0` is the safe value and is what the layouts here use: it is not a width,
 * it is the permission to shrink below the content's intrinsic size, and `flex-1`
 * or `w-full` is what actually sizes the box.
 */

const here = __dirname

/** Every non-test source file in this directory. */
function sourceFiles(): string[] {
  return readdirSync(here)
    .filter((entry) => /\.tsx?$/.test(entry) && !entry.includes('.test.'))
    .map((entry) => join(here, entry))
    .filter((path) => statSync(path).isFile())
}

/**
 * A rem-based minimum width utility, with its own comment stripped.
 *
 * The comments matter: this file and the fix's own comment in `ConnectionsPanel`
 * both quote `min-w-60` as the thing that was wrong, and a scan of the raw text
 * would fail on the explanation of the defect. What is asserted is what Tailwind
 * would compile.
 */
function remMinimums(text: string): string[] {
  const withoutComments = text
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\{\/\*[\s\S]*?\*\/\}/g, '')
    .replace(/\/\/[^\n]*/g, '')
  // `min-w-<number>` is Tailwind's spacing scale, which is rem-based and therefore
  // doubles with the root font size. `min-w-0` is the one safe numeric value — it
  // is not a width but the permission to shrink below the content's intrinsic
  // size — and `min-w-full`/`min-w-fit`/`min-w-max` are relative to the container
  // or to the content, so none of those is a minimum that grows with the text.
  return [...withoutComments.matchAll(/\bmin-w-([1-9]\d*)\b/g)].map((match) => match[0])
}

describe('the calendar screen can shrink to a phone at 200% text', () => {
  it('uses no rem-based minimum width anywhere in the feature', () => {
    const offenders: string[] = []
    for (const path of sourceFiles()) {
      const text = readFileSync(path, 'utf8')
      for (const found of remMinimums(text)) {
        offenders.push(`${path.slice(here.length + 1)}: ${found}`)
      }
    }
    expect(offenders, 'a rem minimum doubles with the text scale and overflows a phone').toEqual([])
  })

  it('keeps the ICS field shrinkable, which is the defect this exists for', () => {
    // The exact property, not only the absence of the utility: the field's label
    // must be allowed to shrink (`min-w-0`) *and* must be the thing that takes the
    // row (`flex-1`). A `min-w-0` with no `flex-1` would collapse the field to its
    // intrinsic width instead, which is a different layout with the same class.
    const panel = readFileSync(join(here, 'ConnectionsPanel.tsx'), 'utf8')
    const label = /<label className="([^"]*)" htmlFor="ics-url">/.exec(panel)
    expect(label, 'the ICS URL label must exist').not.toBeNull()
    expect(label?.[1]).toContain('min-w-0')
    expect(label?.[1]).toContain('flex-1')
  })

  it('pins the week grid against the same class of overflow', () => {
    // The second finding, and the one that survived the `min-w-60` fix: a table's
    // `min-content` width is the sum of its cells' unbreakable content, and that
    // number propagates up the ancestor chain until something gives. Measured at
    // 200% text on a 390px phone: the week grid's table was **361.2px** of
    // min-content against **310px** available, and the page scrolled sideways —
    // `scrollWidth` **430** against `clientWidth` **390**, both themes, nothing at
    // 100%. Three properties together are the fix, and each is asserted separately
    // because dropping any one of them brings the overflow back:
    //
    //   1. `table-fixed`, so the table takes the width it is given instead of
    //      demanding its content's;
    //   2. the column widths declared as fractions, never in rem (a rem width
    //      doubles with the reader's text size — the `min-w-60` mechanism again);
    //   3. no `shrink-0` on a *text* item inside the event row, and `overflow-wrap:
    //      anywhere` where a single word must be able to break.
    //
    // Re-measured after the change: `scrollWidth === clientWidth === 390` at both
    // scales, both themes, 0 clipped and 0 overlapped.
    const grid = readFileSync(join(here, 'WeekGrid.tsx'), 'utf8')
    expect(grid, 'the week table must be table-fixed').toContain('table-fixed')
    // `[^-]` before the `w` so `min-w-0` is not read as a width, and `(?!\/)` so a
    // fraction (`w-1/3`) is not either: the rem scale is the integer form only.
    expect(grid, 'a rem column width doubles with the text scale').not.toMatch(
      /[^-]w-[0-9]{1,2}\b(?!\/)/,
    )
    // The marker and the single-initial chip may keep `shrink-0` — 4px and one
    // character cannot be the cause of an overflow — so this counts rather than
    // forbids: the time label, the title, the course and the read-only word must
    // all be shrinkable.
    const shrinkZero = [...grid.matchAll(/className="([^"]*shrink-0[^"]*)"/g)].map((m) => m[1])
    for (const classes of shrinkZero) {
      expect(
        classes?.includes('course-marker') || classes?.includes('chip'),
        `shrink-0 is only safe on the marker and the initial chip, not on: ${String(classes)}`,
      ).toBe(true)
    }
    expect(grid, 'a word that must break needs overflow-wrap: anywhere').toContain(
      "overflowWrap: 'anywhere'",
    )
  })

  it('detects the pattern it is looking for', () => {
    // A rule that cannot fail is worse than no rule: this proves the scan sees the
    // pre-fix form, and that it does not fire on the safe values or on a comment
    // quoting the defect.
    expect(remMinimums('<label className="flex min-w-60 flex-1">')).toEqual(['min-w-60'])
    expect(remMinimums('<label className="flex min-w-40">')).toEqual(['min-w-40'])
    expect(remMinimums('<span className="min-w-0">')).toEqual([])
    expect(remMinimums('<span className="min-w-full">')).toEqual([])
    expect(remMinimums('/* was min-w-60 */ <label className="min-w-0">')).toEqual([])
    expect(remMinimums('{/* min-w-60 is 15rem */}')).toEqual([])
  })
})
