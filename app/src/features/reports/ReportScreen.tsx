import { useEffect, useRef, useState, type ReactNode } from 'react'
import type { TFunction } from 'i18next'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import {
  EXPORT_FORMATS,
  readableOutline,
  useExportReport,
  useGenerateSection,
  useReport,
  useUpdateOutline,
  type OutlineSection,
  type Report,
  type ReportOutline,
  type ReportSection,
} from '@/api/queries/reports'
import { useSources, type SourceKind } from '@/api/queries/sources'
import { useWorkspace } from '@/api/queries/workspaces'
import { PageFooter, PageHeader, Section } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, LoadingState, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { PLANS, REPORT_CREDITS, planForFormat } from '@/features/billing/plans'
import { saveBlob } from '@/lib/download'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'

/**
 * The report: an editable outline, then one section at a time.
 *
 * The screen is the spec's `## Reports` in two steps, and the reason they are two
 * steps is cost. An outline is one short call; a section is a separate, retried
 * job. A student who can edit the outline before generating pays only for the
 * sections they actually want, and regenerating section 3 does not buy sections 1,
 * 2 and 4 again. So the section index is always the student's, and it is what the
 * request carries — including when the one primary action, "Generate report",
 * asks for every section that has no text yet: that is a loop over the same
 * per-section route, in outline order, and never a bulk call the server does not
 * have.
 *
 * **The honesty rule is structural here, not decorative.** Both prices come from
 * `config/plans.toml`, the same file the server charges from, and each one is on
 * screen *before* the action it belongs to: the per-section cost beside each
 * section's button, and the batch's total above the button that spends it. An
 * outline section whose topic is not supported by the sources says so in words and
 * with an icon — never with a colour alone, which would be invisible in monochrome
 * and silent to a screen reader. Every count on the page is one the API returned or
 * this screen counted from it: `4 SECTIONS · 3 SOURCE-SUPPORTED`, `3 READY / 1
 * UNAVAILABLE`, `3 SECTIONS INCLUDED · 1 GAP` and the draft's length are all read
 * off the payload, and a figure that is absent renders as an empty state rather
 * than as a zero. And the only page count anywhere near this screen is the
 * cheatsheet's, which is read back from the renderer rather than guessed.
 *
 * **The context column is the report's, not the workspace's.** It lists the
 * workspace's sources because that is the pool this report was drawn from and the
 * only list the API exposes — the stored report does not record which of them the
 * outline cited. So it claims nothing it cannot: a source that is still processing
 * has no chunks, cannot be one of the labels the outline quotes, and is named as
 * excluded for that reason.
 *
 * The export is plan-gated through `LockedFeature` and the matrix, never a bespoke
 * conditional: `export_formats` decides which formats exist for this account, and a
 * format the plan does not include is not offered at all on a phone build, which
 * is what the stores review for.
 */

/**
 * The screen's own width.
 *
 * The design's frame is a 1160px two-column body — the outline builder and a
 * 336px context column — so this screen does not take the `max-w-3xl` reading
 * column: at 48rem the builder would be narrower than the rows it holds and the
 * context column would have nowhere to go. The container is the frame's own column
 * (the full width less 32px a side), and the grid below puts the 336px column back
 * with `rem`, not with a pixel width, so it still grows with the student's text size.
 */
function ReportPage({ children }: { children: ReactNode }) {
  return (
    <div className="flex w-full min-w-0 flex-col gap-6 px-5 md:px-[32px] py-8">{children}</div>
  )
}

