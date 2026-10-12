import { useQuery } from '@tanstack/react-query'

import { api } from '../client'

/**
 * The dashboard's two reads.
 *
 * **Every number on this screen traces to a row** (A289, roadmap gate 7(b)). That
 * is the whole design constraint: `GET /dashboard` returns counts and sums over
 * rows the student owns — minutes per course from `study_sessions` joined through
 * `workspaces`, review activity from `flashcard_reviews`, and the totals — and
 * there is no composite score, no predicted grade, no "retention", and no streak
 * invented here. A figure this module cannot read is absent rather than zero, and
 * a topic with no attempts is absent rather than shown as 0%.
 *
 * **The two routes are separate on purpose.** `GET /dashboard/topics` is gated by
 * `Feature::AdvancedAnalytics` (Power) and answers `403 plan_required` on a plan
 * that does not grant it, so the screen can render a locked card for the part that
 * is locked while the rest of the dashboard keeps working. One route with a flag
 * would have made the whole screen a 403 on Free.
 *
 * `from`/`to` are the student's **local** days, computed by the caller in the
 * account's zone (A6): a session at 23:30 Jakarta is today, and a UTC window would
 * ask the server for yesterday.
 */

/** One course's study minutes in the window. */
export interface CourseMinutes {
  workspace_id: string
  title: string
  minutes: number
}

/** One local day's review count. */
export interface ReviewDay {
  local_date: string
  reviews: number
}

export interface DashboardTotals {
  minutes_studied: number
  reviews: number
  cards_due: number
  quizzes_taken: number
}

export interface DashboardResponse {
  from: string
  to: string
  time_per_course: CourseMinutes[]
  review_activity: ReviewDay[]
  totals: DashboardTotals
}

/** One source's quiz mastery. A topic with no attempts is absent, never 0%. */
export interface TopicMastery {
  source_id: string
  title: string
  attempts: number
  correct: number
  accuracy: number
}

export interface TopicsResponse {
  quiz_mastery: TopicMastery[]
  weak_topics: TopicMastery[]
}

export const dashboardKeys = {
  summary: (from: string, to: string) => ['dashboard', from, to] as const,
  topics: ['dashboard', 'topics'] as const,
}

/**
 * The dashboard's figures.
 *
 * `from` and `to` are the local-day bounds the caller computed, sent as
 * `YYYY-MM-DD` because the window is a *local* day rather than an instant (A6).
 */
export function useDashboard(from: string, to: string) {
  return useQuery({
    queryKey: dashboardKeys.summary(from, to),
    queryFn: async (): Promise<DashboardResponse> =>
      await api.get<DashboardResponse>(
        `/dashboard?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
      ),
  })
}

/**
 * The per-topic breakdown, which only a plan with `AdvancedAnalytics` may read.
 *
 * `enabled` is the caller's, and the caller decides it from `GET /usage`'s
 * `features.advanced_analytics`: rendering the query and handling its 403 would
 * put a failed request on the screen for a feature the student's plan simply does
 * not include, which reads as a broken screen rather than as a locked one.
 */
export function useTopicMastery(enabled: boolean) {
  return useQuery({
    queryKey: dashboardKeys.topics,
    queryFn: async (): Promise<TopicsResponse> =>
      await api.get<TopicsResponse>('/dashboard/topics'),
    enabled,
  })
}
