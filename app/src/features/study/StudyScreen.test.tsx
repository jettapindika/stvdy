import { act, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'

import { StudyScreen } from './StudyScreen'
import { previewInterval } from './due'
import i18n from '@/i18n'
import type { Deck, Flashcard } from '@/api/queries/flashcards'
import type { Workspace } from '@/api/queries/workspaces'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The one rule on this screen that protects the schedule rather than the layout:
 * a card gets one review id per showing.
 *
 * The failure path is the case that matters. A review that fails may or may not
 * have reached the server, and the client cannot tell, so the next attempt for
 * the same card has to carry the id that was already sent: the server answers
 * `applied: false` for an id it has seen, which makes the resend a no-op, whereas
 * a fresh id after a failure that did land grades the card twice for one showing
 * and moves `ease`/`interval_days` twice. Choosing a *different* grade is the
 * path a rating-keyed id would get wrong, so that is what this drives.
 */

const DECK: Deck = {
  id: 'd1',
  workspace_id: 'w1',
  title: 'Graf',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 1,
  card_count: 1,
  created_at: '2026-10-11T08:00:00Z',
}

/**
 * The deck's course. The screen reads it for the line above the title, and a test
 * whose mock 404s it would be exercising the branch where the course is unknown
 * while claiming to render the screen.
 */
const WORKSPACE: Workspace = {
  id: 'w1',
  name: 'Struktur Diskrit',
  created_at: '2026-10-01T08:00:00Z',
  updated_at: '2026-10-01T08:00:00Z',
  source_count: 1,
  course_code: 'IF 201',
  color: 0,
  exam_date: null,
}

const CARD: Flashcard = {
  id: 'c1',
  deck_id: 'd1',
  front: 'Apa itu graf?',
  back: 'Kumpulan simpul dan sisi.',
  chunk_id: null,
  ease: 2.5,
  interval_days: 0,
  due_at: '2026-10-10T09:00:00Z',
}

const CARD2: Flashcard = {
  id: 'c2',
  deck_id: 'd1',
  front: 'Apa itu simpul?',
  back: 'Titik pada graf.',
  chunk_id: null,
  ease: 2.5,
  interval_days: 0,
  due_at: '2026-10-10T09:00:00Z',
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

/** The `id` a review request carried, narrowed rather than asserted. */
function reviewIdOf(init: RequestInit): string {
  const body: unknown = JSON.parse(String(init.body))
  if (typeof body === 'object' && body !== null && 'id' in body && typeof body.id === 'string') {
    return body.id
  }
  throw new Error('the review body carried no id')
}

/**
 * A grade button, by the word it starts with.
 *
 * Anchored at the start rather than matched loosely: the design puts the grade's
 * word and its computed interval on the same button, so the accessible name is
 * "Good 4d" and an exact match would pin the interval — a number this test would
 * then have to keep in step with `./due`. The word is what identifies the grade, and
 * the assertion that it is there is the one gate 5(d) needs.
 */
function gradeButton(label: string) {
  return screen.getByRole('button', { name: new RegExp(`^${label}`) })
}

describe('one review id per card showing', () => {
  it('reuses the id when a different grade is chosen after a failure', async () => {
    const sent: { id: string; rating: string }[] = []
    let attempts = 0

    mockFetch((url, init) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [CARD] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/flashcards/c1/review') {
        attempts += 1
        const body: unknown = JSON.parse(String(init.body))
        sent.push({
          id: reviewIdOf(init),
          rating:
            typeof body === 'object' && body !== null && 'rating' in body && typeof body.rating === 'string'
              ? body.rating
              : '',
        })
        // A refusal that reached the server: the review may have applied.
        if (attempts === 1) {
          return jsonResponse(500, {
            code: 'internal',
            message: 'boom',
            min_plan: null,
            limit: null,
            used: null,
          })
        }
        return jsonResponse(200, {
          applied: true,
          card: { id: 'c1', ease: 2.5, interval_days: 1, due_at: '2026-10-12T09:00:00Z' },
        })
      }
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))

    // The failure keeps the card and the reveal on screen, so the grade buttons
    // are still there to be used.
    expect(await screen.findByRole('button', { name: 'Try again' })).toBeInTheDocument()
    expect(screen.getByText('Kumpulan simpul dan sisi.')).toBeInTheDocument()

    // A different grade, not the retry: the same showing, so the same id.
    await user.click(gradeButton('Hard'))

    await vi.waitFor(() => {
      expect(sent).toHaveLength(2)
    })
    expect(sent.map((review) => review.rating)).toEqual(['good', 'hard'])
    expect(sent[1]?.id).toBe(sent[0]?.id)
  })
})

