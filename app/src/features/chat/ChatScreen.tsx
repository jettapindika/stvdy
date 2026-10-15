import { useEffect, useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useChunk, useMessages } from '@/api/queries/chat'
import { useSources } from '@/api/queries/sources'
import { asApiError } from '@/api/errors'
import type { ChunkCitation } from '@/api/sse'
import { Icon } from '@/components/ui/Icon'
import { Toggle } from '@/components/ui/Toggle'
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
/**
 * The source dropdown trigger (410:5647): a compact `@ Sources · N ⌄` button
 * that discloses the full SourcePicker in a floating panel. When closed only the
 * trigger is visible, keeping the composer action row compact.
 */
function SourceDropdown({
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
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const count = selected.length > 0 ? selected.length : sources.length

  // Close on click outside or Escape
  useEffect(() => {
    if (!open) return
    function onDown(event: MouseEvent) {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        setOpen(false)
      }
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        className="btn btn-secondary gap-1"
        disabled={disabled}
        aria-expanded={open}
        aria-controls="source-dropdown-panel"
        onClick={() => { setOpen(!open) }}
      >
        <span>{`@  ${t('workspaces.panel.sources')} · ${String(count)}`}</span>
        <Icon name="chevron-down" size={16} className={`transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>

      {open && (
        <div id="source-dropdown-panel" className="float absolute bottom-full left-0 z-10 mb-2 w-64 p-3">
          <SourcePicker
            workspaceId={workspaceId}
            sources={sources}
            selected={selected}
            onChange={onChange}
            disabled={disabled}
          />
          <button
            type="button"
            className="btn btn-quiet mt-2 w-full"
            onClick={() => { setOpen(false) }}
          >
            {t('actions.done')}
          </button>
        </div>
      )}
    </div>
  )
}
export function ChatScreen({
  workspaceId,
  onViewCitation,
}: {
  workspaceId: string
  /** Called when a citation is opened, so the reference panel can show it. */
  onViewCitation?: (citation: ChunkCitation) => void
}) {
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
            onOpenCitation={(citation) => {
              setOpenCitation(citation)
              onViewCitation?.(citation)
            }}
            onRetry={stream.retry}
            emptyReason={readySources.length === 0 ? noSourcesReason : readyReason}
            onFollowUp={(text) => {
              setDraft(text)
            }}
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

      {/* The floating composer: `.chat-composer` gives it the float treatment
          (border, shadow, sheet radius). It is NOT position:fixed — it is a flow
          sibling inside the flex column, so it stays within its panel's grid
          track. See the advisory about not using fixed positioning. */}
      <form
        className="chat-composer mx-3 mb-3"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <div className="flex flex-col gap-2">
          <label className="text-label font-medium" htmlFor="chat-message">
            {t('chat.composer.label')}
          </label>
          <textarea
            id="chat-message"
            className="field min-h-11"
            value={draft}
            placeholder={t('chat.composer.placeholder')}
            aria-describedby="chat-message-hint"
            onChange={(event) => {
              setDraft(event.target.value)
            }}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' || event.shiftKey) return
              event.preventDefault()
              submit()
            }}
          />
          <span id="chat-message-hint" className="text-xs text-muted">
            {t('chat.composer.hint')}
          </span>
        </div>

        {/* Action row: source dropdown trigger, attach, send */}
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            {/* Source dropdown trigger (410:5647): a compact button that shows
                the count and discloses the full SourcePicker on click. When no
                sources exist the button is omitted entirely so it does not show
                a dead "@ Sources · 0" control. */}
            {readySources.length > 0 && (
              <SourceDropdown
                workspaceId={workspaceId}
                sources={readySources}
                selected={selected}
                onChange={setSelected}
                disabled={streaming}
              />
            )}
            <button type="button" className="answer-action shrink-0" aria-label={t('chat.composer.attach')}>
              <Icon name="paperclip" size={20} />
              <span className="hidden md:inline">{t('chat.composer.attach')}</span>
            </button>
          </div>

          {streaming ? (
            <button type="button" className="btn btn-secondary" onClick={stream.stop}>
              {t('actions.stop')}
            </button>
          ) : (
            <button type="submit" className="btn btn-primary min-w-24" disabled={!canSend}>
              {t('chat.composer.ask')}
            </button>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-4">
          <Toggle
            checked={sourcesOnly}
            title={t('honesty.sourcesOnlyTooltip')}
            describedBy="chat-grounding-hint"
            onChange={(next) => {
              setGroundedByWorkspace((current) => ({ ...current, [workspaceId]: next }))
            }}
          >
            {t('honesty.sourcesOnly')}
          </Toggle>

          <Toggle
            checked={useWeb}
            onChange={(next) => {
              setWebByWorkspace((current) => ({ ...current, [workspaceId]: next }))
            }}
          >
            {t('chat.webToggle.label')}
          </Toggle>
        </div>

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
