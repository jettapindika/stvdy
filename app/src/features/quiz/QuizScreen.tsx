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
import { useWorkspace } from '@/api/queries/workspaces'
import { PageHeader, Section, WidgetHeading } from '@/components/layout/Section'
import { Icon, type IconName } from '@/components/ui/Icon'
import { SourcePassage } from '@/components/ui/SourcePassage'
import { ApiErrorNotice, EmptyState, LoadingState, SkeletonRows } from '@/components/ui/States'

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
 * The layout is the design's two columns: the question card, and a session rail that
 * carries the progress, the question count and the sources the quiz was built from.
 * **The rail stacks under the question on a phone rather than being hidden**, because
 * what it holds is not decoration — it is the only place the screen names where the
 * questions came from.
 *
 * The question navigator is a `<ol>` of text, not a row of buttons. Its cells are 40px,
 * under the 44px phone target floor, and they are not controls in any case: this screen
 * has no jump-to-question behaviour, and adding one would change what an attempt means
 * (a question is answered once, in order). So the navigator reports position and
 * answered-ness, and `aria-current="step"` is what says which cell is the question on
 * screen.
 *
 * Keyboard: `1`-`4` select an MCQ option and `Enter` submits, so a quiz can be taken
 * without the mouse. The option rows are the smallest controls here, so they carry the
 * 44px minimum target at phone width.
 */

type Phase = 'answering' | 'revealed' | 'results'

/**
 * A row's state. The tone and the word are picked from the same value, so a row can
 * never be tinted as one thing and labelled as another.
 */
type RowState = 'idle' | 'selected' | 'correct' | 'chosen-correct' | 'wrong'

const ROW_TONE: Record<RowState, string> = {
  idle: 'border-border bg-surface',
  selected: 'border-primary bg-primary-tint text-primary-text',
  correct: 'border-success bg-success-tint text-success',
  'chosen-correct': 'border-success bg-success-tint text-success',
  wrong: 'border-danger bg-danger-tint text-danger',
}

/**
 * What one option's row is, given the question and the answer on screen.
 *
 * In one place because the reveal has four rows that look alike and mean different
 * things: the answer, the student's answer, both at once, and neither. `answered` is
 * what separates "picked, not yet checked" from a verdict, so a checked question can
 * never render as merely selected.
 */
function rowStateFor(
  option: string,
  answer: string,
  chosen: string | null,
  answered: boolean,
): RowState {
  if (!answered) return option === chosen ? 'selected' : 'idle'
  if (option === answer) return option === chosen ? 'chosen-correct' : 'correct'
  return option === chosen ? 'wrong' : 'idle'
}

/**
 * One answer row.
 *
 * Presentational: the caller decides the state and the word, because only the caller
 * knows whether the question has been checked. The row is the target, not the 20px
 * radio inside it, so it carries `min-h-11` — and `min-w-0` with `break-words` on the
 * text, because a long option at 200% text has to wrap inside the row rather than push
 * the page sideways.
 */
function OptionRow({
  option,
  position,
  state,
  status,
  checked,
  disabled,
  onSelect,
}: {
  option: string
  position: number
  state: RowState
  status: { icon: IconName; word: string } | null
  checked: boolean
  disabled: boolean
  onSelect: () => void
}) {
  return (
    <label
      className={`flex min-h-11 min-w-0 flex-wrap items-center gap-x-3 gap-y-1 rounded-field border px-4 py-2.5 has-[:focus-visible]:border-primary ${
        ROW_TONE[state]
      } ${disabled ? 'cursor-default' : 'cursor-pointer'}`}
    >
      <input
        type="radio"
        name="quiz-option"
        className="h-5 w-5 shrink-0 accent-primary"
        value={option}
        checked={checked}
        disabled={disabled}
        onChange={onSelect}
      />
      <span className="min-w-0 flex-1 basis-40 break-words text-label">{option}</span>
      <span className="ml-auto flex shrink-0 flex-wrap items-center gap-x-3">
        {/* The number key is a shortcut, so it is a hint rather than content: the
            option's own label is what a screen reader should read. */}
        <span className="meta" aria-hidden="true">
          {position + 1}
        </span>
        {status !== null && (
          <span className="flex items-center gap-1.5 text-label font-medium">
            <Icon name={status.icon} size={18} />
            {status.word}
          </span>
        )}
      </span>
    </label>
  )
}

