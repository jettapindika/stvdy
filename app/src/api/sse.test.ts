import { describe, expect, it } from 'vitest'

import { ApiError } from './errors'
import { createChatEventParser, createSyncEventParser, type ChatCitation } from './sse'

/**
 * The stream parser, fed by hand.
 *
 * Framing is tested without a socket on purpose. The three cases that break a
 * hand-written SSE reader are all about *where a chunk boundary lands*, and a
 * test that drove them through a real `ReadableStream` would be asserting on the
 * boundary the reader happened to produce rather than on the one it was asked
 * for. Here the bytes are chosen.
 */

function collect() {
  const tokens: string[] = []
  const citations: ChatCitation[][] = []
  const done: { message_id: string; used_web: boolean }[] = []
  const actions: { pending_action_id: string; tool: string; args: Record<string, unknown> }[] = []
  const errors: ApiError[] = []

  const parser = createChatEventParser({
    onToken: (text) => tokens.push(text),
    onCitations: (list) => citations.push(list),
    onDone: (value) => done.push(value),
    onAction: (action) => actions.push(action),
    onError: (error) => errors.push(error),
  })

  return { parser, tokens, citations, done, actions, errors }
}

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`

describe('the chat stream parser', () => {
  it('parses a frame that is split across two reads', () => {
    const { parser, tokens } = collect()

    // The boundary lands in the middle of the event name and in the middle of
    // the JSON, which is what a real reader hands back.
    parser.feed('event: tok')
    parser.feed('en\ndata: {"t":"Diksi')
    parser.feed('onary"}\n\n')

    expect(tokens).toEqual(['Diksionary'])
  })

  it('parses a frame split at every single character', () => {
    const { parser, tokens, done } = collect()

    const stream =
      frame('token', { t: 'Hasil' }) + frame('done', { message_id: 'm1', used_web: false })
    for (const character of stream) parser.feed(character)

    expect(tokens).toEqual(['Hasil'])
    expect(done).toEqual([{ message_id: 'm1', used_web: false }])
  })

  it('ignores heartbeat comment lines rather than treating them as events', () => {
    const { parser, tokens, errors, done } = collect()

    parser.feed(': heartbeat\n\n')
    parser.feed(frame('token', { t: 'a' }))
    parser.feed(': heartbeat\n\n')
    parser.feed(frame('token', { t: 'b' }))
    parser.feed(': heartbeat\n\n')
    parser.feed(frame('done', { message_id: 'm1', used_web: false }))

    expect(tokens).toEqual(['a', 'b'])
    expect(errors).toEqual([])
    expect(done).toHaveLength(1)
  })

  it('tolerates the upstream [DONE] sentinel without reading it as text', () => {
    const { parser, tokens, done, errors } = collect()

    parser.feed(frame('token', { t: 'ok' }))
    parser.feed('data: [DONE]\n\n')
    parser.feed(frame('done', { message_id: 'm1', used_web: false }))

    expect(tokens).toEqual(['ok'])
    expect(errors).toEqual([])
    expect(done).toHaveLength(1)
  })

  it('keeps token fragments in arrival order, including empty ones', () => {
    const { parser, tokens } = collect()

    parser.feed(frame('token', { t: 'Ja' }))
    parser.feed(frame('token', { t: 'karta' }))
    parser.feed(frame('token', { t: '' }))
    parser.feed(frame('token', { t: ' adalah' }))

    expect(tokens.join('')).toBe('Jakarta adalah')
  })

  it('carries the citations event, including an empty list', () => {
    const { parser, citations } = collect()

    parser.feed(
      frame('citations', {
        citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      }),
    )
    parser.feed(frame('citations', { citations: [] }))

    expect(citations).toEqual([
      [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      [],
    ])
  })

  it('treats a null page as a page the source does not have', () => {
    const { parser, citations } = collect()

    parser.feed(frame('citations', { citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: null }] }))

    const first = citations[0]?.[0]
    expect(first?.type).toBe('chunk')
    // Narrowed rather than cast: `page` exists only on the chunk variant, and the
    // point of the union is that a caller cannot read it off a web citation.
    if (first?.type === 'chunk') expect(first.page).toBeNull()
  })

  it('drops a citation it could not render instead of losing the answer', () => {
    const { parser, citations } = collect()

    parser.feed(
      frame('citations', {
        citations: [
          { type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 3 },
          { type: 'chunk', chunk_id: '', source_id: 's1', page: 4 },
          { type: 'web', url: 'https://example.com' },
          'not an object',
        ],
      }),
    )

    expect(citations).toEqual([[{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 3 }]])
  })

  it('reports a usage event with its counts', () => {
    const usage: { credits_spent: number; credits_remaining: number }[] = []
    const parser = createChatEventParser({
      onUsage: (value) => usage.push(value),
    })

    parser.feed(frame('usage', { credits_spent: 1, credits_remaining: 412 }))

    expect(usage).toEqual([{ credits_spent: 1, credits_remaining: 412 }])
  })

  it('surfaces an error event as the same typed error every other route produces', () => {
    const { parser, errors, tokens } = collect()

    parser.feed(
      frame('error', {
        code: 'insufficient_credits',
        message: 'balance is short',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )

    expect(errors).toHaveLength(1)
    expect(errors[0]?.code).toBe('insufficient_credits')
    expect(errors[0]?.status).toBe(402)
    expect(tokens).toEqual([])
  })

  it('surfaces an error event that arrives after tokens', () => {
    const { parser, errors, tokens } = collect()

    parser.feed(frame('token', { t: 'Sebagian jawaban' }))
    parser.feed(frame('error', { code: 'provider_error', message: 'upstream died' }))

    expect(tokens).toEqual(['Sebagian jawaban'])
    expect(errors[0]?.code).toBe('provider_error')
  })

  it('reports a frame it cannot read rather than throwing', () => {
    const { parser, errors, tokens } = collect()

    parser.feed('event: token\ndata: {not json\n\n')
    parser.feed(frame('token', { t: 'still here' }))

    expect(errors[0]?.code).toBe('provider_error')
    expect(tokens).toEqual(['still here'])
  })

  it('reports a done event without a message id', () => {
    const { parser, errors, done } = collect()

    parser.feed(frame('done', { used_web: false }))

    expect(done).toEqual([])
    expect(errors).toHaveLength(1)
  })

  it('parses an action frame into onAction, mid-answer', () => {
    // `action` is one of the six names the roadmap § 4 fixes, and it is a new event
    // on *this* stream rather than a second stream: a proposal belongs to the turn
    // that produced it. It carries no cost (A277) — the card reads the price from
    // `plans.ts` — so this is the whole payload.
    const { parser, tokens, actions, errors } = collect()

    parser.feed(frame('token', { t: 'Menyiapkan ' }))
    parser.feed(
      frame('action', {
        pending_action_id: 'a1',
        tool: 'create_task',
        args: { title: 'Selesaikan tugas kriptografi' },
      }),
    )
    parser.feed(frame('token', { t: 'kartunya.' }))

    expect(actions).toEqual([
      {
        pending_action_id: 'a1',
        tool: 'create_task',
        args: { title: 'Selesaikan tugas kriptografi' },
      },
    ])
    // The answer keeps streaming around it: the card does not interrupt the text.
    expect(tokens).toEqual(['Menyiapkan ', 'kartunya.'])
    expect(errors).toEqual([])
  })

  it('reports an action frame that names no tool, rather than rendering an empty card', () => {
    const { parser, actions, errors } = collect()

    parser.feed(frame('action', { pending_action_id: 'a1' }))

    expect(actions).toEqual([])
    expect(errors).toHaveLength(1)
  })

  it('reports an action frame whose arguments are not an object', () => {
    // The card renders exactly what was proposed, so an arguments payload that is
    // not an object must not reach it as `{}` — that would show an action with no
    // fields and no way to tell it from a real one.
    const { parser, actions, errors } = collect()

    parser.feed(frame('action', { pending_action_id: 'a1', tool: 'create_task', args: 'nope' }))

    expect(actions).toEqual([])
    expect(errors).toHaveLength(1)
  })

  it('ignores an event name the contract does not define', () => {
    // Forward compatibility, and it is the reason the frame table has a `default`:
    // a server that grows a seventh event kind must not break an older client.
    const { parser, tokens, errors, done } = collect()

    parser.feed(frame('future_thing', { anything: true }))
    parser.feed(frame('token', { t: 'ok' }))

    expect(errors).toEqual([])
    expect(tokens).toEqual(['ok'])
    expect(done).toEqual([])
  })
})

/**
 * The sync stream's parser, which is deliberately a separate one.
 *
 * `GET /sync/stream` shares the reader loop with the chat stream and nothing else:
 * one `seq` event instead of four kinds plus citations and a `[DONE]` sentinel, and
 * a `seq` that means "your change log moved" rather than "here is the change". The
 * cases below are the two that a shared parser would get wrong: a chat event
 * arriving on the sync stream, and a `seq` frame whose number is not a number.
 */
describe('the sync stream parser', () => {
  function collectSeq() {
    const seqs: number[] = []
    const errors: ApiError[] = []
    const parser = createSyncEventParser({
      onSeq: (seq) => seqs.push(seq),
      onError: (error) => errors.push(error),
    })
    return { parser, seqs, errors }
  }

  it('reports the seq of a frame, split across reads', () => {
    const { parser, seqs } = collectSeq()

    parser.feed('event: seq\ndata: {"s')
    parser.feed('eq": 41}\n\n')

    expect(seqs).toEqual([41])
  })

  it('ignores an event name the sync contract does not define', () => {
    const { parser, seqs, errors } = collectSeq()

    // A chat `token` frame must not be read as a seq, and must not be reported as
    // an error either: a server that grows a second event kind must not break an
    // older client.
    parser.feed(frame('token', { t: 'hello' }))
    parser.feed(frame('done', { message_id: 'm1' }))

    expect(seqs).toEqual([])
    expect(errors).toEqual([])
  })

  it('reports a seq frame it cannot read rather than throwing', () => {
    const { parser, seqs, errors } = collectSeq()

    parser.feed('event: seq\ndata: not json\n\n')
    parser.feed(frame('seq', { seq: '41' }))

    expect(seqs).toEqual([])
    expect(errors.map((error) => error.code)).toEqual(['provider_error', 'provider_error'])
  })
})
