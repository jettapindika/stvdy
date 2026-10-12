import { describe, expect, it } from 'vitest'

import {
  NIL_GOAL_ID,
  PINNED_TOPIC,
  acceptBody,
  blockSubject,
  daysUntil,
  gridDays,
  isKeptBlock,
  isPinned,
  localMinutesOfDay,
  moveBlock,
  plannedMinutes,
  todayKey,
  warningCopy,
  weekDays,
  workloadFor,
  type MoveContext,
  type ProposedBlock,
  type StudySettings,
} from './plan'

/**
 * The planner's arithmetic, against fixed instants and named zones.
 *
 * Every test here passes `now` and the zone in, because the module reads no clock:
 * that is the whole reason the arithmetic is in a module of its own rather than
 * inside the screen. The two Jakarta cases are the phase's own gate 6 — the
 * roadmap singles this phase out for `Asia/Jakarta`, and the failure they catch is
 * the silent one, where a UTC-based implementation puts an early-morning exam on
 * the previous day and every assertion that only looks at "a date" still passes.
 */

const SETTINGS: StudySettings = { start_hour: 8, end_hour: 22, max_study_min_per_day: 240 }

const CONTEXT: MoveContext = { timeZone: 'Asia/Jakarta', settings: SETTINGS, busy: [] }

function block(overrides: Partial<ProposedBlock> = {}): ProposedBlock {
  return {
    starts_at: '2026-11-16T01:00:00Z', // 08:00 Jakarta
    ends_at: '2026-11-16T01:50:00Z',
    goal_id: '11111111-1111-1111-1111-111111111111',
    topic: 'Jaringan',
    kind: 'NewMaterial',
    pinned: false,
    ...overrides,
  }
}

describe('the local day boundary', () => {
  it('puts an early-morning Jakarta instant on Jakarta’s day, not UTC’s', () => {
    // 06:00 on the 16th in Jakarta is 23:00Z on the **15th**. A UTC-based
    // implementation reads that instant as the 15th, and the student's exam moves
    // a day earlier with nothing on screen to say so.
    const exam = new Date('2026-11-15T23:00:00Z')
    expect(exam.toISOString().slice(0, 10)).toBe('2026-11-15')
    expect(todayKey(exam, 'Asia/Jakarta')).toBe('2026-11-16')

    // And the other half of the pair, which a UTC implementation gets right by
    // accident: 23:00 Jakarta on the 16th is 16:00Z on the 16th.
    const evening = new Date('2026-11-16T16:00:00Z')
    expect(todayKey(evening, 'Asia/Jakarta')).toBe('2026-11-16')
    expect(localMinutesOfDay(evening, 'Asia/Jakarta')).toBe(23 * 60)
  })

  it('places a block on the local day it starts on, which is the day the planner loads', () => {
    // A block at 06:00 Jakarta: the grid's own window starts at 07:00, so it is
    // clamped for display — but it is still the 16th's block, and `plannedMinutes`
    // counts it against the 16th's limit rather than the 15th's.
    const early = block({
      starts_at: '2026-11-15T23:00:00Z',
      ends_at: '2026-11-15T23:50:00Z',
    })
    const days = weekDays(new Date('2026-11-16T03:00:00Z'), 'Asia/Jakarta')
    expect(days[0]).toBe('2026-11-16')

    const grid = gridDays([early], days, 'Asia/Jakarta')
    expect(grid[0]?.blocks).toHaveLength(1)
    expect(grid[0]?.blocks[0]?.start_min).toBe(7 * 60)
    expect(grid[0]?.blocks[0]?.in_window).toBe(false)
    expect(plannedMinutes([early], '2026-11-16', 'Asia/Jakarta')).toBe(50)
    expect(plannedMinutes([early], '2026-11-15', 'Asia/Jakarta')).toBe(0)
  })

  it('counts days to a deadline in local calendar days, across a DST day', () => {
    const now = new Date('2026-11-02T01:00:00Z')

    // Jakarta has no DST, so this is the plain case.
    expect(daysUntil('2026-11-16T16:00:00Z', now, 'Asia/Jakarta')).toBe(14)

    // London's 2026-03-29 is 23 hours long and 2026-10-25 is 25: a millisecond
    // difference would round one of these to the wrong day count.
    const beforeSpring = new Date('2026-03-28T09:00:00Z')
    expect(daysUntil('2026-03-30T09:00:00Z', beforeSpring, 'Europe/London')).toBe(2)
    const beforeAutumn = new Date('2026-10-24T09:00:00Z')
    expect(daysUntil('2026-10-26T09:00:00Z', beforeAutumn, 'Europe/London')).toBe(2)
  })

  it('names the week Monday first, the way the calendar grid does', () => {
    // 2026-11-16 is a Monday, so the week containing it starts on it.
    const days = weekDays(new Date('2026-11-18T03:00:00Z'), 'Asia/Jakarta')
    expect(days[0]).toBe('2026-11-16')
    expect(days[6]).toBe('2026-11-22')
  })
})

