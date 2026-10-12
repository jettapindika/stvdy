import plansToml from '@config/plans.toml?raw'

/**
 * The plan matrix, read from `config/plans.toml`.
 *
 * This is the same file the server loads at startup
 * (`crates/core/src/config/plans.rs`), imported as text at build time, so the app
 * quotes the numbers the server actually charges rather than a copy that can
 * drift. It is the mechanism `features/billing/policy.ts` already uses for
 * `billing_policy.toml`, and the parser below is deliberately as small and as
 * strict as that one: it understands the shapes this file has, and anything it
 * does not understand falls through to *no number and no format*, which is the
 * honest direction — a price the app guessed at is worse than a price it does not
 * show, and an export format the plan does not include must not be offered.
 *
 * Two facts this module exists for, and neither may be a literal in a component:
 *
 * - what the two generative report steps cost (`[credits]` `report_outline` and
 *   `report_section`), because the honesty rule is that the cost is visible
 *   *before* the action runs;
 * - which export formats a plan includes (`[plans.<id>] export_formats`), because
 *   PDF export is gated by the plan and the server refuses the format it does not
 *   allow with 403 `plan_required`.
 */

export interface PlanFacts {
  /** The plan's display name as the file spells it ("Free", "Pro"), or null. */
  name: string | null
  /** The export formats the plan includes. Empty for an unknown plan. */
  exportFormats: string[]
  /**
   * The reminder channels the plan grants, in the file's order.
   *
   * Read here rather than from `GET /usage` because two different questions are
   * being asked of the same fact: the *screen* asks the server what this account
   * may choose (that is the enforcement), and the *locked message* asks the
   * matrix which plan to name as having it. The second is a fact about the file,
   * and a screen that derived it from the account's own plan could only ever say
   * "not yours".
   */
  reminderChannels: string[]
  /**
   * The plan's boolean grants, keyed as the matrix spells them (`quick_capture`,
   * `task_ai_breakdown`, …). A key the plan block does not state is absent rather
   * than `false`, because "the file does not say" and "the file says no" are
   * different answers and only one of them is a fact about this plan.
   */
  grants: Record<string, boolean>
}

export interface ReportCredits {
  /** `report_outline`: the price of proposing an outline. */
  outline: number | null
  /** `report_section`: the price of writing one section, and of a cheatsheet. */
  section: number | null
}

/**
 * What the matrix charges for the two generative task actions.
 *
 * One number, not two, and that is the matrix's own shape rather than a
 * simplification here (A190): `config/plans.toml [credits]` has a `task_breakdown`
 * key and no `quick_capture` key, and `CreditAction` has no variant for one. The
 * roadmap's rule for a missing key is that the closest *stated* action's price
 * applies rather than one invented in code — the same rule that charges a
 * cheatsheet as `report_section` (A120). So both screens quote this field, and the
 * name says what it is: the price of the breakdown action, which a capture is
 * priced as.
 */
export interface TaskCredits {
  /** `task_breakdown`: the price of a break-down, and of a quick capture (A190). */
  breakdown: number | null
}

export interface ParsedPlans {
  plans: Record<string, PlanFacts>
  credits: ReportCredits
  taskCredits: TaskCredits
}

/**
 * Parses the `[credits]` table and the `[plans.<id>]` blocks.
 *
 * Every other block (`[meta]`, `[provider_cost_ceiling_usd]`) ends the block
 * before it rather than inheriting its values, which is the same rule
 * `parsePolicy` follows: a value after an unexpected section would otherwise be
 * attributed to the wrong plan.
 */
