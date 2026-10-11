import { useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router'

import { asApiError, validationMessage } from '@/api/errors'
import {
  isSettledStatus,
  useDeleteSource,
  useSourceStatus,
  useSources,
  useUploadSource,
  type Source,
  type SourceKind,
  type SourceStatusName,
} from '@/api/queries/sources'
import { useWorkspace } from '@/api/queries/workspaces'
import { Field } from '@/components/ui/Field'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { ChatScreen } from '@/features/chat/ChatScreen'
import { FlashcardsPanel } from '@/features/studio/FlashcardsPanel'
import { QuizPanel } from '@/features/studio/QuizPanel'

/**
 * The workspace view: Sources, Chat, and Studio.
 *
 * Composition differs by form factor on purpose (RHYTHM 2). Wide screens get all
 * three panels side by side, because reading a source while asking about it is
 * the actual work. Phones get one panel at a time behind a tab strip.
 *
 * The panel strip is a complete ARIA tab pattern, not half of one. Announcing
 * `role="tab"` and `aria-selected` without `aria-controls`, `role="tabpanel"`, and
 * `aria-labelledby` tells a screen reader "selected tab" and then gives it no
 * panel to move to, which is worse than plain buttons. The keyboard contract is
 * part of the same pattern: roving tabindex (only the active tab is a tab stop)
 * and Left/Right/Home/End, per the WAI-ARIA authoring practices.
 *
 * The strip is `lg:hidden`: below `lg` the panels are one at a time, at and above
 * it all three are visible and there is no tab interface at all.
 *
 * The root fills the space the shell gives it (`min-h-0 flex-1`) rather than the
 * viewport, so panels scroll internally without an outer page scroll.
 *
 * Sources is the panel Phase 1 built, so it is the one that opens. Chat is the
 * panel Phase 2 built. Studio is the flashcard panel Phase 4 built: it generates
 * a deck from the sources the student picks, and every card it produces is linked
 * to the passage it came from.
 */

type Panel = 'sources' | 'chat' | 'studio'

type StudioSection = 'flashcards' | 'quiz'

const STATUS_CHIP: Record<SourceStatusName, string> = {
  pending: 'chip',
  processing: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

export function WorkspaceScreen() {
  const { t } = useTranslation()
  const { workspaceId = '' } = useParams()
  const workspace = useWorkspace(workspaceId)
  const [active, setActive] = useState<Panel>('sources')
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  // The strip's labels are copy, so they are resolved per render rather than
  // kept in a module-scope table that would have no language to read from.
  const panels: { id: Panel; label: string }[] = [
    { id: 'sources', label: t('workspaces.panel.sources') },
    { id: 'chat', label: t('workspaces.panel.chat') },
    { id: 'studio', label: t('workspaces.panel.studio') },
  ]

  const workspaceError = asApiError(workspace.error)

  // Arrow keys move between tabs and move focus with them, which is what the
  // tablist pattern expects. Without this, Tab walks all three and the arrows do
  // nothing, so the roles would be promising behaviour that is not there.
  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const index = panels.findIndex((p) => p.id === active)
    let next = index
    if (event.key === 'ArrowRight') next = (index + 1) % panels.length
    else if (event.key === 'ArrowLeft') next = (index - 1 + panels.length) % panels.length
    else if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = panels.length - 1
    else return

    const target = panels[next]
    if (!target) return
    event.preventDefault()
    setActive(target.id)
    tabRefs.current[target.id]?.focus()
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex items-center gap-3 px-5 py-3">
        <h1 className="break-words text-base font-semibold">
          {workspace.data?.name ?? t('workspaces.detail.titleFallback')}
        </h1>
      </header>

      {workspaceError !== null && (
        <div className="px-5 pb-3">
          <ApiErrorNotice error={workspaceError} noun={t('workspaces.noun')} />
        </div>
      )}

      {workspaceError === null && (
        <>
          <div
            role="tablist"
            aria-label={t('workspaces.panels.label')}
            onKeyDown={onKeyDown}
            className="mx-3 mb-3 flex gap-1 rounded-pill bg-surface-raised p-1 lg:hidden"
          >
            {panels.map((panel) => {
              const selected = active === panel.id
              return (
                <button
                  key={panel.id}
                  ref={(node) => {
                    tabRefs.current[panel.id] = node
                  }}
                  id={`tab-${panel.id}`}
                  role="tab"
                  type="button"
                  aria-selected={selected}
                  aria-controls={`panel-${panel.id}`}
                  // Roving tabindex: only the active tab is in the tab order.
                  tabIndex={selected ? 0 : -1}
                  onClick={() => {
                    setActive(panel.id)
                  }}
                  className={`btn flex-1 ${selected ? 'btn-secondary' : 'btn-quiet'}`}
                >
                  {panel.label}
                </button>
              )
            })}
          </div>

          <div className="grid min-h-0 flex-1 grid-cols-1 lg:grid-cols-[minmax(200px,1fr)_minmax(320px,2fr)_minmax(220px,1fr)]">
            {panels.map((panel) => (
              <section
                key={panel.id}
                id={`panel-${panel.id}`}
                role="tabpanel"
                aria-labelledby={`tab-${panel.id}`}
                className={`min-h-0 flex-col border-border lg:flex lg:border-r lg:last:border-r-0 ${
                  active === panel.id ? 'flex' : 'hidden'
                }`}
              >
                <div className="border-b border-border px-4 py-2">
                  <span className="label">{panel.label}</span>
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto">
                  {panel.id === 'sources' && <SourcesPanel workspaceId={workspaceId} />}
                  {panel.id === 'chat' && <ChatScreen workspaceId={workspaceId} />}
                  {panel.id === 'studio' && <StudioPanel workspaceId={workspaceId} />}
                </div>
              </section>
            ))}
          </div>
        </>
      )}
    </div>
  )
}

/**
 * Studio: the generated study material for this workspace.
 *
 * The spec's workspace view has three panels and Studio holds three kinds of output
 * (Flashcards, Quiz, Report), so the sub-tabs are the same roving-tabindex strip the
 * outer tabs use: arrow keys move between them and only the active one is in the tab
 * order. A second, different keyboard contract inside the same screen would be a second
 * thing for a student to learn.
 */
function StudioPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const [section, setSection] = useState<StudioSection>('flashcards')
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  const sections: { id: StudioSection; label: string }[] = [
    { id: 'flashcards', label: t('workspaces.studio.flashcards') },
    { id: 'quiz', label: t('workspaces.studio.quiz') },
  ]

  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const index = sections.findIndex((entry) => entry.id === section)
    let next = index
    if (event.key === 'ArrowRight') next = (index + 1) % sections.length
    else if (event.key === 'ArrowLeft') next = (index - 1 + sections.length) % sections.length
    else if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = sections.length - 1
    else return

    const target = sections[next]
    if (target === undefined) return
    event.preventDefault()
    setSection(target.id)
    tabRefs.current[target.id]?.focus()
  }

  return (
    <div className="flex flex-col">
      <div
        role="tablist"
        aria-label={t('workspaces.studio.sections')}
        onKeyDown={onKeyDown}
        className="mx-3 mt-3 flex gap-1 rounded-pill bg-surface-raised p-1"
      >
        {sections.map((entry) => {
          const selected = entry.id === section
          return (
            <button
              key={entry.id}
              ref={(node) => {
                tabRefs.current[entry.id] = node
              }}
              id={`studio-tab-${entry.id}`}
              role="tab"
              type="button"
              aria-selected={selected}
              aria-controls={`studio-panel-${entry.id}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => {
                setSection(entry.id)
              }}
              className={`btn flex-1 ${selected ? 'btn-secondary' : 'btn-quiet'}`}
            >
              {entry.label}
            </button>
          )
        })}
      </div>

      <div
        id={`studio-panel-${section}`}
        role="tabpanel"
        aria-labelledby={`studio-tab-${section}`}
      >
        {section === 'flashcards' && <FlashcardsPanel workspaceId={workspaceId} />}
        {section === 'quiz' && <QuizPanel workspaceId={workspaceId} />}
      </div>
    </div>
  )
}

function SourcesPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const sources = useSources(workspaceId)
  const listError = asApiError(sources.error)

  return (
    <div className="flex flex-col gap-3 px-4 py-4">
      <UploadForm workspaceId={workspaceId} />

      {sources.isPending && (
        <SkeletonRows rows={2} label={t('workspaces.sources.loading')} />
      )}

      {listError !== null && (
        <ApiErrorNotice
          error={listError}
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void sources.refetch()}
            >
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {sources.data !== undefined && sources.data.length === 0 && (
        <EmptyState
          title={t('workspaces.sources.emptyTitle')}
          reason={t('workspaces.sources.emptyReason')}
        />
      )}

      {sources.data !== undefined && sources.data.length > 0 && (
        <ul className="flex flex-col divide-y divide-border border-y border-border">
          {sources.data.map((source) => (
            <SourceRow key={source.id} workspaceId={workspaceId} source={source} />
          ))}
        </ul>
      )}
    </div>
  )
}

function SourceRow({ workspaceId, source }: { workspaceId: string; source: Source }) {
  const { t } = useTranslation()
  const remove = useDeleteSource(workspaceId)
  const [confirming, setConfirming] = useState(false)

  // The word carries the state, so it is copy like any other and lives here
  // rather than in a module-scope table with no language to read from.
  const statusLabel: Record<SourceStatusName, string> = {
    pending: t('workspaces.source.status.pending'),
    processing: t('workspaces.source.status.processing'),
    ready: t('workspaces.source.status.ready'),
    failed: t('workspaces.source.status.failed'),
  }

  // Poll while the job is still moving. The list's status is a snapshot from
  // whenever it was last fetched, so the live value comes from the status
  // endpoint, and it is the one that decides whether the row is finished.
  const live = useSourceStatus(workspaceId, source.id, !isSettledStatus(source.status))

  const status = live.data?.status ?? source.status
  const errorText = live.data?.error ?? source.error
  const chunkCount = live.data?.chunk_count ?? null
  const removeError = asApiError(remove.error)

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 flex-1 break-words font-medium">
          {source.title.length > 0 ? source.title : t('workspaces.source.untitled')}
        </span>
        <span className="meta">
          {source.kind === 'pdf'
            ? t('workspaces.source.kindPdf')
            : t('workspaces.source.kindText')}
        </span>
        {/* The word carries the state; the chip's tone repeats it. Colour is
            never the only signal, and "Ready" is readable in monochrome. */}
        <span className={STATUS_CHIP[status]} role="status">
          {(status === 'pending' || status === 'processing') && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {statusLabel[status]}
        </span>
        {status === 'ready' && chunkCount !== null && (
          <span className="meta">
            {t('workspaces.source.chunkCount', { count: chunkCount })}
          </span>
        )}
        {!confirming && (
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => {
              setConfirming(true)
            }}
          >
            {t('actions.remove')}
          </button>
        )}
      </div>

      {errorText !== null && errorText.length > 0 && (
        <p className="note note-danger text-sm">
          {errorText}
        </p>
      )}

      {confirming && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-muted">{t('workspaces.source.removeConfirm')}</span>
          <button
            type="button"
            className="btn btn-secondary"
            autoFocus
            disabled={remove.isPending}
            onClick={() => {
              remove.mutate(source.id, {
                onSuccess: () => {
                  setConfirming(false)
                },
              })
            }}
          >
            {remove.isPending ? t('workspaces.source.removing') : t('workspaces.source.removeIt')}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => {
              remove.reset()
              setConfirming(false)
            }}
          >
            {t('workspaces.keepIt')}
          </button>
        </div>
      )}

      {removeError !== null && (
        <ApiErrorNotice error={removeError} noun={t('workspaces.source.noun')} />
      )}
    </li>
  )
}

function UploadForm({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const [kind, setKind] = useState<SourceKind>('pdf')
  const [file, setFile] = useState<File | null>(null)
  const [text, setText] = useState('')
  const [title, setTitle] = useState('')
  const fileInput = useRef<HTMLInputElement>(null)
  const fileId = useId()
  const textId = useId()
  const fileErrorId = useId()
  const textErrorId = useId()
  const upload = useUploadSource(workspaceId)

  const uploadError = asApiError(upload.error)
  const validation =
    uploadError?.code === 'validation'
      ? validationMessage(uploadError, ['file', 'text', 'title'])
      : null
  const showNotice = uploadError !== null && (validation === null || validation.field === null)
  const ready = kind === 'pdf' ? file !== null : text.trim().length > 0

  return (
    <form
      className="panel flex flex-col gap-3 px-3 py-3"
      onSubmit={(event) => {
        event.preventDefault()
        if (!ready) return
        upload.mutate(
          kind === 'pdf' && file !== null
            ? { kind: 'pdf', file, title }
            : { kind: 'text', text, title },
          {
            onSuccess: () => {
              setFile(null)
              setText('')
              setTitle('')
              // A file input cannot be cleared by state alone; the element keeps
              // the chosen file until it is reset, which would leave the old name
              // visible over an empty form.
              if (fileInput.current) fileInput.current.value = ''
            },
          },
        )
      }}
    >
      {showNotice && uploadError !== null && (
        <ApiErrorNotice error={uploadError} noun={t('workspaces.upload.noun')} />
      )}

      <fieldset className="flex flex-col gap-1">
        <legend className="label">{t('workspaces.upload.legend')}</legend>
        <div className="flex flex-wrap gap-2">
          {(['pdf', 'text'] as const).map((option) => (
            <label
              key={option}
              className="flex min-h-11 flex-1 cursor-pointer items-center gap-2 rounded-field border border-border px-3 py-2 text-sm has-[:focus-visible]:border-primary has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring"
            >
              <input
                type="radio"
                name="kind"
                value={option}
                checked={kind === option}
                onChange={() => {
                  setKind(option)
                }}
              />
              {option === 'pdf'
                ? t('workspaces.upload.kindPdf')
                : t('workspaces.upload.kindText')}
            </label>
          ))}
        </div>
      </fieldset>

      {kind === 'pdf' ? (
        <div className="flex flex-col gap-1">
          <label className="label" htmlFor={fileId}>
            {t('workspaces.upload.kindPdf')}
          </label>
          <input
            ref={fileInput}
            id={fileId}
            className="field"
            type="file"
            name="file"
            accept="application/pdf"
            aria-invalid={validation?.field === 'file' ? true : undefined}
            aria-describedby={validation?.field === 'file' ? fileErrorId : undefined}
            onChange={(event) => {
              setFile(event.target.files?.[0] ?? null)
            }}
          />
          {validation?.field === 'file' && (
            <span id={fileErrorId} className="text-sm text-danger">
              {validation.message}
            </span>
          )}
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <label className="label" htmlFor={textId}>
            {t('workspaces.upload.textLabel')}
          </label>
          <textarea
            id={textId}
            className="field min-h-28"
            name="text"
            value={text}
            aria-invalid={validation?.field === 'text' ? true : undefined}
            aria-describedby={validation?.field === 'text' ? textErrorId : undefined}
            onChange={(event) => {
              setText(event.target.value)
            }}
          />
          {validation?.field === 'text' && (
            <span id={textErrorId} className="text-sm text-danger">
              {validation.message}
            </span>
          )}
        </div>
      )}

      <Field
        label={t('workspaces.upload.titleLabel')}
        name="title"
        value={title}
        hint={t('workspaces.upload.titleHint')}
        error={validation?.field === 'title' ? validation.message : undefined}
        onChange={(event) => {
          setTitle(event.target.value)
        }}
      />

      <button type="submit" className="btn btn-primary self-start" disabled={!ready || upload.isPending}>
        {upload.isPending ? t('workspaces.upload.uploading') : t('workspaces.upload.submit')}
      </button>
      {/* Upload progress is the ingestion status below, not a fake percentage:
          `fetch` cannot report bytes sent, and a bar that is not measuring
          anything is worse than a word that is true. */}
      <p className="text-xs text-muted">{t('workspaces.upload.afterStored')}</p>
    </form>
  )
}
