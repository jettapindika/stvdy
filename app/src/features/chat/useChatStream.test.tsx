import type { ReactNode } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useChatStream } from './useChatStream'
import { chatKeys } from '@/api/queries/chat'
import {
  installFakePlatform,
  manualStreamResponse,
  mockFetch,
  streamResponse,
  type ManualStream,
  type RecordedCall,
} from '@/test/harness'
import { setAccessToken } from '@/lib/tokens'

/**
 * The streaming hook.
 *
 * The frames are pushed by the test rather than enqueued all at once. A body that
 * arrives in one go is read in a single microtask chain, so React batches every
 * token into one render and the intermediate states this screen exists to show,
 * a partial answer, a citation that lands before the answer ends, are never
 * observable. Pushing one frame at a time is what makes "appends in order" a real
 * assertion rather than a restatement of `join`.
 *
 * The behaviour worth pinning beyond that is what survives a failure. A dropped
 * connection mid-answer must not throw away the text the student has already
 * read, and a refusal that arrives before any token must leave *no* assistant
 * turn at all, because the server persisted nothing and a half-turn on screen
 * would claim a message that does not exist on the student's other device.
 */

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`

function harness(workspaceId = 'w1') {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const view = renderHook(() => useChatStream(workspaceId), {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  })
  return { client, view }
}

/** A mocked stream, wired so an abort fails it the way a real `fetch` does. */
function serveStream(stream: ManualStream): RecordedCall[] {
  return mockFetch((_url, init) => {
    stream.attach(init.signal)
    return stream.response
  })
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('a completed answer', () => {
  it('appends tokens in order as each one arrives', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Apa ibu kota Indonesia?', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    // Each fragment is a state the screen renders, so each is asserted on. The
    // first two are not whole words: `token` carries text, not words.
    act(() => {
      stream.push(frame('token', { t: 'Ibu ' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Ibu ')
    })

    act(() => {
      stream.push(frame('token', { t: 'kota ' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Ibu kota ')
    })

    act(() => {
      stream.push(frame('token', { t: 'adalah Jakarta' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Ibu kota adalah Jakarta')
    })

    expect(view.result.current.turn?.streaming).toBe(true)
    expect(view.result.current.turn?.error).toBeNull()
  })

  it('joins fragments that split a single word', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Apa itu?', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('token', { t: 'Diksi' }))
      stream.push(frame('token', { t: 'on' }))
      stream.push(frame('token', { t: 'ari' }))
    })

    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Diksionari')
    })
  })

  it('resolves the citations event, including an empty list', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('token', { t: 'Jawaban' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Jawaban')
    })

    act(() => {
      stream.push(
        frame('citations', {
          citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
        }),
      )
    })
    await waitFor(() => {
      expect(view.result.current.turn?.citations).toHaveLength(1)
    })

    expect(view.result.current.turn?.citations[0]).toEqual({
      type: 'chunk',
      chunk_id: 'c1',
      source_id: 's1',
      page: 12,
    })

    // A second citations event with nothing in it replaces the first, which is
    // what the contract means by "may arrive with an empty array".
    act(() => {
      stream.push(frame('citations', { citations: [] }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.citations).toEqual([])
    })
  })

  it('reports what the turn spent', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Halo', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('usage', { credits_spent: 1, credits_remaining: 411 }))
    })

    await waitFor(() => {
      expect(view.result.current.turn?.usage).toEqual({ credits_spent: 1, credits_remaining: 411 })
    })
  })

  it('sends the selected sources, and the whole workspace when none are selected', async () => {
    const calls = mockFetch(() =>
      streamResponse([frame('token', { t: 'ok' }), frame('done', { message_id: 'm1', used_web: false })]),
    )

    const { view } = harness()
    act(() => {
      view.result.current.send({
        message: 'Halo',
        sourceIds: ['s1', 's2'],
        useWeb: false,
        sourcesOnly: true,
      })
    })
    await waitFor(() => {
      expect(calls).toHaveLength(1)
    })

    expect(JSON.parse(String(calls[0]?.init.body))).toEqual({
      message: 'Halo',
      source_ids: ['s1', 's2'],
      use_web: false,
      sources_only: true,
    })

    act(() => {
      view.result.current.send({ message: 'Lagi', sourceIds: [], useWeb: false, sourcesOnly: true })
    })
    await waitFor(() => {
      expect(calls).toHaveLength(2)
    })
    // No empty `source_ids`: an empty array would read as "no sources", which is
    // not what "the whole workspace" means. `use_web` is always sent, because the
    // client always knows the toggle's state and an absent field would make "off"
    // and "too old to ask" indistinguishable on the wire.
    expect(JSON.parse(String(calls[1]?.init.body))).toEqual({
      message: 'Lagi',
      use_web: false,
      sources_only: true,
    })

    // The bearer token is on the stream request, which is the reason the
    // transport cannot be `EventSource`.
    expect((calls[0]?.init.headers as Record<string, string>).authorization).toBe('Bearer access-1')
    expect((calls[0]?.init.headers as Record<string, string>).accept).toBe('text/event-stream')
  })

  it('refetches the thread after done, then drops the live turn', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { client, view } = harness()
    const invalidated: unknown[] = []
    const original = client.invalidateQueries.bind(client)
    vi.spyOn(client, 'invalidateQueries').mockImplementation(async (filters) => {
      invalidated.push(filters)
      return await original(filters)
    })

    act(() => {
      view.result.current.send({ message: 'Halo', sourceIds: [], useWeb: false, sourcesOnly: true })
    })
    act(() => {
      stream.push(frame('token', { t: 'ok' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('ok')
    })

    act(() => {
      stream.push(frame('done', { message_id: 'm1', used_web: false }))
    })

    await waitFor(() => {
      expect(view.result.current.turn).toBeNull()
    })
    // The refetch is what makes the finished answer the server's row rather than
    // a local copy, so it is the thing to assert rather than the local text.
    expect(invalidated).toEqual([{ queryKey: chatKeys.messages('w1') }])
  })
})

describe('an error event before any token', () => {
  it('surfaces the error and leaves no partial assistant turn', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(
        frame('error', {
          code: 'insufficient_credits',
          message: 'balance is short',
          min_plan: null,
          limit: null,
          used: null,
        }),
      )
    })

    await waitFor(() => {
      expect(view.result.current.turn?.error?.code).toBe('insufficient_credits')
    })

    const turn = view.result.current.turn
    // No text, so nothing that could be mistaken for an answer. The question is
    // still shown, because the student typed it.
    expect(turn?.text).toBe('')
    expect(turn?.citations).toEqual([])
    expect(turn?.streaming).toBe(false)
    expect(turn?.question).toBe('Tanya')
    expect(turn?.error?.status).toBe(402)
  })

  it('surfaces a refusal that arrives as an HTTP status before the stream opens', async () => {
    mockFetch(() => streamResponse([], 402))

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    await waitFor(() => {
      expect(view.result.current.turn?.error).not.toBeNull()
    })
    // A 402 with no body: the code is derived from the status, which is what the
    // server answers for a spend that was refused before the stream opened.
    expect(view.result.current.turn?.error?.code).toBe('insufficient_credits')
    expect(view.result.current.turn?.text).toBe('')
    expect(view.result.current.turn?.streaming).toBe(false)
  })

  it('surfaces an unverified account as its own code, not as a plan limit', async () => {
    mockFetch(() =>
      streamResponse(
        [
          JSON.stringify({
            code: 'email_not_verified',
            message: 'verify first',
            min_plan: null,
            limit: null,
            used: null,
          }),
        ],
        403,
      ),
    )

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    await waitFor(() => {
      expect(view.result.current.turn?.error?.code).toBe('email_not_verified')
    })
    expect(view.result.current.turn?.error?.isPlanLimit).toBe(false)
  })

  it('surfaces a server that cannot be reached at all', async () => {
    mockFetch(() => {
      throw new TypeError('failed to fetch')
    })

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    await waitFor(() => {
      expect(view.result.current.turn?.error?.code).toBe('network')
    })
    expect(view.result.current.turn?.text).toBe('')
  })
})

describe('a failure mid-stream', () => {
  it('keeps the partial text visible and retries the same question', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('token', { t: 'Sebagian jawaban' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Sebagian jawaban')
    })

    act(() => {
      stream.fail(new TypeError('network error'))
    })

    await waitFor(() => {
      expect(view.result.current.turn?.error).not.toBeNull()
    })
    expect(view.result.current.turn?.text).toBe('Sebagian jawaban')
    expect(view.result.current.turn?.streaming).toBe(false)

    const retry = manualStreamResponse()
    const calls = mockFetch((_url, init) => {
      retry.attach(init.signal)
      return retry.response
    })
    act(() => {
      view.result.current.retry()
    })

    await waitFor(() => {
      expect(calls).toHaveLength(1)
    })
    expect(JSON.parse(String(calls[0]?.init.body))).toEqual({
      message: 'Tanya',
      use_web: false,
      sources_only: true,
    })

    act(() => {
      retry.push(frame('token', { t: 'Utuh' }))
    })

    // The retry starts a fresh turn, so the failed text is replaced by the new
    // answer rather than appended to it.
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Utuh')
    })
    expect(view.result.current.turn?.error).toBeNull()
    expect(view.result.current.turn?.streaming).toBe(true)
  })

  it('keeps the partial text when an error event arrives after tokens', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('token', { t: 'Separuh' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Separuh')
    })

    act(() => {
      stream.push(frame('error', { code: 'provider_error', message: 'the model failed' }))
    })

    await waitFor(() => {
      expect(view.result.current.turn?.error?.code).toBe('provider_error')
    })
    expect(view.result.current.turn?.text).toBe('Separuh')
    expect(view.result.current.turn?.streaming).toBe(false)
  })

  it('treats a body that ends without a done event as finished, not as still streaming', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('token', { t: 'Terpotong' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Terpotong')
    })

    act(() => {
      stream.close()
    })

    await waitFor(() => {
      expect(view.result.current.turn?.streaming).toBe(false)
    })
    expect(view.result.current.turn?.text).toBe('Terpotong')
  })
})

describe('stopping a stream', () => {
  it('abandons the answer and leaves the partial text in place', async () => {
    const stream = manualStreamResponse()
    serveStream(stream)

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Tanya', sourceIds: [], useWeb: false, sourcesOnly: true })
    })

    act(() => {
      stream.push(frame('token', { t: 'Mulai' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Mulai')
    })

    act(() => {
      view.result.current.stop()
    })

    // A frame the server had already sent, arriving after the student stopped.
    act(() => {
      stream.push(frame('token', { t: ' dan lagi' }))
    })

    await waitFor(() => {
      expect(view.result.current.turn?.streaming).toBe(false)
    })
    // Stopping is the student's own action, so it is not reported as a failure,
    // and the text they had already read stays where it is.
    expect(view.result.current.turn?.error).toBeNull()
    expect(view.result.current.turn?.text).toBe('Mulai')
  })

  it('ignores a late token from a stream a second question replaced', async () => {
    const first = manualStreamResponse()
    const second = manualStreamResponse()
    let served = 0
    mockFetch((_url, init) => {
      const stream = served === 0 ? first : second
      served += 1
      stream.attach(init.signal)
      return stream.response
    })

    const { view } = harness()
    act(() => {
      view.result.current.send({ message: 'Pertanyaan pertama', sourceIds: [], useWeb: false, sourcesOnly: true })
    })
    act(() => {
      first.push(frame('token', { t: 'Jawaban pertama' }))
    })
    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Jawaban pertama')
    })

    act(() => {
      view.result.current.send({ message: 'Pertanyaan kedua', sourceIds: [], useWeb: false, sourcesOnly: true })
    })
    await waitFor(() => {
      expect(view.result.current.turn?.question).toBe('Pertanyaan kedua')
    })

    // The abandoned stream had already sent this. It must not be appended to the
    // answer that replaced it, which is what the attempt guard is for.
    act(() => {
      first.push(frame('token', { t: ' tertinggal' }))
    })
    act(() => {
      second.push(frame('token', { t: 'Jawaban kedua' }))
    })

    await waitFor(() => {
      expect(view.result.current.turn?.text).toBe('Jawaban kedua')
    })
    expect(view.result.current.turn?.question).toBe('Pertanyaan kedua')
  })
})
