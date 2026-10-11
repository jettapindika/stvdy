import { useState } from 'react'
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

const STATUS_LABEL: Record<QuizStatus, string> = {
  generating: 'Generating',
  ready: 'Ready',
  failed: 'Failed',
}

const STATUS_CHIP: Record<QuizStatus, string> = {
  generating: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

const DIFFICULTIES: Difficulty[] = ['easy', 'medium', 'hard']
const TYPE_SETTINGS: QuestionTypes[] = ['mcq', 'short', 'mixed']

export function QuizPanel({ workspaceId }: { workspaceId: string }) {
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

  return (
    <div className="flex flex-col gap-4 px-4 py-4">
      <form
        className="panel flex flex-col gap-3 px-3 py-3"
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
          label="Quiz name"
          name="quiz-title"
          value={title}
          hint="What this quiz is for."
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
          legend="Generate from"
          emptyHint="No indexed sources yet. Add one in the Sources panel, then come back."
        />
        <div className="flex flex-wrap gap-3">
          <div className="flex min-w-40 flex-1 flex-col gap-1">
            <label className="label" htmlFor="quiz-difficulty">
              Difficulty
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
                  {value}
                </option>
              ))}
            </select>
          </div>
          <div className="flex min-w-40 flex-1 flex-col gap-1">
            <label className="label" htmlFor="quiz-types">
              Questions
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
                  {value === 'mcq' ? 'multiple choice' : value === 'short' ? 'short answer' : 'mixed'}
                </option>
              ))}
            </select>
          </div>
        </div>
        <label className="label" htmlFor="quiz-count">
          How many questions
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
          <p className="text-xs text-ink-muted">
            Costs {cost.per_batch} credits per {cost.batch_size} questions. The quiz appears here
            while it is generated, usually within a minute.
          </p>
        )}
        <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
          {create.isPending ? 'Starting' : 'Generate quiz'}
        </button>
        {error !== null && <ApiErrorNotice error={error} noun="quizzes" />}
      </form>

      {quizzes.isPending && <SkeletonRows rows={2} label="Loading this workspace's quizzes" />}

      {quizzesList !== undefined && quizzesList.length === 0 && (
        <EmptyState
          title="No quizzes yet"
          reason="Pick the sources above and generate one. Every question is linked to the passage it came from."
        />
      )}

      {quizzesList !== undefined && quizzesList.length > 0 && (
        <ul className="flex flex-col divide-y divide-rule border-y border-rule">
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
  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 flex-1 break-words font-medium">
          {quiz.title.length > 0 ? quiz.title : 'Untitled quiz'}
        </span>
        <span className="meta">
          {quiz.question_count} {quiz.question_count === 1 ? 'question' : 'questions'}
        </span>
        <span className={STATUS_CHIP[quiz.status]} role="status">
          {quiz.status === 'generating' && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {STATUS_LABEL[quiz.status]}
        </span>
        {quiz.status === 'ready' && (
          <Link to={`/quiz/${quiz.id}`} className="btn btn-secondary">
            Take quiz
          </Link>
        )}
      </div>

      {quiz.error !== null && quiz.error.length > 0 && (
        <p className="note note-danger text-sm">{quiz.error}</p>
      )}
    </li>
  )
}
