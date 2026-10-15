import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import {
  useDeck,
  useDueCards,
  useReviewCard,
  useUndoReview,
  type Rating,
} from '@/api/queries/flashcards'
import { useSources } from '@/api/queries/sources'
import { useWorkspace } from '@/api/queries/workspaces'
import { Icon } from '@/components/ui/Icon'
import { SourcePassage } from '@/components/ui/SourcePassage'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { SyncStatus } from '@/features/sync/SyncStatus'
import { dueCards, previewInterval, type StudyCard } from './due'

/**
 * Study mode: one card at a time, four grades.
 *
 * The queue is seeded from the server's due list and then owned locally, so
 * grading a card does not make the list jump. The card is a focusable region and
 * focus moves to it on advance, because the whole point of a keyboard shortcut is
 * that the student never has to find the card again.
 *
 * Swiping is an accelerator, never the only way: every grade has a button and a
 * number key, and the swipe threshold is on the container so a vertical scroll
 * still works (`touch-action: pan-y`).
 *
 * The grade buttons appear only after the answer is revealed, and the number keys
 * are gated on the same state: grading a card whose back the student has not seen
 * would record a judgement they could not have made. Revealing moves focus onto
 * the card, so Tab from there lands on the four grades in order.
 *
 * The schedule is the server's. The one SM-2 implementation lives on the server
 * (`crates/core/src/sm2.rs`), and it is the only thing that writes `ease`,
 * `interval_days` and `due_at`; a second writer here is how two devices start
 * disagreeing. What `./due` computes is the queue — which cards are due, in what
 * order — and a *preview* of the interval each grade would schedule, which is the
 * number the design prints on the four buttons. The receipt under the card reports
 * the interval the server answered with, so the student can see the two agree.
 */

const KEY_RATING: Record<string, Rating> = {
  '1': 'again',
  '2': 'hard',
  '3': 'good',
  '4': 'easy',
}

/**
 * How long the receipt under the card stays. The design's note calls it a
 * five-second opportunity; here it is a report, and a report that outlives the
 * next grade would be describing a card the student has already moved past.
 */
const RECEIPT_MS = 5000

/**
 * A review that failed and is still unresolved.
 *
 * It carries the id that was sent, because the next attempt for this card must
 * resend it: see the rule in `grade`.
 */
interface FailedReview {
  rating: Rating
  id: string
}

/**
 * What the server wrote for the card just graded, so the card's own outcome can be
 * reported under the one that is on screen now.
 */
interface Receipt {
  rating: Rating
  position: number
  days: number
  /**
   * The id the review was sent with — the row's own key — so the Undo control on
   * the receipt can name exactly the review it withdraws rather than the card.
   */
  id: string
}

