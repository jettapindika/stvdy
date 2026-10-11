import { screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { FlashcardsPanel } from './FlashcardsPanel'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The one rule the panel must not break: the cost copy and the request cap come
 * from the server, not from a literal. `GET /workspaces/:id/decks` serves the
 * plan matrix's price alongside the decks, so a config change cannot leave the
 * panel quoting a stale one.
 *
 * The served values are deliberately unlike the numbers the panel used to
 * hardcode (4 per 10, max 50), so a regression to a literal fails here.
 */

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function serveCost(cost: { per_batch: number; batch_size: number; max_cards: number }) {
  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/workspaces/w1/decks') return jsonResponse(200, { decks: [], cost })
    if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [] })
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
}

describe('the credit cost the panel quotes', () => {
  it('renders the price and the cap the server serves', async () => {
    serveCost({ per_batch: 7, batch_size: 5, max_cards: 25 })

    renderWithProviders(<FlashcardsPanel workspaceId="w1" />)

    expect(await screen.findByText(/Costs 7 credits per 5 cards/)).toBeInTheDocument()
    expect(screen.getByLabelText('How many cards')).toHaveAttribute('max', '25')
  })
})
