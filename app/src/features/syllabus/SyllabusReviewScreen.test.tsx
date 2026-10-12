import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { screen, within } from '@testing-library/react'
import userEvent, { type UserEvent } from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'
import { SyllabusReviewScreen } from './SyllabusReviewScreen'

/**
 * The review screen, and the two absences that are the phase's honesty test.
 *
 * 1. **Nothing is written until the student confirms.** The test asserts the
 *    request log: after the draft renders there is no `POST` to the confirm route
 *    at all, and the only one that ever appears is the one the Confirm button
 *    sends. A screen that saved on render would look identical in a screenshot.
 * 2. **No confidence number reaches the screen.** The server sends none (A261) —
 *    it sends a pre-ordered list and a `needs_check` flag — and this test asserts
 *    the rendered text contains no score even when the body tries to carry one,
 *    because the one field that must never render is the one a future server
 *    could start sending again.
 *
 * The evidence quote is asserted directly: it is what lets the student check the
 * item, so an item whose quote is missing is an item they cannot verify.
 */

const USER = {
  id: 'u1',
  email: 'student@example.test',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'pro',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  created_at: '2026-10-11T08:00:00Z',
}

/**
 * The draft body, with the two items the ordering test needs.
 *
 * `confidence` is present on the wire **on purpose**, in this fixture only, so the
 * assertion that no score is rendered has something to catch. The shipped server
 * does not send it; a fixture without it would make the assertion vacuous.
 */
const DRAFT = {
  draft_id: 'd1',
  source_id: 's1',
  status: 'ready',
  error: null,
  course: { name: 'Jaringan Komputer', code: 'IF2230', instructor: 'Dr. Sari' },
  items: [
    {
      index: 0,
      kind: 'deadline',
      title: 'Tugas 1',
      evidence: 'Tugas 1 dikumpulkan 12 Nov',
      needs_check: true,
      when: '2026-11-12T16:59:00Z',
      rrule: null,
      start_time: null,
      end_time: null,
      topics: ['Graphs'],
      weight_percent: 15,
      confidence: 0.4,
    },
    {
      index: 1,
      kind: 'exam',
      title: 'UAS',
      evidence: 'UAS 15 Desember 08:00',
      needs_check: false,
      when: '2026-12-15T01:00:00Z',
      rrule: null,
      start_time: null,
      end_time: null,
      topics: [],
      weight_percent: null,
      confidence: 0.85,
    },
  ],
}

let user: UserEvent

beforeEach(() => {
  window.localStorage.clear()
  setAccessToken(null)
  user = userEvent.setup()
  installFakePlatform('web')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the syllabus review screen', () => {
  it('shows every item with its evidence quote and writes nothing until Confirm', async () => {
    const calls = mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/syllabus/d1/draft') return jsonResponse(200, DRAFT)
      if (path === '/syllabus/d1/confirm') {
        return jsonResponse(200, { draft_id: 'd1', status: 'confirmed', events: 0, tasks: 2, goals: 1 })
      }
      return jsonResponse(404, { code: 'not_found', message: 'no route' })
    })

    renderWithProviders(<SyllabusReviewScreen draftId="d1" />, { route: '/syllabus/d1' })

    // The quote, verbatim: it is the line the student checks the item against.
    expect(await screen.findByText('Tugas 1 dikumpulkan 12 Nov')).toBeInTheDocument()
    expect(screen.getByText('UAS 15 Desember 08:00')).toBeInTheDocument()

    // The honesty header, and the item marked as needing a look: a word plus an
    // icon, which is what makes the state readable in monochrome (gate 5(d)).
    expect(screen.getByText('Nothing is added until you confirm.')).toBeInTheDocument()
    expect(screen.getByText('Needs a check')).toBeInTheDocument()
    expect(screen.getByText('1 item is marked as needing a check.')).toBeInTheDocument()

    // Nothing has been written: no POST to the confirm route has happened at all.
    expect(calls.filter((call) => call.url.endsWith('/confirm'))).toHaveLength(0)
    expect(calls.every((call) => !call.url.includes('/plans/'))).toBe(true)

    // And the confirm button is the only thing that will write, so pressing it
    // produces exactly one.
    await user.click(screen.getByRole('button', { name: 'Confirm and add 2 items' }))
    expect(await screen.findByText(/Added 0 events, 2 tasks and 1 goals/)).toBeInTheDocument()

    const confirms = calls.filter((call) => call.url.endsWith('/confirm'))
    expect(confirms).toHaveLength(1)
    const body = JSON.parse(String(confirms[0]?.init.body)) as {
      items: { index: number; keep: boolean }[]
    }
    expect(body.items.map((item) => item.index)).toEqual([0, 1])
  })

  it('leaves a dropped item out of the confirm body', async () => {
    const calls = mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/syllabus/d1/draft') return jsonResponse(200, DRAFT)
      if (path === '/syllabus/d1/confirm') {
        return jsonResponse(200, { draft_id: 'd1', status: 'confirmed', events: 0, tasks: 1, goals: 0 })
      }
      return jsonResponse(404, { code: 'not_found', message: 'no route' })
    })

    renderWithProviders(<SyllabusReviewScreen draftId="d1" />, { route: '/syllabus/d1' })
    await screen.findByText('Tugas 1 dikumpulkan 12 Nov')

    // Drop the first item, then confirm the second.
    const first = screen.getAllByRole('checkbox')[0]
    expect(first).toBeDefined()
    await user.click(first as HTMLElement)
    await user.click(screen.getByRole('button', { name: 'Confirm and add 1 item' }))

    const confirms = calls.filter((call) => call.url.endsWith('/confirm'))
    expect(confirms).toHaveLength(1)
    const body = JSON.parse(String(confirms[0]?.init.body)) as { items: { index: number }[] }
    // The dropped item has no index in the payload at all: "the student removed it"
    // and "the payload did not mention it" are the same fact, and the server writes
    // only what it is told to keep.
    expect(body.items.map((item) => item.index)).toEqual([1])
  })

  it('renders no confidence score anywhere, even when the body carries one', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/syllabus/d1/draft') return jsonResponse(200, DRAFT)
      return jsonResponse(404, { code: 'not_found', message: 'no route' })
    })

    const { container } = renderWithProviders(<SyllabusReviewScreen draftId="d1" />, {
      route: '/syllabus/d1',
    })
    await screen.findByText('Tugas 1 dikumpulkan 12 Nov')

    // A261: the number orders the list and flags an item, and the honesty rules
    // forbid rendering it. Both of the fixture's scores, and the word itself, are
    // absent from everything the student can read.
    const text = container.textContent ?? ''
    expect(text).not.toContain('0.4')
    expect(text).not.toContain('0.85')
    expect(text).not.toContain('% sure')
    expect(text.toLowerCase()).not.toContain('confidence')
  })

  it('keeps polling while the extraction is still running, and says so rather than failing', async () => {
    // `409 conflict` is the *expected* answer while the model reads the document —
    // the route reports "still reading" that way — so the screen must not render it
    // as an error. This is the state a student sees for the first few seconds.
    let polls = 0
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, USER)
      if (path === '/syllabus/d1/draft') {
        polls += 1
        if (polls === 1) {
          return jsonResponse(409, {
            code: 'conflict',
            message: 'the syllabus is still being read',
          })
        }
        return jsonResponse(200, DRAFT)
      }
      return jsonResponse(404, { code: 'not_found', message: 'no route' })
    })

    const { container } = renderWithProviders(<SyllabusReviewScreen draftId="d1" />, {
      route: '/syllabus/d1',
    })

    // The conflict is not rendered as a failure: no error note, no "Something went
    // wrong" — and the item arrives on the second poll.
    expect(await screen.findByText('UAS 15 Desember 08:00', undefined, { timeout: 4000 })).toBeInTheDocument()
    expect(within(container).queryByText(/Something went wrong/)).toBeNull()
  })
})

