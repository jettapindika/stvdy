import type { WebCitation } from '@/api/sse'
import { platform } from '@/platform'

/**
 * The citation mark: the product's core mechanic and its identity motif.
 *
 * A bracket label (`[1]`, `[W2]`) is how an answer points back at a page in the
 * student's own material. It gets the accent and the one place a pill is
 * correct, because it is the single gesture that makes this interface belong to
 * this product (DESIGN.md).
 *
 * Interactivity tracks reality (R-26). A citation is only a control when it has
 * somewhere to go: a web citation is always a real link, while a chunk citation
 * becomes a button only once a caller passes `onOpen`. With no handler it renders
 * as plain text rather than a button that swallows clicks. Nothing here implies
 * grounding that does not exist either: an empty list renders nothing.
 *
 * The two kinds look different because they *are* different. A chunk mark shows
 * the page, since that is what the server sent and what a bracket label would
 * have to invent. A web mark shows the title and the host, and opens the URL in
 * the system browser.
 */

export type CitationSource =
  | { type: 'chunk'; label: number; sourceTitle: string; page: number | null }
  | WebCitation

export type ChunkCitation = Extract<CitationSource, { type: 'chunk' }>

/**
 * The host a web citation opens on, without the `www.` a reader does not say.
 *
 * Derived from the URL rather than sent by the server: the citation carries the
 * whole URL, and the host is the part that tells two results apart at a glance.
 * A URL the platform cannot parse falls back to the whole string, which is still
 * the truth about where the link goes.
 */
function hostOf(url: string): string {
  try {
    const host = new URL(url).hostname.replace(/^www\./, '')
    return host.length > 0 ? host : url
  } catch {
    return url
  }
}

/** Whether a URL is one a browser can be handed, as opposed to a `javascript:`. */
function isBrowsable(url: string): boolean {
  try {
    const protocol = new URL(url).protocol
    return protocol === 'http:' || protocol === 'https:'
  } catch {
    return false
  }
}

function describe(citation: CitationSource): string {
  if (citation.type === 'chunk') {
    const page = citation.page === null ? 'page unknown' : `page ${citation.page}`
    return `${citation.sourceTitle}, ${page}`
  }
  return `${citation.title}, ${hostOf(citation.url)}`
}

/**
 * A citation of something outside the workspace.
 *
 * It shows the title and the host, because a URL is not readable at chip size
 * and a bare title does not say where the claim came from. It is an anchor with a
 * real `href`, so the URL is visible on hover, copyable, and openable in a new
 * tab by a middle click, and the click handler hands the URL to the platform's
 * `openExternal` so the shell opens the student's own browser. It deliberately
 * does not navigate: a webview inside the app would replace the answer being read
 * with a remote page, and on desktop it would load the open internet into the
 * app's own process.
 *
 * A URL that is not `http(s)` renders as plain text rather than as a control that
 * cannot go anywhere, which is the same rule the chunk mark follows.
 */
export function WebCitationMark({ citation }: { citation: WebCitation }) {
  const host = hostOf(citation.url)
  const label = `${citation.title}, ${host}`

  if (!isBrowsable(citation.url)) {
    return (
      <span className="cite cite-web" title={label} aria-label={`Web source: ${label}`}>
        <span className="cite-title">{citation.title}</span>
        <span className="cite-host">{host}</span>
      </span>
    )
  }

  return (
    <a
      className="cite cite-web"
      href={citation.url}
      title={label}
      aria-label={`Open web source: ${label}`}
      onClick={(event) => {
        // The default is prevented even though the href is correct: without it
        // the browser would also follow the link inside the app.
        event.preventDefault()
        void platform.openExternal(citation.url).catch(() => {
          // Nothing on this screen can report a browser that refused to open.
          // Swallowing the rejection keeps it from surfacing as an unhandled one
          // while the chip stays where it is and the URL is still visible in it.
        })
      }}
    >
      <span className="cite-title">{citation.title}</span>
      <span className="cite-host">{host}</span>
      {citation.doi ? <span className="cite-doi">{citation.doi}</span> : null}
    </a>
  )
}

export function Citation({
  citation,
  onOpen,
}: {
  citation: CitationSource
  /** Opens the cited chunk at its page. Omit when there is nowhere to go. */
  onOpen?: (citation: ChunkCitation) => void
}) {
  if (citation.type === 'web') return <WebCitationMark citation={citation} />

  const label = describe(citation)

  if (!onOpen) {
    // No destination yet, so no control. Plain text with a tooltip still tells
    // the student which source the claim came from.
    return (
      <span className="cite" title={label} aria-label={`Source: ${label}`}>
        {citation.label}
      </span>
    )
  }

  return (
    <button
      type="button"
      className="cite"
      onClick={() => onOpen(citation)}
      title={label}
      aria-label={`Open source: ${label}`}
    >
      {citation.label}
    </button>
  )
}

export function CitationList({
  citations,
  onOpen,
}: {
  citations: CitationSource[]
  onOpen?: (citation: ChunkCitation) => void
}) {
  if (citations.length === 0) return null
  return (
    <span className="inline-flex flex-wrap items-baseline gap-1">
      {citations.map((citation, index) => (
        <Citation
          key={`${citation.type}-${citation.type === 'web' ? citation.url : String(citation.label)}-${String(index)}`}
          citation={citation}
          onOpen={onOpen}
        />
      ))}
    </span>
  )
}
