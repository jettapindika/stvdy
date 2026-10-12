import { describe, expect, it } from 'vitest'

import {
  PLANS,
  REPORT_CREDITS,
  TASK_CREDITS,
  parsePlans,
  planForFeature,
  planForFormat,
  planForReminderChannel,
} from './plans'

/**
 * The client half of two rules that are enforced on the server.
 *
 * `crates/core` loads the same file and charges from it, so these tests are not
 * about correctness on the server's behalf: they are about the app *containing* a
 * number that matches. The honesty gate is that the credit cost is on screen
 * before a generative action, and a price this module got wrong would be a price
 * the student was quoted and not charged — or worse, the other way round.
 *
 * The failure direction is the part worth pinning. A parse that does not
 * understand the file yields *no number* and *no format*, never a default: a
 * format the app offered because a parse failed would be a purchase the plan does
 * not include, which is the one thing the mobile policy exists to prevent.
 */
describe('the plan matrix reader', () => {
  it('reads the report prices the file declares', () => {
    // The two numbers the screens quote. They are the file's, read at build time,
    // so this is the assertion that a change to `config/plans.toml` reaches the
    // app rather than leaving a literal behind.
    expect(REPORT_CREDITS.outline).toBe(3)
    expect(REPORT_CREDITS.section).toBe(6)
  })

  it('reads each plan\'s export formats, and only those', () => {
    // Free gets Markdown and nothing else — no PDF, no DOCX, which is the gate the
    // export menu is built on.
    expect(PLANS.free?.exportFormats).toEqual(['md'])
    expect(PLANS.pro?.exportFormats).toContain('pdf')
    expect(PLANS.power?.exportFormats).toContain('pdf')
  })

  it('names the cheapest plan that first includes a format', () => {
    // Derived from the file rather than written down, so a locked message cannot
    // name a plan the matrix does not grant. `md` is on every plan, so the first
    // plan that includes it is the cheapest; `pdf` is the interesting one.
    expect(planForFormat('pdf')).toBe('Pro')
    expect(planForFormat('docx')).toBe('Pro')
    // A format no plan includes has nothing to upgrade to, and the caller must not
    // offer one.
    expect(planForFormat('epub')).toBeNull()
  })

  it('gives an undeclared plan no formats at all', () => {
    // Deny by default, which is the direction `policy.ts` also fails in: a plan id
    // this file does not declare is a plan whose entitlements are unknown, and
    // unknown is not "everything".
    expect(PLANS['team']).toBeUndefined()
    expect(PLANS['']).toBeUndefined()
  })

  it('leaves a price null when the file does not declare it', () => {
    const parsed = parsePlans('[credits]\nreport_outline = 3\n')
    expect(parsed.credits).toEqual({ outline: 3, section: null })
  })

  it('does not read a value into the wrong block', () => {
    // A `report_section` under `[plans.free]` is not the price, and reading it as
    // one would put a plan limit on a button. The parser only accepts the two keys
    // inside `[credits]`.
    const parsed = parsePlans('[plans.free]\nreport_section = 99\nexport_formats = ["md"]\n')
    expect(parsed.credits).toEqual({ outline: null, section: null })
    expect(parsed.plans.free?.exportFormats).toEqual(['md'])
  })

  it('ignores a value it cannot parse rather than guessing one', () => {
    // A typo in the value must not read as enabled: an unparseable price is no
    // price, and an unparseable format list is no formats.
    const parsed = parsePlans('[credits]\nreport_section = yes\n[plans.pro]\nexport_formats = md\n')
    expect(parsed.credits.section).toBeNull()
    expect(parsed.plans.pro?.exportFormats).toEqual([])
  })

  it('stops a plan block at the next header', () => {
    // Without this, a value under the following section would be attributed to the
    // plan before it, and `[plans.free]` would inherit a paid format.
    const parsed = parsePlans(
      '[plans.free]\nname = "Free"\n[plans.pro]\nexport_formats = ["md", "pdf"]\n',
    )
    expect(parsed.plans.free?.exportFormats).toEqual([])
    expect(parsed.plans.pro?.exportFormats).toEqual(['md', 'pdf'])
    expect(parsed.plans.free?.name).toBe('Free')
  })

  it('reads task_breakdown from the credits table', () => {
    // The number the quick-capture button and the break-down panel show *before*
    // the action runs (gate 7d). It is the file's, read at build time, so this is
    // the assertion that a change to `config/plans.toml` reaches both screens
    // rather than leaving a literal behind.
    expect(TASK_CREDITS.breakdown).toBe(2)
  })

  it('prices a quick capture as the break-down action, because the matrix has no key of its own', () => {
    // A190: `[credits]` has `task_breakdown` and no `quick_capture`, and
    // `CreditAction` has no variant for one. Both screens therefore read this one
    // field — the assertion pins that they cannot drift onto two different numbers
    // without this test moving with them.
    const parsed = parsePlans('[credits]\ntask_breakdown = 5\nreport_outline = 3\n')
    expect(parsed.taskCredits.breakdown).toBe(5)
    expect(parsed.credits.outline).toBe(3)
  })

  it('returns null rather than a guess when the key is absent', () => {
    // The failure direction the report credits also fail in: a missing price is no
    // price, never a default. A button that quoted a number the matrix does not
    // charge is a price the student was shown and not charged — or worse, the
    // other way round.
    const parsed = parsePlans('[credits]\nreport_outline = 3\nreport_section = 6\n')
    expect(parsed.taskCredits.breakdown).toBeNull()
  })

  it('does not read a task_breakdown outside the credits table', () => {
    // `task_breakdown` is also a *feature* name under `[plans.*]`, where it is a
    // boolean grant rather than a price. Reading it there would put `NaN` — or, on
    // a plan that grants it, a `1` — on the button.
    const parsed = parsePlans('[plans.pro]\ntask_breakdown = true\nquick_capture = true\n')
    expect(parsed.taskCredits.breakdown).toBeNull()
  })

  it('names the plan that first grants a feature, from the matrix', () => {
    // The locked quick-capture card says "Available on Pro", and the name has to
    // come from the file: a screen that remembered the pricing table would keep
    // naming Pro after a retune moved the grant to Power.
    expect(planForFeature('quick_capture')).toBe('Pro')
    expect(planForFeature('task_ai_breakdown')).toBe('Pro')
    // A feature no plan grants has nothing to upgrade to, and the caller must not
    // offer one.
    expect(planForFeature('telepathy')).toBeNull()
  })

  it('names the plan that first grants a reminder channel', () => {
    // Free has web and push; email arrives on Pro, telegram and discord on Power.
    expect(planForReminderChannel('email')).toBe('Pro')
    expect(planForReminderChannel('telegram')).toBe('Power')
    expect(planForReminderChannel('web')).toBe('Pro')
  })

  it('treats an unstated grant as absent rather than false', () => {
    // "The file does not say" and "the file says no" are different answers, and
    // only one of them is a fact about the plan. A plan block that omits the key
    // is skipped by the lookup rather than treated as granting it.
    const parsed = parsePlans(
      '[plans.free]\nname = "Free"\n[plans.pro]\nname = "Pro"\nquick_capture = true\n',
    )
    expect(parsed.plans.free?.grants['quick_capture']).toBeUndefined()
    expect(parsed.plans.pro?.grants['quick_capture']).toBe(true)
    expect(parsed.plans.pro?.reminderChannels).toEqual([])
  })
})
