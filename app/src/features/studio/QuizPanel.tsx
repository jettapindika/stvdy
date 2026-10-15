import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCreateQuiz,
  useQuizzes,
  type Difficulty,
  type QuestionTypes,
  type Quiz,
  type QuizStatus,
} from '@/api/queries/quizzes'
import { useSources } from '@/api/queries/sources'
import { Field } from '@/components/ui/Field'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { SourcePicker } from '@/features/workspaces/SourcePicker'

/**
 * Studio → Quiz.
 *
 * Generating is asynchronous and costs credits, so the panel says both things plainly:
 * the button names the cost before it is pressed, and a quiz in flight shows its status
 * rather than a spinner that never resolves. Difficulty and question types are selects
 * rather than free text because the prompt documents the values it accepts, and a
 * typo would be a wasted generation.
 */

const STATUS_CHIP: Record<QuizStatus, string> = {
  generating: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

const DIFFICULTIES: Difficulty[] = ['easy', 'medium', 'hard']
const TYPE_SETTINGS: QuestionTypes[] = ['mcq', 'short', 'mixed']

export function QuizPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const sources = useSources(workspaceId)
  const quizzes = useQuizzes(workspaceId)
  const create = useCreateQuiz(workspaceId)
  const [selected, setSelected] = useState<string[]>([])
  const [title, setTitle] = useState('')
  const [count, setCount] = useState(10)
  const [difficulty, setDifficulty] = useState<Difficulty>('medium')
  const [types, setTypes] = useState<QuestionTypes>('mixed')

  const readySources = (sources.data ?? []).filter((source) => source.status === 'ready')
  const error = asApiError(create.error)
  const ready = title.trim().length > 0 && selected.length > 0 && !create.isPending
  const quizzesList = quizzes.data?.quizzes
  // The cost and the request cap are the server's: the panel renders what
  // `GET /workspaces/:id/quizzes` serves rather than a literal that can drift from
  // the plan matrix.
  const cost = quizzes.data?.cost

  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const difficultyLabel: Record<Difficulty, string> = {
    easy: t('studio.quiz.difficulty.easy'),
    medium: t('studio.quiz.difficulty.medium'),
    hard: t('studio.quiz.difficulty.hard'),
  }

  return (
    <div className="flex flex-col gap-5 px-5 py-5 md:px-4 md:py-4">
      <form
        className="panel flex flex-col gap-3 px-4 py-4"
        onSubmit={(event) => {
          event.preventDefault()
          if (!ready) return
          create.mutate(
            { title: title.trim(), sourceIds: selected, count, difficulty, types },
            {
              onSuccess: () => {
                setTitle('')
                setSelected([])
              },
            },
          )
        }}
      >
        <Field
          label={t('studio.quiz.nameLabel')}
          name="quiz-title"
          value={title}
          hint={t('studio.quiz.nameHint')}
          onChange={(event) => {
            setTitle(event.target.value)
          }}
        />
        <SourcePicker
          workspaceId={workspaceId}
          sources={readySources}
          selected={selected}
          onChange={setSelected}
          disabled={create.isPending}
          legend={t('studio.generateFrom')}
          emptyHint={t('studio.noIndexedSources')}
        />
        <div className="flex flex-wrap gap-3">
          {/* `min-w-40` was `min-width: 10rem` — 160px at a 16px root and **320px at
              a 32px root**, so at 200% text the two children's minimums (320 + 320)
              plus the gap exceeded this panel's 276px content box and the row could
              not wrap out of it: measured in Chromium at 390px with 200% text, the
              row's `scrollWidth` was 320 against a `clientWidth` of 276. The cap is
              what makes the minimum safe — `min(10rem, 100%)` is 160px while there
              is room for two and the container's own width when there is not, so at
              100% the pair still sits side by side exactly as before. */}
          <div className="flex min-w-[min(10rem,100%)] flex-1 flex-col gap-1">
            <label className="label" htmlFor="quiz-difficulty">
              {t('studio.quiz.difficultyLabel')}
            </label>
            <select
              id="quiz-difficulty"
              className="field"
              value={difficulty}
              onChange={(event) => {
                setDifficulty(event.target.value as Difficulty)
              }}
            >
              {DIFFICULTIES.map((value) => (
                <option key={value} value={value}>
                  {difficultyLabel[value]}
                </option>
              ))}
            </select>
          </div>
          <div className="flex min-w-[min(10rem,100%)] flex-1 flex-col gap-1">
            <label className="label" htmlFor="quiz-types">
              {t('studio.quiz.typesLabel')}
            </label>
            <select
              id="quiz-types"
              className="field"
              value={types}
              onChange={(event) => {
                setTypes(event.target.value as QuestionTypes)
              }}
            >
              {TYPE_SETTINGS.map((value) => (
                <option key={value} value={value}>
                  {value === 'mcq'
                    ? t('studio.quiz.types.mcq')
                    : value === 'short'
                      ? t('studio.quiz.types.short')
                      : t('studio.quiz.types.mixed')}
                </option>
              ))}
            </select>
          </div>
        </div>
        <label className="label" htmlFor="quiz-count">
          {t('studio.quiz.countLabel')}
        </label>
        <input
          id="quiz-count"
          className="field"
          type="number"
          min={1}
          max={cost?.max_questions}
          value={count}
          onChange={(event) => {
            const next = event.target.valueAsNumber
            setCount(Number.isNaN(next) ? 0 : next)
          }}
        />
        {cost !== undefined && (
          <p className="text-xs text-muted">
            {t('studio.quiz.cost', { credits: cost.per_batch, count: cost.batch_size })}
          </p>
        )}
        <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
          {create.isPending ? t('studio.starting') : t('studio.quiz.generate')}
        </button>
        {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.quizzes')} />}
      </form>

      {quizzes.isPending && <SkeletonRows rows={2} label={t('studio.quiz.loading')} />}

      {quizzesList !== undefined && quizzesList.length === 0 && (
        <EmptyState title={t('studio.quiz.empty.title')} reason={t('studio.quiz.empty.reason')} />
      )}

      {quizzesList !== undefined && quizzesList.length > 0 && (
        <ul className="flex flex-col divide-y divide-border border-y border-border">
          {quizzesList.map((quiz) => (
            <QuizRow key={quiz.id} quiz={quiz} />
          ))}
        </ul>
      )}
    </div>
  )
}

/**
 * One generated quiz in the list.
 *
 * The chip carries its state in a word, not only in a tone, so "Generating" and
 * "Failed" are still readable in monochrome and to a screen reader.
 */
function QuizRow({ quiz }: { quiz: Quiz }) {
  const { t } = useTranslation()
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const statusLabel: Record<QuizStatus, string> = {
    generating: t('studio.status.generating'),
    ready: t('studio.status.ready'),
    failed: t('studio.status.failed'),
  }
  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="w-full min-w-0 wrap-anywhere font-medium @md:w-auto @md:flex-1">
          {quiz.title.length > 0 ? quiz.title : t('studio.quiz.untitled')}
        </span>
        <span className="meta">{t('studio.quiz.questions', { count: quiz.question_count })}</span>
        <span className={STATUS_CHIP[quiz.status]} role="status">
          {quiz.status === 'generating' && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {statusLabel[quiz.status]}
        </span>
        {quiz.status === 'ready' && (
          <Link to={`/quiz/${quiz.id}`} className="btn btn-secondary">
            {t('studio.quiz.take')}
          </Link>
        )}
      </div>

      {quiz.error !== null && quiz.error.length > 0 && (
        <p className="note note-danger text-sm">{quiz.error}</p>
      )}
    </li>
  )
}
