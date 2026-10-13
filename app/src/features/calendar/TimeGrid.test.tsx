import { describe, expect, it, vi } from 'vitest'
import { screen, within } from '@testing-library/react'

import type { EventRow } from '@/api/queries/events'
import { renderWithProviders } from '@/test/render'
import { localWeek, placeEvents } from './calendar'
import { TimeGrid, eventCellId } from './TimeGrid'

/**
 * The time grid's four load-bearing behaviours, asserted on the rendered tree.
 *
 * What this file deliberately does **not** assert is the date maths: which day an
 * event falls on is `calendar.test.ts`'s, against a fixed clock and an explicit
 * zone. What a rendered grid can prove — and what a screenshot cannot — is:
 *
 *  - an event with **no duration** is in the all-day row and not in a day's list,
 *    which is the honest reading of a schema with no `all_day` column (A199);
 *  - a timed event is in its own day's row, with its time as text, and its block is
 *    positioned by its start and sized by its duration;
 *  - the current-time line is on today's row only, and its position is a function of
 *    the `now` prop and never of a clock read at render;
 *  - an external row's accessible name says it is read-only and the student's own
 *    row has no `aria-label` at all, so the state is in the tree rather than only in
 *    a glyph.
 *
 * `now` is a **fixed instant**, which is the whole reason the component takes it as
 * a prop: a test that read the clock could not assert a percentage, and neither
 * could the screen show the same line twice.
 */

const ZONE = 'Asia/Jakarta'

/** 10:00 in Jakarta on Wednesday 10 June 2026 — inside the 07:00–22:00 window. */
const NOW = new Date('2026-06-10T03:00:00Z')

const OWN_EVENT: EventRow = {
  id: 'e-own',
  workspace_id: 'w1',
  kind: 'class',
  title: 'Struktur Data',
  // 07:00–08:30 local: the block starts at the window's left edge and is 90 of the
  // window's 900 minutes, so its bar is 10% wide from 0%.
  starts_at: '2026-06-10T00:00:00Z',
  ends_at: '2026-06-10T01:30:00Z',
  source: 'app',
  task_id: null,
}

/** The next day, and not the student's own row: a subscribed calendar's event. */
const EXTERNAL_EVENT: EventRow = {
  id: 'e-ext',
  workspace_id: null,
  kind: 'external',
  title: 'Rapat himpunan',
  starts_at: '2026-06-11T02:00:00Z',
  ends_at: '2026-06-11T03:00:00Z',
  source: 'ics',
  task_id: null,
}

/** `ends_at === starts_at`: how a point event — a deadline — is stored. */
const POINT_EVENT: EventRow = {
  id: 'e-point',
  workspace_id: null,
  kind: 'deadline',
  title: 'Tenggat laporan',
  starts_at: '2026-06-12T09:00:00Z',
  ends_at: '2026-06-12T09:00:00Z',
  source: 'app',
  task_id: null,
}

const EVENTS = [OWN_EVENT, EXTERNAL_EVENT, POINT_EVENT]

const WORKSPACE_NAMES = { w1: 'Jaringan Komputer' }

function render(selected: string | null = null) {
  const buckets = placeEvents(EVENTS, localWeek(NOW, ZONE), ZONE)
  const onSelect = vi.fn()
  const result = renderWithProviders(
    <TimeGrid
      buckets={buckets}
      zone={ZONE}
      todayKey="2026-06-10"
      now={NOW}
      selected={selected}
      onSelect={onSelect}
      workspaceNames={WORKSPACE_NAMES}
      detail={(event) => <p>{`detail of ${event.title}`}</p>}
    />,
  )
  const tbody = result.container.querySelector('tbody')
  if (!(tbody instanceof HTMLTableSectionElement)) throw new Error('the grid must render a tbody')
  return { ...result, tbody, onSelect }
}

/** The row an event's control is in, by its id — never by the copy around it. */
function rowOf(eventId: string): HTMLTableRowElement {
  const row = document.getElementById(eventCellId(eventId))?.closest('tr')
  if (!(row instanceof HTMLTableRowElement)) throw new Error(`no row for ${eventId}`)
  return row
}

/** The block bar beside an event's control: the position-and-duration signal. */
function barOf(eventId: string): HTMLElement {
  const bar = document
    .getElementById(eventCellId(eventId))
    ?.closest('li')
    ?.querySelector('span[aria-hidden="true"].bg-primary')
  if (!(bar instanceof HTMLElement)) throw new Error(`no bar for ${eventId}`)
  return bar
}

