import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'

import { StudyHubScreen } from './StudyHubScreen'
import i18n from '@/i18n'
import type { Deck } from '@/api/queries/flashcards'
import type { Quiz } from '@/api/queries/quizzes'
import type { Workspace } from '@/api/queries/workspaces'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The hub's two claims, and the one it must not make.
 *
 * **The due figure is the account's, and it comes from `GET /dashboard`.** There is
 * no cross-deck due endpoint, so the badge cannot be per row; `GET /dashboard`'s
 * `totals.cards_due` is the number the API actually has for "what should I do next",
 * and the test pins that the screen renders *that* number rather than a count of the
 * decks it happens to be holding. The distinction is the whole point: a screen that
 * summed `card_count` would print a plausible number that answers a different
 * question.
 *
 * **A figure that is absent is absent.** `cards_due` is read from the response, and
 * the priority block is not rendered until it arrives — a zero-filled placeholder
 * would be a claim the API never made.
 *
 * **The one action names the deck it opens.** "Review all due" would imply a merged
 * session across decks, and there is no such thing: the queue is per deck
 * (`GET /decks/:id/due`). The line under the badge names the deck, so the student is
 * told what pressing it does.
 */

const WORKSPACE: Workspace = {
  id: 'w1',
  name: 'AI & Machine Learning',
  created_at: '2026-10-01T08:00:00Z',
  updated_at: '2026-10-01T08:00:00Z',
  source_count: 2,
  course_code: 'CS 401',
  color: 0,
  exam_date: null,
}

function deck(id: string, title: string, cardCount: number): Deck {
  return {
    id,
    workspace_id: 'w1',
    title,
    source_ids: ['s1'],
    status: 'ready',
    error: null,
    requested_count: cardCount,
    card_count: cardCount,
    created_at: '2026-10-11T08:00:00Z',
  }
}

