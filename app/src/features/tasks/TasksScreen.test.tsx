import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { fakeCache } from '@/test/syncStore'
import { TasksScreen } from './TasksScreen'

/**
 * The Tasks screen, and the one rule its break-down turns on.
 *
 * A proposal is a proposal: the route saves nothing (A195) and this screen writes
 * nothing until Accept, which is N ordinary creates with the parent and the
 * position the model proposed (A196). A screen that saved the subtasks on receipt
 * would pass every other assertion here while filling a student's list with work
 * they did not agree to — which is why the "writes nothing" assertions come before
 * the accept ones.
 */

const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'pro',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

const USAGE = {
  plan: 'pro',
  name: 'Pro',
  subscription: { status: 'active', period_end: '2026-11-11T00:00:00Z' },
  credits: { monthly: 500, purchased: 0, total: 500 },
  storage_bytes: 0,
  limits: {},
  features: {
    export_formats: ['md'],
    task_ai_breakdown: true,
    quick_capture: true,
    google_calendar: true,
    planner: 'manual',
    agent_actions: true,
    reminder_channels: ['web', 'push'],
    model_tier: 'cheapest',
    byok: false,
    advanced_analytics: false,
  },
  costs: {},
  provider_cost: { paused: false, resets_at: null },
}

const PARENT = {
  id: 'p1',
  workspace_id: null,
  parent_id: null,
  title: 'Selesaikan tugas kripto',
  notes: null,
  due_at: '2026-10-16T09:00:00+07:00',
  estimate_min: 120,
  priority: 2,
  status: 'todo',
  position: 0,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
  deleted_at: null,
}

const CHILD_DONE = { ...PARENT, id: 'c1', parent_id: 'p1', title: 'Baca bab 1', status: 'done', position: 0 }
const CHILD_TODO = { ...PARENT, id: 'c2', parent_id: 'p1', title: 'Latihan soal', status: 'todo', position: 1 }

const PROPOSAL = {
  subtasks: [
    { title: 'Baca bab 1', estimate_min: 30 },
    { title: 'Latihan soal', estimate_min: 45 },
  ],
  total_estimate_min: 75,
  warnings: [],
}

function setup(rows: Record<string, unknown[]>, granted = true) {
  const cache = fakeCache(rows)
  const fake = installFakePlatform('macos', { sync: { read: cache.read } })
  const calls = mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/usage') {
      return jsonResponse(200, { ...USAGE, features: { ...USAGE.features, task_ai_breakdown: granted } })
    }
    if (path === '/tasks/p1/breakdown') {
      return jsonResponse(200, {
        proposal: PROPOSAL,
        credits_spent: 2,
        credits_remaining: 498,
        prompt_version: 'task_breakdown.v1',
      })
    }
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })
  return { fake, calls }
}

beforeEach(() => {
  window.localStorage.clear()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the task list', () => {
  it('shows subtasks under their parent, one level deep, with a done count', async () => {
    setup({ task: [PARENT, CHILD_DONE, CHILD_TODO] })
    renderWithProviders(<TasksScreen />, { route: '/tasks' })

    expect(await screen.findByText('Selesaikan tugas kripto')).toBeInTheDocument()
    // The children are listed, and the parent carries the count rather than a
    // percentage: the number of done children is the fact.
    expect(screen.getByText('1 of 2 subtasks done')).toBeInTheDocument()
    const done = screen.getByText('Done: Baca bab 1')
    expect(done).toBeInTheDocument()
    // One level: a child is not rendered as its own parent's row with its own
    // break-down control.
    expect(screen.queryByText('Done: Done: Baca bab 1')).toBeNull()
  })
})

