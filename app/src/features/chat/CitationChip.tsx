import type { ChatCitation } from '@/api/sse'

/**
 * The citation mark, for a citation the stream delivered.
 *
 * It carries the `.cite` treatment, which is the product's identity motif, and it
 * is a button only because it has somewhere to go: clicking it opens the chunk it
 * names.
 *
 * **It shows the page, not a bracket number.** The design's `[1]` mark needs a
 * label, and the wire contract deliberately does not carry one: the server
 * resolves `[1]` to a chunk, stores the resolved citation, and never persists or
 * sends the label (labels are per-request and per-prompt). Numbering the chips by
 * their position in the `citations` array would print `[1]` beside whatever chunk
 * happened to resolve first, which for an answer that cites `[2]` before `[1]` is
 * simply a false claim about where the text came from. The page is a fact the
 * server did send, so the page is what is shown, and the source's title is in the
 * accessible name and in the panel the chip opens.
 */
export function CitationChip({
  citation,
  sourceTitle,
  onOpen,
}: {
  citation: ChatCitation
  /** The source's title, from the workspace's source list. */
  sourceTitle: string
  onOpen: (citation: ChatCitation) => void
}) {
  const where = citation.page === null ? 'page unknown' : `page ${String(citation.page)}`
  const label = `${sourceTitle}, ${where}`

  return (
    <button
      type="button"
      className="cite"
      onClick={() => {
        onOpen(citation)
      }}
      title={label}
      aria-label={`Open source: ${label}`}
    >
      {/* Pasted text has no pages, so the source's name is the only thing that
          can distinguish one chunk from another. */}
      {citation.page === null ? sourceTitle : `p.${String(citation.page)}`}
    </button>
  )
}