export function ReportScreen() {
  const { t } = useTranslation()
  const { reportId = '' } = useParams()
  const report = useReport(reportId)
  const error = asApiError(report.error)

  const data = report.data?.report
  const outline = data?.outline == null ? null : readableOutline(data.outline)
  // The course this report belongs to, for the breadcrumb and the eyebrow. It is
  // a separate read because the report payload carries the workspace's id and not
  // its name; an unresolved workspace renders neither rather than an empty path.
  const workspace = useWorkspace(data?.workspace_id ?? '')
  // One generation control for the whole screen, so the per-section button in the
  // list and the batch behind "Generate report" cannot both be in flight on the
  // same section — two requests for one index is two charges.
  const generation = useReportGeneration(reportId, data?.sections ?? [])

  if (report.isPending) {
    return (
      <ReportPage>
        <LoadingState label={t('reports.loading')} />
      </ReportPage>
    )
  }

  if (error !== null) {
    return (
      <ReportPage>
        <ApiErrorNotice
          error={error}
          noun={t('errors.nouns.report')}
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              {t('reports.backToWorkspaces')}
            </Link>
          }
        />
      </ReportPage>
    )
  }

  if (data === undefined) return null

  if (data.status === 'failed') {
    return (
      <ReportPage>
        <EmptyState
          title={t('reports.failed.title')}
          reason={data.error ?? t('reports.failed.reason')}
        />
      </ReportPage>
    )
  }

  if (data.outline === null || outline === null) {
    return (
      <ReportPage>
        <EmptyState title={t('reports.noOutline.title')} reason={t('reports.noOutline.reason')} />
      </ReportPage>
    )
  }

  // An outline that is present but names no sections is a row the generator could
  // not have written — `parse_outline` refuses an empty outline — so it is a
  // legacy or hand-written row, and the honest answer is to say so. Rendering the
  // editor with nothing in it would read as a model that produced nothing, and the
  // export below it has no document to build from (the route answers 409 without
  // generated sections), so neither is offered.
  if (outline.sections.length === 0) {
    return (
      <ReportPage>
        <PageHeader title={data.title} />
        <EmptyState
          title={t('reports.brokenOutline.title')}
          reason={t('reports.brokenOutline.reason')}
        />
      </ReportPage>
    )
  }

  const course = workspace.data?.course_code ?? null
  const status =
    data.status === 'draft'
      ? t('reports.status.draft')
      : data.status === 'ready'
        ? t('reports.status.ready')
        : t('reports.status.generating')

  return (
    <ReportPage>
      <nav
        aria-label={t('reports.breadcrumb')}
        className="flex flex-wrap items-center gap-2 text-label text-muted"
      >
        <Icon name="arrow-left" className="shrink-0" />
        {/* A link that is a control carries the target floor: measured bare, a
            `.link` paints its 20px line box against the 44px phone floor. The
            label is the nav item the destination actually is — the report is
            reached through a workspace, and the Study hub holds decks and quizzes
            rather than reports, so "Study" here would name a place that does not
            contain this screen. */}
        <Link to="/workspaces" className="link inline-flex min-h-11 items-center">
          {t('nav.workspaces')}
        </Link>
        {workspace.data !== undefined && (
          <span className="min-w-0 break-words">{workspace.data.name}</span>
        )}
      </nav>

      <PageHeader
        eyebrow={
          course === null ? t('reports.eyebrow') : t('reports.eyebrowCourse', { course })
        }
        title={data.title}
        subtitle={t('reports.subtitle')}
        action={<span className="chip font-mono text-meta">{status}</span>}
      />

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_21rem] lg:items-start">
        <OutlineEditor report={data} outline={outline} />

        <div className="flex min-w-0 flex-col gap-6">
          <SourcesCard workspaceId={data.workspace_id} />
          <DraftSettings report={data} outline={outline} generation={generation} />
          <p className="text-label text-muted">{t('reports.reviewReminder')}</p>
        </div>
      </div>

      <Sections report={data} generation={generation} />
      <ExportMenu reportId={data.id} />
      <ReportFooter report={data} workspaceName={workspace.data?.name ?? null} />
    </ReportPage>
  )
}

/**
 * Every way this screen may ask for a section to be written, in one place.
 *
 * Two controls spend the same credits — the button on a section's row and the
 * batch behind "Generate report" — so they share one mutation. Two `useMutation`
 * hooks over the same route would each hold their own pending flag, and a click on
 * one while the other was running would be a second request for a section that is
 * already being paid for. The batch stops at the first failure for the same
 * reason: the sections before it are written, and asking for them again would
 * charge for them twice.
 */