describe('the invalidation after a review', () => {
  it('does not refetch the due queue when a review lands', async () => {
    // Query keys match by prefix, so an inexact invalidation of `['decks', d1]`
    // would also match `['decks', d1, 'due']` and refetch the queue the screen has
    // taken ownership of — and a failure in that refetch would flip the error gate
    // and tear the card down mid-session.
    const calls = mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [CARD] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/flashcards/c1/review') {
        return jsonResponse(200, {
          applied: true,
          card: { id: 'c1', ease: 2.5, interval_days: 1, due_at: '2026-10-12T09:00:00Z' },
        })
      }
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))

    expect(await screen.findByText('Nothing due in this deck')).toBeInTheDocument()
    await vi.waitFor(() => {
      // The deck itself is refetched, which is the invalidation's real purpose.
      expect(calls.filter((call) => new URL(call.url).pathname === '/decks/d1')).toHaveLength(2)
    })
    expect(calls.filter((call) => new URL(call.url).pathname === '/decks/d1/due')).toHaveLength(1)
  })
})

describe('a background deck error mid-session', () => {
  it('keeps the card and the queue, and only surfaces a notice', async () => {
    // `useReviewCard` invalidates the deck key after every review, so the deck
    // query refetches mid-session. TanStack keeps `data` while setting `error` on
    // a failed refetch, so a gate on `deckError === null` would replace the card,
    // the revealed answer and the review's own error notice with a deck error.
    let deckCalls = 0
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') {
        deckCalls += 1
        if (deckCalls === 1) return jsonResponse(200, { deck: DECK, cards: [CARD, CARD2] })
        return jsonResponse(500, {
          code: 'internal',
          message: 'refetch failed',
          min_plan: null,
          limit: null,
          used: null,
        })
      }
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD, CARD2], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/flashcards/c1/review') {
        return jsonResponse(200, {
          applied: true,
          card: { id: 'c1', ease: 2.5, interval_days: 1, due_at: '2026-10-12T09:00:00Z' },
        })
      }
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))

    // The background failure is surfaced without tearing the session down: the
    // next card is still there and can still be revealed.
    expect(
      await screen.findByText('Could not refresh this deck. Your session is unaffected.'),
    ).toBeInTheDocument()
    expect(screen.getByText('Apa itu simpul?')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Show answer' })).toBeInTheDocument()
  })
})


describe('gate 5(d): no state is carried by colour alone', () => {
  /**
   * The screen's two states, each asserted on the channel a reader who cannot see
   * colour still receives.
   *
   * **Revealed.** The answer's arrival is content, not a tone: the back text and the
   * four grade buttons appear, each with its own word (`Again`/`Hard`/`Good`/`Easy`)
   * and each with the number key it answers to in `aria-keyshortcuts`. A revealed
   * state signalled by the panel changing shade would be invisible here.
   *
   * **In flight.** A grade whose request is outstanding disables the four buttons, and
   * that is the `disabled` attribute — exposed to assistive technology and to the
   * keyboard, unlike the `opacity: 0.45` the class happens to also apply. Asserted on
   * the attribute rather than on the class string, because the class is the *paint* and
   * the attribute is the *state*: the defect this pins is a screen that shows "working"
   * only by dimming.
   */
  it('reveals the answer as content and disables the grades as a state', async () => {
    // A review that never settles, so the in-flight state can be read.
    const held = Promise.withResolvers<Response>()
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [CARD] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/flashcards/c1/review') return held.promise
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    // Unrevealed: the answer is not on screen and the grades do not exist, so there
    // is nothing a reader could mistake for a revealed card.
    await screen.findByRole('button', { name: 'Show answer' })
    expect(screen.queryByText('Kumpulan simpul dan sisi.')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Good/ })).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Show answer' }))
    expect(screen.getByText('Kumpulan simpul dan sisi.')).toBeInTheDocument()
    for (const grade of ['Again', 'Hard', 'Good', 'Easy']) {
      const button = screen.getByRole('button', { name: new RegExp(grade) })
      expect(button).toBeEnabled()
    }
    expect(gradeButton('Good')).toHaveAttribute('aria-keyshortcuts', '3')

    await user.click(gradeButton('Good'))
    await vi.waitFor(() => {
      expect(gradeButton('Good')).toBeDisabled()
    })
    // And the card is still named, so "working" is not the only thing on screen.
    expect(screen.getByText('Card 1 of 1')).toBeInTheDocument()

    // Let the mutation settle so the test does not leave a pending request behind.
    held.resolve(
      jsonResponse(200, {
        applied: true,
        card: { id: 'c1', ease: 2.5, interval_days: 1, due_at: '2026-10-12T09:00:00Z' },
      }),
    )
  })
})

