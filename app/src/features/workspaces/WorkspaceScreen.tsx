import { useId, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router'

import { asApiError, validationMessage } from '@/api/errors'
import {
  HTTP_URL_PATTERN,
  UPLOAD_KINDS,
  acceptFor,
  isSettledStatus,
  useDeleteSource,
  useSourceStatus,
  useSources,
  useUploadSource,
  type Source,
  type SourceKind,
  type SourceStatusName,
  type UploadKind,
} from '@/api/queries/sources'
import { useWorkspace } from '@/api/queries/workspaces'
import { Field } from '@/components/ui/Field'
import { Icon, type IconName } from '@/components/ui/Icon'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { ChatScreen } from '@/features/chat/ChatScreen'
import { FlashcardsPanel } from '@/features/studio/FlashcardsPanel'
import { QuizPanel } from '@/features/studio/QuizPanel'
import { ReportPanel } from '@/features/studio/ReportPanel'
import { formatInZone } from '@/lib/time'

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
 * panel Phase 2 built. Studio is the generated material: flashcards (Phase 4),
 * quizzes (Phase 5), and the report and cheatsheet this workspace's sources
 * become (Phase 6).
 *
 * **The heading carries the course context the design draws**: a breadcrumb back to
 * the list, the name, and a mono line of the facts the API holds — the course code,
 * the source count and the exam date, each present only when it exists. There is no
 * "12 flashcards due · Supervised learning" line: a per-workspace due count is not
 * in the API (the only due figure is the account-wide one the dashboard returns),
 * and the last-opened source is not a fact any row carries.
 */

type Panel = 'sources' | 'chat' | 'studio'

/**
 * The desktop grid's columns, left to right: the source rail, the chat, the
 * reference panel. The design draws exactly that order, and the widths below are
 * its proportions.
 */
const PANEL_ORDER: Panel[] = ['sources', 'chat', 'studio']

/**
 * The phone strip's order, which is the design's and is **not** the grid's: the
 * segmented control reads `Chat | Sources | Studio` in every mobile frame, with the
 * active panel filled, whichever panel is open. One array for both would put the
 * chat in the left-hand column of a 1440px window.
 */
const STRIP_ORDER: Panel[] = ['chat', 'sources', 'studio']

type StudioSection = 'flashcards' | 'quiz' | 'report'

const STATUS_CHIP: Record<SourceStatusName, string> = {
  pending: 'chip',
  processing: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

/** The icon a source row leads with, per kind. Same vocabulary as the kind's word. */
const KIND_ICON: Record<SourceKind, IconName> = {
  pdf: 'file-text',
  docx: 'file-text',
  pptx: 'presentation',
  url: 'link',
  web: 'globe',
  text: 'file-text',
}

export function WorkspaceScreen() {
  const { t } = useTranslation()
  const { workspaceId = '' } = useParams()
  const workspace = useWorkspace(workspaceId)
  const [active, setActive] = useState<Panel>('sources')
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  // The strip's labels are copy, so they are resolved per render rather than
  // kept in a module-scope table that would have no language to read from.
  const labels: Record<Panel, string> = {
    sources: t('workspaces.panel.sources'),
    chat: t('workspaces.panel.chat'),
    studio: t('workspaces.panel.studio'),
  }
  const panels = STRIP_ORDER.map((id) => ({ id, label: labels[id] }))

  const workspaceError = asApiError(workspace.error)
  const detail = workspace.data

  // The facts line, assembled from what the server actually sent. An account-wide
  // due count is deliberately not repeated here: this line is about *this*
  // workspace, and a figure from every workspace would read as this one's.
  const facts = [
    detail?.course_code ?? null,
    detail === undefined
      ? null
      : t('workspaces.detail.sourceCount', { count: detail.source_count }),
    typeof detail?.exam_date !== 'string' || detail.exam_date.length === 0
      ? null
      : t('workspaces.detail.exam', {
          date: formatInZone(new Date(`${detail.exam_date}T00:00:00Z`), 'UTC', {
            day: '2-digit',
            month: 'short',
            year: 'numeric',
          }),
        }),
  ].filter((part): part is string => part !== null)

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
      <header className="flex flex-col gap-2 border-b border-border bg-surface px-5 py-4">
        {/* A link that is a control needs its own target: `inline-flex min-h-11
            items-center` (see the `.link` rule in styles.css). */}
        <Link to="/workspaces" className="link inline-flex min-h-11 items-center gap-2 self-start">
          <Icon name="arrow-left" size={20} />
          {t('nav.workspaces')}
        </Link>
        <h1 className="min-w-0 wrap-anywhere">
          {detail?.name ?? t('workspaces.detail.titleFallback')}
        </h1>
        {facts.length > 0 && <p className="meta uppercase wrap-anywhere">{facts.join(' · ')}</p>}
      </header>

      {workspaceError !== null && (
        <div className="px-5 py-3">
          <ApiErrorNotice error={workspaceError} noun={t('workspaces.noun')} />
        </div>
      )}

      {workspaceError === null && (
        <>
          {/* The strip scrolls, the same answer the phone tab bar gives and for the
              same measured reason: each tab is a `.btn`, which carries
              `min-width: 0` so a long label wraps inside the button instead of
              overflowing — but a *word* cannot wrap, and at 200% text "Sources" is
              110px of glyphs plus 70px of padding, so three tabs need ~540px of a
              326px strip. They shrank to 101px each and their labels were clipped
              (`scrollWidth` 106 against a `clientWidth` of 101; "Flashcards" 124
              against 101). `min-w-max` stops the shrink and `overflow-x-auto` makes
              the remainder reachable by swipe and by `Tab`; at 100% nothing scrolls,
              so the visual is unchanged. */}
          <div
            role="tablist"
            aria-label={t('workspaces.panels.label')}
            onKeyDown={onKeyDown}
            className="mx-3 my-3 flex gap-1 overflow-x-auto overscroll-x-contain rounded-pill bg-surface-raised p-1 lg:hidden"
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
                  className={`btn flex-1 min-w-max ${selected ? 'btn-secondary' : 'btn-quiet'}`}
                >
                  {panel.label}
                </button>
              )
            })}
          </div>

          <div className="grid min-h-0 flex-1 grid-cols-1 lg:grid-cols-[minmax(200px,1fr)_minmax(320px,2fr)_minmax(220px,1fr)]">
            {PANEL_ORDER.map((id) => (
              <section
                key={id}
                id={`panel-${id}`}
                role="tabpanel"
                aria-labelledby={`tab-${id}`}
                className={`min-h-0 flex-col border-border lg:flex lg:border-r lg:last:border-r-0 ${
                  active === id ? 'flex' : 'hidden'
                }`}
              >
                <div className="border-b border-border px-4 py-2">
                  {/* Not a heading: the design draws a 20/26 panel title, and the
                      frozen scale maps that size to `h3` — but this screen's only
                      `h1` is the workspace's name and there is no `h2` above these,
                      so an `h3` here would skip a level and fail the accessibility
                      gate. The panel is named by its tab on a phone and by its
                      column on a desktop, and the label carries the same word. */}
                  <span className="label">{labels[id]}</span>
                </div>
                {/* `wrap-anywhere` (`overflow-wrap: anywhere`) on the container
                    rather than on each text box: it inherits, and it is the
                    *container's* width that these boxes were failing on. At 200%
                    text on a 1365px window each panel track resolves to ~221px and
                    its content box to 106px, while a word like `workspace's` at
                    `text-xs` is ~130px of glyphs — so a paragraph that fits at 100%
                    has an unbreakable word wider than its box. `break-words`
                    (`overflow-wrap: break-word`) is **not** enough: it only lowers a
                    box's *content* width, not its min-content size, so the word hangs
                    out of the box and is cut off — the panel scrolls vertically and
                    the overflow is horizontal, where nothing can reach it.

                    `anywhere` is the value that also affects min-content sizing,
                    which is the property these boxes are failing on — the same
                    reasoning `crates/core`'s week grid records. The cost is honest:
                    a word may break mid-glyph at 200% text. At 100% nothing
                    overflows, so nothing breaks. */}
                <div className="min-h-0 flex-1 overflow-y-auto wrap-anywhere @container">
                  {id === 'sources' && <SourcesPanel workspaceId={workspaceId} />}
                  {id === 'chat' && <ChatScreen workspaceId={workspaceId} />}
                  {id === 'studio' && <StudioPanel workspaceId={workspaceId} />}
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
    { id: 'report', label: t('workspaces.studio.report') },
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
        className="mx-3 mt-3 flex gap-1 overflow-x-auto overscroll-x-contain rounded-pill bg-surface-raised p-1"
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
              className={`btn flex-1 min-w-max ${selected ? 'btn-secondary' : 'btn-quiet'}`}
            >
              {entry.label}
            </button>
          )
        })}
      </div>

      {/* Both panels are rendered and the inactive one is hidden, the pattern the outer
          strip already uses: each tab's `aria-controls` has to resolve to an element that
          exists, and only the active tabpanel is in the accessibility tree. Rendering only
          the active one leaves the inactive tab pointing at nothing. */}
      {sections.map((entry) => (
        <div
          key={entry.id}
          id={`studio-panel-${entry.id}`}
          role="tabpanel"
          aria-labelledby={`studio-tab-${entry.id}`}
          hidden={entry.id !== section}
        >
          {entry.id === 'flashcards' && <FlashcardsPanel workspaceId={workspaceId} />}
          {entry.id === 'quiz' && <QuizPanel workspaceId={workspaceId} />}
          {entry.id === 'report' && <ReportPanel workspaceId={workspaceId} />}
        </div>
      ))}
    </div>
  )
}

function SourcesPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const sources = useSources(workspaceId)
  const listError = asApiError(sources.error)

  return (
    <div className="flex flex-col gap-4 px-4 py-4">
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
        <ul className="flex flex-col gap-3">
          {sources.data.map((source) => (
            <SourceRow key={source.id} workspaceId={workspaceId} source={source} />
          ))}
        </ul>
      )}

      <UploadForm workspaceId={workspaceId} />
    </div>
  )
}

/**
 * What a row calls a source's kind.
 *
 * A function with literal `t()` calls rather than a map of key strings: the i18n
 * suite scans the source for the keys the code asks for, so an indirected key is
 * indistinguishable from a key nothing renders. A switch also fails to compile
 * silently on a new kind, where a map lookup would fall through to "Source".
 *
 * `web` is the "saved from search" kind and says so, since a student who saved a
 * page needs to recognise it as one.
 */
function sourceKindLabel(kind: SourceKind, t: (key: string) => string): string {
  switch (kind) {
    case 'pdf':
      return t('workspaces.source.kindPdf')
    case 'docx':
      return t('workspaces.source.kindDocx')
    case 'pptx':
      return t('workspaces.source.kindPptx')
    case 'url':
      return t('workspaces.source.kindUrl')
    case 'text':
      return t('workspaces.source.kindText')
    case 'web':
      return t('workspaces.source.kindWeb')
    default:
      return t('workspaces.source.kindOther')
  }
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
  // A source can only be read in the viewer once it is indexed: before that there
  // is no chunk for a page to point at, and the design's `Open PDF · page 12`
  // names a page that only exists after ingestion.
  const openable = status === 'ready' && source.url !== null

  return (
    <li className="panel flex flex-col gap-3 p-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="chip chip-source shrink-0" aria-hidden="true">
          <Icon name={KIND_ICON[source.kind]} size={20} />
        </span>
        {/* `w-full … @md:w-auto @md:flex-1`: the same row shape and the same
            measurement as `DeckRow`'s title in `FlashcardsPanel` — the siblings are
            fixed-width, `flex-1` leaves this span the remainder, and with the
            panel container's `overflow-wrap: anywhere` its min-content is one
            character, so the row squeezed it to 18px and 1452px tall. The panel's
            width is a container fact, not a window one, so the test is `@container`'s
            `@md` rather than a viewport breakpoint. */}
        <span className="w-full min-w-0 wrap-anywhere font-medium @md:w-auto @md:flex-1">
          {source.title.length > 0 ? source.title : t('workspaces.source.untitled')}
        </span>
        <span className="meta wrap-anywhere">{sourceKindLabel(source.kind, t)}</span>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {/* The word carries the state; the chip's tone repeats it. Colour is
            never the only signal, and "Ready" is readable in monochrome. */}
        <span className={STATUS_CHIP[status]} role="status">
          {(status === 'pending' || status === 'processing') && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {statusLabel[status]}
        </span>
        {/* A count of what ingestion produced, beside the state that produced it.
            Absent until the server reports it, never zero-filled. */}
        {chunkCount !== null && (
          <span className="meta">
            {t('workspaces.source.chunkCount', { count: chunkCount })}
          </span>
        )}
        {openable && (
          <a
            href={source.url ?? undefined}
            target="_blank"
            rel="noreferrer"
            className="btn btn-secondary"
          >
            {t('workspaces.source.open')}
          </a>
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
        <p className="note note-danger text-small">{errorText}</p>
      )}

      {confirming && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-small text-muted">{t('workspaces.source.removeConfirm')}</span>
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
  const [kind, setKind] = useState<UploadKind>('pdf')
  const [file, setFile] = useState<File | null>(null)
  const [text, setText] = useState('')
  const [url, setUrl] = useState('')
  const [title, setTitle] = useState('')
  const fileInput = useRef<HTMLInputElement>(null)
  const fileId = useId()
  const textId = useId()
  const urlId = useId()
  const fileErrorId = useId()
  const textErrorId = useId()
  const urlErrorId = useId()
  const upload = useUploadSource(workspaceId)

  const uploadError = asApiError(upload.error)
  const validation =
    uploadError?.code === 'validation'
      ? validationMessage(uploadError, t, ['file', 'text', 'url', 'title'])
      : null
  const showNotice = uploadError !== null && (validation === null || validation.field === null)

  // The URL's own error comes from the client, before a round trip: the server is
  // the enforcement, and this is only the message.
  const urlMalformed = kind === 'url' && url.trim().length > 0 && !HTTP_URL_PATTERN.test(url.trim())
  const ready =
    kind === 'text'
      ? text.trim().length > 0
      : kind === 'url'
        ? HTTP_URL_PATTERN.test(url.trim())
        : file !== null

  const kindLabel: Record<UploadKind, string> = {
    pdf: t('workspaces.upload.kindPdf'),
    docx: t('workspaces.upload.kindDocx'),
    pptx: t('workspaces.upload.kindPptx'),
    url: t('workspaces.upload.kindUrl'),
    text: t('workspaces.upload.kindText'),
  }

  return (
    <form
      className="panel flex flex-col gap-3 p-3"
      onSubmit={(event) => {
        event.preventDefault()
        if (!ready) return
        upload.mutate(
          kind === 'text'
            ? { kind: 'text', text, title }
            : kind === 'url'
              ? { kind: 'url', url: url.trim(), title }
              : { kind, file: file as File, title },
          {
            onSuccess: () => {
              setFile(null)
              setText('')
              setUrl('')
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
      <p className="eyebrow">{t('workspaces.upload.legend')}</p>

      {showNotice && uploadError !== null && (
        <ApiErrorNotice error={uploadError} noun={t('workspaces.upload.noun')} />
      )}

      <fieldset className="flex flex-col gap-1">
        <legend className="label">{t('workspaces.upload.legend')}</legend>
        <div className="flex flex-wrap gap-2">
          {UPLOAD_KINDS.map((option) => (
            <label
              key={option}
              className="flex min-h-11 flex-1 cursor-pointer items-center gap-2 rounded-field border border-border px-3 py-2 text-small has-[:focus-visible]:border-primary has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring"
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
              {kindLabel[option]}
            </label>
          ))}
        </div>
      </fieldset>

      {kind === 'text' ? (
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
            <span id={textErrorId} className="text-small text-danger">
              {validation.message}
            </span>
          )}
        </div>
      ) : kind === 'url' ? (
        <div className="flex flex-col gap-1">
          <label className="label" htmlFor={urlId}>
            {t('workspaces.upload.urlLabel')}
          </label>
          <input
            id={urlId}
            className="field"
            type="url"
            name="url"
            inputMode="url"
            placeholder="https://"
            value={url}
            aria-invalid={urlMalformed || validation?.field === 'url' ? true : undefined}
            aria-describedby={urlMalformed || validation?.field === 'url' ? urlErrorId : undefined}
            onChange={(event) => {
              setUrl(event.target.value)
            }}
          />
          {(urlMalformed || validation?.field === 'url') && (
            <span id={urlErrorId} className="text-small text-danger">
              {urlMalformed ? t('workspaces.upload.urlInvalid') : validation?.message}
            </span>
          )}
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <label className="label" htmlFor={fileId}>
            {kindLabel[kind]}
          </label>
          <input
            ref={fileInput}
            id={fileId}
            className="field"
            type="file"
            name="file"
            accept={acceptFor(kind)}
            aria-invalid={validation?.field === 'file' ? true : undefined}
            aria-describedby={validation?.field === 'file' ? fileErrorId : undefined}
            onChange={(event) => {
              setFile(event.target.files?.[0] ?? null)
            }}
          />
          {validation?.field === 'file' && (
            <span id={fileErrorId} className="text-small text-danger">
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
      {/* Upload progress is the ingestion status above, not a fake percentage:
          `fetch` cannot report bytes sent, and a bar that is not measuring
          anything is worse than a word that is true. */}
      <p className="text-caption text-muted">{t('workspaces.upload.afterStored')}</p>
    </form>
  )
}
