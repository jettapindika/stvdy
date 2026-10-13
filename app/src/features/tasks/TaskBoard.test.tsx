import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeAll, describe, expect, it, vi } from 'vitest'

import i18n from '@/i18n'
import { renderWithProviders } from '@/test/render'
import type { TaskRow } from './order'
import { TaskBoard } from './TaskBoard'

/**
 * The board is a view of the same rows the list shows, so what is asserted here
 * is that it reads them the same way: three status columns, one level of
 * subtasks, a count and an effort figure that come from the rows and are omitted
 * rather than zero-filled when absent, and controls that call the panel's own
 * handlers instead of writing themselves.
 *
 * The copy these assertions use is the contract's own (the redesign's keys), set
 * here so the test does not depend on the locale files being merged yet. The
 * values are the same strings the app ships.
 */
beforeAll(() => {
  i18n.addResourceBundle(
    'en',
    'translation',
    {
      tasks: {
        status: { todo: 'To do', doing: 'In progress', done: 'Done' },
        priority: { high: 'High', medium: 'Medium', low: 'Low' },
        subtasks: { count: '{{done}} / {{total}} subtasks', none: 'No subtasks yet' },
        board: {
          effortOpen: '{{duration}} · total estimated effort',
          effortDone: '{{duration}} · completed effort',
          add: 'Add task',
          empty: 'Nothing here yet.',
          doneNote: 'Finished work stays here.',
          doneNoteHint: 'Completing a task tracks the work, not mastery.',
        },
        breakdown: { action: 'Break down' },
        markDone: 'Mark done',
        reopen: 'Reopen',
        done: 'Done: {{title}}',
      },
      planner: {
        duration: { hm: '{{hours}}h {{minutes}}m', h: '{{hours}}h', m: '{{minutes}}m' },
      },
    },
    true,
    true,
  )
})

const base = (over: Partial<TaskRow>): TaskRow => ({
  id: 'id',
  workspace_id: null,
  parent_id: null,
  title: 'Task',
  notes: null,
  due_at: null,
  estimate_min: null,
  priority: 2,
  status: 'todo',
  position: 0,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
  deleted_at: null,
  ...over,
})

// No row in the To do column carries an estimate, so its effort line must be
// absent — not `0m`. The In progress row does, so the same line renders there.
const TODO = base({ id: 'a', title: 'Beli buku', status: 'todo', position: 0 })
const PARENT = base({ id: 'p1', title: 'Crypto assignment', status: 'todo', position: 1 })
const CHILD_DONE_1 = base({ id: 'c1', parent_id: 'p1', title: 'Baca bab 1', status: 'done', position: 0 })
const CHILD_DONE_2 = base({ id: 'c2', parent_id: 'p1', title: 'Latihan kuis', status: 'done', position: 1 })
const CHILD_TODO = base({ id: 'c3', parent_id: 'p1', title: 'Tulis laporan', status: 'todo', position: 2 })
const DOING = base({ id: 'b', title: 'Latihan soal', status: 'doing', estimate_min: 60, priority: 1 })
const DONE = base({ id: 'c', title: 'Skripsi', status: 'done', priority: 3 })

const ROWS = [TODO, PARENT, CHILD_DONE_1, CHILD_DONE_2, CHILD_TODO, DOING, DONE]

function renderBoard(
  rows: TaskRow[] = ROWS,
  courses: Record<string, string> = {},
  handlers: {
    onToggle?: (task: TaskRow) => void
    onBreakdown?: (taskId: string) => void
    onNewTask?: () => void
  } = {},
  courseColors: ReadonlyMap<string, number> = new Map(),
) {
  const props = {
    rows,
    timeZone: 'Asia/Jakarta',
    now: new Date('2026-10-12T10:00:00Z'),
    courses,
    courseColors,
    onToggle: handlers.onToggle ?? (() => {}),
    onBreakdown: handlers.onBreakdown ?? (() => {}),
    onNewTask: handlers.onNewTask ?? (() => {}),
  }
  return { ...renderWithProviders(<TaskBoard {...props} />, { route: '/planner' }), props }
}

/** The column whose heading carries `name`, so a row can be scoped to it. */
function column(name: string): HTMLElement {
  const heading = screen.getByRole('heading', { name })
  const section = heading.closest('section')
  if (section === null) throw new Error(`no column for ${name}`)
  return section as HTMLElement
}

/**
 * The card whose title carries `name`.
 *
 * Found through the heading rather than through the card's own accessible name:
 * an `article` labelled with the same string as the `h4` inside it announces the
 * title twice, so the card is deliberately unnamed and the heading is what names
 * it — the same way a screen reader reads it.
 */
function card(name: string): HTMLElement {
  const heading = screen.getByRole('heading', { name })
  const article = heading.closest('article')
  if (article === null) throw new Error(`no card for ${name}`)
  return article as HTMLElement
}

describe('the three columns', () => {
  it('renders a heading and a count for each, in To do / In progress / Done order', () => {
    renderBoard()

    const headings = screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)
    expect(headings).toEqual(['To do', 'In progress', 'Done'])

    expect(within(column('To do')).getByText('2')).toBeInTheDocument()
    expect(within(column('In progress')).getByText('1')).toBeInTheDocument()
    expect(within(column('Done')).getByText('1')).toBeInTheDocument()
  })

  it('puts a done row under Done and not under To do', () => {
    renderBoard()

    expect(within(column('Done')).getByText('Skripsi')).toBeInTheDocument()
    expect(within(column('To do')).queryByText('Skripsi')).toBeNull()
  })

  it('keeps a zero count and names the cause when a column is empty', () => {
    renderBoard([])

    expect(within(column('To do')).getByText('0')).toBeInTheDocument()
    expect(within(column('In progress')).getByText('0')).toBeInTheDocument()
    expect(within(column('Done')).getByText('0')).toBeInTheDocument()
    expect(within(column('To do')).getByText('Nothing here yet.')).toBeInTheDocument()
    // No rows in Done, so the completion note is absent too.
    expect(screen.queryByText('Finished work stays here.')).toBeNull()
  })
})

