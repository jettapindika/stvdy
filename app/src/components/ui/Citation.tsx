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
 */

export type CitationSource =
  | { type: 'chunk'; label: number; sourceTitle: string; page: number | null }
  | { type: 'web'; label: string; url: string; title: string }

export type ChunkCitation = Extract<CitationSource, { type: 'chunk' }>
export type WebCitation = Extract<CitationSource, { type: 'web' }>

function describe(citation: CitationSource): string {
  if (citation.type === 'chunk') {
    const page = citation.page === null ? 'page unknown' : `page ${citation.page}`
    return `${citation.sourceTitle}, ${page}`
  }
  return `${citation.title}, web source`
}

export function Citation({
  citation,
  onOpen,
}: {
  citation: CitationSource
  /** Opens the cited chunk at its page. Omit when there is nowhere to go. */
  onOpen?: (citation: ChunkCitation) => void
}) {
  if (citation.type === 'web') {
    const label = describe(citation)
    return (
      <a
        className="cite"
        href={citation.url}
        target="_blank"
        rel="noreferrer noopener"
        title={label}
        aria-label={`Web source: ${label}`}
      >
        {citation.label}
      </a>
    )
  }

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
          key={`${citation.type}-${String(citation.label)}-${String(index)}`}
          citation={citation}
          onOpen={onOpen}
        />
      ))}
    </span>
  )
}
