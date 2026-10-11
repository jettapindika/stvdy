import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * Quizzes, questions, attempts and the derived score.
 *
 * A quiz is generated asynchronously, so the quiz query polls while its status is
 * `generating` — the same shape `useDecks` and `useSourceStatus` use, because it is
 * the same problem and a second convention would be a second thing to learn.
 *
 * The score is never stored: `useQuizResults` reads what the server computed from the
 * attempts. The screen renders that summary rather than recomputing one from the
 * attempts it happens to hold, because a device with a partial history would otherwise
 * show a different score than the server reports.
 */

export type QuizStatus = 'generating' | 'ready' | 'failed'
export type QuestionKind = 'mcq' | 'short'
export type Difficulty = 'easy' | 'medium' | 'hard'
export type QuestionTypes = 'mcq' | 'short' | 'mixed'

export interface Quiz {
  id: string
  workspace_id: string
  title: string
  difficulty: string
  question_types: string
  source_ids: string[]
  status: QuizStatus
  error: string | null
  requested_count: number
  question_count: number
  created_at: string
}

/**
 * The credit price descriptor `GET /workspaces/:id/quizzes` serves alongside the
 * quizzes, so the panel quotes the plan matrix's price rather than a literal.
 */
export interface QuizCost {
  per_batch: number
  batch_size: number
  max_questions: number
}

export interface QuizzesResponse {
  quizzes: Quiz[]
  cost: QuizCost
}

export interface QuizQuestion {
  id: string
  quiz_id: string
  kind: QuestionKind
  prompt: string
  /** Four options for an mcq, null for a short answer. */
  options: string[] | null
  answer: string
  explanation: string
  /** What a grader should look for, for a short answer. */
  key_points: string[] | null
  /** The chunk this question came from, or null when the label was not sent. */
  chunk_id: string | null
  /**
   * What the student already answered, or null. This is what makes a quiz resumable:
   * an attempt is stored server-side, so a reload or a second device picks the quiz up
   * where it was left rather than at question 1.
   */
  attempt: QuizAttempt | null
}

export interface QuizAttempt {
  id: string
  user_answer: string
  /** Null means answered but not yet judged, which is not the same as wrong. */
  correct: boolean | null
  answered_at: string
}

export interface TopicRow {
  source_id: string | null
  source_title: string | null
  page: number | null
  answered: number
  graded: number
  correct: number
}

export interface QuizSummary {
  total: number
  answered: number
  graded: number
  correct: number
  /** Null when nothing is graded yet, which is not the same as zero. */
  score: number | null
  topics: TopicRow[]
}

export interface QuizResults {
  summary: QuizSummary
  questions: {
    question_id: string
    kind: QuestionKind
    prompt: string
    answer: string
    explanation: string
    chunk_id: string | null
    user_answer: string | null
    correct: boolean | null
    answered_at: string | null
  }[]
}

export const quizKeys = {
  quizzes: (workspaceId: string) => ['workspaces', workspaceId, 'quizzes'] as const,
  quiz: (quizId: string) => ['quizzes', quizId] as const,
  results: (quizId: string) => ['quizzes', quizId, 'results'] as const,
}

export function useQuizzes(workspaceId: string) {
  return useQuery({
    queryKey: quizKeys.quizzes(workspaceId),
    queryFn: async ({ signal }) =>
      api.get<QuizzesResponse>(`/workspaces/${workspaceId}/quizzes`, { signal }),
    enabled: workspaceId.length > 0,
    // A quiz in flight is the only reason to poll; once every quiz has settled the
    // interval is off, so a parked tab stops asking.
    refetchInterval: (query) =>
      (query.state.data?.quizzes ?? []).some((quiz) => quiz.status === 'generating')
        ? 1500
        : false,
  })
}

export function useQuiz(quizId: string) {
  return useQuery({
    queryKey: quizKeys.quiz(quizId),
    queryFn: async ({ signal }) =>
      api.get<{ quiz: Quiz; questions: QuizQuestion[] }>(`/quizzes/${quizId}`, { signal }),
    enabled: quizId.length > 0,
    refetchInterval: (query) =>
      query.state.data?.quiz.status === 'generating' ? 1500 : false,
  })
}

export function useQuizResults(quizId: string) {
  return useQuery({
    queryKey: quizKeys.results(quizId),
    queryFn: async ({ signal }) => api.get<QuizResults>(`/quizzes/${quizId}/results`, { signal }),
    enabled: quizId.length > 0,
  })
}

export interface CreateQuizInput {
  title: string
  sourceIds: string[]
  count: number
  difficulty: Difficulty
  types: QuestionTypes
}

export function useCreateQuiz(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: CreateQuizInput) =>
      api.post<{ quiz: Quiz }>(`/workspaces/${workspaceId}/quizzes`, {
        title: input.title,
        source_ids: input.sourceIds,
        count: input.count,
        difficulty: input.difficulty,
        types: input.types,
      }),
    onSuccess: () => client.invalidateQueries({ queryKey: quizKeys.quizzes(workspaceId) }),
  })
}

export interface AttemptInput {
  questionId: string
  userAnswer: string
  /** The device's clock; absent means the server's. */
  answeredAt?: string
  /** Only meaningful for a short answer; the server grades an mcq itself. */
  correct?: boolean
  /** Client-generated, so a retry after a lost response cannot apply twice. */
  id?: string
}

export function useRecordAttempt(quizId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: AttemptInput) =>
      api.post<{
        applied: boolean
        attempt: { id: string; question_id: string; correct: boolean | null }
      }>(`/quizzes/${quizId}/attempts`, {
        id: input.id,
        question_id: input.questionId,
        user_answer: input.userAnswer,
        correct: input.correct,
        answered_at: input.answeredAt,
      }),
    // `exact` matters: query keys match by prefix, so an inexact invalidation of
    // `['quizzes', id]` also matches `['quizzes', id, 'results']` and would refetch
    // the results the screen is building a session on top of.
    onSuccess: () =>
      client.invalidateQueries({ queryKey: quizKeys.results(quizId), exact: true }),
  })
}
