import { useEffect, useId, useRef, useState } from 'react'

import { useChunk, useMessages } from '@/api/queries/chat'
import { useSources } from '@/api/queries/sources'
import { asApiError } from '@/api/errors'
import type { ChunkCitation } from '@/api/sse'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { MessageList } from './MessageList'
import { useChatStream } from './useChatStream'

/**
 * Chat: a question about this workspace's sources, answered from them and cited
 * back to the page it came from.
 *
 * Two things this screen deliberately does not do:
 *
 * - **It does not keep the thread in its own state.** The server persists both
 *   turns and `GET /messages` reads them back, so the list on screen is the
 *   list on the student's other device. Only the answer that is still streaming
 *   is local, because it has no row yet.
 * - **It does not promise results it cannot cite.** The web toggle asks the
 *   server to search outside the workspace as well; when that search finds
 *   nothing the answer is still written from the workspace's own material, which
 *   is what the copy beside the toggle says.
 *
 * The composer keeps its focus while an answer streams, and the message list is
 * the live region that announces it. Both are requirements rather than
 * preferences: focus moving out of the field a student is typing in, or an answer
 * arriving in silence, each breaks the screen for a keyboard or screen-reader
 * user even though it looks perfect.
 */
export function ChatScreen({ workspaceId }: { workspaceId: string }) {
  const messages = useMessages(workspaceId)
  const sources = useSources(workspaceId)
  const stream = useChatStream(workspaceId)

  const [draft, setDraft] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const [openCitation, setOpenCitation] = useState<ChunkCitation | null>(null)
  // Keyed by workspace so switching workspaces cannot carry one workspace's
  // answer into another's: a web search costs a credit and reaches the open
  // internet, and the choice to make it is per question, not per session.
  const [webByWorkspace, setWebByWorkspace] = useState<Record<string, boolean>>({})
  const useWeb = webByWorkspace[workspaceId] === true

  const listError = asApiError(messages.error)
  const readySources = (sources.data ?? []).filter((source) => source.status === 'ready')
  const streaming = stream.turn?.streaming === true
  const canSend = draft.trim().length > 0 && !streaming

  function submit() {
    const message = draft.trim()
    if (message.length === 0 || streaming) return
    stream.send({ message, sourceIds: selected, useWeb })
    setDraft('')
  }

  const titleOf = (sourceId: string) =>
    sources.data?.find((source) => source.id === sourceId)?.title ?? 'A source'

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto">
        {messages.isPending && <SkeletonRows rows={3} label="Loading this conversation" />}

        {listError !== null && (
          <div className="px-4 py-4">
            <ApiErrorNotice
              error={listError}
              noun="messages"
              action={
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={() => void messages.refetch()}
                >
                  Try again
                </button>
              }
            />
          </div>
        )}

        {messages.data !== undefined && (
          <MessageList
            messages={messages.data}
            turn={stream.turn}
            sourceTitle={titleOf}
            onOpenCitation={setOpenCitation}
            onRetry={stream.retry}
            emptyReason={
              readySources.length === 0
                ? `Ask once a source has finished indexing. A question is answered from this workspace\u2019s material and cites the page each claim came from.${useWeb ? ' Web results are searched for as well.' : ''}`
                : `Ask about anything in this workspace\u2019s material. The answer cites the page each claim came from.${useWeb ? ' Web results are cited with links to where they came from.' : ''}`
            }
          />
        )}
      </div>

      {openCitation !== null && (
        <CitationPanel
          citation={openCitation}
          sourceTitle={titleOf(openCitation.source_id)}
          onClose={() => {
            setOpenCitation(null)
          }}
        />
      )}

      <form
        className="flex flex-col gap-3 border-t border-rule px-4 py-3"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <SourcePicker
          workspaceId={workspaceId}
          sources={readySources}
          selected={selected}
          onChange={setSelected}
          disabled={streaming}
        />

        <div className="flex flex-col gap-1">
          <label className="label" htmlFor="chat-message">
            Ask about this workspace
          </label>
          <textarea
            id="chat-message"
            className="field min-h-20"
            value={draft}
            aria-describedby="chat-message-hint"
            onChange={(event) => {
              setDraft(event.target.value)
            }}
            onKeyDown={(event) => {
              // Enter sends and Shift+Enter breaks the line, which is the
              // convention a chat box is expected to follow. The form still
              // submits on the button, so nothing here is the only way to send.
              if (event.key !== 'Enter' || event.shiftKey) return
              event.preventDefault()
              submit()
            }}
          />
          <span id="chat-message-hint" className="text-xs text-ink-muted">
            Enter sends. Shift and Enter starts a new line.
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {streaming ? (
            <button type="button" className="btn btn-secondary" onClick={stream.stop}>
              Stop
            </button>
          ) : (
            <button type="submit" className="btn btn-primary" disabled={!canSend}>
              Ask
            </button>
          )}

          {/* A checkbox is its own on/off state: the box is filled when checked
              and the label is the accessible name, so the state is announced as
              "checked" by a screen reader and read as a tick by a sighted user
              without either depending on the accent colour. The row is a 44px
              label wrapping the box, so the whole row is the target on a phone
              and the box itself is a normal tab stop with the shared focus ring. */}
          <label className="flex min-h-11 items-center gap-2 text-sm text-ink-muted">
            <input
              type="checkbox"
              checked={useWeb}
              aria-describedby="chat-web-hint"
              onChange={(event) => {
                const next = event.target.checked
                setWebByWorkspace((current) => ({ ...current, [workspaceId]: next }))
              }}
            />
            Search the web too
          </label>
          <span id="chat-web-hint" className="text-xs text-ink-muted">
            Also searches outside this workspace and cites what it finds, with links. Answers stay
            grounded in your sources, and a search that finds nothing still answers from them.
          </span>
        </div>
      </form>
    </div>
  )
}