describe('the drag-adjust rules', () => {
  it('refuses a drag that would overlap a class before the request is sent', () => {
    // The grid knows the busy intervals it was given, so the refusal is local and
    // immediate. The server checks again (A264) — this is the courtesy, not the
    // control.
    const lecture = {
      starts_at: '2026-11-16T07:00:00Z', // 14:00 Jakarta
      ends_at: '2026-11-16T08:40:00Z',
    }
    const context: MoveContext = { ...CONTEXT, busy: [lecture] }
    const blocks = [block()]

    // 08:00 → 14:00 would land the block inside the lecture.
    const moved = moveBlock(blocks, 0, 6 * 60, context)
    expect(moved).toEqual({ ok: false, reason: 'busy' })

    // A drag that clears it is allowed, and the original list is untouched.
    const clear = moveBlock(blocks, 0, 5 * 60, context)
    expect(clear.ok).toBe(true)
    expect(blocks[0]?.starts_at).toBe('2026-11-16T01:00:00Z')
  })

  it('refuses a drag outside the study window and one past the daily maximum', () => {
    const blocks = [block()]
    // 08:00 Jakarta minus two hours is 06:00, before the 08:00 window.
    expect(moveBlock(blocks, 0, -120, CONTEXT)).toEqual({ ok: false, reason: 'window' })

    // 240 minutes already planned on the day, and this block is another 50.
    const full = [
      block({ starts_at: '2026-11-16T00:00:00Z', ends_at: '2026-11-16T04:00:00Z' }),
      block({ starts_at: '2026-11-16T05:00:00Z', ends_at: '2026-11-16T05:50:00Z' }),
    ]
    expect(moveBlock(full, 1, 0, CONTEXT)).toEqual({ ok: false, reason: 'daily_max' })
  })

  it('marks a dragged block as pinned, so a replan keeps it exactly', () => {
    // This is the one place the UI can create a state the planner must then honour
    // (contract § 6 step 7): a moved block is the student's own placement, and a
    // replan that moved it back would silently undo their adjustment.
    const moved = moveBlock([block()], 0, 60, CONTEXT)
    expect(moved.ok).toBe(true)
    if (!moved.ok) return

    const after = moved.blocks[0]
    expect(after?.pinned).toBe(true)
    expect(isPinned(after as ProposedBlock)).toBe(true)
    // The instant, not the string: `toISOString` writes milliseconds and the server's
    // own serializer writes whole seconds. Both are RFC 3339 and both parse to the
    // same instant, which is what the accept path compares.
    expect(new Date(after?.starts_at ?? '').toISOString()).toBe('2026-11-16T02:00:00.000Z')
    // It keeps its own goal and topic: it is not yet an event, so accept writes it
    // at the dragged time rather than resolving it as a kept block.
    expect(after?.goal_id).toBe(block().goal_id)
    expect(isKeptBlock(after as ProposedBlock)).toBe(false)
  })
})

