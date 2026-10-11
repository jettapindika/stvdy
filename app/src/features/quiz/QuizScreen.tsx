import { useEffect, useRef, useState } from 'react'
import { Link, useParams } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useQuiz,
  useQuizResults,
  useRecordAttempt,
  type QuizQuestion,
} from '@/api/queries/quizzes'
import { useSources } from '@/api/queries/sources'
import { SourcePassage } from '@/components/ui/SourcePassage'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'

/**
 * Quiz mode: one question at a time, then the score and the weak topics.
 *
 * Two rules the screen exists to get right.
 *
 * **An MCQ is graded by the server; a short answer is the student's to mark.** The MCQ
 * shows no "was I right" control, because the server decides it and a student who could
 * disagree would be marking their own paper. A short answer shows the model answer and
 * the key points, then asks, because no string comparison can judge free prose.
 *
 * **The score comes from the server.** The results panel renders the summary it was
 * given rather than recomputing one from the attempts this screen happens to hold: a
 * device with a partial history would otherwise show a different score than the server
 * reports.
 *
 * Keyboard: `1`-`4` select an MCQ option and `Enter` submits, so a quiz can be taken
 * without the mouse. The option rows are the smallest controls here, so they carry the
 * 44px minimum target at phone width.
 */

type Phase = 'answering' | 'revealed' | 'results'