interface ReportGeneration {
  /** The indices with no text yet, in outline order. */
  unwritten: number[]
  write: (index: number) => Promise<void>
  writeAll: () => Promise<void>
  /** A batch is running, so a second request for any of its sections is a second charge. */
  batch: boolean
  /** The index whose request is in flight, or null. */
  pendingIndex: number | null
  error: ApiError | null
}

function useReportGeneration(reportId: string, sections: ReportSection[]): ReportGeneration {
  const generate = useGenerateSection(reportId)
  const [batch, setBatch] = useState(false)

  // A section with no text yet, and not already being written by a job that
  // outlives this tab. `generating` is the server's flag, so a batch started on
  // another device is not asked for again here.
  const unwritten = sections.flatMap((section, index) =>
    section.body_md === null && !section.generating ? [index] : [],
  )

  async function write(index: number) {
    await generate.mutateAsync(index)
  }

  async function writeAll() {
    setBatch(true)
    try {
      for (const index of unwritten) {
        await write(index)
      }
    } catch {
      // The notice under the button states the failure. Swallowing it here keeps
      // the loop from rejecting twice — the mutation's own error is what renders.
    } finally {
      setBatch(false)
    }
  }

  return {
    unwritten,
    write,
    writeAll,
    /** A batch is running, so a second request for any of its sections is a second charge. */
    batch,
    pendingIndex: generate.isPending ? (generate.variables ?? null) : null,
    error: asApiError(generate.error),
  }
}

/**
 * Step 1: the outline, as the model proposed it and as the student then edits it.
 *
 * The edits are local until they are saved, so a rename, a move and a delete are
 * one write rather than three round trips, and a reorder cannot land as two
 * renames if the second request fails. The draft is re-seeded only when the
 * server's outline actually changes: a poll while a section generates must not
 * throw away an edit the student has not saved yet.
 *
 * **A section's purpose is editable, and that is what makes "Add section" mean
 * anything.** `PATCH /reports/:id` validates the body with the core
 * `parse_outline`, which refuses a section with an empty heading *or* an empty
 * purpose — the purpose is prompt input, and the section call has nothing to
 * establish without it. A section added here therefore has to be able to acquire
 * both, and a purpose shown as read-only text could never acquire one. Save is
 * held back while either is blank so the student is told before the request rather
 * than by a 400 after it.
 */