describe('the break-down proposal', () => {
  async function openPanel() {
    const user = userEvent.setup()
    const harness = setup({ task: [PARENT] })
    renderWithProviders(<TasksScreen />, { route: '/tasks' })
    await screen.findByText('Selesaikan tugas kripto')
    await user.click(screen.getByRole('button', { name: 'Break down' }))
    return { user, ...harness }
  }

  it('labels the proposal as suggested and writes only on confirm', async () => {
    const { user, fake } = await openPanel()

    // The cost is on the control before it runs, from the matrix (gate 7d).
    const run = screen.getByRole('button', { name: /Suggest subtasks/ })
    expect(run).toHaveTextContent('2 credits')
    await user.click(run)

    expect(await screen.findByText('Suggested subtasks')).toBeInTheDocument()
    expect(screen.getByText('Nothing is added until you confirm.')).toBeInTheDocument()
    // Nothing was written by the proposal itself.
    expect(fake.sync.writes).toEqual([])

    await user.click(screen.getByRole('button', { name: 'Add these subtasks' }))

    expect(fake.sync.writes).toHaveLength(2)
    expect(fake.sync.writes[0]?.entity).toBe('task')
    expect(fake.sync.writes[0]?.payload.parent_id).toBe('p1')
    // The position is the row's index in the proposal, which is the order the
    // model wrote (A196).
    expect(fake.sync.writes.map((write) => write.payload.position)).toEqual([0, 1])
    expect(fake.sync.writes.map((write) => write.payload.title)).toEqual([
      'Baca bab 1',
      'Latihan soal',
    ])
  })

  it('dismissing the proposal writes nothing', async () => {
    const { user, fake } = await openPanel()
    await user.click(screen.getByRole('button', { name: /Suggest subtasks/ }))
    await screen.findByText('Suggested subtasks')

    await user.click(screen.getByRole('button', { name: 'Dismiss' }))

    expect(fake.sync.writes).toEqual([])
    expect(screen.queryByText('Suggested subtasks')).toBeNull()
  })

  it('writes the edited estimate, not the model\u2019s', async () => {
    const { user, fake } = await openPanel()
    await user.click(screen.getByRole('button', { name: /Suggest subtasks/ }))
    await screen.findByText('Suggested subtasks')

    const minutes = screen.getAllByLabelText('Minutes')
    await user.clear(minutes[0] as HTMLElement)
    await user.type(minutes[0] as HTMLElement, '90')

    // The total is recomputed from what is left, not read from the model's own
    // `total_estimate_min`, which described the list it proposed.
    expect(screen.getByText('135 minutes across the subtasks you keep')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Add these subtasks' }))

    expect(fake.sync.writes[0]?.payload.estimate_min).toBe(90)
    expect(fake.sync.writes[1]?.payload.estimate_min).toBe(45)
  })

  it('writes only the subtasks that are kept', async () => {
    const { user, fake } = await openPanel()
    await user.click(screen.getByRole('button', { name: /Suggest subtasks/ }))
    await screen.findByText('Suggested subtasks')

    const boxes = screen.getAllByRole('checkbox')
    await user.click(boxes[1] as HTMLElement)

    await user.click(screen.getByRole('button', { name: 'Add these subtasks' }))

    // The kept row keeps its own position, so unchecking does not renumber the
    // rest and produce an order the student never saw.
    expect(fake.sync.writes).toHaveLength(1)
    expect(fake.sync.writes[0]?.payload.position).toBe(0)
    expect(fake.sync.writes[0]?.payload.title).toBe('Baca bab 1')
  })

  it('locks the assistant on a plan that does not grant it', async () => {
    const user = userEvent.setup()
    const { fake } = setup({ task: [PARENT] }, false)
    renderWithProviders(<TasksScreen />, { route: '/tasks' })
    await screen.findByText('Selesaikan tugas kripto')
    await user.click(screen.getByRole('button', { name: 'Break down' }))

    expect(await screen.findByText('Available on Pro')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Suggest subtasks/ })).toBeNull()
    expect(fake.sync.writes).toEqual([])
  })
})