describe('the priority chip', () => {
  it('always carries its word, for each of the three priorities', () => {
    renderBoard()

    expect(within(card('Latihan soal')).getByText('High')).toBeInTheDocument()
    expect(within(card('Beli buku')).getByText('Medium')).toBeInTheDocument()
    expect(within(card('Skripsi')).getByText('Low')).toBeInTheDocument()
  })
})

describe('the effort line', () => {
  it('is omitted, not shown as 0m, when no row in the column carries an estimate', () => {
    renderBoard()

    // Every To do row has `estimate_min: null`, so `estimateTotal` is null and the
    // line is absent rather than reading `0m · total estimated effort`.
    const todo = column('To do')
    expect(within(todo).queryByText(/total estimated effort/)).toBeNull()
    expect(within(todo).queryByText(/0m/)).toBeNull()

    // The In progress row does carry one, so the same line renders there.
    expect(
      within(column('In progress')).getByText('1h · total estimated effort'),
    ).toBeInTheDocument()
  })
})

describe('a card with no subtasks', () => {
  it('shows the break-down button and calls onBreakdown with the task id', async () => {
    const user = userEvent.setup()
    const onBreakdown = vi.fn()
    renderBoard(ROWS, {}, { onBreakdown })

    await user.click(within(card('Beli buku')).getByRole('button', { name: 'Break down' }))

    expect(onBreakdown).toHaveBeenCalledWith('a')
  })
})

describe('a card with subtasks', () => {
  it('shows the count and prefixes the done children, one level deep', () => {
    renderBoard()

    const parent = card('Crypto assignment')
    expect(within(parent).getByText('2 / 3 subtasks')).toBeInTheDocument()
    expect(within(parent).getByText('Done: Baca bab 1')).toBeInTheDocument()
    expect(within(parent).getByText('Done: Latihan kuis')).toBeInTheDocument()
    // A live child keeps its own title, un-prefixed.
    expect(within(parent).getByText('Tulis laporan')).toBeInTheDocument()
    // The break-down button belongs to the no-subtask case, so it is not here.
    expect(within(parent).queryByRole('button', { name: 'Break down' })).toBeNull()
  })
})

describe('the mark-done control', () => {
  it('calls onToggle with the row it belongs to', async () => {
    const user = userEvent.setup()
    const onToggle = vi.fn()
    renderBoard(ROWS, {}, { onToggle })

    await user.click(within(card('Beli buku')).getByRole('button', { name: 'Mark done' }))

    expect(onToggle).toHaveBeenCalledWith(TODO)
  })

  it('offers Reopen on a row that is already done', async () => {
    const user = userEvent.setup()
    const onToggle = vi.fn()
    renderBoard(ROWS, {}, { onToggle })

    await user.click(within(card('Skripsi')).getByRole('button', { name: 'Reopen' }))

    expect(onToggle).toHaveBeenCalledWith(DONE)
  })
})

describe('the course line', () => {
  it('names the workspace’s course, and renders nothing when there is none', () => {
    const withCourse = base({ id: 'w1', title: 'Tugas kripto', workspace_id: 'w1' })
    renderBoard([withCourse, TODO], { w1: 'CS 304 · Cryptography' })

    expect(screen.getByText('CS 304 · Cryptography')).toBeInTheDocument()
    // The row with no workspace renders no invented code and no placeholder.
    expect(within(card('Beli buku')).queryByText(/CS 304/)).toBeNull()
  })
})

describe('the course hue', () => {
  it('takes the server’s index, not the id hash, so a card matches its calendar entry', () => {
    const withCourse = base({ id: 'w1', title: 'Tugas kripto', workspace_id: 'w1' })
    // `w1` hashes to `sky`; the server says index 3, which is `amber`. The two
    // disagree, so this fails if the card falls back to the hash the board used to
    // compute — the defect where one course wore two colours.
    renderBoard([withCourse], {}, {}, new Map([['w1', 3]]))

    expect(card('Tugas kripto').style.getPropertyValue('--course-marker')).toBe(
      'var(--course-amber)',
    )
  })

  it('falls back to the id hash for a workspace the screen has not loaded', () => {
    const withCourse = base({ id: 'w1', title: 'Tugas kripto', workspace_id: 'w1' })
    // No index for `w1`: some stable colour beats none, so the hash stands in.
    renderBoard([withCourse])

    expect(card('Tugas kripto').style.getPropertyValue('--course-marker')).toBe(
      'var(--course-sky)',
    )
  })
})

describe('the create control', () => {
  it('is offered on To do only, and calls onNewTask', async () => {
    const user = userEvent.setup()
    const onNewTask = vi.fn()
    renderBoard(ROWS, {}, { onNewTask })

    // The create form always mints a `todo` row, so the control lives in one column.
    expect(within(column('To do')).getByRole('button', { name: 'Add task' })).toBeInTheDocument()
    expect(within(column('In progress')).queryByRole('button', { name: 'Add task' })).toBeNull()

    await user.click(within(column('To do')).getByRole('button', { name: 'Add task' }))
    expect(onNewTask).toHaveBeenCalledTimes(1)
  })
})

describe('the destructive action', () => {
  it('is not on the board', () => {
    renderBoard()

    // Delete stays in the list, so a mis-tap in the dense board cannot tombstone a
    // parent and its children (A197).
    expect(screen.queryByRole('button', { name: 'Delete' })).toBeNull()
  })
})