/**
 * Which sources an answer may draw on.
 *
 * "None selected" means the whole workspace rather than no material, and the
 * hint says so: an empty selection that silently answered nothing would look
 * exactly like a workspace with nothing in it.
 *
 * Each row is a 44px label wrapping its own checkbox, so the whole row is the
 * target at phone width and the label is what the screen reader reads.
 */
function SourcePicker({
  workspaceId,
  sources,
  selected,
  onChange,
  disabled,
}: {
  workspaceId: string
  sources: { id: string; title: string }[]
  selected: string[]
  onChange: (next: string[]) => void
  disabled: boolean
}) {
  const id = useId()

  if (sources.length === 0) {
    return (
      <p className="text-xs text-ink-muted">
        No indexed sources yet. Upload one in the Sources panel and this box will use it.
      </p>
    )
  }

  return (
    <fieldset aria-describedby={id} className="flex flex-col gap-1" disabled={disabled}>
      <legend className="label">Answer from</legend>
      <div className="flex max-h-32 flex-col gap-1 overflow-y-auto">
        {sources.map((source) => (
          <label
            key={source.id}
            className="flex min-h-11 items-center gap-2 rounded-field border border-rule px-3 py-2 text-sm has-[:focus-visible]:border-accent has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent"
          >
            <input
              type="checkbox"
              name={`source-${workspaceId}`}
              checked={selected.includes(source.id)}
              onChange={(event) => {
                onChange(
                  event.target.checked
                    ? [...selected, source.id]
                    : selected.filter((value) => value !== source.id),
                )
              }}
            />
            <span className="min-w-0 flex-1 break-words">{source.title}</span>
          </label>
        ))}
      </div>
      <span id={id} className="text-xs text-ink-muted">
        {selected.length === 0
          ? 'Nothing selected, so the answer uses every ready source in this workspace.'
          : `${String(selected.length)} of ${String(sources.length)} sources selected.`}
      </span>
    </fieldset>
  )
}

/**
 * The chunk a citation points at.
 *
 * The panel is opened by the student, so moving focus into it is correct: the
 * arriving *answer* must never move focus, but a control the user just activated
 * must take them somewhere, or a keyboard user clicks a chip and stays put with
 * no way to read what they asked for. Focus goes to the close button, which is
 * the way back out.
 */
function CitationPanel({
  citation,
  sourceTitle,
  onClose,
}: {
  citation: ChunkCitation
  sourceTitle: string
  onClose: () => void
}) {
  const chunk = useChunk(citation.chunk_id)
  const error = asApiError(chunk.error)
  const closeRef = useRef<HTMLButtonElement>(null)
  const headingId = useId()

  useEffect(() => {
    closeRef.current?.focus()
  }, [])

  return (
    <section
      aria-labelledby={headingId}
      className="panel mx-4 mb-3 flex max-h-64 flex-col gap-2 overflow-y-auto px-3 py-3"
    >
      <div className="flex items-start justify-between gap-2">
        <h3 id={headingId} className="min-w-0 flex-1 break-words">
          {sourceTitle}
          {citation.page !== null && <span className="meta"> page {String(citation.page)}</span>}
        </h3>
        <button ref={closeRef} type="button" className="btn btn-quiet" onClick={onClose}>
          Close
        </button>
      </div>

      {chunk.isPending && <p className="text-sm text-ink-muted">Loading the cited passage</p>}

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
        <p className="whitespace-pre-wrap text-sm text-ink-muted">{chunk.data.content}</p>
      )}
    </section>
  )
}