describe('deleting a parent', () => {
  it('tombstones its live children in the same action', async () => {
    const user = userEvent.setup()
    const { fake } = setup({ task: [PARENT, CHILD_DONE, CHILD_TODO] })
    renderWithProviders(<TasksScreen />, { route: '/tasks' })
    await screen.findByText('Selesaikan tugas kripto')

    await user.click(screen.getByRole('button', { name: 'Delete' }))

    // The server's cascade only fires when it applies the parent's delete, so a
    // device deleting offline has to queue the children itself (A197).
    expect(fake.sync.tombstones.map((row) => row.id).sort()).toEqual(['c1', 'c2', 'p1'])
    expect(fake.sync.tombstones.every((row) => row.entity === 'task')).toBe(true)
  })
})

describe('the create form', () => {
  it('writes one task with an id minted here, so a retry cannot create a second', async () => {
    const user = userEvent.setup()
    const { fake } = setup({ task: [] })
    renderWithProviders(<TasksScreen />, { route: '/tasks' })

    const field = await screen.findByLabelText('What do you have to do?')
    await user.type(field, 'Beli buku')
    await user.click(screen.getByRole('button', { name: 'Add task' }))

    expect(fake.sync.writes).toHaveLength(1)
    const written = fake.sync.writes[0]
    expect(written?.payload.title).toBe('Beli buku')
    expect(typeof written?.id).toBe('string')
    expect(written?.id.length).toBeGreaterThan(0)
  })
})

/**
 * The phone layout, asserted as a source property.
 *
 * jsdom has no layout engine, so `getBoundingClientRect()` in this suite is all
 * zeros and the sideways scroll cannot be reproduced here. What can be asserted is
 * the property that makes it impossible — the technique
 * `components/layout/shellLayout.test.ts` uses for the same reason — and **the
 * number is the browser measurement**, taken in Chromium at 390x844 with 200% text
 * in both themes.
 *
 * The defect: `document.scrollingElement.scrollWidth` was **493** against a 390
 * viewport. `#task-due` is a `<input type="date">` whose *min-content* is 346.6px
 * at 200% text, because Chromium's UA stylesheet gives the date control's shadow
 * DOM an intrinsic width author CSS cannot reach; that minimum propagates to the
 * card (412.6px) and then to the screen root (492.6px), and the root's `width:
 * auto` means fit-content, which cannot go below min-content — so the root stayed
 * 492.6px wide inside a 390px `main`. `w-full` makes the root's width definite
 * instead of content-based, which is what stops *any* wide intrinsic child from
 * resizing the screen. Measured after, both themes: 390/390, and the control
 * intact at 192x44 (100% text) and 244x88 (200%) — not the 59.6px empty box that
 * `max-width: 0` leaves. At 200% the picker indicator does paint over the year's
 * last digit (content box 184.4px, value 168.2px), which is the screen's padding
 * at that scale rather than this screen's to fix; the assertion below is the
 * property, not that number.
 */
describe('the phone layout the browser measured', () => {
  const source = readFileSync(join(__dirname, 'TasksScreen.tsx'), 'utf8')

  it('gives the screen root a definite width, so an intrinsic child cannot widen the page', () => {
    const root = /<div className="([^"]*\bmax-w-3xl\b[^"]*)"/.exec(source)?.[1]
    expect(root, 'the screen root must carry its classes inline').toBeTruthy()
    // The fix, and the only class that carries it: fit-content is what let the
    // date control's 346.6px minimum become a 492.6px page.
    expect(root).toContain('w-full')
    // `mx-auto` stays: it is what centres the column on a desktop, where the
    // measurement is unchanged (768px wide, centred, at a 1280px viewport).
    expect(root).toContain('mx-auto')
  })

  it('keeps the native date control, rather than squeezing it to fit', () => {
    // The alternative was `max-width: 0` on the input, and it is rejected by
    // measurement rather than taste: it drops the page to 390 but leaves the
    // control a **59.6px empty box** with no date and no picker — a worse defect
    // than the scroll. `type="date"` is the platform behaviour this control is for.
    const due = /id="task-due"\s+className="([^"]*)"/.exec(source)?.[1]
    expect(due, 'the due-date input must keep its inline classes').toBeTruthy()
    expect(due).not.toMatch(/\bmax-w-0\b/)
    expect(source).toMatch(/id="task-due"[\s\S]{0,120}type="date"/)
  })
})
