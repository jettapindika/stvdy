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
  /**
   * What the file charges for a month of this plan, or null when it says nothing.
   *
   * Read here for the same reason the export formats are: the pricing screen's
   * *assertion* is that the numbers it renders came from the server, and a test
   * that proves it needs the file's own numbers to compare against. Nothing in
   * the app quotes this field — `GET /plans` is where a rendered price comes from,
   * and it is computed by the same `billing::price` the checkout charges from
   * (A305). A screen that read this instead would be a second source for the one
   * number a student is charged, which is the defect gate 7(d) is written about.
   */
  priceIdrMonthly: number | null
  /** The yearly price, read and consumed exactly as `priceIdrMonthly`. */
  priceIdrYearly: number | null
  /**
   * The student discount the file grants on this plan, in percent, or null.
   *
   * The *effective* student price is the server's (`GET /plans`
   * `student_price_idr_monthly`, computed by `billing::price` under A298's
   * no-stacking rule); this field is the file's statement of the discount, which
   * is what the pricing banner is allowed to say.
   */
  studentPercentOff: number | null
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
  /**
   * The plan's planner mode (`manual` | `auto` | `auto+balancer+weakness_blocks`),
   * or null when the block does not state one.
   *
   * A string rather than a boolean, so `grants` cannot carry it and
   * `planForFeature` cannot see it: whether this account may generate a plan is
   * `planner != manual` (A257), and a screen that asked `grants['plan_generation']`
   * would always be answered "no plan has it".
   */
  planner: string | null
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

/**
 * What the matrix charges for the two generative writes the assistant can propose.
 *
 * These are the prices `POST /actions/:id/confirm` charges when the confirmed tool
 * is `create_flashcards` or `create_quiz`, and they are the *same* keys the deck
 * and quiz routes spend under — the executor calls those routes' own service
 * functions (A276), so there is one price per action and this module reads it
 * rather than restating it.
 *
 * The card that asks for confirmation quotes one of these **before** the confirm,
 * which is honesty gate 7(d): a price the student only learns after tapping yes is
 * not a price they agreed to. Either side is null when the file does not declare
 * it, and the card then shows no price rather than a wrong one.
 *
 * A tool that costs nothing — `create_task`, `update_task`, `create_event`,
 * `schedule_study_block` — has no key in the matrix and no field here. The card
 * shows no price for those rather than a `0`, because "the matrix does not charge
 * for this" and "this costs nothing today" are different claims and only one of
 * them is a fact about the file.
 */
export interface AgentCredits {
  /** `flashcard_batch_10`: what a confirmed `create_flashcards` costs. */
  flashcards: number | null
  /** `quiz_10`: what a confirmed `create_quiz` costs. */
  quiz: number | null
}

/**
 * What the matrix charges for the two actions Phase 11 adds.
 *
 * Two numbers rather than one, because they are two actions at two prices: the
 * syllabus import is `[credits] syllabus_import` (8) and generating a plan is
 * `[credits] plan_generation` (2). Neither is priced as another action's key —
 * unlike a cheatsheet, which the matrix has no key for and which is therefore
 * charged as `report_section` (A120).
 *
 * The two screens quote these before the action runs, which is the honesty rule
 * (gate 7(d)) and the reason `config/plans.toml` is the only place a cost comes
 * from: a literal in a component would be a price the student agreed to and the
 * server did not charge.
 */
export interface PlannerCredits {
  /** `syllabus_import`: what uploading a syllabus costs. */
  syllabusImport: number | null
  /** `plan_generation`: what generating or replanning a plan costs. */
  planGeneration: number | null
}

export interface ParsedPlans {
  plans: Record<string, PlanFacts>
  credits: ReportCredits
  taskCredits: TaskCredits
  agentCredits: AgentCredits
  /**
   * What the matrix charges for the two actions Phase 11 adds.
   *
   * Separate from `credits` and `taskCredits` because they are separate actions:
   * a syllabus import is 8 credits and a plan generation 2, and neither is priced
   * as another action's key. The review screen quotes the first before the upload
   * and the planner the second before the plan runs — honesty gate 7(d), and the
   * reason this file is the only place a cost comes from.
   */
  plannerCredits: PlannerCredits
  /**
   * The one-time credit packs, in the order the file declares them.
   *
   * A pack is named by the credits it grants — that is its identity on the wire
   * (`POST /billing/checkout`'s `credit_pack`), because two packs with the same
   * amount would be two products no screen could tell apart, which is why
   * `PlansFile::validate` rejects them on the server.
   */
  creditPacks: CreditPack[]
  /** The currency the file prices in (`[meta] currency`), or null. */
  currency: string | null
}

