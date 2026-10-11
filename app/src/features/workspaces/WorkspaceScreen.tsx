import { useId, useRef, useState } from 'react'
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

const STUDIO_SECTIONS: { id: StudioSection; label: string }[] = [
  { id: 'flashcards', label: 'Flashcards' },
  { id: 'quiz', label: 'Quiz' },
]

const PANELS: { id: Panel; label: string }[] = [
  { id: 'sources', label: 'Sources' },
  { id: 'chat', label: 'Chat' },
  { id: 'studio', label: 'Studio' },
]

const STATUS_LABEL: Record<SourceStatusName, string> = {
  pending: 'Waiting to start',
  processing: 'Processing',
  ready: 'Ready',
  failed: 'Failed',
}

const STATUS_CHIP: Record<SourceStatusName, string> = {
  pending: 'chip',
  processing: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

export function WorkspaceScreen() {
  const { workspaceId = '' } = useParams()
  const workspace = useWorkspace(workspaceId)
  const [active, setActive] = useState<Panel>('sources')
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  const workspaceError = asApiError(workspace.error)

  // Arrow keys move between tabs and move focus with them, which is what the
  // tablist pattern expects. Without this, Tab walks all three and the arrows do
  // nothing, so the roles would be promising behaviour that is not there.
  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const index = PANELS.findIndex((p) => p.id === active)
    let next = index
    if (event.key === 'ArrowRight') next = (index + 1) % PANELS.length
    else if (event.key === 'ArrowLeft') next = (index - 1 + PANELS.length) % PANELS.length
    else if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = PANELS.length - 1
    else return

    const target = PANELS[next]
    if (!target) return
    event.preventDefault()
    setActive(target.id)
    tabRefs.current[target.id]?.focus()
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex items-center gap-3 px-5 py-3">
        <h1 className="break-words text-base font-semibold">
          {workspace.data?.name ?? 'Workspace'}
        </h1>
      </header>

      {workspaceError !== null && (
        <div className="px-5 pb-3">
          <ApiErrorNotice error={workspaceError} noun="workspaces" />
        </div>
      )}

      {workspaceError === null && (
        <>
          <div
            role="tablist"
            aria-label="Workspace panels"
            onKeyDown={onKeyDown}
            className="mx-3 mb-3 flex gap-1 rounded-pill bg-surface-sunk p-1 lg:hidden"
          >
            {PANELS.map((panel) => {
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
            {PANELS.map((panel) => (
              <section
                key={panel.id}
                id={`panel-${panel.id}`}
                role="tabpanel"
                aria-labelledby={`tab-${panel.id}`}
                className={`min-h-0 flex-col border-rule lg:flex lg:border-r lg:last:border-r-0 ${
                  active === panel.id ? 'flex' : 'hidden'
                }`}
              >
                <div className="border-b border-rule px-4 py-2">
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
  const [section, setSection] = useState<StudioSection>('flashcards')
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const index = STUDIO_SECTIONS.findIndex((entry) => entry.id === section)
    let next = index
    if (event.key === 'ArrowRight') next = (index + 1) % STUDIO_SECTIONS.length
    else if (event.key === 'ArrowLeft') next = (index - 1 + STUDIO_SECTIONS.length) % STUDIO_SECTIONS.length
    else if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = STUDIO_SECTIONS.length - 1
    else return

    const target = STUDIO_SECTIONS[next]
    if (target === undefined) return
    event.preventDefault()
    setSection(target.id)
    tabRefs.current[target.id]?.focus()
  }

  return (
    <div className="flex flex-col">
      <div
        role="tablist"
        aria-label="Studio sections"
        onKeyDown={onKeyDown}
        className="mx-3 mt-3 flex gap-1 rounded-pill bg-surface-sunk p-1"
      >
        {STUDIO_SECTIONS.map((entry) => {
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

      {/* Both panels are rendered and the inactive one is hidden, the pattern the outer
          strip already uses: each tab's `aria-controls` has to resolve to an element
          that exists, and only the active tabpanel is in the accessibility tree. */}
      {STUDIO_SECTIONS.map((entry) => (
        <div
          key={entry.id}
          id={`studio-panel-${entry.id}`}
          role="tabpanel"
          aria-labelledby={`studio-tab-${entry.id}`}
          hidden={entry.id !== section}
        >
          {entry.id === 'flashcards' && <FlashcardsPanel workspaceId={workspaceId} />}
          {entry.id === 'quiz' && <QuizPanel workspaceId={workspaceId} />}
        </div>
      ))}
    </div>
  )
}

function SourcesPanel({ workspaceId }: { workspaceId: string }) {
  const sources = useSources(workspaceId)
  const listError = asApiError(sources.error)

  return (
    <div className="flex flex-col gap-3 px-4 py-4">
      <UploadForm workspaceId={workspaceId} />

      {sources.isPending && <SkeletonRows rows={2} label="Loading this workspace's sources" />}

      {listError !== null && (
        <ApiErrorNotice
          error={listError}
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void sources.refetch()}
            >
              Try again
            </button>
          }
        />
      )}

      {sources.data !== undefined && sources.data.length === 0 && (
        <EmptyState
          title="No sources yet"
          reason="Add a PDF or paste text above. Each one is chunked and indexed, so a question can be answered from any of them and cite the page it came from."
        />
      )}

      {sources.data !== undefined && sources.data.length > 0 && (
        <ul className="flex flex-col divide-y divide-rule border-y border-rule">
          {sources.data.map((source) => (
            <SourceRow key={source.id} workspaceId={workspaceId} source={source} />
          ))}
        </ul>
      )}
    </div>
  )
}

function SourceRow({ workspaceId, source }: { workspaceId: string; source: Source }) {
  const remove = useDeleteSource(workspaceId)
  const [confirming, setConfirming] = useState(false)

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
          {source.title.length > 0 ? source.title : 'Untitled source'}
        </span>
        <span className="meta">{source.kind === 'pdf' ? 'PDF' : 'Text'}</span>
        {/* The word carries the state; the chip's tone repeats it. Colour is
            never the only signal, and "Ready" is readable in monochrome. */}
        <span className={STATUS_CHIP[status]} role="status">
          {(status === 'pending' || status === 'processing') && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {STATUS_LABEL[status]}
        </span>
        {status === 'ready' && chunkCount !== null && (
          <span className="meta">
            {chunkCount} {chunkCount === 1 ? 'chunk' : 'chunks'}
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
            Remove
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
          <span className="text-sm text-ink-muted">Remove this source?</span>
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
            {remove.isPending ? 'Removing' : 'Remove it'}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => {
              remove.reset()
              setConfirming(false)
            }}
          >
            Keep it
          </button>
        </div>
      )}

      {removeError !== null && <ApiErrorNotice error={removeError} noun="sources" />}
    </li>
  )
}

function UploadForm({ workspaceId }: { workspaceId: string }) {
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
        <ApiErrorNotice error={uploadError} noun="uploading sources" />
      )}

      <fieldset className="flex flex-col gap-1">
        <legend className="label">Add material</legend>
        <div className="flex flex-wrap gap-2">
          {(['pdf', 'text'] as const).map((option) => (
            <label
              key={option}
              className="flex min-h-11 flex-1 cursor-pointer items-center gap-2 rounded-field border border-rule px-3 py-2 text-sm has-[:focus-visible]:border-accent has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent"
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
              {option === 'pdf' ? 'PDF file' : 'Pasted text'}
            </label>
          ))}
        </div>
      </fieldset>

      {kind === 'pdf' ? (
        <div className="flex flex-col gap-1">
          <label className="label" htmlFor={fileId}>
            PDF file
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
            Text to add
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
        label="Title"
        name="title"
        value={title}
        hint="Optional. The name this source is listed under."
        error={validation?.field === 'title' ? validation.message : undefined}
        onChange={(event) => {
          setTitle(event.target.value)
        }}
      />

      <button type="submit" className="btn btn-primary self-start" disabled={!ready || upload.isPending}>
        {upload.isPending ? 'Uploading' : 'Add source'}
      </button>
      {/* Upload progress is the ingestion status below, not a fake percentage:
          `fetch` cannot report bytes sent, and a bar that is not measuring
          anything is worse than a word that is true. */}
      <p className="text-xs text-ink-muted">
        After the file is stored, this workspace's source list shows extraction and
        indexing until the source is ready.
      </p>
    </form>
  )
}
