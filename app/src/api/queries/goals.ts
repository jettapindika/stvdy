import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * Goals: the anchor the planner schedules against, and the prep-progress card.
 *
 * A goal is one of the six synced entities (`0012`), but nothing here writes
 * through the outbox: a goal is created from a syllabus confirm or from the
 * planner's own screen, and both of those are online actions whose response the
 * screen needs immediately — the goal's id is what `POST /goals/:id/plan` takes.
 * The reads still go to the server, which is the only source of truth (A42).
 *
 * **`prep_progress` is four honest numbers and no composite.** The spec's honesty
 * rules forbid a "readiness" score and a grade prediction, so the route answers
 * `topic_coverage_pct`, `quiz_accuracy_pct`, `cards_due` and `weak_topics` — and
 * `quiz_accuracy_pct` is `null`, never `0`, when the goal has no attempts. The card
 * renders "no quizzes yet" for that, because `0%` is a claim the app cannot make
 * from nothing.
 */

/** A goal row, as `GET /goals` returns it. */
export interface GoalRow {
  id: string
  workspace_id: string
  title: string
  target_date: string
  source_ids: string[] | null
  created_at?: string | null
  updated_at?: string
}

/**
 * The goal's true components, from `GET /goals/:id/progress`.
 *
 * `topics` is the list the coverage is computed over, so the card can render what
 * the percentage is a percentage *of* rather than a number with nothing behind it.
 */
export interface GoalProgress {
  goal_id: string
  window_days: number
  topic_coverage_pct: number | null
  quiz_accuracy_pct: number | null
  cards_due: number
  weak_topics: string[]
  topics: { name: string; accuracy: number | null }[]
}

export const goalKeys = {
  all: ['goals'] as const,
  progress: (goalId: string) => ['goals', goalId, 'progress'] as const,
}

export function useGoals() {
  return useQuery({
    queryKey: goalKeys.all,
    queryFn: async ({ signal }): Promise<GoalRow[]> => {
      const body = await api.get<{ goals: GoalRow[] }>('/goals', { signal })
      return body.goals
    },
  })
}

/**
 * The prep-progress card's numbers.
 *
 * No `staleTime`: a quiz attempt changes them, and the card is the screen the
 * student opens to decide whether to study, so an answer that is a moment old is
 * the wrong answer.
 */
export function useGoalProgress(goalId: string) {
  return useQuery({
    queryKey: goalKeys.progress(goalId),
    queryFn: async ({ signal }) => api.get<GoalProgress>(`/goals/${goalId}/progress`, { signal }),
    enabled: goalId.length > 0,
  })
}

/** A goal to create, in the shape `POST /goals` takes. */
export interface GoalInput {
  title: string
  target_date: string
  workspace_id: string
  source_ids: string[]
}

/** Creates a goal. 201 `{goal}`. */
export async function createGoal(input: GoalInput): Promise<GoalRow> {
  const body = await api.post<{ goal: GoalRow }>('/goals', input)
  return body.goal
}

/** Invalidates the goal list after a write. */
export function useGoalWrite() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (input: GoalInput) => createGoal(input),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: goalKeys.all })
    },
  })
}
