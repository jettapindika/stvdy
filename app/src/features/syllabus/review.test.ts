import { describe, expect, it } from 'vitest'

import {
  checkCount,
  confirmBody,
  draftFrom,
  keptCount,
  reviewRows,
  type ReviewDraft,
} from './review'

/**
 * The review screen's two absences.
 *
 * The screen exists because a wrong extraction written straight through is the
 * failure the review step prevents, so the properties worth asserting are about
 * what is *not* sent and what is *not* shown: a dropped item has no index in the
 * confirm body, and no row carries the extraction's score — the list arrives
 * pre-ordered with a `needs_check` flag instead, and the evidence quote is what
 * the student checks the item against.
 */

/** One item, in the shape `GET /syllabus/:id/draft` answers. */
function wire(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    index: 0,
    kind: 'deadline',
    title: 'Tugas 1',
    evidence: 'Tugas 1 dikumpulkan 12 Nov',
    needs_check: true,
    when: '2026-11-12T16:59:00Z',
    rrule: null,
    start_time: null,
    end_time: null,
    topics: [],
    weight_percent: 15,
    ...overrides,
  }
}

describe('the review list', () => {
  it('keeps the server’s order and shows each item’s evidence quote', () => {
    // The order is the server's, and it is the point of the score the server does
    // not send: the items it was least sure of are already at the top. A re-sort
    // here would be a second answer to "which item is most likely wrong", and the
    // two would drift the moment the server's threshold moved.
    const rows = reviewRows([
      wire({ index: 3, title: 'Tugas 1', needs_check: true, evidence: 'Tugas 1 dikumpulkan 12 Nov' }),
      wire({ index: 7, title: 'UAS', kind: 'exam', needs_check: false, evidence: 'UAS 15 Desember 08:00' }),
      wire({ index: 9, title: 'Lecture', kind: 'class', needs_check: false, evidence: 'Kuliah Senin 08:00' }),
    ])

    expect(rows.map((row) => row.index)).toEqual([3, 7, 9])
    expect(rows.map((row) => row.evidence)).toEqual([
      'Tugas 1 dikumpulkan 12 Nov',
      'UAS 15 Desember 08:00',
      'Kuliah Senin 08:00',
    ])
    // The flagged item is a word plus a flag the screen renders as text, and the
    // count is what the header says out loud (gate 5(d): never colour alone).
    expect(checkCount(rows)).toBe(1)
  })

  it('never carries the extraction’s score into a row', () => {
    // A261: the score orders the list and flags an item, and the honesty rules
    // forbid rendering it. The row is built field by field rather than spread, so a
    // body that started sending one again could not reach the screen through this
    // module — which is the only path from the wire to a row.
    const [row] = reviewRows([wire({ confidence: 0.42, needs_check: true })])
    expect(row).toBeDefined()
    expect(Object.keys(row as object)).not.toContain('confidence')
    expect(JSON.stringify(row)).not.toContain('0.42')
  })

  it('drops a row it cannot read, rather than rendering it half-empty', () => {
    // An item with no index cannot be confirmed, and one with no evidence quote is
    // an item the student has no way to check — which is the whole reason the
    // review step exists. The server refuses an empty quote at parse time; this is
    // the client refusing to render what it cannot show.
    const rows = reviewRows([
      wire({ index: 1 }),
      wire({ index: 'one' }),
      wire({ index: 2, kind: 'homework' }),
      wire({ index: 4, evidence: '   ' }),
      'not an object',
      null,
    ])
    expect(rows.map((row) => row.index)).toEqual([1])
  })
})

describe('the confirm payload', () => {
  const items = reviewRows([
    wire({ index: 0, title: 'Tugas 1' }),
    wire({ index: 1, title: 'Tugas 2', needs_check: false }),
    wire({ index: 2, title: 'Lecture', kind: 'class', when: null }),
  ])

  it('never includes an item the student dropped', () => {
    // "Nothing is written until the student confirms" is the phase's honesty rule,
    // and its other half is that an item the student removed is not in the body at
    // all — not sent as a `keep: false` the server has to be trusted to honour.
    const drafts: ReviewDraft[] = draftFrom(items).map((draft) =>
      draft.index === 1 ? { ...draft, keep: false } : draft,
    )

    const body = confirmBody(drafts, items)
    expect(body.items.map((item) => item.index)).toEqual([0, 2])
    expect(keptCount(drafts)).toBe(2)
  })

  it('sends only the edits, because an absent field means “keep what the extraction said”', () => {
    const drafts: ReviewDraft[] = draftFrom(items).map((draft) =>
      draft.index === 2 ? { ...draft, title: 'Kuliah Jaringan' } : draft,
    )

    const body = confirmBody(drafts, items)
    // The untouched items carry nothing but their index: sending the extraction's
    // own title back would be indistinguishable from an edit.
    expect(body.items).toEqual([
      { index: 0, keep: true },
      { index: 1, keep: true },
      { index: 2, keep: true, title: 'Kuliah Jaringan' },
    ])
  })

  it('sends a changed date and a changed topic list, and no field the server does not read', () => {
    const drafts: ReviewDraft[] = draftFrom(items).map((draft) =>
      draft.index === 0
        ? { ...draft, when: '2026-11-13T16:59:00Z', topics: ['Bab 4', 'Bab 5'] }
        : draft,
    )

    const body = confirmBody(drafts, items)
    expect(body.items[0]).toEqual({
      index: 0,
      keep: true,
      when: '2026-11-13T16:59:00Z',
      topics: ['Bab 4', 'Bab 5'],
    })
    for (const sent of body.items) {
      expect(Object.keys(sent).every((key) =>
        ['index', 'keep', 'title', 'when', 'rrule', 'topics', 'source_ids'].includes(key),
      )).toBe(true)
    }
  })
})