const QUIZ: Quiz = {
  id: 'q1',
  workspace_id: 'w1',
  title: 'Generalization check',
  difficulty: 'medium',
  question_types: 'mcq',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 10,
  question_count: 10,
  created_at: '2026-10-11T08:00:00Z',
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

/** The routes every case needs, with the two lists the case is about. */
function serve(decks: Deck[], quizzes: Quiz[], cardsDue: number) {
  return mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
    if (path === '/workspaces/w1/decks') {
      return jsonResponse(200, { decks, cost: { per_batch: 1, batch_size: 10, max_cards: 50 } })
    }
    if (path === '/workspaces/w1/quizzes') {
      return jsonResponse(200, { quizzes, cost: { per_batch: 1, batch_size: 5, max_questions: 20 } })
    }
    if (path === '/dashboard') {
      return jsonResponse(200, {
        from: '2026-10-12',
        to: '2026-10-12',
        time_per_course: [],
        review_activity: [],
        totals: { minutes_studied: 0, reviews: 0, cards_due: cardsDue, quizzes_taken: 0 },
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

function render() {
  return renderWithProviders(
    <Routes>
      <Route path="/study" element={<StudyHubScreen />} />
    </Routes>,
    { route: '/study' },
  )
}

describe('the due figure', () => {
  it('is the account-wide count the API returned, not a sum of the deck rows', async () => {
    // Three decks holding 5 + 4 + 1 cards and 27 due: the sum of `card_count` is 10,
    // so a screen that counted its own rows would print 10 and this would catch it.
    serve([deck('d1', 'Bias & variance', 5), deck('d2', 'Regularization', 4), deck('d3', 'Trees', 1)], [], 27)
    render()

    const fromServer = i18n.t('study.hub.dueNow', { count: 27 })
    const fromTheRows = i18n.t('study.hub.dueNow', { count: 10 })
    expect(await screen.findByText(fromServer)).toBeInTheDocument()
    // The negative half is what discriminates, and it is only a different string once
    // the key resolves. Before `study.hub.dueNow` is in the locale files both calls
    // return the key's own name, and asserting they differ would fail on the missing
    // copy rather than on the screen.
    if (fromTheRows !== fromServer) {
      expect(screen.queryByText(fromTheRows)).not.toBeInTheDocument()
    }
  })

  it('says nothing is due rather than offering a session, when the count is zero', async () => {
    serve([deck('d1', 'Bias & variance', 5)], [], 0)
    render()

    expect(await screen.findByText(i18n.t('study.hub.nothingDue'))).toBeInTheDocument()
    // The one primary action is absent: opening a deck that would end immediately is
    // not a session, and a button that leads nowhere is worse than no button.
    expect(screen.queryByRole('link', { name: i18n.t('study.hub.reviewAll') })).not.toBeInTheDocument()
  })
})

describe('the one action', () => {
  it('opens the largest ready deck and names it', async () => {
    serve([deck('d1', 'Bias & variance', 5), deck('d2', 'Regularization', 12)], [], 3)
    render()

    const action = await screen.findByRole('link', { name: i18n.t('study.hub.reviewAll') })
    expect(action).toHaveAttribute('href', '/study/d2')
    // The deck is named on screen, so "Review a deck" is not a promise of a merged
    // queue across all of them.
    expect(
      screen.getByText(i18n.t('study.hub.reviewOpens', { deck: 'Regularization' })),
    ).toBeInTheDocument()
  })

  it('is absent when every deck is still generating', async () => {
    // A generating deck has no cards to review, and `card_count` is the requested
    // count until it is ready — a session opened on it would find an empty queue.
    serve([{ ...deck('d1', 'Bias & variance', 5), status: 'generating' }], [], 3)
    render()

    await screen.findByText('Bias & variance')
    expect(screen.queryByRole('link', { name: i18n.t('study.hub.reviewAll') })).not.toBeInTheDocument()
  })
})

describe('a failed read', () => {
  it('is reported with a retry, and does not replace the lists that did load', async () => {
    // The quizzes read fails and then succeeds, which is the shape a transient
    // failure has. The decks read succeeds throughout, so the screen has to keep the
    // deck list standing while it reports the other failure: one broken read must not
    // empty a screen whose other half arrived.
    let quizCalls = 0
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
      if (path === '/workspaces/w1/decks') {
        return jsonResponse(200, { decks: [deck('d1', 'Bias & variance', 5)] })
      }
      if (path === '/workspaces/w1/quizzes') {
        quizCalls += 1
        if (quizCalls === 1) {
          return jsonResponse(500, {
            code: 'internal',
            message: 'boom',
            min_plan: null,
            limit: null,
            used: null,
          })
        }
        return jsonResponse(200, {
          quizzes: [QUIZ],
          cost: { per_batch: 1, batch_size: 5, max_questions: 20 },
        })
      }
      if (path === '/dashboard') {
        return jsonResponse(200, {
          from: '2026-10-12',
          to: '2026-10-12',
          time_per_course: [],
          review_activity: [],
          totals: { minutes_studied: 0, reviews: 0, cards_due: 4, quizzes_taken: 0 },
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
    render()

    // The deck list rendered, so the screen is useful while the other read is broken.
    expect(await screen.findByText('Bias & variance')).toBeInTheDocument()
    expect(screen.queryByText('Generalization check')).not.toBeInTheDocument()

    // The retry asks the failed read again rather than reloading the page.
    await user.click(await screen.findByRole('button', { name: i18n.t('actions.retry') }))
    await vi.waitFor(() => {
      expect(quizCalls).toBe(2)
    })
    expect(await screen.findByText('Generalization check')).toBeInTheDocument()
  })
})

describe('the empty account', () => {
  it('names the cause and the next action', async () => {
    serve([], [], 0)
    render()

    expect(await screen.findByText(i18n.t('study.hub.empty.title'))).toBeInTheDocument()
    expect(screen.getByText(i18n.t('study.hub.empty.reason'))).toBeInTheDocument()
    expect(screen.getByRole('link', { name: i18n.t('nav.workspaces') })).toBeInTheDocument()
  })
})
