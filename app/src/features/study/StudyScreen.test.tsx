import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'

import { StudyScreen } from './StudyScreen'
import type { Deck, Flashcard } from '@/api/queries/flashcards'
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
    await user.click(screen.getByRole('button', { name: 'Good' }))

    // The failure keeps the card and the reveal on screen, so the grade buttons
    // are still there to be used.
    expect(await screen.findByRole('button', { name: 'Try again' })).toBeInTheDocument()
    expect(screen.getByText('Kumpulan simpul dan sisi.')).toBeInTheDocument()

    // A different grade, not the retry: the same showing, so the same id.
    await user.click(screen.getByRole('button', { name: 'Hard' }))

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
    await user.click(screen.getByRole('button', { name: 'Good' }))

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
    await user.click(screen.getByRole('button', { name: 'Good' }))

    // The background failure is surfaced without tearing the session down: the
    // next card is still there and can still be revealed.
    expect(
      await screen.findByText('Could not refresh this deck. Your session is unaffected.'),
    ).toBeInTheDocument()
    expect(screen.getByText('Apa itu simpul?')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Show answer' })).toBeInTheDocument()
  })
})

describe('the header link back to the workspace', () => {
  /**
   * The one control on this screen that is not a `btn`, and it was a text-height
   * target until Phase 4's accessibility gate measured it.
   *
   * The gate walked the study screen in Chromium at 390px for the first time in the
   * project's history — this phase's box had been unticked since it was written —
   * and the header's "Back to the workspace" measured **151.3x21**: 21px tall
   * against the 44px phone floor, and 41px at 200% text, where a text link grows by
   * its line height rather than to the floor. It is the same defect Phase 7's gate
   * found on the upgrade link (93x19) and the same remedy: `min-h-11` (2.75rem =
   * 44px) plus `inline-flex items-center`, the shape eight other text links in the
   * app already use.
   *
   * **jsdom has no layout engine**, so the 44px cannot be asserted here — this file
   * asserts the classes that decide it, and the number is the gate's own measurement
   * (recorded in the Phase 4 plan). `min-h-11` alone on an inline element still
   * paints the line box, and `inline-flex items-center` alone has no height to
   * centre in, so all three utilities are load-bearing.
   */
  it('is a 44px phone target, not a text-height one', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') return jsonResponse(200, { deck: DECK, cards: [CARD] })
      if (path === '/decks/d1/due') {
        return jsonResponse(200, { cards: [CARD], now: '2026-10-11T09:00:00Z' })
      }
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    renderWithProviders(
      <Routes>
        <Route path="/study/:deckId" element={<StudyScreen />} />
      </Routes>,
      { route: '/study/d1' },
    )

    const link = await screen.findByRole('link', { name: 'Back to the workspace' })
    expect(link.className).toContain('min-h-11')
    expect(link.className).toContain('inline-flex')
    expect(link.className).toContain('items-center')
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
    expect(screen.getByRole('button', { name: /Good/ })).toHaveAttribute('aria-keyshortcuts', '3')

    await user.click(screen.getByRole('button', { name: /Good/ }))
    await vi.waitFor(() => {
      expect(screen.getByRole('button', { name: /Good/ })).toBeDisabled()
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
