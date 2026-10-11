import { createParser, type EventSourceMessage } from 'eventsource-parser'

import { API_BASE_URL, ensureFreshSession } from './client'
import { apiErrorFromBody, ApiError, statusForCode, type ApiErrorCode } from './errors'
import { getAccessToken } from '../lib/tokens'

/**
 * The chat stream: a POST that answers `text/event-stream`, read by hand.
 *
 * `EventSource` cannot be used here and the reason is not stylistic. It only
 * issues GET requests and offers no way to attach a header, and this route needs
 * both a POST body and an `Authorization` header. So the transport is `fetch`
 * plus a `ReadableStream` reader, and the framing is parsed by
 * `eventsource-parser`, which is the same parser the EventSource implementation
 * follows.
 *
 * Three details of the wire contract are load-bearing and each has a test:
 *
 * 1. **A frame can be split across two reads.** The reader hands back arbitrary
 *    byte boundaries, so half an event routinely arrives first. The parser
 *    buffers the remainder rather than being fed only whole frames.
 * 2. **`: heartbeat` comment lines are not events.** The server sends one every
 *    15 seconds to keep proxies from closing an idle stream. A parser that
 *    treated a comment as an event would emit garbage every 15 seconds, and one
 *    that threw on it would break every answer longer than 15 seconds.
 * 3. **`[DONE]` is not data.** The `openai_compat` upstream ends with
 *    `data: [DONE]`. Our server translates that into a named `done` event, so a
 *    `[DONE]` frame here is only tolerated, never acted on: the `done` event is
 *    what says the answer is complete.
 *
 * The `error` event is the fourth: it can arrive at *any* position, including
 * before the first token and after many of them, and it carries the same
 * five-key envelope as every other route. It is delivered to `onError` and the
 * stream is not read further.
 */

/**
 * One resolved citation, exactly as the server's `citations` event carries it.
 *
 * Two shapes share the array. A chunk citation names a passage in the student's
 * own material and is opened in a panel; a web citation names a page outside it
 * and carries no chunk, source or page, because there is no chunk behind it: its
 * destination is the URL. One answer can cite both kinds, so every consumer of
 * this list handles a mixed array rather than assuming the first element's shape.
 */
export interface ChunkCitation {
  type: 'chunk'
  chunk_id: string
  source_id: string
  page: number | null
}

export interface WebCitation {
  type: 'web'
  url: string
  title: string
  /**
   * The DOI where the tier provided one. An academic hit is identified by its
   * DOI rather than by whichever host served it, so it is shown when present.
   *
   * Optional rather than nullable-only: the server sends `null`, but a citation
   * built by hand (a fixture, an older server) may omit the key entirely, and a
   * required field would turn that into a render-time crash.
   */
  doi?: string | null
}

export type ChatCitation = ChunkCitation | WebCitation

export interface ChatUsage {
  credits_spent: number
  credits_remaining: number
}

export interface ChatDone {
  message_id: string
  used_web: boolean
}

export interface ChatStreamHandlers {
  /** A text fragment. Not a word and not a line: concatenate in arrival order. */
  onToken?(text: string): void
  /** The citations for the answer that just streamed. May be empty. */
  onCitations?(citations: ChatCitation[]): void
  onUsage?(usage: ChatUsage): void
  /** The answer is complete and persisted. `message_id` is its row. */
  onDone?(done: ChatDone): void
  /** A refusal or a failure, at any position in the stream. */
  onError?(error: ApiError): void
}

/** Builds the typed error a malformed frame produces. */
function malformed(reason: string): ApiError {
  return new ApiError({ status: 502, code: 'provider_error', message: reason })
}