describe('the time grid', () => {
  it('names itself with the week it spans', () => {
    render()
    // The caption is `sr-only`, so this is also the assertion that the table has an
    // accessible name at all — the heading above the grid is not the table's name.
    expect(screen.getByRole('table', { name: /8 Jun to 14 Jun/ })).toBeInTheDocument()
  })

  it('puts a point event in the all-day row and in no day’s list', () => {
    const { tbody } = render()
    const row = rowOf('e-point')

    // The all-day row is the first row of the body, before any day.
    expect(tbody.rows[0]).toBe(row)
    // And its control is in no day's row: a deadline has no start time to be placed
    // at, so rendering it in Friday's list would be a time the data does not carry.
    const cell = document.getElementById(eventCellId('e-point'))
    for (let index = 1; index < tbody.rows.length; index += 1) {
      expect(tbody.rows[index]?.contains(cell)).toBe(false)
    }
  })

  it('puts a timed event in its own day’s row with its time, positioned by start and duration', () => {
    const { tbody } = render()
    const row = rowOf('e-own')

    // Wednesday 10 June, and not the all-day row.
    expect(row).not.toBe(tbody.rows[0])
    expect(row.textContent).toContain('Wednesday')
    expect(row.textContent).toContain('Struktur Data')
    // The time is text, because a screen reader cannot measure a bar.
    expect(within(row).getByText('07:00–08:30')).toBeInTheDocument()

    // 07:00 is the window's left edge and 90 minutes of 900 is 10% of its width.
    const bar = barOf('e-own')
    expect(bar.style.marginLeft).toBe('0%')
    expect(bar.style.width).toBe('10%')
  })

  it('renders the current-time line on today’s row only', () => {
    render()

    const today = rowOf('e-own')
    expect(today.textContent).toContain('Wednesday')
    const line = today.querySelector<HTMLElement>('span.bg-border-strong')
    expect(line).not.toBeNull()
    // **The rule spans the remaining width; it is not offset by the hour.** It used to
    // carry `marginLeft: '20%'` here, derived from the window's 900 minutes, and that is
    // the defect the 200%-text gate found: a percentage margin is not shrinkable, so late
    // in the day it claimed most of the row, left the "Now" label about 7px, and wrapped
    // it onto three lines. The design's own current-time row (`411:26931`) is
    // `label — flex-1 rule — label` with no positional offset, and a day's events are
    // listed in order rather than placed on a time axis, so the offset was a claim this
    // layout does not make. Asserted as the property: no inline offset, and the rule is
    // the flexible item that gives way.
    expect(line?.style.marginLeft).toBe('')
    expect(line?.className).toContain('flex-1')
    expect(line?.className).toContain('min-w-0')

    // Thursday's row has no line: "now" is not on a day that is not today.
    const other = rowOf('e-ext')
    expect(other.textContent).toContain('Thursday')
    expect(other.querySelector('span.bg-border-strong')).toBeNull()
  })

  it('announces an external event as read-only and leaves the student’s own row unlabelled', () => {
    render()

    const external = screen.getByRole('button', { name: /Rapat himpunan at .*, read-only/ })
    expect(external).toBeInTheDocument()
    // The lock is `aria-hidden`, so the label is the only place the state exists in
    // the accessibility tree — which is why it must be there.
    expect(within(external).getByText('Read-only')).toBeInTheDocument()

    const own = document.getElementById(eventCellId('e-own'))
    expect(own?.getAttribute('aria-label')).toBeNull()
  })

  it('marks a course with a marker and an initial chip, and an event with no workspace with neither', () => {
    render()

    const own = rowOf('e-own')
    expect(own.querySelector('.course-marker')).not.toBeNull()
    // The initial of the workspace's name, and the name itself: the colour is never
    // the only signal.
    expect(within(own).getByText('J')).toBeInTheDocument()
    expect(within(own).getByText('Jaringan Komputer')).toBeInTheDocument()

    const external = rowOf('e-ext')
    expect(external.querySelector('.course-marker')).toBeNull()
  })

  it('opens an event and renders its detail inside that event’s own day', () => {
    const { tbody } = render('e-own')

    const cell = document.getElementById(eventCellId('e-own'))
    expect(cell?.getAttribute('aria-expanded')).toBe('true')
    // The detail row follows its day's row rather than replacing it or leaving the
    // table, so the keyboard user's place on the grid is unchanged.
    const detail = screen.getByText('detail of Struktur Data')
    const row = detail.closest('tr')
    expect(row).not.toBeNull()
    expect(rowOf('e-own').nextElementSibling).toBe(row)
    expect(tbody.contains(row)).toBe(true)
  })
})
