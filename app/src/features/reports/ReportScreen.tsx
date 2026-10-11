import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  EXPORT_FORMATS,
  useExportReport,
  useGenerateSection,
  useReport,
  useUpdateOutline,
  type Report,
  type ReportOutline,
  type OutlineSection,
} from '@/api/queries/reports'
import { Icon } from '@/components/ui/Icon'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { PLANS, REPORT_CREDITS, planForFormat } from '@/features/billing/plans'
import { useSession } from '@/features/auth/useSession'
import { saveBlob } from '@/lib/download'

/**
 * The report: an editable outline, then one section at a time.
 *
 * The screen is the spec's `## Reports` in two steps, and the reason they are two
 * steps is cost. An outline is one short call; a section is a separate, retried
 * job. A student who can edit the outline before generating pays only for the
 * sections they actually want, and regenerating section 3 does not buy sections 1,
 * 2 and 4 again. So the screen never generates the whole report behind one button:
 * the section index is always the student's, and it is what the request carries.
 *
 * **The honesty rule is structural here, not decorative.** Both prices come from
 * `config/plans.toml`, the same file the server charges from, and each one is on
 * screen *before* the action it belongs to. An outline section whose topic is not
 * supported by the sources says so in words and with an icon — never with a colour
 * alone, which would be invisible in monochrome and silent to a screen reader.
 * And the only page count anywhere near this screen is the cheatsheet's, which is
 * read back from the renderer rather than guessed.
 *
 * The export is plan-gated through `LockedFeature` and the matrix, never a bespoke
 * conditional: `export_formats` decides which formats exist for this account, and a
 * format the plan does not include is not offered at all on a phone build, which
 * is what the stores review for.
 */

export function ReportScreen() {
  const { t } = useTranslation()
  const { reportId = '' } = useParams()
  const report = useReport(reportId)
  const error = asApiError(report.error)

  if (report.isPending) return <LoadingState label={t('reports.loading')} />

  if (error !== null) {
    return (
      <div className="px-4 py-4">
        <ApiErrorNotice
          error={error}
          noun={t('errors.nouns.report')}
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              {t('reports.backToWorkspaces')}
            </Link>
          }
        />
      </div>
    )
  }

  const data = report.data?.report
  if (data === undefined) return null

  if (data.status === 'failed') {
    return (
      <div className="px-4 py-4">
        <EmptyState
          title={t('reports.failed.title')}
          reason={data.error ?? t('reports.failed.reason')}
        />
      </div>
    )
  }

  if (data.outline === null) {
    return (
      <div className="px-4 py-4">
        <EmptyState title={t('reports.noOutline.title')} reason={t('reports.noOutline.reason')} />
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-6 px-4 py-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h1 className="min-w-0 flex-1 break-words">{data.title}</h1>
        <span className="meta">
          {t('reports.sectionProgress', {
            written: data.sections.filter((section) => section.body_md !== null).length,
            total: data.outline.sections.length,
          })}
        </span>
      </div>

      <OutlineEditor report={data} outline={data.outline} />
      <Sections report={data} />
      <ExportMenu reportId={data.id} />
    </div>
  )
}

