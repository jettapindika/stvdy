import { Fragment, type ReactNode } from 'react'

import type { Message } from '@/api/queries/chat'
import type { ChatCitation } from '@/api/sse'
import { ApiErrorNotice } from '@/components/ui/States'
import { CitationChip } from './CitationChip'
import type { ChatTurn } from './useChatStream'

/**
 * The thread, plus the one answer that is still arriving.
 *
 * The live region is the list itself. Two things have to be true for a screen
 * reader and neither is visible on screen:
 *
 * - `aria-live="polite"` announces the answer when it lands. Without it the
 *   answer is silent: text appears in the document, but nothing tells the reader
 *   it is there, and a blind student is left with a question and no reply.
 * - `aria-busy` is true for exactly as long as tokens are arriving. A polite
 *   region whose text grows token by token would otherwise be announced
 *   fragmentarily, which is worse than silence: the reader gets a stream of
 *   half-words. Marking it busy defers the announcement to the finished answer.
 *
 * Nothing here moves focus, which is the other half of the same requirement. An
 * answer that steals focus from the composer would take the student out of the
 * field they are typing in the moment their first answer arrives.
 */

/** The inline markup the answers actually use. Nothing else is interpreted. */
const INLINE = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[\d+\])/g
const MARKER = /^\[(\d+)\]$/

/**
 * The chunk behind each `[n]` in an answer, by first appearance.
 *
 * The wire contract never sends a label: the server resolves `[1]` to a chunk,
 * drops the label, and stores only the resolved citation. But `resolve` returns
 * the citations in the order the labels first appear in the text, so the k-th
 * distinct marker in the answer is the k-th citation. That is the mapping used
 * here, and it is the reason a claim can be pointed at individually instead of
 * only cited at the end of the paragraph.
 *
 * A marker with no citation left for it is left as text rather than guessed at:
 * the server strips unknown labels, so this only happens if it failed to, and a
 * chip on the wrong chunk is a false claim about the student's own material.
 */
export function markersFor(text: string, citations: ChatCitation[]): Map<string, ChatCitation> {
  const found = new Map<string, ChatCitation>()
  let next = 0
  for (const match of text.matchAll(/\[\d+\]/g)) {
    const marker = match[0]
    if (found.has(marker)) continue
    const citation = citations[next]
    if (citation === undefined) continue
    found.set(marker, citation)
    next += 1
  }
  return found
}

function inlineNodes(
  text: string,
  markers: Map<string, ChatCitation>,
  sourceTitle: (sourceId: string) => string,
  onOpenCitation: (citation: ChatCitation) => void,
  keyPrefix: string,
): ReactNode[] {
  return text.split(INLINE).map((part, index) => {
    const key = `${keyPrefix}-${String(index)}`
    if (part.length === 0) return null

    if (part.startsWith('`') && part.endsWith('`') && part.length > 1) {
      return <code key={key}>{part.slice(1, -1)}</code>
    }
    if (part.startsWith('**') && part.endsWith('**') && part.length > 3) {
      return <strong key={key}>{part.slice(2, -2)}</strong>
    }
    if (part.startsWith('*') && part.endsWith('*') && part.length > 2) {
      return <em key={key}>{part.slice(1, -1)}</em>
    }
    if (MARKER.test(part)) {
      const citation = markers.get(part)
      if (citation === undefined) return <Fragment key={key}>{part}</Fragment>
      return (
        <CitationChip
          key={key}
          citation={citation}
          sourceTitle={sourceTitle(citation.source_id)}
          onOpen={onOpenCitation}
        />
      )
    }
    return <Fragment key={key}>{part}</Fragment>
  })
}

/**
 * The answer, as much of Markdown as a language model's prose needs.
 *
 * Paragraphs, bullet lists, bold, italic and inline code, and nothing else. A
 * full Markdown renderer would accept raw HTML and links the model invented,
 * which in an answer whose whole point is that it is grounded in the student's
 * own material is the wrong trade.
 */