function errorFromEnvelope(data: string): ApiError {
  let body: unknown
  try {
    body = JSON.parse(data) as unknown
  } catch {
    return malformed('The error event was not readable JSON.')
  }
  const envelope = typeof body === 'object' && body !== null ? (body as Record<string, unknown>) : {}
  const code = typeof envelope.code === 'string' ? envelope.code : null
  if (code === null) return malformed('The error event carried no code.')
  // The status is reconstructed from the code, because a frame inside a 200
  // response never had one. `apiErrorFromBody` still does the validation, so an
  // unrecognised code cannot reach a screen as itself.
  return apiErrorFromBody(statusForCode(code as ApiErrorCode), body)
}

/**
 * Keeps only citations that match the contract.
 *
 * An item the client cannot render is dropped rather than failing the whole
 * answer, which mirrors the server's own rule for labels it cannot resolve: a
 * citation the user cannot open must not be shown, and one bad row must not cost
 * the user the answer it came with.
 *
 * Both shapes are validated the same way, against what the chip needs to render:
 * a chunk needs an id to open and a source to name, a web citation needs a title
 * to show and a URL to open. Anything short of that is dropped, so a malformed
 * row cannot reach a screen as a chip that claims less than it shows.
 */
function citationsFrom(data: string): ChatCitation[] | null {
  let body: unknown
  try {
    body = JSON.parse(data) as unknown
  } catch {
    return null
  }
  const raw = (body as { citations?: unknown } | null)?.citations
  if (!Array.isArray(raw)) return null
  const kept: ChatCitation[] = []
  for (const item of raw) {
    if (typeof item !== 'object' || item === null) continue
    const candidate = item as Record<string, unknown>
    if (candidate.type === 'web') {
      if (typeof candidate.url !== 'string' || candidate.url.length === 0) continue
      if (typeof candidate.title !== 'string' || candidate.title.length === 0) continue
      kept.push({ type: 'web', url: candidate.url, title: candidate.title })
      continue
    }
    if (candidate.type !== 'chunk') continue
    if (typeof candidate.chunk_id !== 'string' || candidate.chunk_id.length === 0) continue
    if (typeof candidate.source_id !== 'string' || candidate.source_id.length === 0) continue
    const page = typeof candidate.page === 'number' ? candidate.page : null
    kept.push({ type: 'chunk', chunk_id: candidate.chunk_id, source_id: candidate.source_id, page })
  }
  return kept
}

/**
 * A parser for the chat stream's frames, fed arbitrary chunks.
 *
 * Separate from the transport so the three framing cases above can be tested by
 * feeding strings, with no socket, no `fetch` and no timing involved.
 */
export function createChatEventParser(handlers: ChatStreamHandlers): {
  feed(chunk: string): void
} {
  const parser = createParser({
    onEvent(message: EventSourceMessage) {
      const { event, data } = message

      // The upstream's end-of-stream sentinel. Our `done` event is the real
      // signal; this frame carries no information and must never be read as text.
      if (data === '[DONE]') return

      // Comments never reach `onEvent` (the parser drops `:` lines), so anything
      // here without a name is a frame the contract does not define. Ignoring it
      // is safer than guessing which event it was.
      if (event === undefined) return

      switch (event) {
        case 'token': {
          let body: unknown
          try {
            body = JSON.parse(data) as unknown
          } catch {
            handlers.onError?.(malformed('A token event was not readable JSON.'))
            return
          }
          const text = (body as { t?: unknown } | null)?.t
          if (typeof text !== 'string') {
            handlers.onError?.(malformed('A token event carried no text.'))
            return
          }
          handlers.onToken?.(text)
          return
        }
        case 'citations': {
          const citations = citationsFrom(data)
          if (citations === null) {
            handlers.onError?.(malformed('A citations event was not a citation list.'))
            return
          }
          handlers.onCitations?.(citations)
          return
        }
        case 'usage': {
          let body: unknown
          try {
            body = JSON.parse(data) as unknown
          } catch {
            handlers.onError?.(malformed('A usage event was not readable JSON.'))
            return
          }
          const usage = body as { credits_spent?: unknown; credits_remaining?: unknown } | null
          if (typeof usage?.credits_spent !== 'number' || typeof usage.credits_remaining !== 'number') {
            handlers.onError?.(malformed('A usage event carried no counts.'))
            return
          }
          handlers.onUsage?.({
            credits_spent: usage.credits_spent,
            credits_remaining: usage.credits_remaining,
          })
          return
        }
        case 'done': {
          let body: unknown
          try {
            body = JSON.parse(data) as unknown
          } catch {
            handlers.onError?.(malformed('A done event was not readable JSON.'))
            return
          }
          const done = body as { message_id?: unknown; used_web?: unknown } | null
          if (typeof done?.message_id !== 'string') {
            handlers.onError?.(malformed('A done event carried no message id.'))
            return
          }
          handlers.onDone?.({ message_id: done.message_id, used_web: done.used_web === true })
          return
        }
        case 'error': {
          handlers.onError?.(errorFromEnvelope(data))
          return
        }
        default:
          return
      }
    },
  })

  return {
    feed(chunk: string) {
      parser.feed(chunk)
    },
  }
}