export function StudyScreen() {
  const { t } = useTranslation()
  const { deckId = '' } = useParams()
  const deck = useDeck(deckId)
  const due = useDueCards(deckId)
  const review = useReviewCard(deckId)
  const undo = useUndoReview(deckId)

  const workspaceId = deck.data?.deck.workspace_id ?? ''
  const sources = useSources(workspaceId)
  const workspace = useWorkspace(workspaceId)

  const [queue, setQueue] = useState<StudyCard[] | null>(null)
  const [seedNow, setSeedNow] = useState<Date | null>(null)
  const [index, setIndex] = useState(0)
  const [revealed, setRevealed] = useState(false)
  const [sourceOpen, setSourceOpen] = useState(false)
  const [failed, setFailed] = useState<FailedReview | null>(null)
  const [receipt, setReceipt] = useState<Receipt | null>(null)
  const [undoError, setUndoError] = useState<ApiError | null>(null)
  const cardRef = useRef<HTMLDivElement>(null)
  const endRef = useRef<HTMLDivElement>(null)
  const advanceFocus = useRef(false)

  // Seeded once, when the due list first arrives. `null` rather than `[]` so an
  // empty deck and a not-yet-loaded deck are different states.
  //
  // The instant is the server's `now`, not the device's clock. The server has
  // already filtered to `due_at <= now`, so re-filtering with a device clock that
  // runs behind would silently drop cards it just returned — and because the queue
  // is seeded once, they would stay missing for the whole session.
  useEffect(() => {
    if (queue === null && due.data !== undefined) {
      const now = new Date(due.data.now)
      setSeedNow(now)
      setQueue(dueCards(due.data.cards, now))
    }
  }, [due.data, queue])

  // Focus the region the session moved to: the card after a grade, or the end
  // state after the last one. Without the second case focus would fall to the
  // body when the card unmounts, and the next Tab would restart at the top of the
  // document.
  useEffect(() => {
    if (advanceFocus.current) {
      advanceFocus.current = false
      ;(cardRef.current ?? endRef.current)?.focus()
    }
  }, [index])

  // The receipt is about the card just left behind, so it is cleared on a timer
  // rather than on the next grade: a student who stops after one card still gets to
  // read what that grade did. The undo notice goes with it, because a refusal whose
  // offer is no longer on screen is a sentence about a control that has left.
  useEffect(() => {
    if (receipt === null) return
    const timer = window.setTimeout(() => {
      setReceipt(null)
      setUndoError(null)
    }, RECEIPT_MS)
    return () => window.clearTimeout(timer)
  }, [receipt])

  const card = queue?.[index] ?? null
  const deckError = asApiError(deck.error)
  const dueError = asApiError(due.error)
  const reviewError = asApiError(review.error)
  const finished = queue !== null && card === null
  // The same instant the queue was filtered with, so the end state's "next due"
  // cannot disagree with the list it is describing.
  const nextDueAt = nextDue(deck.data?.cards ?? [], seedNow ?? new Date())

  // The labels are resolved here rather than at module scope: a constant holding
  // copy is evaluated before there is a language, so it could only ever ship one
  // of them. The keys are shortcuts, not copy, and stay as they are.
  const grades: { rating: Rating; label: string; key: string }[] = [
    { rating: 'again', label: t('study.grade.again'), key: '1' },
    { rating: 'hard', label: t('study.grade.hard'), key: '2' },
    { rating: 'good', label: t('study.grade.good'), key: '3' },
    { rating: 'easy', label: t('study.grade.easy'), key: '4' },
  ]
  const gradeLabel = (rating: Rating): string =>
    grades.find((option) => option.rating === rating)?.label ?? ''

  const course = workspace.data
  const courseLabel = course
    ? course.course_code === null
      ? course.name
      : `${course.course_code} / ${course.name}`
    : null

  function grade(rating: Rating) {
    if (card === null || review.isPending) return
    // One id per card *showing*, not per grade. A failed review may have reached
    // the server, and the only safe assumption is that it did: reusing the id makes
    // the retry idempotent (the server answers `applied: false` for an id it has
    // already seen), while a fresh id after a failure that did land would grade the
    // card twice for one showing. `failed` is cleared on success, so the next card
    // starts with a new id.
    const id = failed !== null ? failed.id : crypto.randomUUID()
    const position = index + 1
    review.mutate(
      { cardId: card.id, input: { rating, id, reviewedAt: new Date().toISOString() } },
      {
        onSuccess: (response) => {
          setFailed(null)
          // The receipt is the undo toast: it already names the grade and lasts the
          // spec's five seconds, so the id travels with it rather than on a second
          // five-second block beside it.
          setReceipt({ rating, position, days: response.card.interval_days, id })
          setUndoError(null)
          advanceFocus.current = true
          setIndex((value) => value + 1)
          setRevealed(false)
          setSourceOpen(false)
        },
        onError: () => {
          setFailed({ rating, id })
        },
      },
    )
  }

  function reveal() {
    setRevealed(true)
    cardRef.current?.focus()
  }

  /**
   * Take the last grade back, while its receipt is still on screen.
   *
   * The server replays the card's history and writes the schedule the remaining
   * reviews produce, so the only local work is putting the card back: the queue is
   * local state and the grade is the one step that advanced past it, so moving the
   * index back to the position the receipt names is what makes the card the one on
   * screen again. `revealed` is cleared rather than carried over — the card is being
   * graded again, and a revealed answer would record a judgement about the showing
   * that was just withdrawn.
   *
   * A refusal is shown as it came: the window's 409 ("too late") and the 404 for a
   * review that is already gone are different facts, and the notice says which. The
   * receipt's window restarts with the notice on it, so a refusal has as long to be
   * read as the offer did — a message that appears under a toast about to expire is
   * a message the student never gets.
   */
  function undoGrade() {
    if (receipt === null || undo.isPending) return
    const withdrawn = receipt
    undo.mutate(withdrawn.id, {
      onSuccess: () => {
        setReceipt(null)
        setUndoError(null)
        setIndex(withdrawn.position - 1)
        setRevealed(false)
        setSourceOpen(false)
        advanceFocus.current = true
      },
      onError: (error) => {
        setUndoError(asApiError(error))
        // A fresh object, so the timer effect re-runs: setting the same reference
        // back would be a no-op to React and would leave the original deadline.
        setReceipt({ ...withdrawn })
      },
    })
  }

  // The keyboard is a first-class way in, not a shortcut bolted on: Space reveals
  // and 1–4 grade, Z undoes, and the same actions are the buttons a pointer or a
  // screen reader uses.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey) return
      // The listener is on `window`, so it sees the keys typed into a field as well
      // as the ones aimed at the card. The screen has no field of its own yet, but
      // `E` (edit) is a spec hotkey and will add one, and a global handler that eats
      // a letter mid-word is the defect that only shows up once it does.
      const target = event.target
      if (
        target instanceof HTMLElement &&
        (target.isContentEditable ||
          target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.tagName === 'SELECT')
      ) {
        return
      }
      if (event.key === ' ') {
        if (!revealed && card !== null) {
          event.preventDefault()
          reveal()
        }
        return
      }
      // Z is offered only while the receipt is up — the five seconds the spec calls
      // the undo opportunity — so an expired one cannot withdraw a review the server
      // would refuse anyway.
      if ((event.key === 'z' || event.key === 'Z') && receipt !== null && !undo.isPending) {
        event.preventDefault()
        undoGrade()
        return
      }
      const rating = KEY_RATING[event.key]
      if (rating !== undefined && revealed && card !== null && !review.isPending) {
        event.preventDefault()
        grade(rating)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
    }
  })

  const touchStart = useRef<{ x: number; y: number } | null>(null)

  function onTouchStart(event: React.TouchEvent) {
    const touch = event.touches[0]
    if (touch) touchStart.current = { x: touch.clientX, y: touch.clientY }
  }

  function onTouchEnd(event: React.TouchEvent) {
    const start = touchStart.current
    touchStart.current = null
    const touch = event.changedTouches[0]
    if (start === null || touch === undefined) return
    const dx = touch.clientX - start.x
    const dy = touch.clientY - start.y
    // A mostly-vertical gesture is a scroll, not a swipe, and a short one is a
    // tap. Both are left alone.
    if (Math.abs(dy) > 20 || Math.abs(dx) < 60) return
    if (revealed) grade(dx < 0 ? 'again' : 'good')
  }

  const reviewed = index
  const total = queue?.length ?? 0
  const progress = total > 0 ? Math.min(100, Math.round((reviewed / total) * 100)) : 0
  // The passage is only ever in the second column when the student asked for it: the
  // frames draw the card alone on the page, and a column reserved for a panel nobody
  // opened is width taken from the card. So both the column count and the page's own
  // width follow this one flag.
  const panelOpen = revealed && sourceOpen && card !== null && card.chunk_id !== null

  return (
    <div className="flex w-full min-w-0 flex-1 flex-col gap-6 pb-[calc(1.5rem+env(safe-area-inset-bottom,0px))]">
      <header className="flex min-h-16 flex-wrap items-center justify-between gap-3 border-b border-border bg-surface px-5 py-2 pt-[calc(0.5rem+env(safe-area-inset-top,0px))] md:min-h-20 md:px-10 md:py-4">
        {workspaceId.length > 0 && (
          // `min-h-11 inline-flex items-center` — the shape every other text link in the
          // app uses (`TodayScreen`, `SettingsScreen`, `States.tsx`), and the one gate 5
          // requires: a bare `link` is text-height, and this one measured **151.3x21** in
          // Chromium at 390px, the same defect Phase 7's gate found on the upgrade link
          // (93x19). `text-label` is kept because it is the header's own scale; the
          // utilities are what make the box 44px rather than the line box of an inline
          // element.
          <Link
            to={`/workspaces/${workspaceId}`}
            aria-label={`${t('app.name')}: ${t('study.backToWorkspace')}`}
            className="inline-flex min-h-11 min-w-0 items-center gap-3 rounded-field text-label text-primary-text"
          >
            <span className="flex size-11 shrink-0 items-center justify-center"><Icon name="arrow-left" /></span>
            <span className="font-heading text-h2 font-semibold leading-[1.25] text-text md:text-[1.75rem]">{t('app.name')}</span>
            <span className="hidden sm:inline">{t('study.backToWorkspace')}</span>
          </Link>
        )}
        {/* Ending the session is leaving the deck, not a second "back": the hub is
            where a session is chosen, so it is where the student lands when they are
            done with this one. Every review is already recorded when this is pressed. */}
        <Link to="/study" aria-label={t('study.endSession')} className="btn btn-quiet ml-auto min-w-11">
          <span className="hidden sm:inline">{t('study.endSession')}</span>
          <Icon name="close" />
        </Link>
        <SyncStatus variant="notice" />
      </header>

      <div className={`@container mx-auto flex w-full min-w-0 flex-col gap-6 px-5 md:px-10 ${panelOpen ? 'max-w-[76rem]' : 'max-w-[52.5rem]'}`}>

      <div className="flex flex-col gap-2">
        {courseLabel !== null && <p className="eyebrow eyebrow-primary">{courseLabel}</p>}
        <h1 className="min-w-0 break-words text-h2">
          {deck.data?.deck.title ?? t('nav.study')}
        </h1>
        {card !== null && (
          <>
            <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
              <p className="meta" role="status">
                {t('study.cardProgress', { current: index + 1, total })}
              </p>
              <p className="meta">
                {t('study.sessionProgress', { reviewed, remaining: total - reviewed })}
              </p>
            </div>
            {/* The bar repeats the position above it; the sentence is the fact, so a
                screen reader user gets the counts rather than a percentage. */}
            <div
              aria-hidden="true"
              className="h-1.5 w-full overflow-hidden rounded-pill bg-surface-raised"
            >
              <div className="h-full bg-primary" style={{ width: `${progress}%` }} />
            </div>
          </>
        )}
      </div>

      {/* A deck that never loaded blocks the session. Once the queue is seeded,
          though, the deck query refetches in the background after every review
          (`useReviewCard` invalidates the deck key), and TanStack keeps `data`
          while setting `error` on a failed refetch — so a background failure must
          only surface a notice, never unmount the card mid-session. */}
      {deckError !== null && queue === null && (
        <ApiErrorNotice
          error={deckError}
          noun={t('errors.nouns.deck')}
          action={
            <button type="button" className="btn btn-secondary" onClick={() => void deck.refetch()}>
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {dueError !== null && queue === null && (
        <ApiErrorNotice
          error={dueError}
          noun={t('errors.nouns.dueCards')}
          action={
            <button type="button" className="btn btn-secondary" onClick={() => void due.refetch()}>
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {deckError !== null && queue !== null && (
        <p className="note note-danger text-sm" role="status">
          {t('study.refreshFailed')}
        </p>
      )}

      {deckError === null && dueError === null && queue === null && (
        <LoadingState label={t('study.loadingDue')} />
      )}

      {queue !== null && card !== null && (
        // **The grid is the "collapsible side panel", and it is collapsed by
        // default.** `SPEC.md:678` asks for a desktop layout of a centred card with a
        // collapsible panel beside it; the frames draw the centred card and nothing
        // else, because a card with no citation has no panel to show. So the template
        // is two columns only while the panel is open, and the passage the student
        // asked for moves into the second column on a desktop rather than growing the
        // card. The explicit `col-start`/`row-start` placements are what keep the
        // phone in the design's order — card, then the passage, then the grades at the
        // foot — while a desktop reads card | passage down the right and the grades
        // under the card.
        <div
          className={`grid min-w-0 gap-6 @min-[64rem]:items-start ${
            panelOpen ? '@min-[64rem]:grid-cols-[minmax(0,1fr)_20rem]' : ''
          }`}
        >
          {/* The card auto-sizes and scrolls *inside* itself: at 200% text a long
              answer is taller than any phone, and a card that clipped it would hide
              the thing the student revealed. `max-h` is what makes the internal
              scroll engage before the page does, which is also what keeps the grade
              row at the foot of the screen without a fixed band over the content —
              the Phase 8 gate's finding was that a band painted over the content has
              to reserve a height that is not a number. */}
          <div
            ref={cardRef}
            tabIndex={-1}
            role="group"
            aria-label={t('study.cardLabel')}
            onTouchStart={onTouchStart}
            onTouchEnd={onTouchEnd}
            className="card flex min-h-[min(20rem,55dvh)] max-h-[70dvh] min-w-0 touch-pan-y flex-col gap-4 overflow-y-auto p-6 sm:min-h-[min(26.25rem,55dvh)] sm:p-10 focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring @min-[64rem]:col-start-1 @min-[64rem]:row-start-1"
          >
            <p className="eyebrow">
              {revealed ? t('study.answerLabel') : t('study.questionLabel')}
            </p>

            {revealed ? (
              <div className="flex min-w-0 flex-col gap-4">
                {/* Once the answer is on screen the question becomes the context for
                    it, which is the design's own hierarchy: the thing being read is
                    the answer. */}
                <p className="min-w-0 break-words whitespace-pre-wrap text-label text-muted">
                  {card.front}
                </p>
                <div className="flex min-w-0 flex-col gap-4 border-t border-border pt-4">
                  <p className="min-w-0 break-words whitespace-pre-wrap text-h4">{card.back}</p>

                  {card.chunk_id !== null && (
                    <button
                      type="button"
                      className="btn btn-quiet self-start"
                      aria-expanded={sourceOpen}
                      onClick={() => {
                        setSourceOpen((open) => !open)
                      }}
                    >
                      {sourceOpen ? t('study.hideSource') : t('study.showSource')}
                    </button>
                  )}
                </div>
              </div>
            ) : (
              <div className="flex min-w-0 flex-1 flex-col justify-center gap-4">
                {/* The design's largest text on the screen: 24/32 on a phone and
                    32/43 on a desktop. It is not a heading — the deck's title above
                    is the page's `h1` — it is the one thing the student is reading,
                    which is why it takes the scale rather than a heading level. */}
                <p className="min-w-0 break-words whitespace-pre-wrap font-heading text-h2 font-semibold sm:text-h1">
                  {card.front}
                </p>
                <p className="text-label text-muted">{t('study.recallHint')}</p>
              </div>
            )}
          </div>

          {panelOpen && card.chunk_id !== null && (
            <aside className="min-w-0 @min-[64rem]:col-start-2 @min-[64rem]:row-span-2 @min-[64rem]:row-start-1">
              <SourcePassage chunkId={card.chunk_id} sources={sources.data ?? []} />
            </aside>
          )}

          <div className="flex min-w-0 flex-col gap-4 @min-[64rem]:col-start-1 @min-[64rem]:row-start-2">
            {!revealed && (
              <div className="flex flex-col items-center gap-3">
                <button
                  type="button"
                  className="btn btn-primary min-h-12 w-full sm:max-w-[17.5rem]"
                  onClick={reveal}
                >
                  {t('study.showAnswer')}
                </button>
                <p className="meta hidden text-center sm:block">{t('study.revealHint')}</p>
              </div>
            )}

            {revealed && (
              <div className="flex flex-col gap-3">
                <p className="text-label text-muted">{t('study.rateRecall')}</p>
                {/* Two columns on a phone and four across on a desktop, which is what
                    the design draws in each. Each grade keeps its tone *and* its word:
                    `Again` is danger, `Hard` warning, `Good` the one primary action,
                    `Easy` success, and the tone is never the only signal — the label
                    says which grade it is. */}
                {/* `auto-fit` with a **rem** floor rather than `sm:grid-cols-4`. Four
                    fixed columns leave each grade 64px at a 32px root, and "Again"
                    renders at 28px — 71px — so the label was clipped. A rem floor grows
                    with the root: four grades fit at 100% (the design's row) and one
                    fits at 200% in the study screen's narrow side-panel column, which is
                    the reflow a fixed count cannot express. **The floor is bounded by
                    that column** (165px at a 32px root), because a floor larger than the
                    container makes the single remaining track wider than the grid —
                    which is what a 6rem floor did here. */}
                <div className="grid grid-cols-2 gap-3 sm:grid-cols-[repeat(auto-fit,minmax(5rem,1fr))]">
                  {grades.map((option) => (
                    <button
                      key={option.rating}
                      type="button"
                      className={`${gradeClass(option.rating)} min-h-12 flex-wrap text-center`}
                      aria-keyshortcuts={option.key}
                      disabled={review.isPending}
                      onClick={() => {
                        grade(option.rating)
                      }}
                    >
                      {option.label}
                      {/* The interval this grade would schedule, from `./due`: the
                          preview the design draws on the button, and the one number
                          that tells the four grades apart. */}
                      <span className="meta">
                        {t('study.interval.days', {
                          days: previewInterval(card, option.rating),
                        })}
                      </span>
                    </button>
                  ))}
                </div>
                {/* The key hint is desktop-only, which is what the design draws: the
                    mobile frame's grade row is four buttons and nothing else, because
                    a phone has no number row to press. The hint is not a control, so
                    it is a sentence rather than a row of keycaps. */}
                <p className="meta hidden sm:block">
                  {t('study.keyHints', {
                    again: gradeLabel('again'),
                    hard: gradeLabel('hard'),
                    good: gradeLabel('good'),
                    easy: gradeLabel('easy'),
                  })}
                </p>
                <p className="meta">{t('study.rateHint')}</p>
              </div>
            )}

            {reviewError !== null && (
              <ApiErrorNotice
                error={reviewError}
                noun={t('errors.nouns.review')}
                action={
                  <button
                    type="button"
                    className="btn btn-secondary"
                    onClick={() => {
                      if (failed !== null) grade(failed.rating)
                    }}
                  >
                    {t('actions.retry')}
                  </button>
                }
              />
            )}
          </div>
        </div>
      )}

      {/* What the grade did, from the server's own answer — the interval here is the
          schedule, where the number on the button was a preview of it. It is outside
          the card block on purpose: grading the last card of a session is exactly when
          the receipt matters, and inside the block it would unmount with the card.

          It is also the undo toast (`SPEC.md:87`). The receipt already names the grade
          and lives for the five seconds the spec calls the opportunity, so Undo belongs
          on it rather than on a second five-second block: one element that says what
          happened and offers to take it back. The row wraps, so the button drops to its
          own line at a phone width in Indonesian rather than pushing the page sideways. */}
      {receipt !== null && (
        <div className="card flex flex-wrap items-center gap-x-3 gap-y-2 p-4">
          <Icon name="undo" />
          {/* The live region is the text, not the card: `role="status"` around a control
              announces the button as part of the message (see `States.tsx`). */}
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1" role="status">
            <p className="text-label">
              {t('study.receipt.rated', { grade: gradeLabel(receipt.rating) })}
            </p>
            <p className="meta">
              {t('study.receipt.next', {
                position: receipt.position,
                days: t('study.interval.days', { days: receipt.days }),
              })}
            </p>
          </div>
          {/* A real button, not a text-height link: undo is an action, and `.btn` carries
              the 44px phone floor the header link needed the same utilities for. */}
          <button
            type="button"
            className="btn btn-secondary"
            onClick={undoGrade}
            disabled={undo.isPending}
          >
            {t('actions.undo')}
          </button>
        </div>
      )}

      {/* The server's refusal, in its own words. The window's 409 and the 404 for a
          review that is already gone are different facts, and a generic "undo failed"
          would say neither — the 409 means the grade stands, which is the one the
          student needs to read. The receipt stays until its timer runs out, so a
          refusal that was only the network can be tried again. */}
      {undoError !== null && (
        <ApiErrorNotice
          error={undoError}
          noun={t('errors.nouns.review')}
          title={undoError.code === 'conflict' ? t('study.undo.tooLate.title') : undefined}
          reason={undoError.code === 'conflict' ? t('study.undo.tooLate.reason') : undefined}
        />
      )}

      {finished && (
        // Focusable, because the card region held focus a moment ago: when the
        // last card unmounts this is where the session's focus has to land, or it
        // falls to the body and the next Tab restarts at the top of the document.
        <div
          ref={endRef}
          tabIndex={-1}
          className="focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring"
        >
          <EmptyState
            title={t('study.nothingDue.title')}
            reason={
              nextDueAt === null
                ? t('study.nothingDue.scheduled')
                : t('study.nothingDue.next', { when: nextDueAt })
            }
            action={
              workspaceId.length > 0 ? (
                <Link to={`/workspaces/${workspaceId}`} className="btn btn-secondary">
                  {t('study.backToWorkspace')}
                </Link>
              ) : undefined
            }
          />
        </div>
      )}
      </div>
    </div>
  )
}

/**
 * The tone of a grade's button.
 *
 * `Good` is the one primary action of the four — the grade most sessions should
 * reach for — and the other three keep the secondary surface with the token that
 * names their meaning on the edge and the label. A fill for each tone would make
 * four primary actions compete; the word on the button is what identifies it.
 */
function gradeClass(rating: Rating): string {
  if (rating === 'good') return 'btn btn-primary'
  if (rating === 'again') return 'btn btn-secondary border-danger text-danger'
  if (rating === 'hard') return 'btn btn-secondary border-warning text-warning'
  return 'btn btn-secondary border-success text-success'
}

/**
 * When the soonest card is due, as a readable string, or `null` when the deck has
 * nothing scheduled ahead of now.
 */
function nextDue(cards: { due_at: string }[], now: Date): string | null {
  const upcoming = cards
    .map((card) => Date.parse(card.due_at))
    .filter((time) => time > now.getTime())
  if (upcoming.length === 0) return null
  return new Date(Math.min(...upcoming)).toLocaleString()
}
