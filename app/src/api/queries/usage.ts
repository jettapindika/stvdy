import { useQuery } from '@tanstack/react-query'

import { api } from '../client'

/**
 * `GET /usage` — the one source of every number the plan and usage screen shows.
 *
 * The route is the contract and this module is a transcription of it, not a
 * design: `crates/server/src/usage.rs` builds the body from the same snapshot the
 * entitlement decisions use, so a number this view reports and a number a route
 * charges cannot drift. Two consequences are load-bearing:
 *
 * - **Nothing here is defaulted.** A field the server did not send stays
 *   `undefined` and the screen shows no number for it, because a `0` standing in
 *   for "unknown" is an invented number (roadmap gate 7a). That is also why
 *   `used` is nullable per axis rather than `number`: the server sends `null` for
 *   an axis with no running counter, which is a different fact from `0` used.
 * - **The costs are the server's.** `costs` is built by `credits::costs`, the one
 *   function that builds the table, so the price a panel shows before an action
 *   is the price the route charges for it.
 *
 * The response carries no prices in currency and no checkout: `provider_cost`
 * says only whether the ceiling is reached and when it resets. The dollars are in
 * `GET /admin/margin`, which the app never calls.
 */

/** The subscription status as the wire spells it. */
export type SubStatus = 'none' | 'active' | 'grace' | 'expired' | 'canceled'

/**
 * One axis of the plan matrix: its ceiling, and the count against it.
 *
 * `limit` is `null` for an axis the plan does not cap ("Unlimited"), and `used`
 * is `null` for an axis with no running counter — a file's size ceiling, the
 * monthly credit allowance, the Google-account ceiling. Neither `null` means
 * zero.
 */
export interface UsageLimit {
  limit: number | null
  used: number | null
}

export interface UsageLimits {
  workspaces: UsageLimit
  sources_total: UsageLimit
  max_file_mb: UsageLimit
  storage_mb: UsageLimit
  devices_max: UsageLimit
  credits_per_month: UsageLimit
  chat_msgs_per_day: UsageLimit
  web_searches_per_day: UsageLimit
  flashcards_per_month: UsageLimit
  quizzes_per_month: UsageLimit
  reports_per_month: UsageLimit
  syllabus_imports_total: UsageLimit
  google_accounts_max: UsageLimit
}

/** The plan's switches, as the server's matrix declares them. */
export interface UsageFeatures {
  /** The formats the account may export, e.g. `["md", "pdf", "docx"]`. */
  export_formats: string[]
  task_ai_breakdown: boolean
  quick_capture: boolean
  google_calendar: boolean
  /** The planner mode, e.g. `"manual"`, `"auto"`. */
  planner: string
  agent_actions: boolean
  reminder_channels: string[]
  /** e.g. `"cheapest"`, `"best"`. */
  model_tier: string
  byok: boolean
  advanced_analytics: boolean
}

/**
 * What each action costs, from `credits::costs`.
 *
 * The last two are not an action: they are the long-document surcharge, which the
 * server applies above `long_doc_threshold_tokens` at
 * `long_doc_credits_per_1k_tokens` per further thousand tokens. They are quoted
 * from the same block as the per-action prices so the screen has one cost table
 * rather than a second one that can disagree with it.
 */
export interface UsageCosts {
  chat_message: number
  web_search: number
  flashcard_batch_10: number
  quiz_10: number
  report_outline: number
  report_section: number
  task_breakdown: number
  syllabus_import: number
  plan_generation: number
  agent_turn: number
  long_doc_threshold_tokens: number
  long_doc_credits_per_1k_tokens: number
}

/** The body of `GET /usage`, field for field. */
export interface Usage {
  /** The plan id the config uses, e.g. `"free"`. */
  plan: string
  /** The plan's display name, e.g. `"Free"`. */
  name: string
  subscription: {
    status: SubStatus
    /** When the prepaid period ends, or `null` when no period is running. */
    period_end: string | null
  }
  /** The two buckets and their sum, as the ledger holds them. */
  credits: {
    monthly: number
    purchased: number
    total: number
  }
  /**
   * Bytes held by live sources.
   *
   * Bytes rather than a rounded megabyte count, because the storage axis has no
   * running counter to report and a rounded number would be one the server
   * invented.
   */
  storage_bytes: number
  limits: UsageLimits
  features: UsageFeatures
  costs: UsageCosts
  provider_cost: {
    /** True when this account has reached its plan's provider-cost ceiling. */
    paused: boolean
    /** When the pause lifts: the start of the user's next local month. */
    resets_at: string
  }
}

export const usageKey = ['usage'] as const

/**
 * The account's plan, limits, credits and costs.
 *
 * No `staleTime`: the balance changes whenever anything generative runs, and the
 * meter is the one place it is shown, so an answer that is a moment old is the
 * wrong answer. React Query's defaults refetch on mount and on window focus,
 * which is the moment a student comes back to the tab after spending credits
 * elsewhere.
 */
export function useUsage() {
  return useQuery({
    queryKey: usageKey,
    queryFn: async ({ signal }) => api.get<Usage>('/usage', { signal }),
  })
}
