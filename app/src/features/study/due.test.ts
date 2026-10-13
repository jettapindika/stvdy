/**
 * The due queue is computed on the client from the cards the server sent, because
 * the study screen must not refetch between reviews: a refetch mid-session would
 * reshuffle the queue under the student's finger. The order is therefore part of
 * the contract, not an implementation detail.
 */
import { describe, expect, it } from 'vitest'

import { dueCards, previewInterval, type StudyCard } from './due'

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

describe('previewInterval', () => {
  // The cases are `sm2.rs`'s ladder, read off `replay`: a failure restarts at the
  // first step, the first pass is one day, the second is six, and from there the
  // interval is multiplied by the card's ease. The preview has to agree with the
  // schedule the server writes, or the number on the button is a promise the app
  // does not keep.
  const fresh = { ease: 2.5, interval_days: 0 }

  it('gives a card that has never been reviewed one day, whatever the grade', () => {
    expect(previewInterval(fresh, 'again')).toBe(1)
    expect(previewInterval(fresh, 'hard')).toBe(1)
    expect(previewInterval(fresh, 'good')).toBe(1)
    expect(previewInterval(fresh, 'easy')).toBe(1)
  })

  it('takes a failure back to the first step rather than shortening the interval', () => {
    expect(previewInterval({ ease: 2.5, interval_days: 40 }, 'again')).toBe(1)
  })

  it("lengthens the ladder by the card's own ease once it is past the first two steps", () => {
    // `sm2.rs`'s third pass is `round(interval_days * ease)` — `f32`, so the
    // product is rounded at single precision before `round`.
    expect(previewInterval({ ease: 2.5, interval_days: 6 }, 'good')).toBe(15)
    expect(previewInterval({ ease: 1.3, interval_days: 15 }, 'easy')).toBe(20)
  })

  it('never returns less than one day, so a button cannot promise "0d"', () => {
    for (const rating of ['hard', 'good', 'easy'] as const) {
      expect(previewInterval({ ease: 1.3, interval_days: 1 }, rating)).toBeGreaterThanOrEqual(1)
    }
  })
})
