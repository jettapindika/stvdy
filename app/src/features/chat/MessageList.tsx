import { Fragment, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { Message } from '@/api/queries/chat'
import type { ChatCitation, ChunkCitation } from '@/api/sse'
import { ApiErrorNotice } from '@/components/ui/States'
import { WebCitationMark } from '@/components/ui/Citation'
import { Icon, type IconName } from '@/components/ui/Icon'
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
 * Inline markup the answers use. Extended to handle strikethrough and links.
 *
 * Every group inside the outer capture MUST be non-capturing (`(?:…)`).
 * `String.split` splices capture-group matches into the result array, so a
 * capturing group leaks its match as a separate literal-text entry beside the
 * full match. The link pattern uses `(?:…)` for both its label and URL parts.
 */
const INLINE =
  /(\[(?:[^\]]+)\]\((?:https?:\/\/[^\s)]+)\)|~~[^~\n]+~~|`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[(?:[1-9]\d{0,2}|[wW][1-9]\d?)\])/g
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
    if (part === undefined || part.length === 0) return null

    // Link: [text](url) — the INLINE groups are non-capturing, so split
    // yields only the full `[text](url)` match. We extract parts with exec.
    const linkMatch = /^\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)$/.exec(part)
    if (linkMatch !== null) {
      return (
        <a
          key={key}
          href={linkMatch[2]}
          target="_blank"
          rel="noopener noreferrer"
          className="text-primary-text underline underline-offset-2"
        >
          {linkMatch[1]}
        </a>
      )
    }
    // Strikethrough: ~~text~~
    if (part.startsWith('~~') && part.endsWith('~~') && part.length > 3) {
      return <del key={key}>{part.slice(2, -2)}</del>
    }
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

// ---------------------------------------------------------------------------
// Callout types — the `> [!TYPE]` admonition blocks
// ---------------------------------------------------------------------------

type CalloutKind =
  | 'note'
  | 'tip'
  | 'important'
  | 'warning'
  | 'caution'
  | 'example'
  | 'definition'
  | 'formula'
  | 'memorize'
  | 'step'
  | 'analogy'
  | 'trap'
  | 'question'
  | 'summary'
  | 'quote'
  | 'success'
  | 'highlight'
  | 'code'

const CALLOUT_META: Record<CalloutKind, { icon: IconName; className: string }> = {
  note:       { icon: 'help',         className: 'callout-note' },
  tip:        { icon: 'sparkles',     className: 'callout-tip' },
  important:  { icon: 'alert',        className: 'callout-important' },
  warning:    { icon: 'alert',        className: 'callout-warning' },
  caution:    { icon: 'alert',        className: 'callout-caution' },
  example:    { icon: 'list-checks',  className: 'callout-example' },
  definition: { icon: 'study',        className: 'callout-definition' },
  formula:    { icon: 'chart',        className: 'callout-formula' },
  memorize:   { icon: 'bookmark',     className: 'callout-memorize' },
  step:       { icon: 'play',         className: 'callout-step' },
  analogy:    { icon: 'link',         className: 'callout-analogy' },
  trap:       { icon: 'alert',        className: 'callout-trap' },
  question:   { icon: 'help',         className: 'callout-question' },
  summary:    { icon: 'layers',       className: 'callout-summary' },
  quote:      { icon: 'file-text',    className: 'callout-quote' },
  success:    { icon: 'circle-check', className: 'callout-success' },
  highlight:  { icon: 'sparkles',     className: 'callout-highlight' },
  code:       { icon: 'presentation', className: 'callout-code' },
}

/**
 * Detect if a line is a callout header: `> [!TYPE] optional title`
 *
 * Aliases cover common model drift: `[!INFO]` → note, `[!DANGER]` → caution,
 * `[!WARN]` → warning, `[!HINT]` → tip, `[!BUG]` → trap, `[!ABSTRACT]` → summary,
 * `[!CHECK]` → success, `[!FAQ]` → question, `[!CITE]` → quote, `[!MEMO]` → memorize,
 * `[!CONCEPT]` → definition, `[!MATH]` → formula, `[!NOTICE]` → note,
 * `[!PITFALL]` → trap, `[!REMEMBER]` → memorize, `[!SNIPPET]` → code.
 */
const CALLOUT_ALIASES: Record<string, CalloutKind> = {
  info: 'note',
  notice: 'note',
  danger: 'caution',
  warn: 'warning',
  hint: 'tip',
  bug: 'trap',
  pitfall: 'trap',
  abstract: 'summary',
  tldr: 'summary',
  check: 'success',
  done: 'success',
  faq: 'question',
  cite: 'quote',
  citation: 'quote',
  memo: 'memorize',
  remember: 'memorize',
  concept: 'definition',
  math: 'formula',
  equation: 'formula',
  snippet: 'code',
}

function parseCalloutHeader(line: string): { kind: CalloutKind; title: string } | null {
  const m = /^>\s*\[!(\w+)\]\s*(.*)$/.exec(line)
  if (m === null) return null
  const raw = m[1]!.toLowerCase()
  const kind: CalloutKind | undefined =
    (raw in CALLOUT_META ? raw as CalloutKind : undefined) ?? CALLOUT_ALIASES[raw]
  if (kind === undefined) return null
  return { kind, title: m[2]!.trim() }
}

/**
 * The answer body, now handling the full range of Markdown a language model
 * produces for educational content.
 *
 * Supported block types (in parse priority order):
 * 1. Fenced code blocks (``` with optional language)
 * 2. Callout/admonition blocks (> [!TYPE] title)
 * 3. Headings (## .. ####, no h1 — the answer card is the h1)
 * 4. Horizontal rules (--- or ***)
 * 5. Tables (| col | col |)
 * 6. Ordered lists (1. / 2. / ...)
 * 7. Unordered lists (- / * prefix)
 * 8. Blockquotes (> prefix, plain, non-callout)
 * 9. Paragraphs (everything else)
 *
 * Inline: bold, italic, inline code, strikethrough, links, citation markers.
 *
 * Security: no raw HTML. Links are sanitised to https? only. The renderer
 * cannot be tricked into executing a `javascript:` URL or rendering markup
 * from a source document.
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
  const { t } = useTranslation()
  const markers = markersFor(text, citations)

  const renderInline = (s: string, kp: string) =>
    inlineNodes(s, markers, sourceTitle, onOpenCitation, kp)

  // Split into raw lines for block-level parsing
  const rawLines = text.split('\n')
  const elements: ReactNode[] = []
  let i = 0

  while (i < rawLines.length) {
    const line = rawLines[i]!

    // --- Fenced code block ---
    const fenceMatch = /^(`{3,}|~{3,})(\w*)/.exec(line)
    if (fenceMatch !== null) {
      const fence = fenceMatch[1]!
      const lang = fenceMatch[2] ?? ''
      const codeLines: string[] = []
      i += 1
      while (i < rawLines.length && !rawLines[i]!.startsWith(fence)) {
        codeLines.push(rawLines[i]!)
        i += 1
      }
      i += 1 // skip closing fence
      elements.push(
        <div key={`code-${String(elements.length)}`} className="answer-code-block">
          {lang.length > 0 && (
            <span className="answer-code-lang">{lang}</span>
          )}
          <pre><code>{codeLines.join('\n')}</code></pre>
        </div>,
      )
      continue
    }

    // --- Callout block: > [!TYPE] title, then > body lines ---
    const calloutHeader = parseCalloutHeader(line)
    if (calloutHeader !== null) {
      const { kind, title } = calloutHeader
      const meta = CALLOUT_META[kind]
      const bodyLines: string[] = []
      i += 1
      while (i < rawLines.length) {
        const cl = rawLines[i]!
        // Continuation: starts with > or is blank (inside callout)
        if (/^>\s?/.test(cl)) {
          bodyLines.push(cl.replace(/^>\s?/, ''))
          i += 1
        } else if (cl.trim() === '' && i + 1 < rawLines.length && /^>\s?/.test(rawLines[i + 1]!)) {
          // Blank line between callout paragraphs
          bodyLines.push('')
          i += 1
        } else {
          break
        }
      }
      const displayTitle = title.length > 0 ? title : t(`chat.callout.${kind}`)
      elements.push(
        <div key={`callout-${String(elements.length)}`} className={`answer-callout ${meta.className}`}>
          <div className="answer-callout-header">
            <Icon name={meta.icon} size={18} />
            <span className="answer-callout-title">{displayTitle}</span>
          </div>
          {bodyLines.length > 0 && (
            <div className="answer-callout-body">
              {renderInnerBlocks(bodyLines, renderInline, elements.length)}
            </div>
          )}
        </div>,
      )
      continue
    }

    // --- Heading (## to ####) ---
    const headingMatch = /^(#{2,4})\s+(.+)$/.exec(line)
    if (headingMatch !== null) {
      const level = headingMatch[1]!.length as 2 | 3 | 4
      const Tag = `h${String(level)}` as 'h2' | 'h3' | 'h4'
      elements.push(
        <Tag key={`h-${String(elements.length)}`} className="answer-heading">
          {renderInline(headingMatch[2]!, `h-${String(elements.length)}`)}
        </Tag>,
      )
      i += 1
      continue
    }

    // --- Horizontal rule ---
    if (/^(?:[-*_]){3,}\s*$/.test(line)) {
      elements.push(<hr key={`hr-${String(elements.length)}`} className="border-border" />)
      i += 1
      continue
    }

    // --- Table: requires a header row + separator row (|---|) minimum ---
    if (
      line.trim().startsWith('|') &&
      i + 1 < rawLines.length &&
      /^[|\s:-]+$/.test(rawLines[i + 1]!.trim())
    ) {
      const tableLines: string[] = [line]
      i += 1
      while (i < rawLines.length && rawLines[i]!.trim().startsWith('|')) {
        tableLines.push(rawLines[i]!)
        i += 1
      }
      const parseRow = (row: string) =>
        row.split('|').slice(1, -1).map((c) => c.trim())
      const headers = parseRow(tableLines[0]!)
      // Skip separator line (|---|---|)
      const rows = tableLines.slice(2).map(parseRow)
      elements.push(
        <div key={`tbl-${String(elements.length)}`} className="answer-table-wrap">
          <table className="answer-table">
            <thead>
              <tr>
                {headers.map((h, hi) => (
                  <th key={`th-${String(hi)}`}>
                    {renderInline(h, `th-${String(elements.length)}-${String(hi)}`)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, ri) => (
                <tr key={`tr-${String(ri)}`}>
                  {row.map((cell, ci) => (
                    <td key={`td-${String(ci)}`}>
                      {renderInline(cell, `td-${String(elements.length)}-${String(ri)}-${String(ci)}`)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      )
      continue
    }

    // --- Ordered list ---
    if (/^\s*\d+[.)]\s+/.test(line)) {
      const items: string[] = []
      while (i < rawLines.length && /^\s*\d+[.)]\s+/.test(rawLines[i]!)) {
        items.push(rawLines[i]!.replace(/^\s*\d+[.)]\s+/, ''))
        i += 1
      }
      elements.push(
        <ol key={`ol-${String(elements.length)}`} className="answer-ol">
          {items.map((item, li) => (
            <li key={`ol-${String(elements.length)}-${String(li)}`}>
              {renderInline(item, `ol-${String(elements.length)}-${String(li)}`)}
            </li>
          ))}
        </ol>,
      )
      continue
    }

    // --- Unordered list ---
    if (/^\s*[-*+]\s+/.test(line)) {
      const items: string[] = []
      while (i < rawLines.length && /^\s*[-*+]\s+/.test(rawLines[i]!)) {
        items.push(rawLines[i]!.replace(/^\s*[-*+]\s+/, ''))
        i += 1
      }
      elements.push(
        <ul key={`ul-${String(elements.length)}`} className="list-disc pl-5">
          {items.map((item, li) => (
            <li key={`ul-${String(elements.length)}-${String(li)}`}>
              {renderInline(item, `ul-${String(elements.length)}-${String(li)}`)}
            </li>
          ))}
        </ul>,
      )
      continue
    }

    // --- Plain blockquote (not a callout) ---
    if (/^>\s?/.test(line)) {
      const quoteLines: string[] = []
      while (i < rawLines.length && /^>\s?/.test(rawLines[i]!)) {
        quoteLines.push(rawLines[i]!.replace(/^>\s?/, ''))
        i += 1
      }
      elements.push(
        <blockquote key={`bq-${String(elements.length)}`} className="answer-blockquote">
          {quoteLines.map((ql, qi) => (
            <p key={`bq-${String(elements.length)}-${String(qi)}`}>
              {renderInline(ql, `bq-${String(elements.length)}-${String(qi)}`)}
            </p>
          ))}
        </blockquote>,
      )
      continue
    }

    // --- Blank line: skip ---
    if (line.trim() === '') {
      i += 1
      continue
    }

    // --- Paragraph: collect consecutive non-special lines ---
    const paraLines: string[] = []
    while (
      i < rawLines.length &&
      rawLines[i]!.trim() !== '' &&
      !/^(`{3,}|~{3,})/.test(rawLines[i]!) &&
      !/^#{2,4}\s+/.test(rawLines[i]!) &&
      !/^(?:[-*_]){3,}\s*$/.test(rawLines[i]!) &&
      !/^\s*[-*+]\s+/.test(rawLines[i]!) &&
      !/^\s*\d+[.)]\s+/.test(rawLines[i]!) &&
      !/^>\s?/.test(rawLines[i]!)
    ) {
      paraLines.push(rawLines[i]!)
      i += 1
    }
    if (paraLines.length > 0) {
      elements.push(
        <p key={`p-${String(elements.length)}`} className="whitespace-pre-wrap">
          {paraLines.map((pl, pi) => (
            <Fragment key={`p-${String(elements.length)}-${String(pi)}`}>
              {pi > 0 && '\n'}
              {renderInline(pl, `p-${String(elements.length)}-${String(pi)}`)}
            </Fragment>
          ))}
        </p>,
      )
    } else {
      // Safety: a line matched no block branch and the paragraph exclusions
      // rejected it (e.g. a partial heading `## ` mid-stream). Advance past
      // it so the outer loop cannot spin forever.
      i += 1
    }
  }

  return <>{elements}</>
}