describe('the proposal preview', () => {
  it('never prints the nil goal id as a subject', () => {
    // A pinned block comes back from the server with `goal_id: Uuid::nil()` and the
    // topic "Pinned" (A265), so a preview that printed the goal id would show a raw
    // UUID where the student expects a subject.
    const kept = block({ goal_id: NIL_GOAL_ID, topic: PINNED_TOPIC, pinned: true })
    const subject = blockSubject(kept, () => undefined)
    expect(subject).toEqual({ kind: 'kept' })
    expect(isKeptBlock(kept)).toBe(true)

    // A block with a goal renders the goal's title, and falls back to the topic the
    // planner chose when the goal is not in the list the screen holds.
    expect(blockSubject(block(), () => 'UAS Jaringan')).toEqual({
      kind: 'goal',
      text: 'UAS Jaringan',
    })
    expect(blockSubject(block(), () => undefined)).toEqual({ kind: 'goal', text: 'Jaringan' })
  })

  it('sends a kept block back as null plus the pinned topic, and omits the client’s own flag', () => {
    // `AcceptBlock` is `deny_unknown_fields`, so an extra field is a 400 for the
    // whole batch: the drag marker is the client's bookkeeping and the server reads
    // only the times, the goal and the topic.
    const body = acceptBody([
      block(),
      block({
        goal_id: NIL_GOAL_ID,
        topic: PINNED_TOPIC,
        pinned: true,
        starts_at: '2026-11-17T01:00:00Z',
        ends_at: '2026-11-17T01:50:00Z',
      }),
      block({ kind: 'Review', topic: 'Review' }),
    ])

    expect(body.blocks[0]).toEqual({
      starts_at: '2026-11-16T01:00:00Z',
      ends_at: '2026-11-16T01:50:00Z',
      goal_id: block().goal_id,
      topic: 'Jaringan',
      kind: 'new_material',
    })
    expect(body.blocks[1]?.goal_id).toBeNull()
    expect(body.blocks[1]?.topic).toBe(PINNED_TOPIC)
    expect(body.blocks[2]?.kind).toBe('review')
    for (const sent of body.blocks) {
      expect(Object.keys(sent).sort()).toEqual(
        ['starts_at', 'ends_at', 'goal_id', 'kind', 'topic'].sort(),
      )
    }
  })
})

describe('the workload meter', () => {
  it('counts minutes, not blocks', () => {
    // The spec's meter is "planned minutes today vs the user's daily limit". Four
    // short blocks and one long one are one block versus four and 200 versus 200
    // minutes — a count of blocks cannot be compared with a limit in minutes.
    const day = '2026-11-16'
    const short = [
      block({ starts_at: '2026-11-16T01:00:00Z', ends_at: '2026-11-16T01:20:00Z' }),
      block({ starts_at: '2026-11-16T02:00:00Z', ends_at: '2026-11-16T02:20:00Z' }),
      block({ starts_at: '2026-11-16T03:00:00Z', ends_at: '2026-11-16T03:20:00Z' }),
    ]
    expect(short).toHaveLength(3)
    expect(plannedMinutes(short, day, 'Asia/Jakarta')).toBe(60)

    const one = [block({ starts_at: '2026-11-16T01:00:00Z', ends_at: '2026-11-16T02:00:00Z' })]
    expect(one).toHaveLength(1)
    expect(plannedMinutes(one, day, 'Asia/Jakarta')).toBe(60)

    // And the meter's own numbers, against the student's limit rather than a
    // capacity the client recomputed.
    expect(workloadFor(short, day, 'Asia/Jakarta', 240)).toEqual({
      planned_min: 60,
      limit_min: 240,
      over: false,
    })
    const heavy = [block({ starts_at: '2026-11-16T01:00:00Z', ends_at: '2026-11-16T06:00:00Z' })]
    expect(workloadFor(heavy, day, 'Asia/Jakarta', 240).over).toBe(true)
  })

  it('leaves a block that starts on another local day out of the day’s total', () => {
    // The boundary is Jakarta's: 23:30Z on the 15th is 06:30 on the 16th.
    const lateNight = block({
      starts_at: '2026-11-15T23:30:00Z',
      ends_at: '2026-11-16T00:20:00Z',
    })
    expect(plannedMinutes([lateNight], '2026-11-16', 'Asia/Jakarta')).toBe(50)
    expect(plannedMinutes([lateNight], '2026-11-15', 'Asia/Jakarta')).toBe(0)
  })
})

describe('the warnings', () => {
  it('renders both variants as a sentence with every value filled', () => {
    // Gate 5(d): a state is never carried by colour alone. The overloaded warning
    // carries the count of days, because a sentence naming seven dates is not a
    // sentence — the dates are on the grid.
    const overloaded = warningCopy({ Overloaded: { days: ['2026-11-16', '2026-11-17'] } }, () => '')
    expect(overloaded).toEqual({ key: 'planner.overloaded', count: 2 })

    const past = warningCopy(
      { PastDeadline: { goal_id: '22222222-2222-2222-2222-222222222222' } },
      () => 'UAS Kripto',
    )
    expect(past).toEqual({ key: 'planner.pastDeadline', count: 0, goal: 'UAS Kripto' })

    // A goal the screen cannot name still produces a sentence rather than the raw
    // id, for the same reason a pinned block is not labelled with a nil UUID.
    const unknown = warningCopy(
      { PastDeadline: { goal_id: '22222222-2222-2222-2222-222222222222' } },
      () => undefined,
    )
    expect(unknown.goal).toBe('')
  })
})
