import type { Usage } from '@/api/queries/usage'

import { jsonResponse, mockFetch } from './harness'

/**
 * The plan screens' fixtures, in one place.
 *
 * Two screens read the same `GET /usage` body — `PlanUsageScreen` on web and
 * desktop, `MobilePlanScreen` on iOS and Android — and both need a payload of the
 * full wire shape, because every limit axis is read unconditionally. A second
 * hand-written copy of that shape is a fixture that drifts from the first, and
 * the drift would show up as a screen that renders nothing rather than as a
 * failing test.
 *
 * The fixture is a **free** account at the ceilings, because that is the state
 * the screens exist for: an account that has been refused something. It is shaped
 * exactly as `crates/server/src/usage.rs` builds the body, including the two kinds
 * of `null` the payload distinguishes — an axis the plan does not cap, and an axis
 * with no counter to report — because collapsing either into `0` is the defect
 * the honesty assertion in `PlanUsageScreen.test.tsx` is written to catch.
 */

/** The session the screens read their timezone from. */
export const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-01T08:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-01T08:00:00Z',
}

export const PERIOD_END = '2026-11-01T07:00:00Z'
export const RESETS_AT = '2026-11-01T07:00:00Z'

export const USAGE: Usage = {
  plan: 'free',
  name: 'Free',
  subscription: { status: 'active', period_end: PERIOD_END },
  credits: { monthly: 60, purchased: 12, total: 72 },
  storage_bytes: 1048576,
  limits: {
    workspaces: { limit: 2, used: 2 },
    sources_total: { limit: 20, used: 3 },
    max_file_mb: { limit: 20, used: null },
    storage_mb: { limit: 200, used: null },
    devices_max: { limit: 2, used: 1 },
    credits_per_month: { limit: 60, used: null },
    chat_msgs_per_day: { limit: 20, used: 4 },
    web_searches_per_day: { limit: 3, used: 3 },
    flashcards_per_month: { limit: 30, used: 12 },
    quizzes_per_month: { limit: 2, used: 0 },
    reports_per_month: { limit: 1, used: 1 },
    syllabus_imports_total: { limit: 1, used: 0 },
    google_accounts_max: { limit: 0, used: null },
  },
  features: {
    export_formats: ['md'],
    task_ai_breakdown: false,
    quick_capture: false,
    google_calendar: false,
    planner: 'manual',
    agent_actions: false,
    reminder_channels: ['web', 'push'],
    model_tier: 'cheapest',
    byok: false,
    advanced_analytics: false,
  },
  costs: {
    chat_message: 1,
    web_search: 2,
    flashcard_batch_10: 4,
    quiz_10: 6,
    report_outline: 3,
    report_section: 6,
    task_breakdown: 2,
    syllabus_import: 8,
    plan_generation: 2,
    agent_turn: 2,
    long_doc_threshold_tokens: 20000,
    long_doc_credits_per_1k_tokens: 1,
  },
  provider_cost: { paused: false, resets_at: RESETS_AT },
}

/** Serves the two routes a plan screen reads, and nothing else. */
export function serveUsage(usage: Usage = USAGE) {
  return mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/usage') return jsonResponse(200, usage)
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
}