describe('the interval on a grade button, and the receipt after it', () => {
  /**
   * The design puts the computed next interval on each of the four grades
   * (`SPEC.md:86`), and the two numbers on this screen are different claims:
   *
   *   - on the button, `previewInterval`'s — what this grade *would* schedule,
   *     computed from the card on screen by the rules in `./due`;
   *   - in the receipt, `response.card.interval_days` — what the server *did*
   *     write, which is the schedule every other device will read.
   *
   * The expected strings are read back from the same i18n instance the screen
   * renders through, rather than written out. What is being pinned here is the
   * *wiring* — that each button carries its own grade's preview and that the receipt
   * carries the server's answer — and a literal would pin the copy instead, failing
   * on every wording change while passing a screen that showed one interval four
   * times. The card is mid-ladder (six days) on purpose: a fresh card previews one
   * day for all four grades, which is exactly the state in which a broken wiring
   * looks correct.
   */
  const REVIEWED: Flashcard = { ...CARD, interval_days: 6 }

  it("previews each grade's own interval and reports the server's answer after", async () => {
    const sent: { rating: string }[] = []
    mockFetch((url, init) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [REVIEWED] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [REVIEWED], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/flashcards/c1/review') {
        const body: unknown = JSON.parse(String(init.body))
        sent.push({
          rating:
            typeof body === 'object' && body !== null && 'rating' in body && typeof body.rating === 'string'
              ? body.rating
              : '',
        })
        // A schedule no button on this card offered: the previews below are 1 and 15,
        // so a receipt reading 6 can only have come from the server's answer.
        return jsonResponse(200, {
          applied: true,
          card: { id: 'c1', ease: 2.5, interval_days: 6, due_at: '2026-10-17T09:00:00Z' },
        })
      }
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))

    for (const [grade, rating] of [
      ['Again', 'again'],
      ['Hard', 'hard'],
      ['Good', 'good'],
      ['Easy', 'easy'],
    ] as const) {
      const days = previewInterval(REVIEWED, rating)
      expect(gradeButton(grade).textContent).toContain(i18n.t('study.interval.days', { days }))
    }
    // And the four are not all the same number, so the loop above is not vacuous.
    expect(previewInterval(REVIEWED, 'again')).not.toBe(previewInterval(REVIEWED, 'good'))

    await user.click(gradeButton('Good'))
    expect(sent.map((review) => review.rating)).toEqual(['good'])

    // The receipt: the server's number, the grade it was about, and the position of
    // the card it was about — so it cannot be read as being about the card now on
    // screen.
    expect(
      await screen.findByText(
        i18n.t('study.receipt.next', { position: 1, days: i18n.t('study.interval.days', { days: 6 }) }),
      ),
    ).toBeInTheDocument()
    expect(screen.getByText(i18n.t('study.receipt.rated', { grade: 'Good' }))).toBeInTheDocument()
  })

  it('shows the grade row only after the answer is revealed', async () => {
    // The honesty core, restated for the new markup: a grade button is a judgement
    // about an answer the student has to have seen, and the design's four buttons
    // with their intervals are exactly the control this gates.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [CARD] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    await screen.findByRole('button', { name: 'Show answer' })
    expect(screen.queryByText(i18n.t('study.rateRecall'))).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /^Again/ })).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Show answer' }))
    expect(screen.getByText(i18n.t('study.rateRecall'))).toBeInTheDocument()
    expect(gradeButton('Again')).toBeInTheDocument()
  })
})

