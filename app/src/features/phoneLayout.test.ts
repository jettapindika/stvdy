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

describe('the workspace list survives 200% text', () => {
  it('gives the name a box it can wrap in, and lets the card controls wrap around it', () => {
    // The redesign turned the row into a **card in a grid**, and that changes which
    // defect is possible here rather than removing the class of defect.
    //
    // The row's old failure: `flex-1` on the name beside three effectively
    // fixed-width siblings left it **44px** of a 390px phone at 200% text — measured
    // `scrollWidth` 81 against a `clientWidth` of 44, the workspace's own name
    // wrapping six lines at 100% text and twelve at 200%. The name is no longer in
    // that row at all: it is the card's heading, in a vertical flex column, so
    // nothing competes with it for width and `flex-1` is not the property to assert.
    //
    // What is left to get wrong is the box itself — a card in a grid track is a
    // definite width, but a long unbroken word still overflows one unless it may
    // break — and the header row inside the card, where the source-count chip does
    // sit beside Rename and Delete and must wrap out of the way at 200%.
    const file = read('features/workspaces/WorkspaceListScreen.tsx')
    const name = classOf(file, /className="([^"]*text-h4[^"]*)"\s*>\s*<Link/)
    expect(name, 'the workspace name heading must exist').not.toBeNull()
    expect(name).toContain('min-w-0')
    expect(name).toContain('wrap-anywhere')

    // The header row: the chip plus two controls, which at 200% text do not fit one
    // line of a phone card.
    const header = classOf(file, /className="(flex flex-wrap items-center justify-between[^"]*)"/)
    expect(header, 'the card header row must exist').not.toBeNull()
    expect(header).toContain('flex-wrap')
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

describe('the workspace panels survive 200% text', () => {
  it('lets a word break inside the panel rather than out of it', () => {
    // The desktop leg's largest cluster, and it is one rule in one place. At 200%
    // text on a 1365px window each of the grid's three tracks resolves to ~221px and
    // its content box to **106px**, while a word like `workspace's` at `text-xs`
    // (24px at a 32px root) is ~130px of glyphs. Every paragraph, label and heading
    // in every panel that carries such a word therefore had a run of glyphs wider
    // than its box, and with `overflow: visible` it simply hung out of it: measured
    // on both themes, `p.text-xs.text-muted "After the file is stored, this
    // workspace"` was `scrollWidth` 144 against a `clientWidth` of 106, `"Costs 4
    // credits per 10 cards…"` 123 against 107, `label.label "Questions"` 118 against
    // 107, `h2.font-medium "Cheatsheet"` 223 against 107, and the report form 230
    // against 155.
    //
    // `break-words` (`overflow-wrap: break-word`) is **not** enough for any of them,
    // which is why the cluster is fixed with `anywhere` on the container rather than
    // by adding `break-words` to each box: `break-word` only lowers a box's *content*
    // width, not its **min-content** size, and min-content is what these boxes were
    // failing on — the word hung out and nothing could reach it, because the panel
    // scrolls vertically and the overflow is horizontal. `anywhere` is the value that
    // affects min-content sizing, and it inherits, so one rule on the container
    // reaches every descendant.
    const file = read('features/workspaces/WorkspaceScreen.tsx')
    const container = classOf(file, /className="([^"]*overflow-y-auto[^"]*@container[^"]*)"/)
    expect(container, 'the panel content container must exist').not.toBeNull()
    expect(container).toContain('wrap-anywhere')
    // The container, not each box: the rule is about the width the boxes are laid
    // out in, and a second copy on every paragraph would be the same rule written
    // several times and drifting apart.
    expect(container).toContain('min-h-0')
  })

  it('gives every title row the same permission, not only the panel container', () => {
    // The four title rows are the one place inside the panels where `break-words`
    // was already present and still lost, and where the *row* also has to change. Each
    // is a `<span>` with `min-w-0 flex-1` in a wrapping row whose other children are
    // fixed-width (a count, a status chip, a link), so its width is the remainder the
    // flex algorithm leaves it — and with the container's `anywhere` its min-content
    // is **one character**, so nothing stops the squeeze. Measured in Chromium on the
    // workspace fixture: the deck title rendered **24px wide and 308px tall at 100%
    // text** — a line per word, with no overflow for the clip check to report — and
    // 18px wide and 1452px tall at 200%, where it *is* reported (`scrollWidth` 28
    // against a `clientWidth` of 18).
    //
    // The fix is `w-full … @md:w-auto @md:flex-1`, and the breakpoint is a
    // **container** one because the width that decides the row is the panel's, not the
    // window's: at 1365px a three-column grid leaves ~157px of panel content while a
    // 390px phone leaves ~350px, so the viewport `sm:` these rows used to carry fired
    // on the desktop window and handed the title back the squeeze. `@md` is 28rem *of
    // container* and rem-based, so its threshold doubles with the reader's text size.
    const titles = [
      read('features/workspaces/WorkspaceScreen.tsx'),
      read('features/studio/FlashcardsPanel.tsx'),
      read('features/studio/QuizPanel.tsx'),
      read('features/studio/ReportPanel.tsx'),
    ].flatMap((source) =>
      [...source.matchAll(/className="([^"]*min-w-0[^"]*font-medium[^"]*)"/g)].map(
        (match) => match[1],
      ),
    )
    expect(titles.length, 'the four title spans must exist').toBe(4)
    for (const title of titles) {
      expect(title).toContain('wrap-anywhere')
      expect(title).not.toContain('break-words')
      expect(title).toContain('w-full')
      expect(title).toContain('@md:w-auto')
      expect(title).toContain('@md:flex-1')
    }
    // The container the `@md:` in those rows is measured against has to exist, or the
    // variant falls back to the viewport and the desktop window gets the squeeze back.
    const file = read('features/workspaces/WorkspaceScreen.tsx')
    const container = classOf(file, /className="([^"]*overflow-y-auto[^"]*@container[^"]*)"/)
    expect(container, 'the panel content container must exist').not.toBeNull()
    expect(container).toContain('@container')
    // The source picker's row is the same shape without `font-medium`, and it is the
    // one the report form's own `scrollWidth` 230 against 155 came from.
    const picker = read('features/workspaces/SourcePicker.tsx')
    const row = classOf(picker, /className="([^"]*shrink-0[^"]*)"\s*>/)
    expect(row, 'the source picker row must exist').not.toBeNull()
    expect(row).toContain('shrink-0')
    expect(picker).toContain('min-w-0 flex-1 wrap-anywhere')
  })
})