export function QuizScreen() {
  const { quizId = '' } = useParams()
  const quiz = useQuiz(quizId)
  const results = useQuizResults(quizId)
  const sources = useSources(quiz.data?.quiz.workspace_id ?? '')
  const record = useRecordAttempt(quizId)

  const [index, setIndex] = useState(0)
  const [phase, setPhase] = useState<Phase>('answering')
  const [choice, setChoice] = useState<string | null>(null)
  const [written, setWritten] = useState('')
  // The last grade the server reported for the question on screen, so the reveal can
  // say whether the MCQ was right without re-deriving it here.
  const [graded, setGraded] = useState<boolean | null>(null)

  const questionRef = useRef<HTMLDivElement>(null)
  const quizError = asApiError(quiz.error)

  const questions = quiz.data?.questions ?? []
  const question: QuizQuestion | undefined = questions[index]

  // Resume from what is already stored. An attempt lives server-side, so without this a
  // reload, or the second device the whole product is premised on, would restart at
  // question 1 with every answer lost while the rows sat in the database. The seed runs
  // once per quiz and only moves the pointer forward, so a session in progress is never
  // yanked backwards by a background refetch.
  const seeded = useRef<string | null>(null)
  useEffect(() => {
    if (quiz.data === undefined || seeded.current === quizId) return
    seeded.current = quizId

    const answered = questions.filter((entry) => entry.attempt !== null)
    if (answered.length === 0) return

    // The first question without an attempt is where the student left off; a quiz that
    // is fully answered opens on its results.
    const next = questions.findIndex((entry) => entry.attempt === null)
    if (next === -1) {
      setIndex(questions.length - 1)
      setPhase('results')
      return
    }
    setIndex(next)
  }, [quiz.data, quizId, questions])

  // Focus moves to the question on advance, because the whole point of a keyboard
  // shortcut is that the student never has to go looking for the next control.
  useEffect(() => {
    questionRef.current?.focus()
  }, [index, phase])

  // An MCQ is answered with a number key, gated on the question being on screen and
  // unanswered: a key that graded a question the student had not read would record a
  // judgement they could not have made.
  useEffect(() => {
    if (phase !== 'answering' || question?.kind !== 'mcq') return
    function onKey(event: KeyboardEvent) {
      const options = question?.options ?? []
      const number = Number(event.key)
      if (!Number.isInteger(number) || number < 1 || number > options.length) return
      event.preventDefault()
      setChoice(options[number - 1] ?? null)
    }
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('keydown', onKey)
    }
  }, [phase, question])

  if (quiz.isPending) return <LoadingState label="Loading this quiz" />

  if (quizError !== null) {
    return (
      <div className="px-4 py-4">
        <ApiErrorNotice
          error={quizError}
          noun="this quiz"
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              Back to workspaces
            </Link>
          }
        />
      </div>
    )
  }

  if (quiz.data !== undefined && quiz.data.quiz.status === 'generating') {
    return <LoadingState label="This quiz is still being generated" />
  }

  if (quiz.data !== undefined && quiz.data.quiz.status === 'failed') {
    return (
      <div className="px-4 py-4">
        <EmptyState
          title="This quiz could not be generated"
          reason={quiz.data.quiz.error ?? 'The generation failed. Your credits were refunded.'}
        />
      </div>
    )
  }

  if (questions.length === 0) {
    return (
      <div className="px-4 py-4">
        <EmptyState title="This quiz has no questions" reason="Generate it again from the Studio." />
      </div>
    )
  }

  const total = questions.length
  // The question on screen counts as answered when the student just submitted it *or*
  // when a stored attempt says they answered it earlier. Without the second half, a
  // resumed question would show its answer as if it were still open.
  const stored = question?.attempt ?? null
  const answered = phase === 'revealed' || stored !== null
  const verdict = phase === 'revealed' ? graded : (stored?.correct ?? null)
  const priorAnswer = phase === 'revealed' ? null : (stored?.user_answer ?? null)

  function submit() {
    if (question === undefined) return
    const answer = question.kind === 'mcq' ? (choice ?? '') : written.trim()
    if (answer.length === 0) return
    record.mutate(
      { questionId: question.id, userAnswer: answer },
      {
        onSuccess: (response) => {
          setGraded(response.attempt.correct)
          setPhase('revealed')
        },
      },
    )
  }

  function mark(correct: boolean) {
    if (question === undefined) return
    record.mutate(
      { questionId: question.id, userAnswer: written.trim(), correct },
      { onSuccess: () => void results.refetch() },
    )
    advance()
  }

  function advance() {
    setChoice(null)
    setWritten('')
    setGraded(null)
    if (index + 1 >= total) {
      setPhase('results')
      return
    }
    setIndex(index + 1)
    setPhase('answering')
  }

  const attemptError = asApiError(record.error)

  return (
    <div className="flex flex-col gap-4 px-4 py-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h1 className="min-w-0 flex-1 break-words">{quiz.data?.quiz.title ?? 'Quiz'}</h1>
        <span className="meta">
          {phase === 'results' ? `${String(total)} questions` : `Question ${String(index + 1)} of ${String(total)}`}
        </span>
      </div>

      {phase !== 'results' && question !== undefined && (
        <div ref={questionRef} tabIndex={-1} className="panel flex flex-col gap-3 px-3 py-3">
          <p className="font-medium break-words">{question.prompt}</p>

          {question.kind === 'mcq' && question.options !== null && (
            // `min-w-0` is load-bearing, not decoration. A `fieldset` carries the UA
            // default `min-inline-size: min-content`, so it refuses to shrink below its
            // widest option's minimum and overflows its parent: measured at 200% text
            // scale on a 390px viewport, the panel's content box was 324px while the
            // fieldset rendered 325px and its options reached 24px past the panel's
            // right edge. This is the same failure the `.btn` rule documents for
            // `min-width: auto`, and it is invisible at 100%.
            <fieldset className="flex min-w-0 flex-col gap-2">
              <legend className="label">Choose one</legend>
              {question.options.map((option, position) => (
                <label
                  // Keyed by position, not by text: `validate()` requires four
                  // non-empty options and an answer among them, not four *distinct*
                  // ones, so a model that repeats a distractor would produce duplicate
                  // React keys and a mis-render.
                  key={position}
                  className={`flex min-h-11 min-w-0 cursor-pointer items-center gap-3 rounded-card border border-rule px-3 py-2 ${
                    choice === option ? 'border-accent' : ''
                  } ${answered ? 'cursor-default' : ''}`}
                >
                  <input
                    type="radio"
                    name="quiz-option"
                    className="h-4 w-4 shrink-0"
                    value={option}
                    checked={choice === option || priorAnswer === option}
                    disabled={answered}
                    onChange={() => {
                      setChoice(option)
                    }}
                  />
                  <span className="min-w-0 break-words">{option}</span>
                  <span className="meta ml-auto shrink-0" aria-hidden="true">
                    {position + 1}
                  </span>
                </label>
              ))}
            </fieldset>
          )}

          {question.kind === 'short' && (
            <>
              <label className="label" htmlFor="quiz-answer">
                Your answer
              </label>
              <textarea
                id="quiz-answer"
                className="field min-h-24"
                value={phase === 'revealed' ? written : (priorAnswer ?? written)}
                readOnly={answered}
                onChange={(event) => {
                  setWritten(event.target.value)
                }}
              />
            </>
          )}

          {!answered && (
            <button
              type="button"
              className="btn btn-primary self-start"
              disabled={
                record.isPending || (question.kind === 'mcq' ? choice === null : written.trim().length === 0)
              }
              onClick={submit}
            >
              {record.isPending ? 'Checking' : 'Submit answer'}
            </button>
          )}

          {answered && (
            <div className="flex flex-col gap-3">
              {/* The verdict is a word, not only a tone: "Correct" and "Not correct"
                  are readable in monochrome and to a screen reader. */}
              {question.kind === 'mcq' && (
                <p
                  className={verdict === true ? 'note note-ready' : 'note note-danger'}
                  role="status"
                >
                  {verdict === true
                    ? 'Correct'
                    : `Not correct. The answer is: ${question.answer}`}
                </p>
              )}

              {question.kind === 'short' && (
                <>
                  <p className="note" role="status">
                    <span className="meta">A good answer</span>
                    <br />
                    {question.answer}
                  </p>
                  {question.key_points !== null && question.key_points.length > 0 && (
                    <div className="flex flex-col gap-1">
                      <p className="meta">Did you say</p>
                      <ul className="flex list-disc flex-col gap-1 pl-5 text-sm">
                        {question.key_points.map((point) => (
                          <li key={point}>{point}</li>
                        ))}
                      </ul>
                    </div>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <button type="button" className="btn btn-primary" onClick={() => void mark(true)}>
                      I got it
                    </button>
                    <button
                      type="button"
                      className="btn btn-secondary"
                      onClick={() => void mark(false)}
                    >
                      I missed it
                    </button>
                  </div>
                </>
              )}

              <p className="whitespace-pre-wrap break-words text-sm text-ink-muted">
                {question.explanation}
              </p>

              {question.chunk_id !== null && (
                <SourcePassage
                  chunkId={question.chunk_id}
                  sources={sources.data ?? []}
                  label="Where this question came from"
                />
              )}

              {question.kind === 'mcq' && (
                <button type="button" className="btn btn-secondary self-start" onClick={advance}>
                  {index + 1 >= total ? 'See results' : 'Next question'}
                </button>
              )}
            </div>
          )}

          {attemptError !== null && <ApiErrorNotice error={attemptError} noun="this answer" />}
        </div>
      )}

      {phase === 'results' && <Results quizId={quizId} />}
    </div>
  )
}

/**
 * The score and the weak topics, as the server computed them.
 *
 * The list is weakest first, which is the order the server sorts it in; the screen does
 * not re-sort it, because the ordering rule belongs in one place. A topic whose source
 * could not be resolved is shown as "Source unavailable" rather than dropped: it is the
 * visible symptom of a label that never resolved, and hiding it would hide the bug.
 */
function Results({ quizId }: { quizId: string }) {
  const results = useQuizResults(quizId)
  const error = asApiError(results.error)

  if (results.isPending) return <LoadingState label="Loading your results" />
  if (error !== null) return <ApiErrorNotice error={error} noun="your results" />

  const summary = results.data?.summary
  if (summary === undefined) return null

  return (
    <div className="flex flex-col gap-4">
      <div className="panel flex flex-col gap-2 px-3 py-3">
        <h2 className="font-medium">Your score</h2>
        {summary.score === null ? (
          <p className="text-sm text-ink-muted">
            Nothing is marked yet. Answer a question and your score appears here.
          </p>
        ) : (
          <p className="text-2xl font-medium">
            {Math.round(summary.score * 100)}%
            <span className="meta ml-2">
              {summary.correct} of {summary.graded} correct
            </span>
          </p>
        )}
        <p className="text-sm text-ink-muted">
          {summary.answered} of {summary.total} answered
          {summary.answered > summary.graded &&
            `, ${String(summary.answered - summary.graded)} waiting for you to mark`}
        </p>
      </div>

      {summary.topics.length > 0 && (
        <div className="flex flex-col gap-2">
          <h2 className="font-medium">Where to look again</h2>
          <ul className="flex flex-col divide-y divide-rule border-y border-rule">
            {summary.topics.map((topic) => (
              <li
                key={`${topic.source_id ?? 'unlinked'}-${String(topic.page ?? 0)}`}
                className="flex flex-wrap items-center gap-x-3 gap-y-1 py-3"
              >
                <span className="min-w-0 flex-1 break-words">
                  {topic.source_title ?? 'Source unavailable'}
                  {topic.page !== null && <span className="meta"> page {String(topic.page)}</span>}
                </span>
                <span className="meta">
                  {topic.correct} of {topic.graded} correct
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex flex-col gap-2">
        <h2 className="font-medium">Every question</h2>
        <ul className="flex flex-col divide-y divide-rule border-y border-rule">
          {(results.data?.questions ?? []).map((question) => (
            <li key={question.question_id} className="flex flex-col gap-1 py-3">
              <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <span className="min-w-0 flex-1 break-words">{question.prompt}</span>
                <span className="meta">
                  {question.correct === true
                    ? 'Correct'
                    : question.correct === false
                      ? 'Not correct'
                      : 'Not marked'}
                </span>
              </div>
              <p className="text-sm text-ink-muted">{question.explanation}</p>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
