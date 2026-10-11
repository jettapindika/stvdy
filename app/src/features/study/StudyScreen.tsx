import { useEffect, useRef, useState } from 'react'
import { Link, useParams } from 'react-router'

import { asApiError } from '@/api/errors'
import { useChunk } from '@/api/queries/chat'
import { useDeck, useDueCards, useReviewCard, type Rating } from '@/api/queries/flashcards'
import { useSources } from '@/api/queries/sources'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { dueCards, type StudyCard } from './due'

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
 * The schedule is the server's. This screen computes due-ness and order only
 * (`./due`), never an interval: the one SM-2 implementation lives on the server,
 * and a second one here is how two devices start disagreeing.
 */

const GRADES: { rating: Rating; label: string; key: string }[] = [
  { rating: 'again', label: 'Again', key: '1' },
  { rating: 'hard', label: 'Hard', key: '2' },
  { rating: 'good', label: 'Good', key: '3' },
  { rating: 'easy', label: 'Easy', key: '4' },
]

const KEY_RATING: Record<string, Rating> = {
  '1': 'again',
  '2': 'hard',
  '3': 'good',
  '4': 'easy',
}

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

export function StudyScreen() {
  const { deckId = '' } = useParams()
  const deck = useDeck(deckId)
  const due = useDueCards(deckId)
  const review = useReviewCard(deckId)

  const workspaceId = deck.data?.deck.workspace_id ?? ''
  const sources = useSources(workspaceId)

  const [queue, setQueue] = useState<StudyCard[] | null>(null)
  const [seedNow, setSeedNow] = useState<Date | null>(null)
  const [index, setIndex] = useState(0)
  const [revealed, setRevealed] = useState(false)
  const [sourceOpen, setSourceOpen] = useState(false)
  const [failed, setFailed] = useState<FailedReview | null>(null)
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

  const card = queue?.[index] ?? null
  const deckError = asApiError(deck.error)
  const dueError = asApiError(due.error)
  const reviewError = asApiError(review.error)
  const finished = queue !== null && card === null
  // The same instant the queue was filtered with, so the end state's "next due"
  // cannot disagree with the list it is describing.
  const nextDueAt = nextDue(deck.data?.cards ?? [], seedNow ?? new Date())

  function grade(rating: Rating) {
    if (card === null || review.isPending) return
    // One id per card *showing*, not per grade. A failed review may have reached
    // the server, and the only safe assumption is that it did: reusing the id makes
    // the retry idempotent (the server answers `applied: false` for an id it has
    // already seen), while a fresh id after a failure that did land would grade the
    // card twice for one showing. `failed` is cleared on success, so the next card
    // starts with a new id.
    const id = failed !== null ? failed.id : crypto.randomUUID()
    review.mutate(
      { cardId: card.id, input: { rating, id, reviewedAt: new Date().toISOString() } },
      {
        onSuccess: () => {
          setFailed(null)
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

  // The keyboard is a first-class way in, not a shortcut bolted on: Space reveals
  // and 1–4 grade, and the same actions are the buttons a pointer or a screen
  // reader uses.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey) return
      if (event.key === ' ') {
        if (!revealed && card !== null) {
          event.preventDefault()
          reveal()
        }
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

  return (
    <div className="mx-auto flex min-h-0 w-full max-w-2xl flex-col gap-4 px-4 py-6">
      <header className="flex flex-col gap-1 sm:flex-row sm:flex-wrap sm:items-center sm:gap-x-3">
        <h1 className="min-w-0 break-words text-base font-semibold sm:flex-1">
          {deck.data?.deck.title ?? 'Study'}
        </h1>
        {workspaceId.length > 0 && (
          <Link to={`/workspaces/${workspaceId}`} className="link self-start text-sm sm:self-auto">
            Back to the workspace
          </Link>
        )}
      </header>

      {deckError !== null && (
        <ApiErrorNotice
          error={deckError}
          noun="this deck"
          action={
            <button type="button" className="btn btn-secondary" onClick={() => void deck.refetch()}>
              Try again
            </button>
          }
        />
      )}

      {deckError === null && dueError !== null && (
        <ApiErrorNotice
          error={dueError}
          noun="the due cards"
          action={
            <button type="button" className="btn btn-secondary" onClick={() => void due.refetch()}>
              Try again
            </button>
          }
        />
      )}

      {deckError === null && dueError === null && (deck.isPending || due.isPending || queue === null) && (
        <LoadingState label="Loading this deck's due cards" />
      )}

      {deckError === null && dueError === null && queue !== null && card !== null && (
        <>
          <p className="meta" role="status">
            Card {index + 1} of {queue.length}
          </p>

          <div
            ref={cardRef}
            tabIndex={-1}
            role="group"
            aria-label="Flashcard"
            onTouchStart={onTouchStart}
            onTouchEnd={onTouchEnd}
            className="panel touch-pan-y flex flex-col gap-4 px-4 py-5 focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-accent"
          >
            <p className="whitespace-pre-wrap break-words text-base">{card.front}</p>

            {revealed && (
              <div className="flex flex-col gap-4 border-t border-rule pt-4">
                <p className="whitespace-pre-wrap break-words text-base text-ink-muted">
                  {card.back}
                </p>

                {card.chunk_id !== null && (
                  <div className="flex flex-col gap-2">
                    <button
                      type="button"
                      className="btn btn-quiet self-start"
                      aria-expanded={sourceOpen}
                      onClick={() => {
                        setSourceOpen((open) => !open)
                      }}
                    >
                      {sourceOpen ? 'Hide source' : 'Show source'}
                    </button>
                    {sourceOpen && (
                      <SourcePanel
                        chunkId={card.chunk_id}
                        sources={sources.data ?? []}
                      />
                    )}
                  </div>
                )}
              </div>
            )}
          </div>

          {!revealed && (
            <button type="button" className="btn btn-primary self-start" onClick={reveal}>
              Show answer
            </button>
          )}

          {revealed && (
            <div className="flex flex-wrap gap-2">
              {GRADES.map((option) => (
                <button
                  key={option.rating}
                  type="button"
                  className="btn btn-secondary"
                  aria-keyshortcuts={option.key}
                  disabled={review.isPending}
                  onClick={() => {
                    grade(option.rating)
                  }}
                >
                  {option.label}
                  <span className="meta" aria-hidden="true">
                    {option.key}
                  </span>
                </button>
              ))}
            </div>
          )}

          {reviewError !== null && (
            <ApiErrorNotice
              error={reviewError}
              noun="this review"
              action={
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={() => {
                    if (failed !== null) grade(failed.rating)
                  }}
                >
                  Try again
                </button>
              }
            />
          )}
        </>
      )}

      {deckError === null && dueError === null && finished && (
        // Focusable, because the card region held focus a moment ago: when the
        // last card unmounts this is where the session's focus has to land, or it
        // falls to the body and the next Tab restarts at the top of the document.
        <div
          ref={endRef}
          tabIndex={-1}
          className="focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-accent"
        >
          <EmptyState
            title="Nothing due in this deck"
            reason={
              nextDueAt === null
                ? 'Every card in this deck has been scheduled. Come back after the next one is due.'
                : `Every card here is scheduled. The next one comes due ${nextDueAt}.`
            }
            action={
              workspaceId.length > 0 ? (
                <Link to={`/workspaces/${workspaceId}`} className="btn btn-secondary">
                  Back to the workspace
                </Link>
              ) : undefined
            }
          />
        </div>
      )}
    </div>
  )
}

/**
 * The passage a card came from.
 *
 * The card carries only `chunk_id`, so the source's title is looked up from the
 * workspace's source list by the chunk's `source_id`. Both queries are the ones
 * the chat screen already uses and are cached, so this adds no new API surface.
 */
function SourcePanel({
  chunkId,
  sources,
}: {
  chunkId: string
  sources: { id: string; title: string }[]
}) {
  const chunk = useChunk(chunkId)
  const error = asApiError(chunk.error)
  const title =
    chunk.data !== undefined
      ? (sources.find((source) => source.id === chunk.data.source_id)?.title ?? 'A source')
      : null

  return (
    <section aria-label="Source passage" className="note flex flex-col gap-2">
      {chunk.isPending && <p className="text-sm text-ink-muted">Loading the source passage</p>}

      {error !== null && (
        <ApiErrorNotice
          error={error}
          noun="this passage"
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void chunk.refetch()}
            >
              Try again
            </button>
          }
        />
      )}

      {chunk.data !== undefined && (
        <>
          <p className="meta">
            {title}
            {chunk.data.page !== null && ` page ${String(chunk.data.page)}`}
          </p>
          <p className="whitespace-pre-wrap break-words text-sm text-ink-muted">
            {chunk.data.content}
          </p>
        </>
      )}
    </section>
  )
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