describe('the collapsible source panel', () => {
  /**
   * `SPEC.md:678` asks for the passage behind the card in a panel, and the design
   * asks for the card to be the only thing on screen until the student goes looking.
   * So the toggle is part of the revealed card, the passage is a sibling of it — which
   * is what lets a desktop put it in a second column — and the two-column template is
   * only applied while the panel is open, so the closed state is the frame's centred
   * card rather than a card beside an empty column.
   */
  function serveSource(card: Flashcard) {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [card] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [card], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') {
        return jsonResponse(200, { sources: [{ id: 's1', title: 'Kriptografi' }] })
      }
      if (path === '/chunks/k1') {
        return jsonResponse(200, {
          id: 'k1',
          source_id: 's1',
          page: 42,
          content: 'Kunci publik dipakai untuk memverifikasi tanda tangan.',
        })
      }
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })
  }

  it('offers the passage only after the answer is revealed, and closes the panel with it', async () => {
    serveSource({ ...CARD, chunk_id: 'k1' })
    const user = userEvent.setup()
    const { container } = renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    // Unrevealed: the toggle does not exist, so nothing can pull the source out of
    // the card before the student has seen what the card says.
    await screen.findByRole('button', { name: 'Show answer' })
    expect(screen.queryByRole('button', { name: 'Show source' })).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Show answer' }))
    await user.click(screen.getByRole('button', { name: 'Show source' }))

    // The panel is an `aside` beside the card, which is the element a desktop puts in
    // the second column; the closed state has no such element at all.
    expect(await screen.findByText(/page 42/)).toBeInTheDocument()
    expect(container.querySelector('aside')).not.toBeNull()
    expect(screen.getByRole('button', { name: 'Hide source' })).toHaveAttribute(
      'aria-expanded',
      'true',
    )

    await user.click(screen.getByRole('button', { name: 'Hide source' }))
    expect(container.querySelector('aside')).toBeNull()
  })

  it('leaves the toggle out entirely for a card with no citation', async () => {
    // `chunk_id` is null for a card whose label was not sent. A "Show source" button
    // that opened an empty panel would be a control with nothing behind it.
    serveSource({ ...CARD, chunk_id: null })
    const user = userEvent.setup()
    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    expect(screen.queryByRole('button', { name: 'Show source' })).not.toBeInTheDocument()
  })
})

