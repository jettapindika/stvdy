import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
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
  const { t } = useTranslation()
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

  if (quiz.isPending) return <LoadingState label={t('quiz.loading')} />

  if (quizError !== null) {
    return (
      <div className="px-4 py-4">
        <ApiErrorNotice
          error={quizError}
          noun={t('errors.nouns.quiz')}
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              {t('quiz.backToWorkspaces')}
            </Link>
          }
        />
      </div>
    )
  }

  if (quiz.data !== undefined && quiz.data.quiz.status === 'generating') {
    return <LoadingState label={t('quiz.generating')} />
  }

  if (quiz.data !== undefined && quiz.data.quiz.status === 'failed') {
    return (
      <div className="px-4 py-4">
        <EmptyState
          title={t('quiz.generateFailed.title')}
          reason={quiz.data.quiz.error ?? t('quiz.generateFailed.reason')}
        />
      </div>
    )
  }

  if (questions.length === 0) {
    return (
      <div className="px-4 py-4">
        <EmptyState title={t('quiz.empty.title')} reason={t('quiz.empty.reason')} />
      </div>
    )
  }

  const answered = phase === 'revealed'
  const total = questions.length

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
        <h1 className="min-w-0 flex-1 break-words">{quiz.data?.quiz.title ?? t('quiz.defaultTitle')}</h1>
        <span className="meta">
          {phase === 'results'
            ? t('quiz.count', { count: total })
            : t('quiz.questionProgress', { current: index + 1, total })}
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
              <legend className="label">{t('quiz.chooseOne')}</legend>
              {question.options.map((option, position) => (
                <label
                  key={option}
                  className={`flex min-h-11 min-w-0 cursor-pointer items-center gap-3 rounded-card border border-border px-3 py-2 ${
                    choice === option ? 'border-primary' : ''
                  } ${answered ? 'cursor-default' : ''}`}
                >
                  <input
                    type="radio"
                    name="quiz-option"
                    className="h-4 w-4 shrink-0"
                    value={option}
                    checked={choice === option}
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
                {t('quiz.yourAnswer')}
              </label>
              <textarea
                id="quiz-answer"
                className="field min-h-24"
                value={written}
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
              {record.isPending ? t('quiz.checking') : t('quiz.submit')}
            </button>
          )}

          {answered && (
            <div className="flex flex-col gap-3">
              {/* The verdict is a word, not only a tone: "Correct" and "Not correct"
                  are readable in monochrome and to a screen reader. */}
              {question.kind === 'mcq' && (
                <p className={graded === true ? 'note note-ready' : 'note note-danger'} role="status">
                  {graded === true
                    ? t('quiz.correct')
                    : t('quiz.notCorrectWithAnswer', { answer: question.answer })}
                </p>
              )}

              {question.kind === 'short' && (
                <>
                  <p className="note" role="status">
                    <span className="meta">{t('quiz.goodAnswer')}</span>
                    <br />
                    {question.answer}
                  </p>
                  {question.key_points !== null && question.key_points.length > 0 && (
                    <div className="flex flex-col gap-1">
                      <p className="meta">{t('quiz.didYouSay')}</p>
                      <ul className="flex list-disc flex-col gap-1 pl-5 text-sm">
                        {question.key_points.map((point) => (
                          <li key={point}>{point}</li>
                        ))}
                      </ul>
                    </div>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <button type="button" className="btn btn-primary" onClick={() => void mark(true)}>
                      {t('quiz.gotIt')}
                    </button>
                    <button
                      type="button"
                      className="btn btn-secondary"
                      onClick={() => void mark(false)}
                    >
                      {t('quiz.missedIt')}
                    </button>
                  </div>
                </>
              )}

              <p className="whitespace-pre-wrap break-words text-sm text-muted">
                {question.explanation}
              </p>

              {question.chunk_id !== null && (
                <SourcePassage
                  chunkId={question.chunk_id}
                  sources={sources.data ?? []}
                  label={t('quiz.sourceLabel')}
                />
              )}

              {question.kind === 'mcq' && (
                <button type="button" className="btn btn-secondary self-start" onClick={advance}>
                  {index + 1 >= total ? t('quiz.seeResults') : t('quiz.nextQuestion')}
                </button>
              )}
            </div>
          )}

          {attemptError !== null && (
            <ApiErrorNotice error={attemptError} noun={t('errors.nouns.answer')} />
          )}
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
  const { t } = useTranslation()
  const results = useQuizResults(quizId)
  const error = asApiError(results.error)

  if (results.isPending) return <LoadingState label={t('quiz.results.loading')} />
  if (error !== null) {
    return <ApiErrorNotice error={error} noun={t('errors.nouns.results')} />
  }

  const summary = results.data?.summary
  if (summary === undefined) return null

  return (
    <div className="flex flex-col gap-4">
      <div className="panel flex flex-col gap-2 px-3 py-3">
        <h2 className="font-medium">{t('quiz.results.score')}</h2>
        {summary.score === null ? (
          <p className="text-sm text-muted">{t('quiz.results.nothingMarked')}</p>
        ) : (
          <p className="text-2xl font-medium">
            {Math.round(summary.score * 100)}%
            <span className="meta ml-2">
              {t('quiz.results.correctOf', {
                correct: summary.correct,
                graded: summary.graded,
              })}
            </span>
          </p>
        )}
        <p className="text-sm text-muted">
          {summary.answered > summary.graded
            ? t('quiz.results.answeredOfWaiting', {
                answered: summary.answered,
                total: summary.total,
                waiting: summary.answered - summary.graded,
              })
            : t('quiz.results.answeredOf', {
                answered: summary.answered,
                total: summary.total,
              })}
        </p>
      </div>

      {summary.topics.length > 0 && (
        <div className="flex flex-col gap-2">
          <h2 className="font-medium">{t('quiz.results.topics')}</h2>
          <ul className="flex flex-col divide-y divide-border border-y border-border">
            {summary.topics.map((topic) => (
              <li
                key={`${topic.source_id ?? 'unlinked'}-${String(topic.page ?? 0)}`}
                className="flex flex-wrap items-center gap-x-3 gap-y-1 py-3"
              >
                <span className="min-w-0 flex-1 break-words">
                  {topic.source_title ?? t('quiz.results.sourceUnavailable')}
                  {topic.page !== null && (
                    <span className="meta"> {t('citation.page', { page: topic.page })}</span>
                  )}
                </span>
                <span className="meta">
                  {t('quiz.results.correctOf', { correct: topic.correct, graded: topic.graded })}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex flex-col gap-2">
        <h2 className="font-medium">{t('quiz.results.everyQuestion')}</h2>
        <ul className="flex flex-col divide-y divide-border border-y border-border">
          {(results.data?.questions ?? []).map((question) => (
            <li key={question.question_id} className="flex flex-col gap-1 py-3">
              <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <span className="min-w-0 flex-1 break-words">{question.prompt}</span>
                <span className="meta">
                  {question.correct === true
                    ? t('quiz.correct')
                    : question.correct === false
                      ? t('quiz.notCorrect')
                      : t('quiz.notMarked')}
                </span>
              </div>
              <p className="text-sm text-muted">{question.explanation}</p>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
