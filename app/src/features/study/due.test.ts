/**
 * The due queue is computed on the client from the cards the server sent, because
 * the study screen must not refetch between reviews: a refetch mid-session would
 * reshuffle the queue under the student's finger. The order is therefore part of
 * the contract, not an implementation detail.
 */
import { describe, expect, it } from 'vitest'

import { dueCards, type StudyCard } from './due'

function card(id: string, dueAt: string): StudyCard {
  return { id, front: `f-${id}`, back: `b-${id}`, chunk_id: null, due_at: dueAt, ease: 2.5, interval_days: 0 }
}

describe('dueCards', () => {
  it('keeps only cards whose due_at has arrived', () => {
    const now = new Date('2026-10-11T09:00:00Z')
    const queue = dueCards(
      [card('a', '2026-10-11T08:00:00Z'), card('b', '2026-10-12T09:00:00Z')],
      now,
    )
    expect(queue.map((c) => c.id)).toEqual(['a'])
  })

  it('orders by due_at and then by id, so a refetch cannot reshuffle it', () => {
    const now = new Date('2026-10-11T09:00:00Z')
    const queue = dueCards(
      [
        card('c', '2026-10-10T09:00:00Z'),
        card('a', '2026-10-10T09:00:00Z'),
        card('b', '2026-10-09T09:00:00Z'),
      ],
      now,
    )
    expect(queue.map((c) => c.id)).toEqual(['b', 'a', 'c'])
  })

  it('is empty for an empty deck rather than throwing', () => {
    expect(dueCards([], new Date())).toEqual([])
  })
})
