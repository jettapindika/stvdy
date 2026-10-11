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
