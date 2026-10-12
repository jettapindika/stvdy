import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The uncovered surfaces, pinned against the class of defect Phase 14's phone
 * measurement found in them.
 *
 * These are **source** assertions, not rendered ones, and the reason is the harness:
 * jsdom has no layout engine, so `getBoundingClientRect()` and `scrollWidth` here are
 * all zeros and no overflow can be measured in this suite. What can be asserted is
 * the property that makes the overflow impossible — the technique
 * `components/layout/shellLayout.test.ts`, `tokens/invariants.test.ts` and
 * `features/calendar/layout.test.ts` already use. **The numbers are the browser
 * measurements**, taken by `scripts/measure-mobile.mjs` at 390x844, both themes, at
 * 100% and 200% text; this file is what stops each defect coming back without a
 * browser.
 *
 * The surfaces are the ones Phase 14's walk did not reach: the workspace list, the
 * workspace screen and its Studio panels, the chat thread, the plan screen's limits
 * and the Devices rename form. Each defect below was invisible at 100% text and
 * measured in Chromium; every assertion here fails against its pre-fix form.
 *
 * The class of defect, because it is the thing to recognise rather than any one
 * instance: **a `min-w-<n>` utility is `min-width: n × 0.25rem`, and rem doubles with
 * the reader's text size.** `min-w-40` is 160px at a 16px root and 320px at a 32px
 * root, so on a 390px phone at 200% text anything above `min-w-24` is a candidate and
 * what actually overflows depends on the container. `min-w-0` is the opposite and is
 * usually what a flex child wants, because a flex item's default `min-width: auto` is
 * what makes a long word refuse to shrink.
 */

const appRoot = join(__dirname, '..')
const read = (path: string) => readFileSync(join(appRoot, path), 'utf8')

/**
 * A file without its comments.
 *
 * The comments matter: each fix's own comment quotes the utility it removed, so a
 * scan of the raw text would fail on the explanation of the defect. What is asserted
 * is what the browser applies. Both comment forms are stripped, because these files
 * carry both.
 */
function withoutComments(text: string): string {
  return text
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\{\/\*[\s\S]*?\*\/\}/g, '')
    .replace(/\/\/[^\n]*/g, '')
}

/**
 * The rem-based minimum widths in a file, as Tailwind would compile them.
 *
 * `min-w-<number>` is the spacing scale and is rem-based, so it doubles with the root
 * font size. `min-w-0` is not a width but the permission to shrink below the
 * content's intrinsic size; `min-w-full`/`min-w-fit`/`min-w-max` are relative to the
 * container or the content; and a bracket form is whatever it says, so
 * `min-w-[min(10rem,100%)]` is deliberately not matched — it is capped, which is the
 * fix rather than the defect.
 */
function remMinimums(text: string): string[] {
  return [...withoutComments(text).matchAll(/\bmin-w-([1-9]\d*)\b/g)].map((match) => match[0])
}

/**
 * The `className` the first JSX element matching `opening` carries, or `null`.
 *
 * One expression, but not inlined at its call sites: it is the capture-group read
 * six assertions share, and a missed `?.[1]` at one of them would read as an
 * assertion about a class that does not exist.
 */
const classOf = (text: string, opening: RegExp): string | null => opening.exec(text)?.[1] ?? null

describe('the workspace list row survives 200% text', () => {
  it('gives the name its own line at phone width', () => {
    // `flex-1` gives the name a flex basis of `0`, and the row's other three children
    // (a count, Rename, Delete) are all effectively fixed-width. At 200% text they
    // take the whole line, so the name was left **44px** of a 390px phone: measured
    // `scrollWidth` 81 against a `clientWidth` of 44, the workspace's own name
    // wrapping six lines at 100% text and twelve at 200%. `w-full` at phone width
    // pushes the other three to the next line; `sm:` restores the one-line row where
    // there is room for all four.
    const file = read('features/workspaces/WorkspaceListScreen.tsx')
    const name = classOf(file, /className="([^"]*)"\s*>\s*\{workspace\.name\}/)
    expect(name, 'the workspace name link must exist').not.toBeNull()
    expect(name).toContain('w-full')
    expect(name).toContain('sm:w-auto')
    expect(name).toContain('sm:flex-1')
    // And the property, not only the utilities: the name must still be allowed to
    // shrink below its intrinsic width, or it overflows a narrow row instead.
    expect(name).toContain('min-w-0')
  })
})

