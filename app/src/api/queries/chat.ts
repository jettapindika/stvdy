import { useQuery } from '@tanstack/react-query'

import { api } from '../client'
import type { ChatCitation } from '../sse'

/**
 * The persisted chat thread, and one chunk of the material it cites.
 *
 * The thread is the server's, not the client's: `POST /chat` persists both turns
 * and `GET /messages` reads them back, which is what makes the same conversation
 * visible on the student's other device. So the screen never accumulates turns in
 * its own state. It renders this query, plus the one turn currently streaming.
 * Two sources of truth for the same list would drift the moment a device that
 * missed a `done` event refetched.
 */

export type MessageRole = 'user' | 'assistant'

export interface Message {
  id: string
  workspace_id: string
  role: MessageRole
  content: string
  /** Already resolved by the server: a label it could not match was dropped. */
  citations: ChatCitation[]
  used_web: boolean
  created_at: string
}

export interface Chunk {
  id: string
  source_id: string
  /** 1-based, and null for pasted text, which has no pages. */
  page: number | null
  idx: number
  content: string
}

export const chatKeys = {
  messages: (workspaceId: string) => ['workspaces', workspaceId, 'messages'] as const,
  chunk: (chunkId: string) => ['chunks', chunkId] as const,
}

export function useMessages(workspaceId: string) {
  return useQuery({
    queryKey: chatKeys.messages(workspaceId),
    queryFn: async ({ signal }) => {
      const body = await api.get<{ messages: Message[] }>(`/workspaces/${workspaceId}/messages`, {
        signal,
      })
      return body.messages
    },
    enabled: workspaceId.length > 0,
  })
}

/**
 * One cited chunk, fetched when the student opens its chip.
 *
 * Fetched rather than carried in the citation: the `citations` event names a
 * chunk by id, and shipping the text of every cited chunk in the stream would
 * send the whole context back over the wire on every answer. `GET /chunks/:id`
 * is the same route the sources panel's material lives behind, so a citation and
 * the source list agree about what a chunk is.
 */
export function useChunk(chunkId: string | null) {
  return useQuery({
    queryKey: chatKeys.chunk(chunkId ?? ''),
    queryFn: async ({ signal }) => api.get<Chunk>(`/chunks/${chunkId ?? ''}`, { signal }),
    enabled: chunkId !== null && chunkId.length > 0,
  })
}