describe('the citation chip is an inline target, and the desktop floor does not apply', () => {
  it('records the decision where the rule lives, and applies no desktop min-height', () => {
    // The desktop walk reports `button.cite "p.12"` as undersized on every workspace
    // surface: **72.8x30.4 at 200% text and 36.4x15.2 at 100%**, both themes. That it
    // is the same finding at both scales is the first clue it is not a text-scale
    // defect; the second is that its height at 100% is the line-height of the prose it
    // sits in (measured 15.2px against a 15.225px line-height), because it is a target
    // **inside the answer's own sentence**.
    //
    // WCAG 2.2 SC 2.5.8 exempts exactly that — "the target is in a sentence or its
    // size is otherwise constrained by the line-height of non-target text" — and the
    // desktop floor's own "≥ 32px with **8px gaps**" clause cannot be met by a mark
    // that has to sit between two words. A `min-height` here would double the line box
    // of every line carrying a citation at 100%, a visible cost to reading for a case
    // the rule exempts. Below 767px the same trade is made the other way on purpose,
    // because touch has no hover and no precise pointer — and that is where the only
    // `min-height` on the interactive chip lives.
    //
    // So the finding is: the rule is right and the chip is exempt. Pinned here because
    // the alternative — silently leaving twelve reported findings with no verdict —
    // is how a documented exemption turns back into a defect somebody re-fixes.
    const css = read('styles.css')
    const start = css.indexOf('.cite {')
    const phone = css.indexOf('@media (max-width: 767px)', start)
    expect(start, 'the .cite rule must exist').toBeGreaterThan(-1)
    expect(phone, 'the phone treatment must follow it').toBeGreaterThan(start)
    // No `min-height` on the desktop rule: the chip's height is its line-height.
    expect(css.slice(start, phone)).not.toContain('min-height')
    // The phone rule does carry one, which is the deliberate half of the decision.
    expect(css.slice(phone, phone + 260)).toContain('min-height: 2.75rem')
    // And the reason is written where the next reader meets the rule, not only here.
    expect(css).toContain('SC 2.5.8')
  })
})


describe('the quiz screen survives 200% text', () => {
  const quiz = read('features/quiz/QuizScreen.tsx')

  it('uses no rem-based minimum width, and lets an option row wrap', () => {
    // The option row is the control gate 5(c) names on this screen, and it holds three
    // things that each want to be on their own line at 200%: the radio, the option
    // text (an option is a sentence, in Indonesian often a long one) and the state
    // word. `flex-wrap` plus `min-w-0` on the text is what puts the word on its own
    // line rather than out of the card.
    expect(remMinimums(quiz)).toEqual([])
    // The row's className is a template literal (it picks the tone from the state), so
    // the assertion is on the literal's own prefix rather than on a capture.
    expect(quiz).toContain('flex min-h-11 min-w-0 flex-wrap items-center gap-x-3')
    expect(quiz).toContain('min-w-0 flex-1 basis-40 break-words text-label')
  })

  it('keeps the question navigator out of the tab order and off the target floor', () => {
    // The frame draws ten 40x40 cells, and 40px is under the phone floor. They are
    // therefore not controls: this screen has no jump-to-question behaviour, and adding
    // one would change what an attempt means. So the navigator is an `<ol>` whose
    // position is announced with `aria-current`, and the slice between the navigator's
    // own label and the legend under it holds no control at all.
    expect(quiz).toContain("aria-label={t('quiz.session.navigatorLabel')}")
    expect(quiz).toContain('aria-current={current ?')
    expect(quiz).toContain('h-10 w-10')

    const from = quiz.indexOf("quiz.session.navigatorLabel")
    const to = quiz.indexOf("quiz.session.legend")
    expect(from, 'the navigator must exist').toBeGreaterThan(-1)
    expect(to, 'the legend must follow it').toBeGreaterThan(from)
    const cells = quiz.slice(from, to)
    expect(cells).toContain('<li')
    expect(cells).not.toContain('<button')
    expect(cells).not.toContain('<input')
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
    // The panel content container, before it carried the wrapping rule: `overflow-y-auto`
    // and no `wrap-anywhere`, which is the form the 106px-wide boxes were measured in.
    expect('min-h-0 flex-1 overflow-y-auto').not.toContain('wrap-anywhere')
    expect('min-h-0 flex-1 overflow-y-auto').not.toContain('@container')
    // A title row, before it was given its own line in a narrow panel — the form in
    // which the deck title measured 24px wide and 308px tall at 100% text.
    expect('min-w-0 flex-1 break-words font-medium').not.toContain('w-full')
    expect('min-w-0 flex-1 wrap-anywhere font-medium').not.toContain('@md:flex-1')
    // And the viewport `sm:` these rows used to carry, which is the form that fired on
    // the desktop window and handed the title back the squeeze.
    expect('w-full min-w-0 break-words font-medium sm:w-auto sm:flex-1').not.toContain('@md:')
  })
})
