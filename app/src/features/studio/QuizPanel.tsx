import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCreateQuiz,
  useDeleteQuiz,
  useQuizzes,
  useRenameQuiz,
  type Difficulty,
  type QuestionTypes,
  type Quiz,
  type QuizStatus,
} from '@/api/queries/quizzes'
import { useSources } from '@/api/queries/sources'
import { useBulkDelete, useReorderContent } from '@/api/queries/workspaces'
import { Field } from '@/components/ui/Field'
import { Select } from '@/components/ui/Select'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { ContentList, type RowBindings } from '@/features/studio/ContentList'
import { ContentRow } from '@/features/studio/ContentRow'
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
  const bulk = useBulkDelete(workspaceId)
  const bulkError = asApiError(bulk.error)
  const reorder = useReorderContent(workspaceId)
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
          <div className="min-w-[min(10rem,100%)] flex-1">
            <Select
              label={t('studio.quiz.difficultyLabel')}
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
            </Select>
          </div>
          <div className="min-w-[min(10rem,100%)] flex-1">
            <Select
              label={t('studio.quiz.typesLabel')}
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
            </Select>
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
        <ContentList
          items={quizzesList}
          noun={t('errors.nouns.quizzes')}
          onBulkDelete={(ids) => {
            bulk.mutate({ quizIds: ids })
          }}
          bulkPending={bulk.isPending}
          bulkError={bulkError}
          onReorder={async (ids) => {
            await reorder.mutateAsync({ quizIds: ids })
          }}
          row={(quiz, _index, bindings) => (
            <QuizRow key={quiz.id} quiz={quiz} workspaceId={workspaceId} bindings={bindings} />
          )}
        />
      )}
    </div>
  )
}

/**
 * One generated quiz in the list.
 *
 * The chip carries its state in a word, not only in a tone, so "Generating" and
 * "Failed" are still readable in monochrome and to a screen reader. The row's own
 * layout and its rename/delete modes live in `ContentRow`.
 */
function QuizRow({
  quiz,
  workspaceId,
  bindings,
}: {
  quiz: Quiz
  workspaceId: string
  bindings: RowBindings
}) {
  const { t } = useTranslation()
  const remove = useDeleteQuiz(workspaceId)
  const rename = useRenameQuiz(workspaceId)
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const statusLabel: Record<QuizStatus, string> = {
    generating: t('studio.status.generating'),
    ready: t('studio.status.ready'),
    failed: t('studio.status.failed'),
  }
  const displayName = quiz.title.length > 0 ? quiz.title : t('studio.quiz.untitled')
  return (
    <ContentRow
      title={displayName}
      meta={<span className="meta">{t('studio.quiz.questions', { count: quiz.question_count })}</span>}
      statusChip={STATUS_CHIP[quiz.status]}
      statusLabel={statusLabel[quiz.status]}
      working={quiz.status === 'generating'}
      action={
        quiz.status === 'ready' ? (
          <Link to={`/quiz/${quiz.id}`} className="btn btn-secondary">
            {t('studio.quiz.take')}
          </Link>
        ) : undefined
      }
      error={quiz.error}
      rename={{
        label: t('studio.quiz.rename'),
        onSubmit: (title) => {
          rename.mutate({ quizId: quiz.id, title })
        },
        pending: rename.isPending,
        error: asApiError(rename.error),
        reset: rename.reset,
      }}
      deleteConfirm={t('studio.quiz.deleteConfirm', { name: displayName })}
      deleteBody={
        quiz.question_count === 0
          ? t('studio.quiz.deleteNoQuestions')
          : t('studio.quiz.deleteQuestions', { count: quiz.question_count })
      }
      deleteIt={t('studio.quiz.deleteIt')}
      deleting={t('studio.quiz.deleting')}
      deleteNoun={t('errors.nouns.quiz')}
      onDelete={(onSuccess) => {
        remove.mutate(quiz.id, { onSuccess })
      }}
      deletePending={remove.isPending}
      deleteError={asApiError(remove.error)}
      resetDelete={remove.reset}
      bindings={bindings}
    />
  )
}