function OutlineEditor({ report, outline }: { report: Report; outline: ReportOutline }) {
  const { t } = useTranslation()
  const update = useUpdateOutline(report.id)
  const [draft, setDraft] = useState<ReportOutline>(outline)
  const seeded = useRef<string>(JSON.stringify(outline))
  const error = asApiError(update.error)

  useEffect(() => {
    const signature = JSON.stringify(outline)
    if (seeded.current === signature) return
    seeded.current = signature
    setDraft(outline)
  }, [outline])

  const dirty = JSON.stringify(draft) !== JSON.stringify(outline)
  const incomplete = draft.sections.some(
    (section) => section.heading.trim() === '' || section.purpose.trim() === '',
  )
  const supported = draft.sections.filter((section) => !section.gaps).length

  function replaceSections(sections: OutlineSection[]) {
    setDraft({ title: draft.title, sections })
  }

  function move(index: number, by: number) {
    const next = [...draft.sections]
    const target = index + by
    const moved = next[index]
    const displaced = next[target]
    if (moved === undefined || displaced === undefined) return
    next[index] = displaced
    next[target] = moved
    replaceSections(next)
  }

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <div className="flex flex-col gap-1">
        <label className="label" htmlFor="report-title">
          {t('reports.outline.titleLabel')}
        </label>
        <input
          id="report-title"
          className="field"
          value={draft.title}
          onChange={(event) => {
            setDraft({ title: event.target.value, sections: draft.sections })
          }}
        />
        {/* The outline came from the model, and the label says so: a shape the
            student did not write must not read as one they did. */}
        <p className="text-caption text-muted">
          {t('honesty.suggested', { what: t('reports.outline.what') })}
        </p>
      </div>

      <Section
        title={t('reports.outline.title')}
        action={
          <span className="meta min-w-0 break-words">
            {t('reports.outline.meta', { count: draft.sections.length, supported })}
          </span>
        }
      >
        <p className="text-label text-muted">{t('reports.outline.hint')}</p>

        <ol className="flex flex-col gap-3">
          {draft.sections.map((section, index) => (
            // Keyed by position, not by heading: this is a positional editor, and a
            // key that changed with the heading would remount the input the student
            // is typing into, losing focus on the first keystroke.
            <li key={index} className="card flex gap-3 p-4">
              {/* The handle is the design's affordance for the row being movable.
                  It is decoration here and not a control: reordering is done by the
                  two labelled buttons below, which are the ones a keyboard and a
                  screen reader can reach, and a drag handle that did nothing would
                  be a control that lies. */}
              <span
                aria-hidden="true"
                className="flex h-11 w-8 shrink-0 items-center justify-center text-subtle"
              >
                <Icon name="grip-vertical" />
              </span>

              <div className="flex min-w-0 flex-1 flex-col gap-3">
                <div className="flex flex-wrap items-start gap-3">
                  <div className="flex min-w-0 flex-1 flex-col gap-1">
                    <label className="label" htmlFor={`report-section-${String(index)}`}>
                      {t('reports.outline.headingLabel', { position: index + 1 })}
                    </label>
                    <input
                      id={`report-section-${String(index)}`}
                      className="field"
                      value={section.heading}
                      onChange={(event) => {
                        replaceSections(
                          draft.sections.map((entry, at) =>
                            at === index ? { ...entry, heading: event.target.value } : entry,
                          ),
                        )
                      }}
                    />
                  </div>
                  <button
                    type="button"
                    className="btn btn-quiet w-11 shrink-0 px-0"
                    aria-label={t('reports.outline.remove')}
                    onClick={() => {
                      replaceSections(draft.sections.filter((_, at) => at !== index))
                    }}
                  >
                    <Icon name="close" />
                  </button>
                </div>

                <div className="flex flex-col gap-1">
                  <label
                    className="label"
                    htmlFor={`report-section-${String(index)}-purpose`}
                  >
                    {t('reports.outline.purposeLabel', { position: index + 1 })}
                  </label>
                  <textarea
                    id={`report-section-${String(index)}-purpose`}
                    className="field"
                    rows={2}
                    value={section.purpose}
                    onChange={(event) => {
                      replaceSections(
                        draft.sections.map((entry, at) =>
                          at === index ? { ...entry, purpose: event.target.value } : entry,
                        ),
                      )
                    }}
                  />
                </div>

                {section.labels.length > 0 && (
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="meta">
                      {t('reports.outline.supports', { count: section.labels.length })}
                    </span>
                    {section.labels.map((label) => (
                      <span key={`${String(index)}-${String(label)}`} className="chip chip-source">
                        [{label}]
                      </span>
                    ))}
                  </div>
                )}

                {/* The caveat the spec names: a word, a sentence and an icon, never
                    a colour. A student revising from this outline has to know which
                    sections their sources cannot support before they spend credits
                    on them, and the meta line says what actually happens to the
                    section — it is left out of the draft. */}
                {section.gaps && (
                  <div className="flex flex-col items-start gap-2">
                    <span className="chip chip-due">{t('reports.outline.gap')}</span>
                    <p className="flex items-start gap-2 text-label">
                      <Icon name="alert" className="mt-0.5 shrink-0" />
                      <span>{t('honesty.notInSources')}</span>
                    </p>
                    <p className="meta">{t('reports.outline.gapMeta')}</p>
                  </div>
                )}

                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    className="btn btn-quiet"
                    disabled={index === 0}
                    onClick={() => {
                      move(index, -1)
                    }}
                  >
                    {t('reports.outline.moveUp')}
                  </button>
                  <button
                    type="button"
                    className="btn btn-quiet"
                    disabled={index === draft.sections.length - 1}
                    onClick={() => {
                      move(index, 1)
                    }}
                  >
                    {t('reports.outline.moveDown')}
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ol>

        <button
          type="button"
          className="btn btn-secondary w-full"
          onClick={() => {
            // A section added by hand cites nothing, so it starts as a gap: the
            // server re-validates `labels` against the sources on generation, and
            // claiming "source-supported" for a section with no labels would be the
            // one dishonest field in the outline.
            replaceSections([
              ...draft.sections,
              { heading: '', purpose: '', labels: [], gaps: true },
            ])
          }}
        >
          <Icon name="plus" />
          {t('reports.outline.addSection')}
        </button>

        <button
          type="button"
          className="btn btn-secondary self-start"
          disabled={!dirty || incomplete || update.isPending}
          onClick={() => {
            update.mutate(draft)
          }}
        >
          {update.isPending ? t('reports.outline.saving') : t('reports.outline.save')}
        </button>

        {dirty && incomplete && (
          <p className="meta">{t('reports.outline.incomplete')}</p>
        )}

        {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.report')} />}
      </Section>
    </div>
  )
}

/**
 * The sources this report draws on, and the one that cannot be drawn on.
 *
 * The list is the workspace's — the only source list the API exposes, and the pool
 * the outline was proposed from. What the card refuses to do is imply more than
 * that: it does not number the rows as citation labels, because the outline's
 * `[n]` marks are chunk labels assigned per generation and a client cannot map one
 * to a source without guessing. The marker tile carries the row's position, the
 * badge carries the format in the words the rest of the app uses, and the meta
 * line carries the ingestion status the server reported.
 */
function SourcesCard({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const sources = useSources(workspaceId)
  const error = asApiError(sources.error)
  const [copied, setCopied] = useState<string | null>(null)

  const list = sources.data ?? []
  const ready = list.filter((source) => source.status === 'ready').length
  const unavailable = list.length - ready
  // A source that is not ready has no chunks, so it cannot be one of the labels
  // the outline quotes: "not used in this outline" is a fact about it rather than a
  // guess. One is named because a count is not a sentence.
  const held = list.find((source) => source.status !== 'ready') ?? null

  return (
    <Section
      title={t('reports.sources.title')}
      action={
        <span className="meta min-w-0 break-words">
          {unavailable === 0
            ? t('reports.sources.metaReady', { ready })
            : t('reports.sources.meta', { ready, unavailable })}
        </span>
      }
    >
      <div className="card flex flex-col gap-4 p-5">
        {sources.isPending && <SkeletonRows rows={2} label={t('workspaces.sources.loading')} />}

        {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.report')} />}

        {sources.data !== undefined && list.length === 0 && (
          <EmptyState
            title={t('workspaces.sources.emptyTitle')}
            reason={t('workspaces.sources.emptyReason')}
            padded={false}
          />
        )}

        {list.length > 0 && (
          <ul className="flex flex-col gap-3">
            {list.map((source, index) => (
              <li key={source.id} className="flex flex-wrap items-center gap-3">
                <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-field bg-source-tint">
                  <span className="meta text-source">{index + 1}</span>
                </span>
                <span className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="break-words text-label font-medium">{source.title}</span>
                  <span className="meta">{t(`workspaces.source.status.${source.status}`)}</span>
                </span>
                <span className="chip chip-source">{sourceKindLabel(source.kind, t)}</span>
                <button
                  type="button"
                  className="btn btn-quiet w-11 shrink-0 px-0"
                  aria-label={t('reports.sources.copy', { title: source.title })}
                  onClick={() => {
                    // The clipboard can refuse (an insecure context, a permission),
                    // and a copy button that silently did nothing is the failure the
                    // confirmation below exists to avoid.
                    void navigator.clipboard
                      .writeText(source.url ?? source.title)
                      .then(() => {
                        setCopied(source.id)
                      })
                      .catch(() => {
                        setCopied(null)
                      })
                  }}
                >
                  <Icon name="copy" />
                </button>
              </li>
            ))}
          </ul>
        )}

        {held !== null && (
          <p className="text-label text-muted">
            {held.status === 'failed'
              ? t('reports.sources.failed', { title: held.title })
              : t('reports.sources.processing', { title: held.title })}
          </p>
        )}

        {copied !== null && (
          <span className="meta" role="status">
            {t('reports.sources.copied')}
          </span>
        )}

        {/* The one thing this screen can do about a missing source is send the
            student to where sources are added, which is the workspace. */}
        <Link to={`/workspaces/${workspaceId}`} className="btn btn-secondary">
          <Icon name="plus" />
          {t('reports.sources.add')}
        </Link>
      </div>
    </Section>
  )
}

/**
 * The format badge's word, in the same keys the workspace's own source list uses,
 * so a source is named the same way wherever it is shown.
 */
function sourceKindLabel(kind: SourceKind, t: TFunction): string {
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

/**
 * How the draft will be built, and the screen's one primary action.
 *
 * **Every figure here is one the payload carries or this screen counted from it.**
 * "Format" and "Citations" are the document's shape and say so in words; "Length"
 * is the stored sections' word count, and it renders the sections' own "Not
 * written yet" rather than a zero while there is nothing written; the footer's
 * `N SECTIONS INCLUDED · M GAP` is the outline's own `gaps` flags, and the gap half
 * is left off rather than shown as `0 GAP`.
 *
 * **"Include source citations" cannot be turned off, and the row says why rather
 * than pretending.** The server's section prompt is built from the outline's chunk
 * labels and the route has no flag to suppress them, so a live checkbox would be a
 * control whose state the request never carries. It is rendered checked and
 * disabled, with the reason beside it.
 */
function DraftSettings({
  report,
  outline,
  generation,
}: {
  report: Report
  outline: ReportOutline
  generation: ReportGeneration
}) {
  const { t } = useTranslation()
  const credits = REPORT_CREDITS.section
  const words = report.sections.reduce(
    (total, section) =>
      total + (section.body_md === null ? 0 : section.body_md.split(/\s+/u).filter(Boolean).length),
    0,
  )
  const written = report.sections.some((section) => section.body_md !== null)
  const included = outline.sections.filter((section) => !section.gaps).length
  const gaps = outline.sections.length - included
  const sections = generation.unwritten.length
  const cost = credits === null ? null : sections * credits
  const busy = generation.batch || generation.pendingIndex !== null

  return (
    <Section title={t('reports.settings.title')}>
      <div className="card flex flex-col gap-4 p-5">
        <dl className="flex flex-col gap-3">
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <dt className="text-label text-muted">{t('reports.settings.format')}</dt>
            <dd className="text-label">{t('reports.settings.formatValue')}</dd>
          </div>
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <dt className="text-label text-muted">{t('reports.settings.length')}</dt>
            <dd className="text-label">
              {written
                ? t('reports.settings.words', { count: words })
                : t('reports.sections.notWritten')}
            </dd>
          </div>
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <dt className="text-label text-muted">{t('reports.settings.citations')}</dt>
            <dd className="text-label">{t('reports.settings.citationsValue')}</dd>
          </div>
        </dl>

        <div aria-hidden="true" className="h-px bg-border" />

        <div className="flex min-h-11 items-center gap-3">
          <input
            id="report-citations"
            type="checkbox"
            className="size-5 shrink-0"
            checked
            disabled
            readOnly
            aria-describedby="report-citations-why"
          />
          <label id="report-citations-why" htmlFor="report-citations" className="text-label">
            {t('reports.settings.includeCitations')}
          </label>
        </div>
        <p className="text-caption text-muted">{t('reports.settings.citationsAlways')}</p>

        <p className="text-label text-muted">{t('reports.settings.policy')}</p>

        {cost !== null && (
          <p className="meta">
            {t('reports.settings.cost', { count: sections, credits: cost })}
          </p>
        )}

        <button
          type="button"
          className="btn btn-primary"
          disabled={sections === 0 || busy}
          onClick={() => {
            void generation.writeAll()
          }}
        >
          {generation.batch ? t('reports.settings.generating') : t('reports.settings.generate')}
        </button>

        {sections === 0 && <p className="meta">{t('reports.settings.complete')}</p>}

        {generation.error !== null && (
          <ApiErrorNotice error={generation.error} noun={t('errors.nouns.report')} />
        )}

        <p className="meta">
          {t('reports.settings.sections', { count: included })}
          {gaps > 0 && ` · ${t('reports.settings.gaps', { count: gaps })}`}
        </p>
      </div>
    </Section>
  )
}

/**
 * Step 2: the sections, generated one at a time.
 *
 * `generating` is the server's flag, not this component's pending mutation. The
 * job outlives the tab that started it, so a client-side flag would show "idle"
 * for work that is still running on the other device, and "Regenerate" for a
 * section that has no text yet. The local pending index is a second signal rather
 * than a replacement: it is what makes the row react on the click, before the
 * refetch that carries the server's flag has landed.
 */
function Sections({ report, generation }: { report: Report; generation: ReportGeneration }) {
  const { t } = useTranslation()
  const written = report.sections.filter((section) => section.body_md !== null).length

  return (
    <Section
      title={t('reports.sections.title')}
      action={
        <span className="meta min-w-0 break-words">
          {t('reports.sectionProgress', { written, total: report.sections.length })}
        </span>
      }
    >
      <p className="text-label text-muted">{t('reports.sections.hint')}</p>

      <ol className="flex flex-col gap-3">
        {report.sections.map((section, index) => {
          const done = section.body_md !== null
          // Only the row whose request is in flight says so. A batch writes one
          // section at a time, so the rows it has not reached yet must not claim to
          // be writing — they are only held back from a second request.
          const writing = section.generating || generation.pendingIndex === index
          const held = writing || generation.batch
          return (
            <li key={index} className="card flex flex-col gap-3 p-4">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                {/* Same defect and the same fix as the report row in
                    `ReportPanel`: with three fixed-width siblings on the line
                    (a status chip, the price, a button), a `flex-1` heading has a
                    flex basis of `0` and is left whatever is over. Measured in
                    Chromium at 390px, "2. Perbandingan TCP dan UDP" rendered
                    **29px wide over 10 lines** at 100% text and 44px over 13 lines
                    at 200%. It takes the whole line on a phone instead, and goes
                    back to `flex-1` from `sm` up. */}
                <h3 className="w-full min-w-0 break-words font-medium sm:w-auto sm:flex-1">
                  {t('reports.sections.heading', {
                    position: index + 1,
                    heading: section.heading,
                  })}
                </h3>
                <span
                  className={writing ? 'chip chip-working' : done ? 'chip chip-ready' : 'chip'}
                  role="status"
                >
                  {writing && (
                    <span
                      className="h-2 w-2 animate-working rounded-full bg-current"
                      aria-hidden="true"
                    />
                  )}
                  {writing
                    ? t('reports.sections.writing')
                    : done
                      ? t('reports.sections.written')
                      : t('reports.sections.notWritten')}
                </span>
                {/* The price of this one action, before the button that spends
                    it. A cost that appears after the fact is not a cost shown
                    before the action. */}
                {REPORT_CREDITS.section !== null && (
                  <span className="meta">
                    {t('reports.sections.cost', { credits: REPORT_CREDITS.section })}
                  </span>
                )}
                <button
                  type="button"
                  className="btn btn-secondary"
                  disabled={held}
                  onClick={() => {
                    void generation.write(index)
                  }}
                >
                  {writing
                    ? t('reports.sections.writing')
                    : done
                      ? t('reports.sections.regenerate')
                      : t('reports.sections.generate')}
                </button>
              </div>

              {section.error !== null && section.error.length > 0 && (
                <p className="note note-danger text-small">{section.error}</p>
              )}

              {/* The stored Markdown, as the server wrote it. It is shown
                  verbatim in a preformatted block rather than rendered to HTML:
                  the document is the server's, and a second Markdown renderer
                  here would be a second answer to what the report says. The
                  `[n]` marks in it are the outline's chunk labels. */}
              {section.body_md !== null && (
                <p className="whitespace-pre-wrap break-words text-body">{section.body_md}</p>
              )}
            </li>
          )
        })}
      </ol>

      {generation.error !== null && (
        <ApiErrorNotice error={generation.error} noun={t('errors.nouns.report')} />
      )}
    </Section>
  )
}

/**
 * The export.
 *
 * The formats offered are the account's `export_formats`, read from
 * `config/plans.toml`. A format the plan does not include is not a disabled item
 * in a menu: it is `LockedFeature`, which names the plan on web and desktop and
 * says only "Not available on your plan" on a phone, with no price and no
 * purchase link.
 *
 * The download goes through the API layer because the route needs the bearer
 * token, so the document arrives as bytes and `saveBlob` hands them to the
 * platform. The filename carries the format's extension, since the server's
 * `content-disposition` is not available to a fetch this way.
 */
function ExportMenu({ reportId }: { reportId: string }) {
  const { t } = useTranslation()
  const session = useSession()
  const exportReport = useExportReport(reportId)
  const error = asApiError(exportReport.error)
  // The formats are the account's, so they are read only once the account is
  // known. Treating an unresolved session as "no formats" would render a locked
  // message naming a plan this screen has not been told about — a claim about the
  // plan made before knowing it, which is the same defect as inventing a number.
  const allowed =
    session.status === 'signed-in' ? (PLANS[session.user.plan_id]?.exportFormats ?? []) : null
  const offered = EXPORT_FORMATS.filter((format) => allowed?.includes(format) ?? false)

  return (
    <Section title={t('reports.export.title')}>
      {allowed === null && <LoadingState label={t('reports.export.loading')} />}

      {allowed !== null && offered.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {offered.map((format) => (
            <button
              key={format}
              type="button"
              className="btn btn-secondary"
              disabled={exportReport.isPending}
              onClick={() => {
                exportReport.mutate(format, {
                  onSuccess: ({ blob }) => {
                    saveBlob(blob, `${t('reports.export.filename')}.${format}`)
                  },
                })
              }}
            >
              {exportReport.isPending
                ? t('reports.export.preparing')
                : t(`reports.export.format.${format}`)}
            </button>
          ))}
        </div>
      )}

      {allowed !== null && offered.length === 0 && (
        <p className="text-label text-muted">{t('reports.export.none')}</p>
      )}

      {/* One entry per format the matrix withholds, spelled out rather than
          generated from the format list: the keys are literal so the i18n
          contract test can see that every one of them is used, and a `whatKey`
          built by interpolation would be invisible to it and rot. */}
      {allowed !== null && !allowed.includes('md') && (
        <LockedFeature
          whatKey="reports.locked.md"
          minPlan={planForFormat('md') ?? 'Pro'}
          reasonKey="reports.locked.mdReason"
        />
      )}
      {allowed !== null && !allowed.includes('pdf') && (
        <LockedFeature
          whatKey="reports.locked.pdf"
          minPlan={planForFormat('pdf') ?? 'Pro'}
          reasonKey="reports.locked.pdfReason"
        />
      )}

      {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.export')} />}
    </Section>
  )
}

/**
 * The foot of the page: when this report last changed, and which course it is for.
 *
 * `updated_at` is the row's own timestamp, so the line says what it is — the report
 * was updated — rather than "outline saved", which a section generation also moves
 * and which would then be a claim about a write that did not happen. It is rendered
 * in the account's zone, like every other instant in the app.
 */
function ReportFooter({
  report,
  workspaceName,
}: {
  report: Report
  workspaceName: string | null
}) {
  const { t } = useTranslation()
  const session = useSession()
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  return (
    <PageFooter
      left={t('reports.footer.updated', {
        time: formatInZone(new Date(report.updated_at), timezone, { timeStyle: 'short' }),
      })}
      right={workspaceName ?? undefined}
    />
  )
}
