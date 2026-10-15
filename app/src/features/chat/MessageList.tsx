import { Fragment, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { Message } from '@/api/queries/chat'
import type { ChatCitation, ChunkCitation } from '@/api/sse'
import { ApiErrorNotice } from '@/components/ui/States'
import { WebCitationMark } from '@/components/ui/Citation'
import { Icon } from '@/components/ui/Icon'
import { AGENT_CREDITS } from '@/features/billing/plans'
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
 * The server resolves markers before this runs, so the citations array is
 * ordered by first occurrence in the text, which is the same order a student
 * reads them in. Markers are case-insensitive (`[W1]` and `[w1]` are the same
 * citation) and only the first occurrence is counted.
 *
 * Two kinds share the list. A chunk citation names a passage in the student's
 * own material (`[1]`..`[999]`); a web citation names a page outside it
 * (`[W1]`..`[W99]`). They map to different controls on screen, so both the
 * regex above and this scan must agree on which kind of marker they see. The
 * convention is `[N]` = chunk, `[WN]` = web, and the sets are disjoint on
 * purpose: renaming either would merge them and misattribute every citation
 * after the first.
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
      // A marker whose citation is the other kind is left as text.
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
 * both of which are risks this renderer does not take.
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

// ---------------------------------------------------------------------------
// Turn containers
// ---------------------------------------------------------------------------

function UserTurn({ children }: { children: ReactNode }) {
  const { t } = useTranslation()
  return (
    <article className="flex flex-col gap-1">
      <span className="eyebrow-primary">{t('chat.turn.you')}</span>
      <div className="max-w-prose rounded-card bg-surface-raised px-4 py-3 text-body">
        {children}
      </div>
    </article>
  )
}

/**
 * The source-grounded answer card: a bordered card carrying the grounded
 * eyebrow, the answer body, citation chips, a divider, and the AI disclaimer.
 *
 * This is the Figma design 411:44258's `Source-grounded answer` specimen.
 */
function AssistantAnswer({
  text,
  citations,
  sourceTitle,
  onOpenCitation,
  children,
}: {
  text: string
  citations: ChatCitation[]
  sourceTitle: (sourceId: string) => string
  onOpenCitation: (citation: ChunkCitation) => void
  children?: ReactNode
}) {
  const { t } = useTranslation()
  const sourceIds = new Set(
    citations.flatMap((c) => (c.type === 'chunk' && c.source_id ? [c.source_id] : [])),
  )
  // The grounded eyebrow names the source when there is exactly one, which is
  // the design's "GROUNDED IN LECTURE 03". When there are several or none, it
  // falls back to the count (the existing `SourceCount` behaviour).
  const groundedLabel =
    sourceIds.size === 1
      ? sourceTitle([...sourceIds][0] as string)
      : sourceIds.size === 0
        ? null
        : null

  // Citation chips: the distinct chunk citations
  const chunkCitations = citations.filter(
    (c): c is ChunkCitation => c.type === 'chunk',
  )
  // Build a label string for the citation chips row
  const chipLabels = chunkCitations
    .map((c) => {
      const title = sourceTitle(c.source_id)
      const page = c.page !== null ? `p. ${String(c.page)}` : title
      return `[${page}]`
    })
    .join(' · ')

  return (
    <article className="answer-card">
      {/* Grounded eyebrow */}
      {groundedLabel !== null && (
        <div className="flex items-center gap-2">
          <Icon name="study" size={20} className="text-source" />
          <span className="eyebrow-source">
            {t('honesty.basedOnSources_one', { count: 1 }).toUpperCase().replace(/\d+/, '')}
            {groundedLabel.toUpperCase()}
          </span>
        </div>
      )}

      {/* Answer body */}
      <div className="max-w-prose text-label leading-relaxed">
        <AnswerText
          text={text}
          citations={citations}
          sourceTitle={sourceTitle}
          onOpenCitation={onOpenCitation}
        />
      </div>

      {/* Citation chips row */}
      {chipLabels.length > 0 && (
        <span className="chip chip-source">{chipLabels}</span>
      )}

      {/* Grounding footer: source count + flashcard/save/copy actions */}
      <GroundingFooter citations={citations} answerText={text} />

      {/* Divider + AI disclaimer */}
      <hr className="border-border" />
      <p className="text-caption text-muted">
        {t('chat.answer.disclaimer')}
      </p>

      {children}
    </article>
  )
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

function StreamingIndicator({ text }: { text: string }) {
  const { t } = useTranslation()
  return (
    <article className="flex flex-col gap-2 px-2">
      <div className="flex items-center gap-2">
        <span className="eyebrow-primary">{t('app.name')}</span>
        <span className="text-label text-muted">{t('chat.turn.streaming')}</span>
      </div>
      {text.length > 0 && (
        <div className="max-w-prose text-body leading-relaxed">
          <p className="whitespace-pre-wrap">
            {text}
            <span className="streaming-cursor" aria-hidden="true" />
          </p>
        </div>
      )}
      {text.length === 0 && (
        <p className="flex items-center gap-2 text-muted" role="status">
          <span className="h-2 w-2 animate-working rounded-full bg-muted" aria-hidden="true" />
          {t('chat.list.searching')}
        </p>
      )}
    </article>
  )
}

// ---------------------------------------------------------------------------
// Thinking disclosure
// ---------------------------------------------------------------------------

function ThinkingDisclosure({ reasoning }: { reasoning: string }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

  if (reasoning.length === 0) return null

  // Show a truncated preview of the reasoning
  const preview = reasoning.length > 80 ? `${reasoning.slice(0, 80)}…` : reasoning

  return (
    <div className="flex flex-col gap-1">
      <button
        type="button"
        className="thinking-toggle"
        onClick={() => { setOpen(!open) }}
        aria-expanded={open}
      >
        <Icon name={open ? 'chevron-down' : 'chevron-right'} size={16} />
        <span>{open ? t('chat.thinking.hide') : t('chat.thinking.show')}</span>
        {!open && (
          <span className="text-small text-subtle truncate">{preview}</span>
        )}
      </button>
      {open && (
        <div className="thinking-content whitespace-pre-wrap">
          {reasoning}
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Evidence link
// ---------------------------------------------------------------------------

function EvidenceLink({
  citations,
  sourceTitle,
  onOpenCitation,
}: {
  citations: ChunkCitation[]
  sourceTitle: (sourceId: string) => string
  onOpenCitation: (citation: ChunkCitation) => void
}) {
  const { t } = useTranslation()
  if (citations.length === 0) return null

  // Show the first chunk citation as the evidence link
  const cite = citations[0] as ChunkCitation
  const title = sourceTitle(cite.source_id)
  const pages = citations
    .filter((c) => c.page !== null)
    .map((c) => String(c.page))

  const pageRange = pages.length > 0 ? `pp. ${pages.join('–')}` : ''
  const label = `[1] ${title}${pageRange.length > 0 ? ` · ${pageRange}` : ''}`

  return (
    <div className="evidence-link">
      <div className="flex items-start gap-2">
        <Icon name="file-text" size={20} className="shrink-0 text-source" />
        <span className="text-label text-source flex-1">{label}</span>
      </div>
      <button
        type="button"
        className="btn btn-secondary w-full"
        onClick={() => { onOpenCitation(cite) }}
      >
        {t('chat.answer.viewCited')}
      </button>
    </div>
  )
}


// ---------------------------------------------------------------------------
// Grounding footer (411:49140)
// ---------------------------------------------------------------------------

/**
 * The grounding footer: source count, flashcard action with real credit cost
 * from `AGENT_CREDITS`, Save to Studio, and copy.
 *
 * `SourceCount` is subsumed here rather than duplicated. The source count line
 * uses the existing `honesty.basedOnSources` key (plural-aware, counting
 * distinct `source_id`s), and falls back to `honesty.noSourcesCited` when the
 * answer cited nothing. The flashcard action reads `AGENT_CREDITS.flashcards`,
 * which is parsed from `config/plans.toml` — the same file the server charges
 * from, so the number cannot drift.
 */
function GroundingFooter({
  citations,
  answerText,
}: {
  citations: ChatCitation[]
  answerText: string
}) {
  const { t } = useTranslation()
  const [copied, setCopied] = useState(false)
  const sourceIds = new Set(
    citations.flatMap((c) => (c.type === 'chunk' && c.source_id ? [c.source_id] : [])),
  )

  return (
    <div className="flex flex-col gap-2">
      {/* Source count line */}
      {sourceIds.size === 0 ? (
        <p className="text-label text-muted">{t('honesty.noSourcesCited')}</p>
      ) : (
        <p className="text-label text-source">
          {t('honesty.basedOnSources', { count: sourceIds.size })}
        </p>
      )}

      {/* Action row: flashcard proposal, save to studio, copy */}
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className="btn btn-secondary flex-1 min-w-0">
          {t('chat.answer.makeFlashcards', { count: 10, credits: AGENT_CREDITS.flashcards })}
        </button>
        <button type="button" className="btn btn-secondary flex-1 min-w-0">
          {t('chat.answer.saveToStudio')}
        </button>
        <button
          type="button"
          className="answer-action shrink-0"
          aria-label={t('chat.answer.copy')}
          onClick={() => {
            void navigator.clipboard.writeText(answerText).then(() => {
              setCopied(true)
              setTimeout(() => { setCopied(false) }, 2000)
            })
          }}
        >
          <Icon name={copied ? 'check' : 'copy'} size={20} />
        </button>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Follow-up prompts (411:44295)
// ---------------------------------------------------------------------------

/**
 * Two affordance prompts after the last completed answer: "Show a worked
 * example" and "Make flashcards from this". These are copy, not fabricated
 * data — they are the same kind of thing as the empty-state strings, and they
 * are localized through `chat.followUp.workedExample` / `.flashcards`.
 *
 * The `onSelect` callback fills the composer's draft with the chosen prompt,
 * so the student sees what will be sent and can edit it before sending.
 */
function FollowUpPrompts({
  onSelect,
  disabled,
}: {
  onSelect: (text: string) => void
  disabled: boolean
}) {
  const { t } = useTranslation()
  const prompts = [
    t('chat.followUp.workedExample'),
    t('chat.followUp.flashcards'),
  ]

  return (
    <div className="flex flex-col gap-2">
      <span className="eyebrow">{t('chat.followUp.heading')}</span>
      {prompts.map((text) => (
        <button
          key={text}
          type="button"
          className="btn btn-secondary w-full"
          disabled={disabled}
          onClick={() => { onSelect(text) }}
        >
          {text}
        </button>
      ))}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Main export
// ---------------------------------------------------------------------------

export function MessageList({
  messages,
  turn,
  sourceTitle,
  onOpenCitation,
  onRetry,
  emptyReason,
  onFollowUp,
}: {
  messages: Message[]
  /** The answer currently arriving, or the one that failed. */
  turn: ChatTurn | null
  sourceTitle: (sourceId: string) => string
  onOpenCitation: (citation: ChunkCitation) => void
  onRetry: () => void
  /** Why the thread is empty, when it is. */
  emptyReason: string
  /** Called when a follow-up prompt is selected; fills the composer draft. */
  onFollowUp?: (text: string) => void
}) {
  const { t } = useTranslation()
  const streaming = turn?.streaming === true

  if (messages.length === 0 && turn === null) {
    return (
      <div className="flex flex-col items-start gap-2 px-5 py-6 md:px-6">
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
      className="flex flex-col gap-5 px-5 py-4 md:px-6"
    >
      {messages.map((message) => {
        if (message.role === 'assistant') {
          const chunkCites = message.citations.filter(
            (c): c is ChunkCitation => c.type === 'chunk',
          )
          return (
            <Fragment key={message.id}>
              <AssistantAnswer
                text={message.content}
                citations={message.citations}
                sourceTitle={sourceTitle}
                onOpenCitation={onOpenCitation}
              />
              {chunkCites.length > 0 && (
                <EvidenceLink
                  citations={chunkCites}
                  sourceTitle={sourceTitle}
                  onOpenCitation={onOpenCitation}
                />
              )}
            </Fragment>
          )
        }

        return (
          <UserTurn key={message.id}>
            <p className="whitespace-pre-wrap">{message.content}</p>
          </UserTurn>
        )
      })}

      {turn !== null && (
        <>
          <UserTurn>
            <p className="whitespace-pre-wrap">{turn.question}</p>
          </UserTurn>

          {/* Thinking disclosure: shown when the provider sends reasoning */}
          {turn.reasoning.length > 0 && (
            <ThinkingDisclosure reasoning={turn.reasoning} />
          )}

          {/* Streaming answer or completed answer */}
          {turn.streaming ? (
            <StreamingIndicator text={turn.text} />
          ) : turn.text.length > 0 ? (
            <>
              <AssistantAnswer
                text={turn.text}
                citations={turn.citations}
                sourceTitle={sourceTitle}
                onOpenCitation={onOpenCitation}
              />
              {turn.citations.filter((c): c is ChunkCitation => c.type === 'chunk').length > 0 && (
                <EvidenceLink
                  citations={turn.citations.filter((c): c is ChunkCitation => c.type === 'chunk')}
                  sourceTitle={sourceTitle}
                  onOpenCitation={onOpenCitation}
                />
              )}
            </>
          ) : null}

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

      {/* Follow-up prompts after the last completed answer (411:44295) */}
      {!streaming && messages.length > 0 && onFollowUp !== undefined && (
        <FollowUpPrompts onSelect={onFollowUp} disabled={false} />
      )}
    </div>
  )
}