/** One `[credit_packs]` entry: what it grants, and what it costs. */
export interface CreditPack {
  credits: number
  priceIdr: number
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
  const agentCredits: AgentCredits = { flashcards: null, quiz: null }
  const plannerCredits: PlannerCredits = { syllabusImport: null, planGeneration: null }
  const creditPacks: CreditPack[] = []
  let currency: string | null = null
  let current: PlanFacts | null = null
  let inCredits = false
  let inMeta = false
  let inPacks = false
  // `packs = [` opens a table that spans lines, so its body is accumulated until
  // the closing bracket rather than read off one line. An unterminated one is
  // simply never parsed, which leaves the app with no pack to offer — the same
  // deny-by-default direction every other branch here takes.
  let packBody: string | null = null

  for (const rawLine of toml.split('\n')) {
    // Comments are stripped before anything else, so a `#` inside a value cannot
    // be mistaken for structure. This file has no quoted `#`.
    const line = (rawLine.split('#')[0] ?? '').trim()
    if (packBody !== null) {
      if (line === '') continue
      packBody += ` ${line}`
      if (line.includes(']')) {
        creditPacks.push(...parsePacks(packBody))
        packBody = null
      }
      continue
    }
    if (line === '') continue

    const header = /^\[([a-z_.]+)\]$/.exec(line)?.[1]
    if (header !== undefined) {
      const planId = /^plans\.([a-z_]+)$/.exec(header)?.[1]
      current = null
      if (planId !== undefined) {
        current = {
          name: null,
          priceIdrMonthly: null,
          priceIdrYearly: null,
          studentPercentOff: null,
          exportFormats: [],
          reminderChannels: [],
          grants: {},
          planner: null,
        }
        plans[planId] = current
      }
      inCredits = header === 'credits'
      inMeta = header === 'meta'
      inPacks = header === 'credit_packs'
      continue
    }

    // Digits are part of a key name, not a value: `flashcard_batch_10` and `quiz_10`
    // are the matrix's own names for two prices, and a pattern of `[a-z_]+` reads
    // neither of them — silently, because an unmatched line is skipped rather than
    // reported. That is the failure direction this parser otherwise gets right, so
    // the gap is closed here rather than worked around at the two call sites.
    const entry = /^([a-z0-9_]+)\s*=\s*(.+)$/.exec(line)
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
      if (key === 'flashcard_batch_10') agentCredits.flashcards = Number(digits)
      if (key === 'quiz_10') agentCredits.quiz = Number(digits)
      if (key === 'syllabus_import') plannerCredits.syllabusImport = Number(digits)
      if (key === 'plan_generation') plannerCredits.planGeneration = Number(digits)
      continue
    }

    if (inMeta) {
      // The currency the file prices in. It is the one value the app takes from
      // `[meta]`, and it is what the amount formatter formats *with* — so no
      // component carries a currency literal and the money copy cannot disagree
      // with the file the server charges from.
      if (key !== 'currency') continue
      const quoted = /^"([^"]*)"$/.exec(value)?.[1]
      if (quoted !== undefined) currency = quoted
      continue
    }

    if (inPacks) {
      if (key !== 'packs') continue
      const body = /^\[(.*)\]$/.exec(value)?.[1]
      if (body === undefined) {
        // `packs = [` — the entries are on the following lines.
        packBody = ''
        continue
      }
      creditPacks.push(...parsePacks(body))
      continue
    }

    if (current === null) continue

    if (key === 'name') {
      const quoted = /^"([^"]*)"$/.exec(value)?.[1]
      if (quoted !== undefined) current.name = quoted
      continue
    }

    if (key === 'price_idr_monthly' || key === 'price_idr_yearly' || key === 'student_percent_off') {
      const digits = /^(\d+)$/.exec(value)?.[1]
      if (digits === undefined) continue
      if (key === 'price_idr_monthly') current.priceIdrMonthly = Number(digits)
      if (key === 'price_idr_yearly') current.priceIdrYearly = Number(digits)
      if (key === 'student_percent_off') current.studentPercentOff = Number(digits)
      continue
    }

    if (key === 'planner') {
      // The planner mode, quoted. It is not a grant: whether an account may
      // generate a plan is `planner != manual` (A257), so a screen that asked
      // `grants['plan_generation']` would be told no plan has it.
      const quoted = /^"([^"]*)"$/.exec(value)?.[1]
      if (quoted !== undefined) current.planner = quoted
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

  return { plans, credits, taskCredits, agentCredits, plannerCredits, creditPacks, currency }
}

