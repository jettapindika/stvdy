import { useCallback, useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { actionKeys } from '@/api/queries/actions'
import { chatKeys } from '@/api/queries/chat'
import { streamChat, type ChatCitation, type ChatUsage } from '@/api/sse'
import { ApiError } from '@/api/errors'

/**
 * One chat turn, from the question being sent to the answer being finished.
 *
 * `text` is kept rather than discarded on failure, because a half-finished answer
 * that the student already read is worth more than an empty box. The retry
 * affordance sits beside it, and the partial text stays on screen until the
 * student retries or types a new question.
 *
 * The turn is local state and the rest of the thread is a server query. That
 * split is deliberate: an answer that has not finished streaming has no server
 * row yet, and inventing one would make the screen show a message that does not
 * exist on the student's other device.
 */
export interface ChatTurn {
  /** The question, echoed while its answer streams. */
  question: string
  /** The answer so far. Empty until the first token arrives. */
  text: string
  citations: ChatCitation[]
  /** What the server spent, when it said. */
  usage: ChatUsage | null
  /** Set when the attempt failed at any point. `text` above is kept. */
  error: ApiError | null
  /** True from `send` until the stream ends, however it ends. */
  streaming: boolean
}

export interface ChatRequest {
  message: string
  /** Narrows retrieval to these sources. Empty means the whole workspace. */
  sourceIds: string[]
  useWeb: boolean
  /**
   * Whether the answer is written only from the selected sources.
   *
   * Kept separate from `useWeb` because they answer different questions: this one
   * decides whether the model may use its own memory, that one whether it may
   * leave the workspace. Both on is a coherent request — the student wants a
   * grounded answer that may also cite a page they can open.
   */
  sourcesOnly: boolean
}

export interface ChatStreamState {
  /** The in-flight or failed turn. `null` when there is nothing to show. */
  turn: ChatTurn | null
  send(request: ChatRequest): void
  /** Re-sends the last question after a failure. */
  retry(): void
  /** Abandons the stream. The partial text stays where it is. */
  stop(): void
}

export function useChatStream(workspaceId: string): ChatStreamState {
  const queryClient = useQueryClient()
  const [turn, setTurn] = useState<ChatTurn | null>(null)
  const [last, setLast] = useState<ChatRequest | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  // Every attempt gets a number, and a handler from a superseded attempt is
  // ignored. Without it, a token that arrives from a stream the student already
  // abandoned would be appended to the answer that replaced it.
  const attemptRef = useRef(0)

  const run = useCallback(
    async (request: ChatRequest) => {
      const attempt = (attemptRef.current += 1)
      const isCurrent = () => attemptRef.current === attempt

      abortRef.current?.abort()
      const controller = new AbortController()
      abortRef.current = controller

      setLast(request)
      setTurn({
        question: request.message,
        text: '',
        citations: [],
        usage: null,
        error: null,
        streaming: true,
      })

      try {
        await streamChat(
          {
            workspaceId,
            message: request.message,
            sourceIds: request.sourceIds,
            useWeb: request.useWeb,
            sourcesOnly: request.sourcesOnly,
            signal: controller.signal,
          },
          {
            onToken(text) {
              if (!isCurrent()) return
              setTurn((current) =>
                current === null ? current : { ...current, text: current.text + text },
              )
            },
            onCitations(citations) {
              if (!isCurrent()) return
              setTurn((current) => (current === null ? current : { ...current, citations }))
            },
            onAction() {
              if (!isCurrent()) return
              // A proposal arrived mid-answer, and the card renders it from
              // `GET /actions/pending` rather than from this event: the event
              // carries no cost and no status, and a card built from it could not
              // show that the proposal had already been resolved on another device.
              // So the list is refetched, which is also what makes a *second*
              // device's tap land here.
              void queryClient
                .invalidateQueries({ queryKey: actionKeys.pending })
                .catch(() => undefined)
            },
            onUsage(usage) {
              if (!isCurrent()) return
              setTurn((current) => (current === null ? current : { ...current, usage }))
            },
            onDone() {
              if (!isCurrent()) return
              // The thread is refetched *before* the live turn is dropped, so the
              // finished answer is never taken off screen and put back: the list
              // swaps from the streamed copy to the persisted one in one paint.
              void queryClient
                .invalidateQueries({ queryKey: chatKeys.messages(workspaceId) })
                // The refetch is what replaces the live turn with the persisted
                // copy, and it rejects on a network blip. Without this catch the
                // live turn would stay on screen indefinitely and the rejection
                // would be unhandled, so dropping the turn is correct either way:
                // the answer is already on screen from the stream.
                .catch(() => undefined)
                .then(() => {
                  if (isCurrent()) setTurn(null)
                })
            },
            onError(error) {
              if (!isCurrent()) return
              setTurn((current) =>
                current === null ? current : { ...current, error, streaming: false },
              )
            },
          },
        )
      } catch {
        // The only throw `streamChat` makes is the abort, which is the student's
        // own action rather than a failure to report.
      }

      if (!isCurrent()) return
      // A body that ends without a `done` event (a dropped connection, a proxy
      // that closed the stream) leaves the turn no longer claiming to be live,
      // and the error handler has already said what went wrong.
      setTurn((current) => (current === null ? current : { ...current, streaming: false }))
    },
    [queryClient, workspaceId],
  )

  const send = useCallback(
    (request: ChatRequest) => {
      void run(request)
    },
    [run],
  )

  const retry = useCallback(() => {
    if (last === null) return
    void run(last)
  }, [last, run])

  const stop = useCallback(() => {
    abortRef.current?.abort()
  }, [])

  useEffect(
    () => () => {
      // A stream that outlives its screen would keep the reader, the connection
      // and the server's work alive for an answer nobody will see.
      abortRef.current?.abort()
    },
    [],
  )

  return { turn, send, retry, stop }
}