/**
 * Step 1: the outline, as the model proposed it and as the student then edits it.
 *
 * The edits are local until they are saved, so a rename, a move and a delete are
 * one write rather than three round trips, and a reorder cannot land as two
 * renames if the second request fails. The draft is re-seeded only when the
 * server's outline actually changes: a poll while a section generates must not
 * throw away an edit the student has not saved yet.
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
    <section aria-labelledby="report-outline-heading" className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 id="report-outline-heading" className="font-medium">
          {t('reports.outline.title')}
        </h2>
        {/* The outline came from the model, and the label says so: a shape the
            student did not write must not read as one they did. */}
        <p className="label">{t('honesty.suggested', { what: t('reports.outline.what') })}</p>
        <p className="text-sm text-muted">{t('reports.outline.hint')}</p>
      </div>

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
      </div>

      <ol className="flex flex-col gap-3">
        {draft.sections.map((section, index) => (
          // Keyed by position, not by heading: this is a positional editor, and a
          // key that changed with the heading would remount the input the student
          // is typing into, losing focus on the first keystroke.
          <li key={index} className="panel flex flex-col gap-2 px-3 py-3">
            <div className="flex flex-col gap-1">
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

            <p className="text-sm text-muted">{section.purpose}</p>

            {/* The caveat the spec names: text and an icon, never a colour. A
                student revising from this outline has to know which sections
                their sources cannot support before they spend credits on them. */}
            {section.gaps && (
              <p className="note note-warning flex items-start gap-2">
                <Icon name="alert" className="mt-0.5 shrink-0" />
                <span className="text-sm">{t('honesty.notInSources')}</span>
              </p>
            )}

            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              {section.labels.length > 0 && (
                <span className="meta">
                  {t('reports.outline.supports', { count: section.labels.length })}{' '}
                  {section.labels.map((label) => `[${String(label)}]`).join(' ')}
                </span>
              )}
              <div className="ml-auto flex flex-wrap gap-2">
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
                <button
                  type="button"
                  className="btn btn-quiet"
                  onClick={() => {
                    replaceSections(draft.sections.filter((_, at) => at !== index))
                  }}
                >
                  {t('reports.outline.remove')}
                </button>
              </div>
            </div>
          </li>
        ))}
      </ol>

      <button
        type="button"
        className="btn btn-primary self-start"
        disabled={!dirty || update.isPending}
        onClick={() => {
          update.mutate(draft)
        }}
      >
        {update.isPending ? t('reports.outline.saving') : t('reports.outline.save')}
      </button>

      {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.report')} />}
    </section>
  )
}

/**
 * Step 2: the sections, generated one at a time.
 *
 * `generating` is the server's flag, not this component's pending mutation. The
 * job outlives the tab that started it, so a client-side flag would show "idle"
 * for work that is still running on the other device, and "Regenerate" for a
 * section that has no text yet.
 */
function Sections({ report }: { report: Report }) {
  const { t } = useTranslation()
  const generate = useGenerateSection(report.id)
  const error = asApiError(generate.error)

  return (
    <section aria-labelledby="report-sections-heading" className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 id="report-sections-heading" className="font-medium">
          {t('reports.sections.title')}
        </h2>
        <p className="text-sm text-muted">{t('reports.sections.hint')}</p>
      </div>

      <ol className="flex flex-col gap-3">
        {report.sections.map((section, index) => {
          const written = section.body_md !== null
          // Only the row whose request is in flight says so; the others are
          // disabled because a second write to the same report's body would race
          // the first, but they must not claim to be writing it.
          const pendingHere = generate.isPending && generate.variables === index
          const inFlight = section.generating || pendingHere
          return (
            <li key={index} className="panel flex flex-col gap-2 px-3 py-3">
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
                  className={inFlight ? 'chip chip-working' : written ? 'chip chip-ready' : 'chip'}
                  role="status"
                >
                  {inFlight && (
                    <span
                      className="h-2 w-2 animate-working rounded-full bg-current"
                      aria-hidden="true"
                    />
                  )}
                  {inFlight
                    ? t('reports.sections.writing')
                    : written
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
                  disabled={inFlight}
                  onClick={() => {
                    generate.mutate(index)
                  }}
                >
                  {inFlight
                    ? t('reports.sections.writing')
                    : written
                      ? t('reports.sections.regenerate')
                      : t('reports.sections.generate')}
                </button>
              </div>

              {section.error !== null && section.error.length > 0 && (
                <p className="note note-danger text-sm">{section.error}</p>
              )}

              {/* The stored Markdown, as the server wrote it. It is shown
                  verbatim in a preformatted block rather than rendered to HTML:
                  the document is the server's, and a second Markdown renderer
                  here would be a second answer to what the report says. The
                  `[n]` marks in it are the outline's chunk labels. */}
              {section.body_md !== null && (
                <p className="whitespace-pre-wrap break-words text-sm">{section.body_md}</p>
              )}
            </li>
          )
        })}
      </ol>

      {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.report')} />}
    </section>
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
    <section aria-labelledby="report-export-heading" className="flex flex-col gap-3">
      <h2 id="report-export-heading" className="font-medium">
        {t('reports.export.title')}
      </h2>

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
        <p className="text-sm text-muted">{t('reports.export.none')}</p>
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
    </section>
  )
}