describe('the workspace panel strips survive 200% text', () => {
  it('scrolls the outer strip instead of clipping its tabs', () => {
    // Each tab is a `.btn`, which carries `min-width: 0` so a long label wraps inside
    // the button rather than overflowing — but a *word* cannot wrap, and at 200% text
    // "Sources" is 110px of glyphs plus 70px of padding, so three tabs need ~540px of
    // a 326px strip. They shrank to 101px each and their labels were clipped:
    // measured `scrollWidth` 106 against a `clientWidth` of 101, and "Flashcards" 124
    // against 101. `min-w-max` stops the shrink; `overflow-x-auto` makes the
    // remainder reachable by swipe and by `Tab`, the same answer the phone tab bar
    // gives for the same reason.
    const file = read('features/workspaces/WorkspaceScreen.tsx')
    const strip = classOf(file, /role="tablist"[\s\S]{0,240}?className="([^"]*)"\s*>/)
    expect(strip, 'the outer panel strip must exist').not.toBeNull()
    expect(strip).toContain('overflow-x-auto')
    expect(strip).toContain('lg:hidden')
    expect(file).toContain('btn flex-1 min-w-max')
  })

  it('scrolls the Studio sub-strip for the same reason', () => {
    const file = read('features/workspaces/WorkspaceScreen.tsx')
    const strips = [...file.matchAll(/className="([^"]*rounded-pill[^"]*)"/g)].map(
      (match) => match[1],
    )
    expect(strips.length, 'both strips carry the pill').toBe(2)
    for (const strip of strips) expect(strip).toContain('overflow-x-auto')
    // Both strips' tabs, not just the outer one's: the sub-strip is the one whose
    // "Flashcards" label measured 124px of content in a 101px box.
    expect([...file.matchAll(/btn flex-1 min-w-max/g)]).toHaveLength(2)
  })
})

describe('the source picker rows survive 200% text', () => {
  it('refuses to shrink a row whose text cannot', () => {
    // The list is capped at `max-h-32` (128px, which is 256px at 200% text), so when
    // the rows' natural heights exceed the cap the flex container shrank them — and a
    // row is a label whose text cannot shrink with it, so the text overflowed the box
    // it was clipped to: measured at 200% text, a row's `scrollHeight` was 147
    // against a `clientHeight` of 135, a line of the source's title cut off inside a
    // scrolling list that was scrolling. `shrink-0` makes the container scroll
    // instead, which is what `overflow-y-auto` is there for.
    const file = read('features/workspaces/SourcePicker.tsx')
    const row = classOf(file, /key=\{source\.id\}\s*className="([^"]*)"/)
    expect(row, 'the source row must exist').not.toBeNull()
    expect(row).toContain('shrink-0')
    // The cap stays: at 100% the rows fit in it and nothing scrolls.
    expect(file).toContain('max-h-32')
    expect(file).toContain('overflow-y-auto')
  })
})

describe('the Studio quiz form survives 200% text', () => {
  it('uses no rem-based minimum width', () => {
    // `min-w-40` was `min-width: 10rem` — 160px at a 16px root and **320px at a 32px
    // root** — so the two children's minimums plus the gap exceeded the panel's 276px
    // content box and the row could not wrap out of it: measured `scrollWidth` 320
    // against a `clientWidth` of 276. The fix is the *same minimum, capped*:
    // `min-w-[min(10rem,100%)]` is 160px while there is room for two and the
    // container's own width when there is not, so at 100% the pair still sits side by
    // side.
    const file = read('features/studio/QuizPanel.tsx')
    expect(remMinimums(file)).toEqual([])
    expect([...withoutComments(file).matchAll(/min-w-\[min\(10rem,100%\)\]/g)]).toHaveLength(2)
  })
})

describe('the plan screen survives 200% text', () => {
  it('lets a limits row wrap rather than pushing its chip out of the box', () => {
    // `justify-between` on a row that cannot wrap puts the label and the value on one
    // line whatever it costs, and the value is a count *plus* an "At the limit" chip:
    // measured at 390px with 200% text, the Workspaces row's `scrollWidth` was 327
    // against a `clientWidth` of 310. The credits and costs `<dl>`s share the row
    // shape and were measured with it; both are label plus a short number and neither
    // overflows, so neither is changed.
    const file = read('features/billing/PlanUsageSections.tsx')
    const row = classOf(file, /key=\{limit\.label\}\s*className="([^"]*)"/)
    expect(row, 'the limits row must exist').not.toBeNull()
    expect(row).toContain('flex-wrap')
    expect(row).toContain('justify-between')
  })
})