export interface ChatStreamInput {
  workspaceId: string
  message: string
  /** Narrows retrieval to these sources. Omitted means the whole workspace. */
  sourceIds?: string[]
  /**
   * Whether this turn may also search outside the workspace.
   *
   * Required rather than optional, and always sent: the client always knows the
   * answer (the toggle is either on or off), and a body that sometimes omits the
   * field makes "the toggle was off" and "the client is too old to have one"
   * indistinguishable on the wire.
   */
  useWeb: boolean
  signal?: AbortSignal
}

async function readJsonBody(response: Response): Promise<unknown> {
  try {
    const text = await response.text()
    return text.length === 0 ? undefined : (JSON.parse(text) as unknown)
  } catch {
    return undefined
  }
}

function post(input: ChatStreamInput): Promise<Response> {
  const body: Record<string, unknown> = { message: input.message, use_web: input.useWeb }
  if (input.sourceIds !== undefined && input.sourceIds.length > 0) body.source_ids = input.sourceIds

  const token = getAccessToken()
  return fetch(`${API_BASE_URL}/workspaces/${input.workspaceId}/chat`, {
    method: 'POST',
    headers: {
      accept: 'text/event-stream',
      'content-type': 'application/json',
      ...(token === null ? {} : { authorization: `Bearer ${token}` }),
    },
    body: JSON.stringify(body),
    ...(input.signal ? { signal: input.signal } : {}),
  })
}

/**
 * Sends a message and drives the handlers as the answer arrives.
 *
 * A refusal that arrives *before* the stream (a 402 from the middleware, say) and
 * one that arrives *inside* it both end up in `onError` with the same typed
 * error, so the screen has one branch rather than two.
 */
export async function streamChat(
  input: ChatStreamInput,
  handlers: ChatStreamHandlers,
): Promise<void> {
  let response: Response | null = null

  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      response = await post(input)
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') throw error
      handlers.onError?.(
        new ApiError({
          status: 0,
          code: 'network',
          message: `Could not reach the server at ${API_BASE_URL}.`,
        }),
      )
      return
    }

    if (response.ok) break

    const error = apiErrorFromBody(response.status, await readJsonBody(response))
    // The same single-flight refresh as the JSON client: an expired access token
    // must not surface as a failed answer.
    if (attempt === 0 && error.isAuthFailure && (await ensureFreshSession())) continue
    handlers.onError?.(error)
    return
  }

  if (response === null || !response.ok || response.body === null) {
    handlers.onError?.(malformed('The answer stream did not start.'))
    return
  }

  const parser = createChatEventParser(handlers)
  const reader = response.body.getReader()
  const decoder = new TextDecoder()

  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      parser.feed(decoder.decode(value, { stream: true }))
    }
  } catch (error) {
    if (error instanceof Error && error.name === 'AbortError') throw error
    handlers.onError?.(
      new ApiError({
        status: 0,
        code: 'network',
        message: 'The connection to the server dropped before the answer finished.',
      }),
    )
  } finally {
    reader.releaseLock()
  }
}