/**
 * The two defects the accessibility walk found on this screen, pinned where jsdom can
 * see them.
 *
 * Both are **layout** defects, and jsdom has no layout engine — so the assertions are on
 * the classes that decide the geometry, which is this project's convention for a
 * browser-measured fix (`TasksScreen.test.tsx`'s "the phone layout the browser measured"
 * block, `LockedFeature.test.tsx`'s 44px link case). The measured numbers are in the
 * comments and in the phase plan; the classes are what a later edit can break.
 */
describe('the layout the browser measured', () => {
  const source = readFileSync(join(__dirname, 'SyllabusReviewScreen.tsx'), 'utf8')

  /**
   * **The page was 680px wide inside a 390px viewport** at 200% text, with 25 elements
   * past the right edge.
   *
   * The root is a column flex container, so without `w-full` its width is its
   * *fit-content* — and fit-content can never go below min-content, so an intrinsic-width
   * child (here the `.field` inputs and the chip row) sets the page width instead of being
   * constrained by it. `w-full` makes the width definite. Measured after: **390/390**.
   *
   * Both roots are checked, because the screen has two — the confirmed state and the
   * review state — and only one of them is on screen in any given run.
   */
  it('gives both screen roots a definite width, so an intrinsic child cannot widen the page', () => {
    const roots = [...source.matchAll(/<div className="([^"]*\bmax-w-3xl\b[^"]*)"/g)].map((m) => m[1])
    expect(roots.length, 'both roots must carry their classes inline').toBe(2)
    for (const root of roots) {
      expect(root).toContain('w-full')
      // `mx-auto` stays: it is what centres the column on a desktop, where the
      // measurement is unchanged.
      expect(root).toContain('mx-auto')
    }
  })

  /**
   * **The `Keep this` control was 61.3x22 against the 44px phone floor** — and at 100%
   * text, so an overflow-only fix would have looked complete.
   *
   * The cause was the input and its label being *siblings*: the wrapper carried
   * `min-h-11`, but the harness measures a checkbox's **label** (`hitArea` resolves
   * `el.closest('label')` or `label[for=…]`), and a text-height label is 22px. Nesting
   * the input inside the label — the app's pattern (`SourcePicker`, `WorkspaceScreen`'s
   * radios) — is what makes the 44px row the measured and the tappable thing.
   */
  it('nests the keep input inside its label, which is what carries the 44px target', () => {
    const label = /<label\s+htmlFor=\{`keep-\$\{String\(row\.index\)\}`\}\s+className="([^"]*)"/.exec(source)?.[1]
    expect(label, 'the keep label must carry its classes inline').toBeTruthy()
    expect(label).toContain('min-h-11')
    // The row's ring, on top of the global `:focus-visible` rule's ring on the box:
    // the box alone is correct but small, and the row is the 44px target.
    expect(label).toContain('has-[:focus-visible]:outline-ring')
    // And the input is inside it, not a sibling — the whole reason the label is the
    // measured element.
    expect(source).toMatch(/<label[^>]*htmlFor=\{`keep-[\s\S]{0,600}?<input[\s\S]{0,600}?<\/label>/)
  })
})
