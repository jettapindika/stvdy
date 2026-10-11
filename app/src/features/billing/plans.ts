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
}

export interface ReportCredits {
  /** `report_outline`: the price of proposing an outline. */
  outline: number | null
  /** `report_section`: the price of writing one section, and of a cheatsheet. */
  section: number | null
}

export interface ParsedPlans {
  plans: Record<string, PlanFacts>
  credits: ReportCredits
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
        current = { name: null, exportFormats: [] }
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
    }
  }

  return { plans, credits }
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
 * The plans the file declares, keyed by plan id.
 *
 * A screen reads `PLANS[user.plan_id]?.exportFormats ?? []`, which is the
 * deny-by-default direction: an undeclared plan offers no format the matrix did
 * not grant it.
 */
export const PLANS: Record<string, PlanFacts> = PARSED.plans

/**
 * The plan an export format first appears on, named the way `LockedFeature`
 * spells it.
 *
 * Derived from the matrix rather than written down, so adding a format to a plan
 * in `config/plans.toml` cannot leave a locked message naming the wrong plan. The
 * plans are read in the order the file declares them, which is cheapest first —
 * the same order the pricing table is written in.
 *
 * Null when no plan includes the format, in which case there is nothing to
 * upgrade to and the caller must not offer one.
 */
export function planForFormat(format: string): 'Pro' | 'Power' | null {
  for (const facts of Object.values(PLANS)) {
    if (!facts.exportFormats.includes(format)) continue
    if (facts.name === 'Pro' || facts.name === 'Power') return facts.name
  }
  return null
}
