import { useQuery } from '@tanstack/react-query'

import { api } from '../client'

/**
 * The admin console's three reads.
 *
 * **The console shows what the server computes and nothing else.** Every figure
 * here is a sum of rows or a count of them: `GET /admin/users` counts
 * `usage_events` and live `sources` in a window, `GET /admin/payments` returns the
 * payment rows themselves, and `GET /admin/margin` returns the month report the
 * tripwires are read off. There is no engagement score, no health metric, no
 * churn prediction and no blended margin — a number this module cannot read is
 * absent rather than zero, because `0` and "not measured" are different facts and
 * an operator reads these figures to decide things.
 *
 * **The window travels with the data.** Each response carries its own `from`/`to`,
 * and the screen prints them. The design's own admin frame does the same thing
 * ("ALL TABLES AND METRICS USE THIS SAME SAMPLE") for the same reason: a table
 * whose period is implied by a heading is a table a reader will attribute to the
 * wrong month.
 *
 * **Every route answers 404 to a non-admin** (`AdminUser`'s extractor, A7), not
 * 403: whether an admin endpoint exists is not something a non-admin needs to
 * learn. The screen therefore renders "not found" for a signed-in student rather
 * than a permission error, which is also what makes it safe to register the route
 * for everyone.
 */

export interface AdminUserRow {
  id: string
  email: string
  plan_id: string
  created_at: string | null
  /** `usage_events` rows in the window. */
  requests: number
  /** Live sources created in the window. */
  sources_added: number
  /** The newest action ever, not windowed. `null` means this account has never acted. */
  last_active_at: string | null
  is_admin: boolean
}

export interface AdminUserList {
  from: string
  to: string
  users: AdminUserRow[]
}

export interface AdminPayment {
  id: string
  user_id: string
  kind: string
  plan_id: string | null
  billing_period: string | null
  credits: number | null
  provider: string
  order_id: string
  amount_idr: number
  gateway_fee_idr: number | null
  status: string
  review_reason: string | null
  expires_at: string
  paid_at: string | null
  created_at: string | null
}

/** One plan's line inside a month, as `GET /admin/margin` builds it. */
export interface PlanMargin {
  plan_id: string
  /** Our provider cost for this plan's spend, excluding BYOK. */
  provider_cost_usd: number
  /** Calls on a student's own key: its own column, inside neither tier's figure. */
  byok_cost_usd: number
  revenue_idr: number
  /** `null` when either side of the subtraction is unknown — never a measured profit. */
  gross_margin_idr: number | null
  ceiling_usd: number | null
}

export interface MonthMargin {
  month: string
  per_plan: PlanMargin[]
  revenue_idr: number
  gateway_fee_idr: number
  /** `null` when nothing was granted in the month: `0/0` is unanswerable, not zero. */
  free_utilization: number | null
  paying: number
  free_accounts: number
  conversion_of_free: number | null
  conversion_of_total: number | null
  as_of: string
}

/**
 * The four thresholds the roadmap watches, as booleans.
 *
 * Each is `boolean | null`, and the distinction is the whole reason they are not
 * plain booleans: `null` means the range cannot answer the question, which is a
 * different fact from `false` ("it is not happening"). A tripwire rendered as
 * "not triggered" when it had no data would report safety it never measured.
 */
export interface Tripwires {
  free_utilization_over_30: boolean | null
  conversion_under_3: boolean | null
  free_spend_over_quarter_of_revenue: boolean | null
  plan_loss_two_months_running: boolean | null
}

export interface MarginReport {
  from: string
  to: string
  /** The rate every IDR figure below was computed with, printed as the input it is. */
  idr_per_usd: number
  months: MonthMargin[]
  tripwires: Tripwires
}

export const adminKeys = {
  users: (from: string, to: string) => ['admin', 'users', from, to] as const,
  payments: ['admin', 'payments'] as const,
  margin: (from: string, to: string) => ['admin', 'margin', from, to] as const,
}

export function useAdminUsers(window: { from: string; to: string }) {
  return useQuery({
    queryKey: adminKeys.users(window.from, window.to),
    queryFn: async ({ signal }) =>
      api.get<AdminUserList>(`/admin/users?from=${window.from}&to=${window.to}`, { signal }),
    // A non-admin's 404 is the answer, not a failure to retry.
    retry: false,
  })
}

export function useAdminPayments() {
  return useQuery({
    queryKey: adminKeys.payments,
    queryFn: async ({ signal }) => api.get<{ payments: AdminPayment[] }>('/admin/payments', { signal }),
    retry: false,
  })
}

export function useAdminMargin(window: { from: string; to: string }) {
  return useQuery({
    queryKey: adminKeys.margin(window.from, window.to),
    queryFn: async ({ signal }) =>
      api.get<MarginReport>(`/admin/margin?from=${window.from}&to=${window.to}`, { signal }),
    retry: false,
  })
}
