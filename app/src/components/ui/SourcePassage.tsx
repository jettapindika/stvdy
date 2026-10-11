import { asApiError } from '@/api/errors'
import { useChunk } from '@/api/queries/chat'
import { ApiErrorNotice } from '@/components/ui/States'

/**
 * The passage a generated item came from.
 *
 * A card or a question carries only `chunk_id`, so the source's title is looked up from
 * the workspace's source list by the chunk's `source_id`. Both queries are the ones the
 * chat screen already uses and are cached, so this adds no new API surface.
 *
 * Shared by the study screen and the quiz screen rather than written twice: both show
 * the passage behind a generated item, and a second copy would be a second place for the
 * loading, error and page-label behaviour to drift.
 */
export function SourcePassage({
  chunkId,
  sources,
  label = 'Source passage',
}: {
  chunkId: string
  sources: { id: string; title: string }[]
  label?: string
}) {
  const chunk = useChunk(chunkId)
  const error = asApiError(chunk.error)
  const title =
    chunk.data !== undefined
      ? (sources.find((source) => source.id === chunk.data.source_id)?.title ?? 'A source')
      : null

  return (
    <section aria-label={label} className="note flex flex-col gap-2">
      {chunk.isPending && <p className="text-sm text-ink-muted">Loading the source passage</p>}

      {error !== null && (
        <ApiErrorNotice
          error={error}
          noun="this passage"
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void chunk.refetch()}
            >
              Try again
            </button>
          }
        />
      )}

      {chunk.data !== undefined && (
        <>
          <p className="meta">
            {title}
            {chunk.data.page !== null && ` page ${String(chunk.data.page)}`}
          </p>
          <p className="whitespace-pre-wrap break-words text-sm text-ink-muted">
            {chunk.data.content}
          </p>
        </>
      )}
    </section>
  )
}