/**
 * The `{ credits = 100, price_idr = 3000 }` entries of a `[credit_packs]` table.
 *
 * Both fields are required and both must be digits: a pack missing a price is a
 * product the checkout would have to invent a number for, and a pack missing its
 * credit amount cannot be named on the wire at all. Either way the entry is
 * dropped rather than defaulted, which is the same direction every other branch
 * here takes — and the server's own `PlansFile::validate` is what makes a
 * malformed table a boot failure rather than a silent omission.
 */
function parsePacks(body: string): CreditPack[] {
  const packs: CreditPack[] = []
  for (const match of body.matchAll(
    /\{\s*credits\s*=\s*(\d+)\s*,\s*price_idr\s*=\s*(\d+)\s*\}/g,
  )) {
    const credits = match[1]
    const priceIdr = match[2]
    if (credits === undefined || priceIdr === undefined) continue
    packs.push({ credits: Number(credits), priceIdr: Number(priceIdr) })
  }
  return packs
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
 * What the plan matrix charges for the two generative writes the assistant proposes.
 *
 * Read at import like the two tables above, and the confirmation card quotes one of
 * these before the confirm — never a literal, and never `TASK_CREDITS`. A change to
 * `config/plans.toml` therefore reaches the button the server charges from, which is
 * the same file `crates/server/src/flashcards/routes.rs` and `.../quizzes/routes.rs`
 * spend under (A276).
 */
export const AGENT_CREDITS: AgentCredits = PARSED.agentCredits

/**
 * What the plan matrix charges for the syllabus import and the plan generation.
 *
 * Read at import like the tables above, and the two Phase 11 screens quote one of
 * these **before** the action runs — the review screen's upload and the planner's
 * "Suggest a plan" button. Never a literal, and never another action's table: a
 * price the student only learns after tapping yes is not a price they agreed to
 * (honesty gate 7(d)), and `config/plans.toml` is the file the server charges from.
 */
export const PLANNER_CREDITS: PlannerCredits = PARSED.plannerCredits

/**
 * The plans the file declares, keyed by plan id.
 *
 * A screen reads `PLANS[user.plan_id]?.exportFormats ?? []`, which is the
 * deny-by-default direction: an undeclared plan offers no format the matrix did
 * not grant it.
 */
export const PLANS: Record<string, PlanFacts> = PARSED.plans

/**
 * The one-time credit packs, in the file's order.
 *
 * The **buy-credits section renders the server's packs**, not these: `GET /plans`
 * carries `credit_packs` with their prices, computed from the same table this
 * reader parses, and the checkout is named by the credits it grants. This export
 * exists so a test can pin that the screen shows the server's numbers — the
 * fixture's pack price and the file's differ on purpose in
 * `BillingScreen.test.tsx`, and the file's is the one that must not appear.
 */
export const CREDIT_PACKS: CreditPack[] = PARSED.creditPacks

/**
 * The currency the plan file prices in, or null when it does not say.
 *
 * This is the one price-related value the app takes from the file at *runtime*,
 * and it has to: `orderLines` in `checkout.ts` is pure and is handed a payment
 * view, so the currency it formats `amount_idr` with cannot come from a payload it
 * does not receive. Taking it from the same file the server charges from is what
 * keeps the money copy from disagreeing with the charge — and it means no
 * component carries a currency literal. Null means the file is unreadable, and the
 * formatter then shows the amount with no symbol rather than guessing one.
 */
export const PLAN_CURRENCY: string | null = PARSED.currency

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

/**
 * The plan that first offers a planner at all.
 *
 * `planner != "manual"` is the whole rule, and it is the server's
 * (`crates/core/src/entitlements/check.rs`: `Feature::PlanGeneration =>
 * cfg.planner != PlannerMode::Manual`), which is why the free plan's refusal is a
 * 403 `plan_required` naming `pro` with no second rule anywhere (A257). The locked
 * card quotes this rather than `planForFeature('plan_generation')`: `planner` is a
 * string in the matrix, not a boolean grant, so the generic lookup could never
 * find it and would name no plan at all.
 */
export function planForPlanner(): 'Pro' | 'Power' | null {
  return firstPlan((facts) => facts.planner !== null && facts.planner !== 'manual')
}