export function QuizScreen() {
  const { t } = useTranslation()
  const { quizId = '' } = useParams()
  const quiz = useQuiz(quizId)
  const results = useQuizResults(quizId)
  const workspaceId = quiz.data?.quiz.workspace_id ?? ''
  const sources = useSources(workspaceId)
  const workspace = useWorkspace(workspaceId)
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
  const sourcesError = asApiError(sources.error)

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

  // This screen owns its width (it is the design's two columns, not the reading column),
  // so it states the width itself rather than relying on the shell's. The states below
  // have no second column and are prose, so they take the reading column instead.
  // `w-full` is load-bearing on both: a root whose width is its fit-content cannot go
  // below its content's min-content, so one long word would push the page sideways
  // instead of wrapping inside it.
  const pageClass = 'mx-auto flex w-full max-w-6xl flex-col gap-6 px-5 md:px-[32px] py-8'
  const stateClass = 'mx-auto flex w-full max-w-3xl flex-col gap-6 px-5 md:px-[32px] py-8'

  if (quiz.isPending) {
    return (
      <div className={stateClass}>
        <SkeletonRows rows={4} label={t('quiz.loading')} />
      </div>
    )
  }

  if (quizError !== null) {
    return (
      <div className={stateClass}>
        <ApiErrorNotice
          error={quizError}
          noun={t('errors.nouns.quiz')}
          action={
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                className="btn btn-secondary"
                onClick={() => void quiz.refetch()}
              >
                {t('actions.retry')}
              </button>
              <Link to="/workspaces" className="btn btn-quiet">
                {t('quiz.backToWorkspaces')}
              </Link>
            </div>
          }
        />
      </div>
    )
  }

  if (quiz.data !== undefined && quiz.data.quiz.status === 'generating') {
    return (
      <div className={stateClass}>
        <LoadingState label={t('quiz.generating')} />
      </div>
    )
  }

  if (quiz.data !== undefined && quiz.data.quiz.status === 'failed') {
    return (
      <div className={stateClass}>
        <EmptyState
          title={t('quiz.generateFailed.title')}
          reason={quiz.data.quiz.error ?? t('quiz.generateFailed.reason')}
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              {t('quiz.backToWorkspaces')}
            </Link>
          }
        />
      </div>
    )
  }

  if (questions.length === 0) {
    return (
      <div className={stateClass}>
        <EmptyState
          title={t('quiz.empty.title')}
          reason={t('quiz.empty.reason')}
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              {t('quiz.backToWorkspaces')}
            </Link>
          }
        />
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
  // What the student has picked for this question, whether they picked it a moment ago
  // or on another device. The choice made now wins, because a submitted answer is still
  // in `choice` while the reveal is on screen.
  const chosenOption = choice ?? priorAnswer
  const answeredCount = questions.filter(
    (entry, position) => entry.attempt !== null || (position === index && answered),
  ).length
  const answeredPercent = Math.round((answeredCount / total) * 100)

  const sourceList = sources.data
  // `null` until the source list arrives, so a title that has not been resolved yet is
  // not printed as "Source unavailable" — that line means the label never resolved, and
  // saying it during a load would be a claim about the data that is not true yet.
  const quizSources =
    sourceList === undefined
      ? null
      : (quiz.data?.quiz.source_ids ?? []).map((id) => ({
          id,
          title: sourceList.find((source) => source.id === id)?.title ?? null,
        }))
  const sourceTitles = (quizSources ?? []).flatMap((entry) =>
    entry.title === null ? [] : [entry.title],
  )

  const course = workspace.data
  // The course code and name, exactly as `StudyScreen` composes them: a code the student
  // has not given is absent rather than invented.
  const courseLabel =
    course === undefined
      ? null
      : course.course_code === null
        ? course.name
        : `${course.course_code} / ${course.name}`
  const quizTitle = quiz.data?.quiz.title ?? t('quiz.defaultTitle')
  // In the results the heading is the panel, not the quiz, so the eyebrow can carry the
  // quiz's own name without restating the heading below it.
  const resultsEyebrow =
    course === undefined || course.course_code === null
      ? quizTitle
      : `${course.course_code} / ${quizTitle}`

  const guidance = answered
    ? question?.kind === 'mcq'
      ? t('quiz.guidance.recorded')
      : t('quiz.guidance.markIt')
    : question?.kind === 'mcq'
      ? choice === null
        ? t('quiz.guidance.choose')
        : t('quiz.guidance.selected')
      : written.trim().length === 0
        ? t('quiz.guidance.write')
        : t('quiz.guidance.written')

  function submit() {
    if (question === undefined) return
    const answer = question.kind === 'mcq' ? (choice ?? '') : written.trim()
    if (answer.length === 0) return
    record.mutate(
      {
        questionId: question.id,
        userAnswer: answer,
        // The device's clock, as a review sends `reviewedAt`: an attempt is one of the
        // kinds a client may write offline, so the moment the student answered is this
        // device's to report. Without it the server stamps its own sync time, and two
        // attempts made at different moments can land on the same instant.
        answeredAt: new Date().toISOString(),
      },
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
    // The answer being marked is the one on screen, and on a resumed question that is
    // the stored one while `written` is still empty. Sending `written` alone would
    // overwrite the answer the student actually wrote with nothing.
    const answer = written.trim().length > 0 ? written.trim() : (stored?.user_answer ?? '')
    record.mutate(
      {
        questionId: question.id,
        userAnswer: answer,
        correct,
        answeredAt: new Date().toISOString(),
      },
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
    <div className={pageClass}>
      <nav
        aria-label={t('quiz.breadcrumbLabel')}
        className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1"
      >
        {/* `min-h-11 inline-flex items-center`: a `.link` that is a control is a 44px
            target on a phone, not the height of its own line box. */}
        <Link to="/study" className="link inline-flex min-h-11 items-center gap-2 text-label">
          <Icon name="arrow-left" />
          {t('nav.study')}
        </Link>
        <p className="min-w-0 break-words text-label text-muted">
          {courseLabel === null
            ? `/ ${t('study.hub.quizzes')}`
            : `/ ${courseLabel} / ${t('study.hub.quizzes')}`}
        </p>
      </nav>

      <PageHeader
        eyebrow={phase === 'results' ? resultsEyebrow : (courseLabel ?? undefined)}
        title={phase === 'results' ? t('quiz.results.title') : quizTitle}
        subtitle={
          phase === 'results'
            ? t('quiz.results.subtitle')
            : sourceTitles.length > 0
              ? t('quiz.fromSources', { sources: sourceTitles.join(' · ') })
              : undefined
        }
        action={
          // Ending early is a local decision — the attempts are already recorded, so the
          // summary is whatever the server holds. Nothing is thrown away by asking for it.
          phase === 'results' ? undefined : (
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => {
                setPhase('results')
              }}
            >
              {t('quiz.endQuiz')}
            </button>
          )
        }
      />

      {phase === 'results' ? (
        <Results quizId={quizId} />
      ) : (
        <div className="flex flex-col gap-6 lg:flex-row">
          <div className="flex min-w-0 flex-1 flex-col gap-4">
            {question !== undefined && (
              <div ref={questionRef} tabIndex={-1} className="card flex flex-col gap-6 p-6">
                <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                  <p className="eyebrow">
                    {t('quiz.questionProgress', { current: index + 1, total })}
                  </p>
                  <p className="eyebrow">
                    {question.kind === 'mcq'
                      ? t('quiz.oneCorrect')
                      : t('quiz.writeYourAnswer')}
                  </p>
                </div>

                <h2 className="min-w-0 break-words">{question.prompt}</h2>

                {question.kind === 'mcq' && question.options !== null && (
                  // `min-w-0` is load-bearing, not decoration: a `fieldset` carries the UA
                  // default `min-inline-size: min-content`, so it refuses to shrink below its
                  // widest option's minimum and overflows the card instead of letting the
                  // options wrap. This is the same failure the `.btn` rule documents for
                  // `min-width: auto`, and it is invisible at 100% text.
                  <fieldset className="flex min-w-0 flex-col gap-3">
                    {/* The visible line above the options is the context row's "one correct
                        answer"; this names the group for a screen reader, which the row of
                        labels cannot. */}
                    <legend className="sr-only">{t('quiz.chooseOne')}</legend>
                    {question.options.map((option, position) => {
                      const state = rowStateFor(option, question.answer, chosenOption, answered)
                      const status: { icon: IconName; word: string } | null =
                        state === 'selected'
                          ? { icon: 'circle-check', word: t('quiz.selected') }
                          : state === 'chosen-correct'
                            ? { icon: 'circle-check', word: t('quiz.correctYourAnswer') }
                            : state === 'correct'
                              ? { icon: 'circle-check', word: t('quiz.correctAnswer') }
                              : state === 'wrong'
                                ? { icon: 'alert', word: t('quiz.yourAnswer') }
                                : null
                      return (
                        <OptionRow
                          // Keyed by position, not by text: `validate()` requires four
                          // non-empty options and an answer among them, not four *distinct*
                          // ones, so a model that repeats a distractor would produce duplicate
                          // React keys and a mis-render.
                          key={position}
                          option={option}
                          position={position}
                          state={state}
                          status={status}
                          checked={choice === option || priorAnswer === option}
                          disabled={answered}
                          onSelect={() => {
                            setChoice(option)
                          }}
                        />
                      )
                    })}
                  </fieldset>
                )}

                {question.kind === 'short' && (
                  <div className="flex flex-col gap-2">
                    <label className="label" htmlFor="quiz-answer">
                      {t('quiz.yourAnswer')}
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
                  </div>
                )}

                {answered && (
                  <div className="flex flex-col gap-4">
                    {question.kind === 'mcq' && (
                      // The verdict is a word, not only a tone: "Correct" and "Not correct"
                      // are readable in monochrome and to a screen reader.
                      <div
                        className={`note flex items-start gap-3 ${
                          verdict === true ? 'note-success' : 'note-danger'
                        }`}
                        role="status"
                      >
                        <Icon
                          name={verdict === true ? 'circle-check' : 'alert'}
                          className="mt-0.5 shrink-0"
                        />
                        <p className="min-w-0 break-words font-semibold">
                          {verdict === true
                            ? t('quiz.correct')
                            : t('quiz.notCorrectWithAnswer', { answer: question.answer })}
                        </p>
                      </div>
                    )}

                    {question.kind === 'short' && (
                      <>
                        <div className="note flex flex-col gap-2">
                          <p className="eyebrow">{t('quiz.goodAnswer')}</p>
                          <p className="min-w-0 whitespace-pre-wrap break-words">
                            {question.answer}
                          </p>
                        </div>
                        {question.key_points !== null && question.key_points.length > 0 && (
                          <div className="flex flex-col gap-2">
                            <p className="eyebrow">{t('quiz.didYouSay')}</p>
                            <ul className="flex list-disc flex-col gap-1 pl-5 text-label">
                              {question.key_points.map((point) => (
                                <li key={point} className="break-words">
                                  {point}
                                </li>
                              ))}
                            </ul>
                          </div>
                        )}
                        <div className="flex flex-wrap gap-2">
                          <button
                            type="button"
                            className="btn btn-primary"
                            onClick={() => void mark(true)}
                          >
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

                    <p className="min-w-0 whitespace-pre-wrap break-words text-small text-muted">
                      {question.explanation}
                    </p>

                    {question.chunk_id !== null && (
                      <SourcePassage
                        chunkId={question.chunk_id}
                        sources={sources.data ?? []}
                        label={t('quiz.sourceLabel')}
                      />
                    )}
                  </div>
                )}

                <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border pt-6">
                  {/* The row states the condition the button is in, so a disabled
                      "Check answer" is explained rather than just dimmed. */}
                  <p className="min-w-0 flex-1 break-words text-label text-muted">{guidance}</p>
                  {!answered && (
                    <button
                      type="button"
                      className={
                        (question.kind === 'mcq' ? choice !== null : written.trim().length > 0)
                          ? 'btn btn-primary'
                          : 'btn btn-secondary'
                      }
                      disabled={
                        record.isPending ||
                        (question.kind === 'mcq'
                          ? choice === null
                          : written.trim().length === 0)
                      }
                      onClick={submit}
                    >
                      {record.isPending ? t('quiz.checking') : t('quiz.submit')}
                    </button>
                  )}
                  {answered && question.kind === 'mcq' && (
                    <button type="button" className="btn btn-primary" onClick={advance}>
                      {index + 1 >= total ? t('quiz.seeResults') : t('quiz.nextQuestion')}
                    </button>
                  )}
                </div>

                {attemptError !== null && (
                  <ApiErrorNotice error={attemptError} noun={t('errors.nouns.answer')} />
                )}
              </div>
            )}

            <p className="meta">{t('quiz.generatedNote')}</p>
          </div>

          <div className="flex min-w-0 flex-col gap-4 lg:w-70 lg:shrink-0">
            <div className="card flex flex-col gap-4 p-6">
              <WidgetHeading title={t('quiz.session.title')} />
              <p className="eyebrow">
                {t('quiz.results.answeredOf', { answered: answeredCount, total })}
              </p>
              {/* The bar repeats the count above it; the count is the fact, so a screen
                  reader user gets the sentence rather than a percentage. */}
              <div
                aria-hidden="true"
                className="h-1.5 w-full overflow-hidden rounded-pill bg-surface-raised"
              >
                <div className="h-full bg-primary" style={{ width: `${answeredPercent}%` }} />
              </div>
              <ol aria-label={t('quiz.session.navigatorLabel')} className="flex flex-wrap gap-2">
                {questions.map((entry, position) => {
                  const current = position === index
                  const done = entry.attempt !== null || (current && answered)
                  return (
                    <li
                      key={entry.id}
                      aria-current={current ? 'step' : undefined}
                      className={`flex h-10 w-10 items-center justify-center rounded-field border ${
                        current
                          ? 'border-primary bg-primary-tint'
                          : done
                            ? 'border-border bg-surface-raised'
                            : 'border-border bg-surface'
                      }`}
                    >
                      <span
                        className={current ? 'meta text-primary-text' : 'meta'}
                        aria-hidden="true"
                      >
                        {String(position + 1).padStart(2, '0')}
                      </span>
                      <span className="sr-only">{position + 1}</span>
                    </li>
                  )
                })}
              </ol>
              <p className="text-label text-muted">{t('quiz.session.legend')}</p>
            </div>

            <div className="card flex flex-col gap-4 p-6">
              <p className="eyebrow">{t('quiz.session.focus')}</p>

              {/* `isLoading`, not `isPending`: a query the screen cannot ask (no workspace
                  id on the quiz) is not a fetch in flight, and a "loading" line that never
                  resolves is a claim about the app that is not true. */}
              {sources.isLoading && <LoadingState label={t('workspaces.sources.loading')} />}

              {sourcesError !== null && (
                <ApiErrorNotice
                  error={sourcesError}
                  action={
                    <button
                      type="button"
                      className="btn btn-secondary"
                      onClick={() => void sources.refetch()}
                    >
                      {t('actions.retry')}
                    </button>
                  }
                />
              )}

              {quizSources !== null && quizSources.length === 0 && (
                <EmptyState
                  padded={false}
                  title={t('quiz.session.noSourcesTitle')}
                  reason={t('quiz.session.noSourcesReason')}
                  action={
                    <Link to="/workspaces" className="btn btn-secondary">
                      {t('quiz.backToWorkspaces')}
                    </Link>
                  }
                />
              )}

              {quizSources !== null && quizSources.length > 0 && (
                <ul className="flex flex-col gap-1">
                  {quizSources.map((entry) => (
                    <li key={entry.id} className="min-w-0 break-words text-label">
                      {entry.title ?? t('quiz.results.sourceUnavailable')}
                    </li>
                  ))}
                </ul>
              )}

              <div className="flex flex-col gap-2 border-t border-border pt-4">
                <p className="eyebrow">
                  {t('quiz.count', { count: total })} · {t('quiz.session.noTimeLimit')}
                </p>
                <p className="text-label text-muted">{t('quiz.session.hint')}</p>
              </div>
            </div>
          </div>
        </div>
      )}
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

  if (results.isPending) return <SkeletonRows rows={3} label={t('quiz.results.loading')} />

  if (error !== null) {
    return (
      <ApiErrorNotice
        error={error}
        noun={t('errors.nouns.results')}
        action={
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => void results.refetch()}
          >
            {t('actions.retry')}
          </button>
        }
      />
    )
  }

  const summary = results.data?.summary
  // Unreachable while the query is settled: `summary` is required on the response, so a
  // defined `data` always carries one. Kept as a narrowing guard rather than asserted,
  // so a server that ever omits it renders nothing instead of throwing.
  if (summary === undefined) return null

  // How many of the answers that were judged came back wrong. This is a difference of
  // two numbers the server sent, not a score recomputed from the attempts on screen.
  const missed = summary.graded - summary.correct

  return (
    <div className="grid gap-6 lg:grid-cols-3">
      <div className="card flex flex-col gap-6 p-6">
        <p className="eyebrow">{t('quiz.results.score')}</p>

        {summary.score === null ? (
          <p className="text-body text-muted">{t('quiz.results.nothingMarked')}</p>
        ) : (
          <div className="flex flex-col gap-3">
            <p className="text-display font-semibold">{Math.round(summary.score * 100)}%</p>
            <p className="eyebrow">
              {t('quiz.results.correctOf', {
                correct: summary.correct,
                graded: summary.graded,
              })}
            </p>
            {/* The bar is decorative: the number above it is the fact, and a screen
                reader user gets the sentence below rather than a percentage. */}
            <div
              aria-hidden="true"
              className="h-2 w-full overflow-hidden rounded-pill bg-surface-raised"
            >
              <div
                className="h-full bg-primary"
                style={{ width: `${Math.round(summary.score * 100)}%` }}
              />
            </div>
            {missed > 0 && (
              <p className="text-body">{t('quiz.results.reflection', { count: missed })}</p>
            )}
          </div>
        )}

        <p className="text-small text-muted">
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

        <Link to="/study" className="btn btn-secondary">
          {t('quiz.results.backToStudy')}
        </Link>
      </div>

      <div className="flex min-w-0 flex-col gap-6 lg:col-span-2">
        {summary.topics.length > 0 && (
          <Section title={t('quiz.results.topics')} count={summary.topics.length}>
            <div className="card">
              <ul className="divide-y divide-border">
                {summary.topics.map((topic) => (
                  <li
                    key={`${topic.source_id ?? 'unlinked'}-${String(topic.page ?? 0)}`}
                    className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 px-4 py-3"
                  >
                    <p className="min-w-0 flex-1 break-words text-label">
                      {topic.source_title ?? t('quiz.results.sourceUnavailable')}
                      {topic.page !== null && (
                        <span className="meta"> {t('citation.page', { page: topic.page })}</span>
                      )}
                    </p>
                    <span className="meta">
                      {t('quiz.results.correctOf', {
                        correct: topic.correct,
                        graded: topic.graded,
                      })}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          </Section>
        )}

        <Section
          title={t('quiz.results.everyQuestion')}
          count={results.data?.questions.length}
        >
          <div className="card">
            <ul className="divide-y divide-border">
              {(results.data?.questions ?? []).map((entry) => (
                <li key={entry.question_id} className="flex flex-col gap-1 px-4 py-3">
                  <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                    <p className="min-w-0 flex-1 break-words text-label font-medium">
                      {entry.prompt}
                    </p>
                    {/* The chip carries the word: the tone only repeats it. */}
                    <span
                      className={
                        entry.correct === true
                          ? 'chip chip-ready'
                          : entry.correct === false
                            ? 'chip chip-failed'
                            : 'chip'
                      }
                    >
                      {entry.correct === true
                        ? t('quiz.correct')
                        : entry.correct === false
                          ? t('quiz.notCorrect')
                          : t('quiz.notMarked')}
                    </span>
                  </div>
                  <p className="text-small text-muted">{entry.explanation}</p>
                </li>
              ))}
            </ul>
          </div>
        </Section>
      </div>
    </div>
  )
}