/**
 * Renders inner blocks within a callout body (supports paragraphs, lists,
 * code blocks inside callouts).
 */
function renderInnerBlocks(
  lines: string[],
  inline: (t: string, kp: string) => ReactNode[],
  parentKey: number,
): ReactNode[] {
  const out: ReactNode[] = []
  let j = 0
  while (j < lines.length) {
    const ln = lines[j]!

    // Fenced code inside callout
    const fenceM = /^(`{3,}|~{3,})(\w*)/.exec(ln)
    if (fenceM !== null) {
      const fence = fenceM[1]!
      const lang = fenceM[2] ?? ''
      const codeLines: string[] = []
      j += 1
      while (j < lines.length && !lines[j]!.startsWith(fence)) {
        codeLines.push(lines[j]!)
        j += 1
      }
      j += 1
      out.push(
        <div key={`ic-${String(parentKey)}-${String(out.length)}`} className="answer-code-block">
          {lang.length > 0 && <span className="answer-code-lang">{lang}</span>}
          <pre><code>{codeLines.join('\n')}</code></pre>
        </div>,
      )
      continue
    }

    // Ordered list inside callout
    if (/^\s*\d+[.)]\s+/.test(ln)) {
      const items: string[] = []
      while (j < lines.length && /^\s*\d+[.)]\s+/.test(lines[j]!)) {
        items.push(lines[j]!.replace(/^\s*\d+[.)]\s+/, ''))
        j += 1
      }
      out.push(
        <ol key={`io-${String(parentKey)}-${String(out.length)}`} className="answer-ol">
          {items.map((item, li) => (
            <li key={`io-${String(parentKey)}-${String(out.length)}-${String(li)}`}>
              {inline(item, `io-${String(parentKey)}-${String(out.length)}-${String(li)}`)}
            </li>
          ))}
        </ol>,
      )
      continue
    }

    // Bullet list inside callout
    if (/^\s*[-*+]\s+/.test(ln)) {
      const items: string[] = []
      while (j < lines.length && /^\s*[-*+]\s+/.test(lines[j]!)) {
        items.push(lines[j]!.replace(/^\s*[-*+]\s+/, ''))
        j += 1
      }
      out.push(
        <ul key={`iu-${String(parentKey)}-${String(out.length)}`} className="list-disc pl-5">
          {items.map((item, li) => (
            <li key={`iu-${String(parentKey)}-${String(out.length)}-${String(li)}`}>
              {inline(item, `iu-${String(parentKey)}-${String(out.length)}-${String(li)}`)}
            </li>
          ))}
        </ul>,
      )
      continue
    }

    // Blank line
    if (ln.trim() === '') {
      j += 1
      continue
    }

    // Paragraph
    const pLines: string[] = []
    while (
      j < lines.length &&
      lines[j]!.trim() !== '' &&
      !/^(`{3,}|~{3,})/.test(lines[j]!) &&
      !/^\s*[-*+]\s+/.test(lines[j]!) &&
      !/^\s*\d+[.)]\s+/.test(lines[j]!)
    ) {
      pLines.push(lines[j]!)
      j += 1
    }
    if (pLines.length > 0) {
      out.push(
        <p key={`ip-${String(parentKey)}-${String(out.length)}`} className="whitespace-pre-wrap">
          {pLines.map((pl, pi) => (
            <Fragment key={`ip-${String(parentKey)}-${String(out.length)}-${String(pi)}`}>
              {pi > 0 && '\n'}
              {inline(pl, `ip-${String(parentKey)}-${String(out.length)}-${String(pi)}`)}
            </Fragment>
          ))}
        </p>,
      )
    } else {
      j += 1
    }
  }
  return out
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

function StreamingIndicator({
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
  const { t } = useTranslation()
  return (
    <article className="flex flex-col gap-2 px-2">
      <div className="flex items-center gap-2">
        <span className="eyebrow-primary">{t('app.name')}</span>
        <span className="text-label text-muted">{t('chat.turn.streaming')}</span>
      </div>
      {text.length > 0 && (
        <div className="max-w-prose text-body leading-relaxed">
          <AnswerText
            text={text}
            citations={citations}
            sourceTitle={sourceTitle}
            onOpenCitation={onOpenCitation}
          />
          <span className="streaming-cursor" aria-hidden="true" />
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
            <StreamingIndicator
              text={turn.text}
              citations={turn.citations}
              sourceTitle={sourceTitle}
              onOpenCitation={onOpenCitation}
            />
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
