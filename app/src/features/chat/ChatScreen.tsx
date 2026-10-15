import { useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useChunk, useMessages } from '@/api/queries/chat'
import { useSources } from '@/api/queries/sources'
import { asApiError } from '@/api/errors'
import type { ChunkCitation } from '@/api/sse'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { SourcePicker } from '@/features/workspaces/SourcePicker'
import { DEFAULT_TIMEZONE } from '@/lib/time'
import { ActionCardList } from './ActionCard'
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
  const { t } = useTranslation()
  const messages = useMessages(workspaceId)
  const sources = useSources(workspaceId)
  const stream = useChatStream(workspaceId)
  const session = useSession()
  // The account's zone (A203), never the device's: a proposal the model resolved
  // to a Jakarta evening must read as that evening on a phone set to UTC, and the
  // fallback is the product's own default rather than the machine's zone.
  const timeZone =
    session.status === 'signed-in' && session.user.timezone.length > 0
      ? session.user.timezone
      : DEFAULT_TIMEZONE

  const [draft, setDraft] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const [openCitation, setOpenCitation] = useState<ChunkCitation | null>(null)
  // Keyed by workspace so switching workspaces cannot carry one workspace's
  // answer into another's: a web search costs a credit and reaches the open
  // internet, and the choice to make it is per question, not per session.
  const [webByWorkspace, setWebByWorkspace] = useState<Record<string, boolean>>({})
  const useWeb = webByWorkspace[workspaceId] === true
  // Grounded by default, and keyed by workspace for the same reason as the web
  // toggle: the setting belongs to the question being asked, not to the session.
  // An explicit `false` is a real choice, so the map is consulted rather than
  // defaulted at the call site.
  const [groundedByWorkspace, setGroundedByWorkspace] = useState<Record<string, boolean>>({})
  const sourcesOnly = groundedByWorkspace[workspaceId] !== false

  const listError = asApiError(messages.error)
  const readySources = (sources.data ?? []).filter((source) => source.status === 'ready')
  const streaming = stream.turn?.streaming === true
  const canSend = draft.trim().length > 0 && !streaming

  function submit() {
    const message = draft.trim()
    if (message.length === 0 || streaming) return
    stream.send({ message, sourceIds: selected, useWeb, sourcesOnly })
    setDraft('')
  }

  const titleOf = (sourceId: string) =>
    sources.data?.find((source) => source.id === sourceId)?.title ??
    t('citation.unknownSource')

  // The four empty-state sentences are spelled out rather than assembled from a
  // shared head and a web clause: a translated sentence is a unit, and two halves
  // glued together cannot be reordered by whoever writes the Indonesian.
  const noSourcesReason = useWeb
    ? t('chat.empty.noSourcesWithWeb')
    : t('chat.empty.noSources')
  const readyReason = useWeb ? t('chat.empty.defaultWithWeb') : t('chat.empty.default')

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto">
        {messages.isPending && <SkeletonRows rows={3} label={t('chat.list.loading')} />}

        {listError !== null && (
          <div className="px-5 py-4 md:px-6">
            <ApiErrorNotice
              error={listError}
              noun={t('errors.nouns.messages')}
              action={
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={() => void messages.refetch()}
                >
                  {t('actions.retry')}
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
            emptyReason={readySources.length === 0 ? noSourcesReason : readyReason}
          />
        )}

        {/* The proposals this turn — or an earlier one — left for the student to
            decide. It renders nothing when there is nothing pending, and it reads
            `GET /actions/pending` rather than the stream's `action` event, so a
            card confirmed on another device shows as confirmed here. */}
        <ActionCardList timeZone={timeZone} />
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
        className="flex flex-col gap-3 border-t border-border px-5 py-3 md:px-6"
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
            {t('chat.composer.label')}
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
          <span id="chat-message-hint" className="text-xs text-muted">
            {t('chat.composer.hint')}
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {streaming ? (
            <button type="button" className="btn btn-secondary" onClick={stream.stop}>
              {t('actions.stop')}
            </button>
          ) : (
            <button type="submit" className="btn btn-primary" disabled={!canSend}>
              {t('chat.composer.ask')}
            </button>
          )}

          {/* The grounding toggle, default on. It sits first because it is the
              setting that changes what an answer *is*, and it is a checkbox so
              the state is announced as checked/unchecked rather than conveyed by
              colour. The tooltip is `title` rather than a custom popover: the
              text is one sentence, and a custom one would need its own focus and
              dismissal behaviour to be worth the accessibility cost. */}
          <label
            className="flex min-h-11 items-center gap-2 text-sm text-muted"
            title={t('honesty.sourcesOnlyTooltip')}
          >
            <input
              type="checkbox"
              checked={sourcesOnly}
              aria-describedby="chat-grounding-hint"
              onChange={(event) => {
                const next = event.target.checked
                setGroundedByWorkspace((current) => ({ ...current, [workspaceId]: next }))
              }}
            />
            {t('honesty.sourcesOnly')}
          </label>

          {/* A checkbox is its own on/off state: the box is filled when checked
              and the label is the accessible name, so the state is announced as
              "checked" by a screen reader and read as a tick by a sighted user
              without either depending on the accent colour. The row is a 44px
              label wrapping the box, so the whole row is the target on a phone
              and the box itself is a normal tab stop with the shared focus ring. */}
          <label className="flex min-h-11 items-center gap-2 text-sm text-muted">
            <input
              type="checkbox"
              checked={useWeb}
              aria-describedby="chat-web-hint"
              onChange={(event) => {
                const next = event.target.checked
                setWebByWorkspace((current) => ({ ...current, [workspaceId]: next }))
              }}
            />
            {t('chat.webToggle.label')}
          </label>
          <span id="chat-web-hint" className="text-xs text-muted">
            {t('chat.webToggle.hint')}
          </span>
        </div>

        {/* The honesty line. It is part of the composer rather than a tooltip:
            the spec requires it under every composer, and a warning nobody has to
            open is a warning most people never read. */}
        <p id="chat-grounding-hint" className="composer-hint">
          {t('honesty.composerHint')}
        </p>
      </form>
    </div>
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
  const { t } = useTranslation()
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
          {citation.page !== null && (
            <span className="meta"> {t('citation.page', { page: citation.page })}</span>
          )}
        </h3>
        <button ref={closeRef} type="button" className="btn btn-quiet" onClick={onClose}>
          {t('actions.close')}
        </button>
      </div>

      {chunk.isPending && <p className="text-sm text-muted">{t('citation.loading')}</p>}

      {error !== null && (
        <ApiErrorNotice
          error={error}
          noun={t('errors.nouns.passage')}
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void chunk.refetch()}
            >
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {chunk.data !== undefined && (
        <p className="whitespace-pre-wrap text-sm text-muted">{chunk.data.content}</p>
      )}
    </section>
  )
}
