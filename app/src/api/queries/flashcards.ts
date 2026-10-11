import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * Decks, cards and reviews.
 *
 * A deck is generated asynchronously, so the deck query polls while its status is
 * `generating` — the same shape `useSourceStatus` uses for ingestion, because it
 * is the same problem and a second convention would be a second thing to learn.
 */

export type DeckStatus = 'generating' | 'ready' | 'failed'
export type Rating = 'again' | 'hard' | 'good' | 'easy'

export interface Deck {
  id: string
  workspace_id: string
  title: string
  source_ids: string[]
  status: DeckStatus
  error: string | null
  requested_count: number
  card_count: number
  created_at: string
}

/**
 * The credit price descriptor `GET /workspaces/:id/decks` serves alongside the
 * decks, so the panel quotes the plan matrix's price rather than a literal.
 */
export interface DeckCost {
  per_batch: number
  batch_size: number
  max_cards: number
}

export interface DecksResponse {
  decks: Deck[]
  cost: DeckCost
}

export interface Flashcard {
  id: string
  deck_id: string
  front: string
  back: string
  /** The chunk this card came from, or null when the model's label was not sent. */
  chunk_id: string | null
  ease: number
  interval_days: number
  due_at: string
}

export const flashcardKeys = {
  decks: (workspaceId: string) => ['workspaces', workspaceId, 'decks'] as const,
  deck: (deckId: string) => ['decks', deckId] as const,
  due: (deckId: string) => ['decks', deckId, 'due'] as const,
}

export function useDecks(workspaceId: string) {
  return useQuery({
    queryKey: flashcardKeys.decks(workspaceId),
    queryFn: async ({ signal }) =>
      api.get<DecksResponse>(`/workspaces/${workspaceId}/decks`, { signal }),
    enabled: workspaceId.length > 0,
    // A deck in flight is the only reason to poll; once every deck has settled the
    // interval is off, so a parked tab stops asking.
    refetchInterval: (query) =>
      (query.state.data?.decks ?? []).some((deck) => deck.status === 'generating') ? 1500 : false,
  })
}

export function useDeck(deckId: string) {
  return useQuery({
    queryKey: flashcardKeys.deck(deckId),
    queryFn: async ({ signal }) =>
      api.get<{ deck: Deck; cards: Flashcard[] }>(`/decks/${deckId}`, { signal }),
    enabled: deckId.length > 0,
    refetchInterval: (query) =>
      query.state.data?.deck.status === 'generating' ? 1500 : false,
  })
}

export interface CreateDeckInput {
  title: string
  sourceIds: string[]
  count: number
}

export function useCreateDeck(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: CreateDeckInput) =>
      api.post<{ deck: Deck }>(`/workspaces/${workspaceId}/decks`, {
        title: input.title,
        source_ids: input.sourceIds,
        count: input.count,
      }),
    onSuccess: () => client.invalidateQueries({ queryKey: flashcardKeys.decks(workspaceId) }),
  })
}

export function useDueCards(deckId: string) {
  return useQuery({
    queryKey: flashcardKeys.due(deckId),
    queryFn: async ({ signal }) =>
      api.get<{ cards: Flashcard[]; now: string }>(`/decks/${deckId}/due?limit=100`, { signal }),
    enabled: deckId.length > 0,
  })
}

export interface ReviewInput {
  rating: Rating
  /** Client-generated, so a retry after a lost response cannot apply twice. */
  id: string
  reviewedAt: string
}

export function useReviewCard(deckId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async ({ cardId, input }: { cardId: string; input: ReviewInput }) =>
      api.post<{ applied: boolean; card: { id: string; ease: number; interval_days: number; due_at: string } }>(
        `/flashcards/${cardId}/review`,
        { id: input.id, rating: input.rating, reviewed_at: input.reviewedAt },
      ),
    // The queue is local state during a session, so only the deck's own data is
    // invalidated. `exact` matters: query keys match by prefix, so an inexact
    // invalidation of `['decks', id]` also matches `['decks', id, 'due']` and
    // refetches the queue the study screen has deliberately taken ownership of. A
    // failure in that refetch would also flip the screen's error gate and tear the
    // card down mid-session.
    onSuccess: () =>
      client.invalidateQueries({ queryKey: flashcardKeys.deck(deckId), exact: true }),
  })
}
