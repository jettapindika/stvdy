import { Fragment, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { Message } from '@/api/queries/chat'
import type { ChatCitation, ChunkCitation } from '@/api/sse'
import { ApiErrorNotice } from '@/components/ui/States'
import { WebCitationMark } from '@/components/ui/Citation'
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

/**
 * The inline markup the answers actually use. Nothing else is interpreted.
 *
 * Both label families are recognised, and recognised *exactly* as the server
 * parses them: `[1]`..`[999]` for chunks and `[W1]`..`[W99]` for web results, no
 * leading zero. The strictness is load-bearing rather than tidiness. The wire
 * carries no labels, so the mapping below is positional, and a marker this regex
 * counted that the server did not would shift every citation after it onto the
 * wrong claim. `[0]`, `[01]` and `[1000]` are therefore text, not markers.
 */
const INLINE = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[(?:[1-9]\d{0,2}|[wW][1-9]\d?)\])/g
const MARKER = /^\[(?:[1-9]\d{0,2}|[wW][1-9]\d?)\]$/

/**
 * The citation behind each marker in an answer, by first appearance.
 *
 * The wire contract never sends a label: the server resolves `[1]` to a chunk,
 * drops the label, and stores only the resolved citation. But `resolve` returns
 * the citations in the order the labels first appear in the text, so the k-th
 * distinct marker in the answer is the k-th citation. That is the mapping used
 * here, and it is the reason a claim can be pointed at individually instead of
 * only cited at the end of the paragraph.
 *
 * Chunk and web labels are scanned in one pass, because the server resolves them
 * in one pass: an answer that cites a web result before a page sends the web
 * citation first, and a client that scanned only `[n]` would hand that web
 * citation to the page's marker. Case is folded to the server's own reading, so
 * `[w1]` and `[W1]` are one label rather than two.
 *
 * A marker with no citation left for it is left as text rather than guessed at:
 * the server strips unknown labels, so this only happens if it failed to, and a
 * chip on the wrong chunk is a false claim about the student's own material.
 */
export function markersFor(text: string, citations: ChatCitation[]): Map<string, ChatCitation> {
  const found = new Map<string, ChatCitation>()
  let next = 0
  for (const match of text.matchAll(/\[(?:[1-9]\d{0,2}|[wW][1-9]\d?)\]/g)) {
    const marker = match[0].toUpperCase()
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
  onOpenCitation: (citation: ChunkCitation) => void,
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
      const citation = markers.get(part.toUpperCase())
      // A marker whose citation is the other kind is left as text. The server
      // resolves each label against its own family, so this only happens when the
      // two scans disagree, and a web citation rendered as a page (or the
      // reverse) would misattribute the claim rather than merely look wrong.
      if (citation === undefined) return <Fragment key={key}>{part}</Fragment>
      if (citation.type === 'web') return <WebCitationMark key={key} citation={citation} />
      if (part.toUpperCase().startsWith('[W')) return <Fragment key={key}>{part}</Fragment>
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
  onOpenCitation: (citation: ChunkCitation) => void
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
  const { t } = useTranslation()
  const isUser = role === 'user'
  return (
    <article className="flex flex-col gap-1">
      <span className="label">{isUser ? t('chat.turn.you') : t('app.name')}</span>
      <div
        className={
          isUser
            ? 'max-w-prose rounded-card bg-surface-raised px-3 py-2 text-sm'
            : 'max-w-prose text-sm leading-relaxed'
        }
      >
        {children}
      </div>
    </article>
  )
}

/**
 * How many distinct sources the answer drew on.
 *
 * Distinct sources, not citations: an answer that cites three pages of one
 * textbook is "based on 1 source", which is the fact a student can act on. The
 * number is counted from the citations the server resolved, so it cannot claim
 * more grounding than the answer actually has, and it is hidden entirely when
 * there are none — an answer from general knowledge or from web results alone
 * must not be dressed up with a source count of zero.
 */
function SourceCount({ citations }: { citations: ChatCitation[] }) {
  const { t } = useTranslation()
  const sourceIds = new Set(
    citations.flatMap((citation) =>
      citation.type === 'chunk' && citation.source_id ? [citation.source_id] : [],
    ),
  )
  if (sourceIds.size === 0) return null
  return <p className="meta mt-1">{t('honesty.basedOnSources', { count: sourceIds.size })}</p>
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
  onOpenCitation: (citation: ChunkCitation) => void
  onRetry: () => void
  /** Why the thread is empty, when it is. */
  emptyReason: string
}) {
  const { t } = useTranslation()
  const streaming = turn?.streaming === true

  if (messages.length === 0 && turn === null) {
    return (
      <div className="flex flex-col items-start gap-2 px-4 py-6">
        <p className="font-medium">{t('chat.empty.title')}</p>
        <p className="max-w-prose text-sm text-muted">{emptyReason}</p>
      </div>
    )
  }

  return (
    <div
      role="log"
      aria-live="polite"
      aria-busy={streaming}
      aria-label={t('chat.list.label')}
      className="flex flex-col gap-5 px-4 py-4"
    >
      {messages.map((message) => (
        <Turn key={message.id} role={message.role}>
          {message.role === 'assistant' ? (
            <>
              <AnswerText
                text={message.content}
                citations={message.citations}
                sourceTitle={sourceTitle}
                onOpenCitation={onOpenCitation}
              />
              <SourceCount citations={message.citations} />
            </>
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
              <SourceCount citations={turn.citations} />
            </Turn>
          )}

          {turn.streaming && turn.text.length === 0 && (
            <p className="flex items-center gap-2 text-muted" role="status">
              <span
                className="h-2 w-2 animate-working rounded-full bg-muted"
                aria-hidden="true"
              />
              {t('chat.list.searching')}
            </p>
          )}

          {turn.error !== null && (
            <ApiErrorNotice
              error={turn.error}
              noun={t('errors.nouns.answers')}
              action={
                <button type="button" className="btn btn-secondary" onClick={onRetry}>
                  {t('chat.retry')}
                </button>
              }
            />
          )}
        </>
      )}
    </div>
  )
}