describe('the devices screen survives 200% text', () => {
  it('uses no rem-based minimum width', () => {
    // The rename form's label carried `min-w-40`, i.e. 320px at a 32px root — wider
    // than the 310px content box it sat in: measured `scrollWidth` 320 against a
    // `clientWidth` of 310. `min-w-0` with `flex-1` is the pairing
    // `ConnectionsPanel`'s ICS field uses.
    const file = read('features/devices/DevicesScreen.tsx')
    expect(remMinimums(file)).toEqual([])
    const label = classOf(file, /className="(flex [^"]*flex-col gap-1)"\s*>\s*<span className="label">/)
    expect(label, 'the rename label must exist').not.toBeNull()
    expect(label).toContain('min-w-0')
    expect(label).toContain('flex-1')
  })

  it('gives the device name its own line at phone width', () => {
    // The same defect as the workspace list row: `flex-1` on the name plus four
    // fixed-width siblings left it **20px** of a 390px phone at 200% text — measured
    // `scrollWidth` 29 against a `clientWidth` of 20, a device name wrapping a
    // character per line.
    const file = read('features/devices/DevicesScreen.tsx')
    const name = classOf(file, /className="([^"]*break-words font-medium[^"]*)"/)
    expect(name, 'the device name must exist').not.toBeNull()
    expect(name).toContain('w-full')
    expect(name).toContain('sm:w-auto')
    expect(name).toContain('sm:flex-1')
  })
})

describe('the checks detect the pattern they exist for', () => {
  // A rule that cannot fail is worse than no rule: each of these proves the scan sees
  // the pre-fix form and does not fire on the safe values or on a comment quoting the
  // defect.
  it('reads a rem minimum width, and only a rem one', () => {
    expect(remMinimums('<div className="flex min-w-40 flex-1">')).toEqual(['min-w-40'])
    expect(remMinimums('<label className="flex min-w-60">')).toEqual(['min-w-60'])
    expect(remMinimums('<span className="min-w-0">')).toEqual([])
    expect(remMinimums('<span className="min-w-full">')).toEqual([])
    expect(remMinimums('<div className="min-w-[min(10rem,100%)]">')).toEqual([])
    expect(remMinimums('/* was min-w-60 */ <label className="min-w-0">')).toEqual([])
    expect(remMinimums('{/* min-w-40 is 10rem */}')).toEqual([])
  })

  it('reads a className out of the JSX shape each assertion uses', () => {
    expect(classOf('<span className="a b">\n  {workspace.name}\n</span>', /className="([^"]*)"\s*>\s*\{workspace\.name\}/)).toBe('a b')
    expect(classOf('<div className="flex flex-wrap gap-3">', /className="([^"]*)"/)).toBe('flex flex-wrap gap-3')
  })

  // Each assertion above, run against the exact pre-fix string it exists for. A
  // class assertion is one `toContain` away from passing on anything, so these pin
  // the *negative*: the pre-fix form fails the property the assertion checks.
  it('fails on each defect in its pre-fix form', () => {
    // The workspace list row and the devices row, before the name was given its own
    // line: no `w-full`, no `sm:` restore.
    expect('flex min-h-11 min-w-0 flex-1 items-center break-words font-medium').not.toContain(
      'w-full',
    )
    expect('min-w-0 flex-1 break-words font-medium').not.toContain('w-full')
    // Both panel strips, before they scrolled.
    expect('mx-3 mb-3 flex gap-1 rounded-pill bg-surface-raised p-1 lg:hidden').not.toContain(
      'overflow-x-auto',
    )
    expect('btn flex-1 btn-secondary').not.toContain('min-w-max')
    // The source picker row, before it stopped shrinking.
    expect('flex min-h-11 items-center gap-2 rounded-field border border-border').not.toContain(
      'shrink-0',
    )
    // The plan limits row, before it could wrap.
    expect('flex items-baseline justify-between gap-4 py-1.5').not.toContain('flex-wrap')
    // The devices rename label, which is the rem minimum the scan is about.
    expect(remMinimums('<label className="flex min-w-40 flex-1 flex-col gap-1">')).toEqual([
      'min-w-40',
    ])
  })
})
