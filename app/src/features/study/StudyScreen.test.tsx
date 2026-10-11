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