describe('undoing the grade the receipt reports', () => {
  /**
   * The spec's reversible-action rule (`SPEC.md:87`): a grade is reversible, so it
   * gets an Undo rather than a confirmation, and the Undo lives on the receipt the
   * grade already produces. `onUndo` lets a test refuse the delete, which is the
   * branch the server's window (409) exercises.
   */
  function serveDeck(onUndo?: () => Response) {
    return mockFetch((url, init) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [CARD, CARD2] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD, CARD2], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      if (path === '/flashcards/c1/review') {
        return jsonResponse(200, {
          applied: true,
          card: { id: 'c1', ease: 2.5, interval_days: 1, due_at: '2026-10-12T09:00:00Z' },
        })
      }
      if (init.method === 'DELETE' && path.startsWith('/flashcard-reviews/')) {
        return onUndo === undefined ? jsonResponse(204) : onUndo()
      }
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })
  }

  function renderScreen() {
    return renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )
  }

  it('deletes the review that was posted and returns the card to the queue', async () => {
    const calls = serveDeck()
    const user = userEvent.setup()
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))

    // The grade advanced the queue, so the second card is the one on screen now.
    expect(await screen.findByText('Apa itu simpul?')).toBeInTheDocument()

    const posted = calls.find((call) => new URL(call.url).pathname === '/flashcards/c1/review')
    if (posted === undefined) throw new Error('no review was posted')

    await user.click(await screen.findByRole('button', { name: 'Undo' }))

    // The delete names the id the review was sent with — the row's own key — rather
    // than the card's: `/flashcard-reviews/:id` is a review, and the card id would be
    // the wrong resource.
    await vi.waitFor(() => {
      expect(
        calls.filter(
          (call) =>
            call.init.method === 'DELETE' &&
            new URL(call.url).pathname === `/flashcard-reviews/${reviewIdOf(posted.init)}`,
        ),
      ).toHaveLength(1)
    })

    // And the graded card is back, unrevealed, ready to be graded again.
    expect(await screen.findByText('Apa itu graf?')).toBeInTheDocument()
    expect(screen.queryByText('Apa itu simpul?')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Show answer' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /^Good/ })).not.toBeInTheDocument()
  })

  it('takes the toast away after five seconds when nothing is done', async () => {
    // The spec's number is five seconds, and an expired opportunity must not sit on
    // screen offering a review the server would by then refuse. Fake timers are the
    // only way to reach the fifth second without waiting for it.
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    serveDeck()
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))
    expect(await screen.findByRole('button', { name: 'Undo' })).toBeInTheDocument()

    await act(async () => {
      vi.advanceTimersByTime(5000)
    })
    expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument()
  })

  it('offers the undo as a real button, not a text-height link', async () => {
    // Undo is an action, so it is a `.btn` — the 44px phone target the header link
    // needed `min-h-11 inline-flex items-center` to reach. A link here would be the
    // same text-height defect Phase 4's gate found on that link.
    const user = userEvent.setup()
    serveDeck()
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))

    const undo = await screen.findByRole('button', { name: 'Undo' })
    expect(undo.tagName).toBe('BUTTON')
    expect(undo.className).toContain('btn')
    expect(screen.queryByRole('link', { name: 'Undo' })).not.toBeInTheDocument()
  })

  it('undoes on Z while the toast is up, and ignores a modified Z', async () => {
    // `SPEC.md:679` lists `Z` among the study hotkeys, so the keyboard route has to
    // reach the same mutation the button does. A modified `Z` is not the hotkey:
    // ⌘Z is the platform's own undo, and answering it here would act on a review
    // the student was not addressing.
    const calls = serveDeck()
    const user = userEvent.setup()
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))
    await screen.findByRole('button', { name: 'Undo' })

    await user.keyboard('{Meta>}z{/Meta}')
    expect(calls.filter((call) => call.init.method === 'DELETE')).toHaveLength(0)

    await user.keyboard('z')
    await vi.waitFor(() => {
      expect(calls.filter((call) => call.init.method === 'DELETE')).toHaveLength(1)
    })
    expect(await screen.findByText('Apa itu graf?')).toBeInTheDocument()
  })

  it('leaves Z alone when the caret is in a field', async () => {
    // The listener is on `window`, so it sees the keys typed into a field as well as
    // the ones aimed at the card. The study screen has no field of its own yet, and
    // `E` (edit) is a spec hotkey that will add one, so the guard is written now: a
    // global handler that eats a letter mid-word is a defect that only shows up once
    // the field exists, which is the worst time to find it. The field here stands in
    // for that one.
    const calls = serveDeck()
    const user = userEvent.setup()
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))
    await screen.findByRole('button', { name: 'Undo' })

    const field = document.createElement('input')
    document.body.append(field)
    field.focus()
    await user.keyboard('z')
    expect(calls.filter((call) => call.init.method === 'DELETE')).toHaveLength(0)
    // And the letter landed in the field, which is what the student typed it for.
    expect(field.value).toBe('z')

    // The same key with the caret outside it does undo, so the assertion above is
    // about the field rather than about a handler that never ran.
    field.remove()
    await user.keyboard('z')
    await vi.waitFor(() => {
      expect(calls.filter((call) => call.init.method === 'DELETE')).toHaveLength(1)
    })
  })

  it("shows the window's own refusal rather than the generic conflict copy", async () => {
    // A 409 is the server saying the review is older than the window and cannot be
    // undone; the general `conflict` copy ("Something else changed this first.
    // Reload and try again.") is about lost updates and is wrong advice here. The
    // grade stands, and that is what the notice has to say.
    serveDeck(() =>
      jsonResponse(409, {
        code: 'conflict',
        message: 'this review is older than 5 minutes and cannot be undone',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )
    const user = userEvent.setup()
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))
    await user.click(await screen.findByRole('button', { name: 'Undo' }))

    expect(await screen.findByText(i18n.t('study.undo.tooLate.reason'))).toBeInTheDocument()
    // The review still stands, so the card is not returned to the queue.
    expect(screen.queryByText('Apa itu graf?')).not.toBeInTheDocument()
  })

  it('keeps the refusal for the toast’s own window and then lets both go', async () => {
    // The refusal restarts the receipt's deadline, so a message that arrives just
    // before the toast would have expired still gets a full five seconds to be read.
    // The undo is clicked late in the window on purpose: without the restart the
    // notice would be gone within a second, and the test would pass anyway.
    vi.useFakeTimers({ shouldAdvanceTime: true })
    serveDeck(() =>
      jsonResponse(409, {
        code: 'conflict',
        message: 'this review is older than 5 minutes and cannot be undone',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    renderScreen()

    await user.click(await screen.findByRole('button', { name: 'Show answer' }))
    await user.click(gradeButton('Good'))
    const undo = await screen.findByRole('button', { name: 'Undo' })

    await act(async () => {
      vi.advanceTimersByTime(4000)
    })
    await user.click(undo)
    expect(await screen.findByText(i18n.t('study.undo.tooLate.reason'))).toBeInTheDocument()

    // Past the deadline the toast was on when the refusal arrived, and it is still
    // there to be read.
    await act(async () => {
      vi.advanceTimersByTime(2000)
    })
    expect(screen.getByText(i18n.t('study.undo.tooLate.reason'))).toBeInTheDocument()

    // And then both leave together: an offer that has gone must not leave a sentence
    // about it behind.
    await act(async () => {
      vi.advanceTimersByTime(5000)
    })
    expect(screen.queryByText(i18n.t('study.undo.tooLate.reason'))).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument()
  })
})