function AnswerText({
  text,
  citations,
  sourceTitle,
  onOpenCitation,
}: {
  text: string
  citations: ChatCitation[]
  sourceTitle: (sourceId: string) => string
  onOpenCitation: (citation: ChatCitation) => void
}) {
  const markers = markersFor(text, citations)
  const blocks = text.split(/\n{2,}/)

  return (
    <>
      {blocks.map((block, blockIndex) => {
        const lines = block.split('\n')
        const isList = lines.every((line) => /^\s*[-*]\s+/.test(line))

        if (isList) {
          return (
            <ul key={`b${String(blockIndex)}`} className="list-disc pl-5">
              {lines.map((line, lineIndex) => (
                <li key={`b${String(blockIndex)}-${String(lineIndex)}`}>
                  {inlineNodes(
                    line.replace(/^\s*[-*]\s+/, ''),
                    markers,
                    sourceTitle,
                    onOpenCitation,
                    `b${String(blockIndex)}-${String(lineIndex)}`,
                  )}
                </li>
              ))}
            </ul>
          )
        }

        return (
          <p key={`b${String(blockIndex)}`} className="whitespace-pre-wrap">
            {lines.map((line, lineIndex) => (
              <Fragment key={`b${String(blockIndex)}-${String(lineIndex)}`}>
                {lineIndex > 0 && '\n'}
                {inlineNodes(
                  line,
                  markers,
                  sourceTitle,
                  onOpenCitation,
                  `b${String(blockIndex)}-${String(lineIndex)}`,
                )}
              </Fragment>
            ))}
          </p>
        )
      })}
    </>
  )
}

function Turn({
  role,
  children,
}: {
  role: 'user' | 'assistant'
  children: ReactNode
}) {
  const isUser = role === 'user'
  return (
    <article className="flex flex-col gap-1">
      <span className="label">{isUser ? 'You' : 'Stvdy'}</span>
      <div
        className={
          isUser
            ? 'max-w-prose rounded-card bg-surface-sunk px-3 py-2 text-sm'
            : 'max-w-prose text-sm leading-relaxed'
        }
      >
        {children}
      </div>
    </article>
  )
}

export function MessageList({
  messages,
  turn,
  sourceTitle,
  onOpenCitation,
  onRetry,
  emptyReason,
}: {
  messages: Message[]
  /** The answer currently arriving, or the one that failed. */
  turn: ChatTurn | null
  sourceTitle: (sourceId: string) => string
  onOpenCitation: (citation: ChatCitation) => void
  onRetry: () => void
  /** Why the thread is empty, when it is. */
  emptyReason: string
}) {
  const streaming = turn?.streaming === true

  if (messages.length === 0 && turn === null) {
    return (
      <div className="flex flex-col items-start gap-2 px-4 py-6">
        <p className="font-medium">No questions yet</p>
        <p className="max-w-prose text-sm text-ink-muted">{emptyReason}</p>
      </div>
    )
  }

  return (
    <div
      role="log"
      aria-live="polite"
      aria-busy={streaming}
      aria-label="Conversation"
      className="flex flex-col gap-5 px-4 py-4"
    >
      {messages.map((message) => (
        <Turn key={message.id} role={message.role}>
          {message.role === 'assistant' ? (
            <AnswerText
              text={message.content}
              citations={message.citations}
              sourceTitle={sourceTitle}
              onOpenCitation={onOpenCitation}
            />
          ) : (
            <p className="whitespace-pre-wrap">{message.content}</p>
          )}
        </Turn>
      ))}

      {turn !== null && (
        <>
          <Turn role="user">
            <p className="whitespace-pre-wrap">{turn.question}</p>
          </Turn>

          {/* An answer only exists once there is text. A refusal before the first
              token left nothing behind on the server and nothing here either, so
              the notice is not attributed to a reply: a "Stvdy" label over a
              message that was never written claims a turn that does not exist. */}
          {turn.text.length > 0 && (
            <Turn role="assistant">
              <AnswerText
                text={turn.text}
                citations={turn.citations}
                sourceTitle={sourceTitle}
                onOpenCitation={onOpenCitation}
              />
            </Turn>
          )}

          {turn.streaming && turn.text.length === 0 && (
            <p className="flex items-center gap-2 text-ink-muted" role="status">
              <span
                className="h-2 w-2 animate-working rounded-full bg-ink-muted"
                aria-hidden="true"
              />
              Searching this workspace
            </p>
          )}

          {turn.error !== null && (
            <ApiErrorNotice
              error={turn.error}
              noun="answers"
              action={
                <button type="button" className="btn btn-secondary" onClick={onRetry}>
                  Ask again
                </button>
              }
            />
          )}
        </>
      )}
    </div>
  )
}
