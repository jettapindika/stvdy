import type { Rating } from '@/api/queries/flashcards'

/**
 * The study queue, computed on the client.
 *
 * The server's `GET /decks/:id/due` returns the queue once, at the start of a
 * session. From then on the screen owns it, because a refetch between reviews
 * would reshuffle the cards under the student's finger. The order therefore has to
 * be reproducible here: `due_at` ascending, ties broken by id.
 *
 * This is due-ness only. The card's `ease`/`interval_days`/`due_at` after a review
 * are the server's, computed by the one SM-2 implementation in
 * `crates/core/src/sm2.rs`; a second implementation in TypeScript would be a second
 * schedule, and two schedules is how two devices start disagreeing.
 */

export interface StudyCard {
  id: string
  front: string
  back: string
  chunk_id: string | null
  due_at: string
  ease: number
  interval_days: number
}

export function dueCards(cards: StudyCard[], now: Date): StudyCard[] {
  return cards
    .filter((card) => Date.parse(card.due_at) <= now.getTime())
    .sort((left, right) => {
      const byDue = Date.parse(left.due_at) - Date.parse(right.due_at)
      return byDue !== 0 ? byDue : left.id.localeCompare(right.id)
    })
}

/** The SM-2 quality grade per rating, matching `sm2.rs`'s `Rating::quality`. */
const QUALITY: Record<Rating, number> = { again: 0, hard: 3, good: 4, easy: 5 }

/** `FIRST_INTERVAL_DAYS` and `SECOND_INTERVAL_DAYS`, in `sm2.rs`'s words. */
const FIRST_INTERVAL_DAYS = 1
const SECOND_INTERVAL_DAYS = 6

/**
 * The interval a grade would schedule, in days — a **preview**, never the schedule.
 *
 * The design puts the interval on each grade button (`SPEC.md:86`, `SPEC.md:700`),
 * and the server answers only *after* the review has been recorded, so the label
 * has to be computed here, by the same rules `crates/core/src/sm2.rs` applies. The
 * review still goes to the server, which replays the card's reviews and writes the
 * state every other reader sees; the toast after the grade reports the interval the
 * server actually wrote, and that number is the one that counts.
 *
 * **The one thing the stored row cannot tell us is how many times the card has
 * passed.** `sm2.rs` counts repetitions while replaying, and `flashcards` keeps
 * only `ease`, `interval_days` and `due_at` — the counter is not on the wire. The
 * ladder position is therefore read back from the interval: 0 has never been
 * reviewed, 1 is the first pass, 6 is the second, and anything else has been
 * through the multiplicative step. That read-back is exact at every interval except
 * one day, which is also what a lapse writes: a card that failed yesterday and a
 * card that passed its first review yesterday both sit at one day, and the first
 * comes back in one day where this says six. A one-day interval is a first pass far
 * more often than it is a lapse from an ease above 3.16, so the first pass is what
 * is assumed; the cost of the guess is a button that occasionally promises longer
 * than the server delivers, which is the price of the number the design asks the
 * four buttons to differ by.
 *
 * The product is rounded in `f32` like the server's `(interval_days as f32) * ease`:
 * the rounding boundary is exactly where a double-precision product would round the
 * other way and print a different day count than the schedule the student gets.
 */
export function previewInterval(
  card: Pick<StudyCard, 'ease' | 'interval_days'>,
  rating: Rating,
): number {
  // A failure restarts the ladder rather than lengthening it: `sm2.rs` sets the
  // interval to the first step whatever the card's history was.
  if (QUALITY[rating] < 3) return FIRST_INTERVAL_DAYS

  const interval = card.interval_days
  if (interval <= 0) return FIRST_INTERVAL_DAYS
  if (interval === FIRST_INTERVAL_DAYS) return SECOND_INTERVAL_DAYS
  // Six days is the second pass's fixed interval *and* what the multiplicative step
  // can compute, and both produce the same next interval: the step is applied to
  // the interval the card holds now.
  return Math.max(1, Math.round(Math.fround(interval * card.ease)))
}