export function parsePlans(toml: string): ParsedPlans {
  const plans: Record<string, PlanFacts> = {}
  const credits: ReportCredits = { outline: null, section: null }
  const taskCredits: TaskCredits = { breakdown: null }
  let current: PlanFacts | null = null
  let inCredits = false

  for (const rawLine of toml.split('\n')) {
    // Comments are stripped before anything else, so a `#` inside a value cannot
    // be mistaken for structure. This file has no quoted `#`.
    const line = (rawLine.split('#')[0] ?? '').trim()
    if (line === '') continue

    const header = /^\[([a-z_.]+)\]$/.exec(line)?.[1]
    if (header !== undefined) {
      const planId = /^plans\.([a-z_]+)$/.exec(header)?.[1]
      current = null
      if (planId !== undefined) {
        current = { name: null, exportFormats: [], reminderChannels: [], grants: {} }
        plans[planId] = current
      }
      inCredits = header === 'credits'
      continue
    }

    const entry = /^([a-z_]+)\s*=\s*(.+)$/.exec(line)
    const key = entry?.[1]
    const raw = entry?.[2]
    if (key === undefined || raw === undefined) continue
    const value = raw.trim()

    if (inCredits) {
      const digits = /^(\d+)$/.exec(value)?.[1]
      if (digits === undefined) continue
      if (key === 'report_outline') credits.outline = Number(digits)
      if (key === 'report_section') credits.section = Number(digits)
      if (key === 'task_breakdown') taskCredits.breakdown = Number(digits)
      continue
    }

    if (current === null) continue

    if (key === 'name') {
      const quoted = /^"([^"]*)"$/.exec(value)?.[1]
      if (quoted !== undefined) current.name = quoted
      continue
    }

    if (key === 'export_formats') {
      const body = /^\[(.*)\]$/.exec(value)?.[1]
      if (body === undefined) continue
      current.exportFormats = [...body.matchAll(/"([^"]*)"/g)].flatMap((match) =>
        match[1] === undefined ? [] : [match[1]],
      )
      continue
    }

    if (key === 'reminder_channels') {
      const body = /^\[(.*)\]$/.exec(value)?.[1]
      if (body === undefined) continue
      current.reminderChannels = [...body.matchAll(/"([^"]*)"/g)].flatMap((match) =>
        match[1] === undefined ? [] : [match[1]],
      )
      continue
    }

    // Every other boolean in the block is a grant. They are recorded generically
    // rather than by name so a feature the matrix gains does not need a line here
    // — and so a locked message can name the plan that has one, which is the fact
    // `planForFeature` reads.
    const flag = /^(true|false)$/.exec(value)?.[1]
    if (flag !== undefined) current.grants[key] = flag === 'true'
  }

  return { plans, credits, taskCredits }
}

const PARSED = parsePlans(plansToml)

/**
 * What the plan matrix charges for the report's two generative steps.
 *
 * A number, so it is resolved once at import rather than per render. Either side
 * is null when the file does not declare it, and the screens then show no price
 * rather than a wrong one.
 */
export const REPORT_CREDITS: ReportCredits = PARSED.credits

/**
 * What the plan matrix charges for the two generative task actions.
 *
 * Both are the `task_breakdown` price (A190). The screens quote this field before
 * the action runs — never a literal, and never `REPORT_CREDITS` — so a change to
 * `config/plans.toml` reaches the button.
 */
export const TASK_CREDITS: TaskCredits = PARSED.taskCredits

/**
 * The plans the file declares, keyed by plan id.
 *
 * A screen reads `PLANS[user.plan_id]?.exportFormats ?? []`, which is the
 * deny-by-default direction: an undeclared plan offers no format the matrix did
 * not grant it.
 */
export const PLANS: Record<string, PlanFacts> = PARSED.plans

/**
 * The cheapest plan that satisfies `has`, named the way `LockedFeature` spells
 * it, or null when no plan does.
 *
 * The plans are read in the order the file declares them, which is cheapest
 * first — the same order the pricing table is written in. Deriving this from the
 * matrix rather than writing it down is what stops a locked message from naming a
 * plan the file does not grant the feature on, which is the one thing a locked
 * message must never do.
 *
 * Null means "no plan has it", and the caller must then offer no upgrade: a
 * feature nothing grants has nothing to upgrade to, and naming a plan anyway would
 * be a promise the matrix does not make.
 */
function firstPlan(has: (facts: PlanFacts) => boolean): 'Pro' | 'Power' | null {
  for (const facts of Object.values(PLANS)) {
    if (!has(facts)) continue
    if (facts.name === 'Pro' || facts.name === 'Power') return facts.name
  }
  return null
}

/**
 * The plan an export format first appears on.
 *
 * Derived from the matrix rather than written down, so adding a format to a plan
 * in `config/plans.toml` cannot leave a locked message naming the wrong plan.
 *
 * Null when no plan includes the format, in which case there is nothing to
 * upgrade to and the caller must not offer one.
 */
export function planForFormat(format: string): 'Pro' | 'Power' | null {
  return firstPlan((facts) => facts.exportFormats.includes(format))
}

/**
 * The plan a reminder channel first appears on.
 *
 * The same shape as `planForFormat`, over `reminder_channels` instead of
 * `export_formats`: the settings screen shows a channel the account's plan does
 * not grant as a locked row naming the plan that does, and that name has to come
 * from the file rather than from a screen's memory of the pricing table.
 */
export function planForReminderChannel(channel: string): 'Pro' | 'Power' | null {
  return firstPlan((facts) => facts.reminderChannels.includes(channel))
}

/**
 * The plan a boolean grant first appears on (`quick_capture`,
 * `task_ai_breakdown`, …).
 *
 * A key a plan block does not state is not a grant, so a plan that omits it is
 * skipped rather than treated as granting it — the same deny-by-default direction
 * the server's `plans.get(plan)` reads in.
 */
export function planForFeature(feature: string): 'Pro' | 'Power' | null {
  return firstPlan((facts) => facts.grants[feature] === true)
}
