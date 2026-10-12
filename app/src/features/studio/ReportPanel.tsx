import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCreateCheatsheet,
  type CheatsheetDepth,
} from '@/api/queries/cheatsheets'
import {
  readableOutline,
  useCreateOutline,
  useReports,
  type Report,
  type ReportStatus,
} from '@/api/queries/reports'
import { useSources } from '@/api/queries/sources'
import { Field } from '@/components/ui/Field'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { REPORT_CREDITS } from '@/features/billing/plans'
import { SourcePicker } from '@/features/workspaces/SourcePicker'

/**
 * Studio → Report.
 *
 * The first of the report's two steps. Proposing an outline is asynchronous and
 * costs credits, so the panel says both things plainly: the button names the
 * price of the outline *and* of each section that follows it before either is
 * pressed, and a report in flight shows its status rather than a spinner that
 * never resolves.
 *
 * The panel also creates cheatsheets, which is why there are two forms here rather
 * than a fourth sub-tab. `docs/API.md` has no list route for a cheatsheet, so
 * there is nothing for a panel to list: a form is the only entry point, and the
 * document itself is read on its own screen. Putting the form in the tab that
 * already owns this workspace's printable output keeps the strip at the three
 * kinds of thing the spec's workspace view has room for.
 *
 * The prices are read from `config/plans.toml` — the file the server charges
 * from — rather than typed here, because a price the app invented is exactly the
 * kind of number the honesty gate exists to keep off the screen. When the file
 * does not declare one, no price is shown at all: a wrong number is worse than a
 * missing one.
 *
 * Style and length are free text rather than selects. The outline prompt takes
 * them as prose ("audience/style", "length target") and documents no set of
 * accepted values, so a dropdown would be this panel inventing a contract the
 * prompt does not have.
 */

const STATUS_CHIP: Record<ReportStatus, string> = {
  draft: 'chip',
  generating: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

export function ReportPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const sources = useSources(workspaceId)
  const reports = useReports(workspaceId)
  const create = useCreateOutline(workspaceId)
  const [selected, setSelected] = useState<string[]>([])
  const [topic, setTopic] = useState('')
  const [style, setStyle] = useState('')
  const [length, setLength] = useState('')

  const readySources = (sources.data ?? []).filter((source) => source.status === 'ready')
  const error = asApiError(create.error)
  const ready =
    topic.trim().length > 0 &&
    style.trim().length > 0 &&
    length.trim().length > 0 &&
    selected.length > 0 &&
    !create.isPending
  const reportsList = reports.data?.reports

  return (
    <div className="flex flex-col gap-4 px-4 py-4">
      <form
        className="panel flex flex-col gap-3 px-3 py-3"
        onSubmit={(event) => {
          event.preventDefault()
          if (!ready) return
          create.mutate(
            {
              topic: topic.trim(),
              style: style.trim(),
              length: length.trim(),
              sourceIds: selected,
            },
            {
              onSuccess: () => {
                setTopic('')
                setSelected([])
              },
            },
          )
        }}
      >
        <Field
          label={t('studio.report.topicLabel')}
          name="report-topic"
          value={topic}
          hint={t('studio.report.topicHint')}
          onChange={(event) => {
            setTopic(event.target.value)
          }}
        />
        <Field
          label={t('studio.report.styleLabel')}
          name="report-style"
          value={style}
          hint={t('studio.report.styleHint')}
          onChange={(event) => {
            setStyle(event.target.value)
          }}
        />
        <Field
          label={t('studio.report.lengthLabel')}
          name="report-length"
          value={length}
          hint={t('studio.report.lengthHint')}
          onChange={(event) => {
            setLength(event.target.value)
          }}
        />
        <SourcePicker
          workspaceId={workspaceId}
          sources={readySources}
          selected={selected}
          onChange={setSelected}
          disabled={create.isPending}
          legend={t('studio.generateFrom')}
          emptyHint={t('studio.noIndexedSources')}
        />
        {/* Both prices, before either action: the outline is what this form
            starts, and the sections it leads to are what the student is really
            buying. A number that appears only after the first purchase is not a
            cost shown before the action. */}
        {REPORT_CREDITS.outline !== null && REPORT_CREDITS.section !== null && (
          <p className="text-xs text-muted">
            {t('studio.report.cost', {
              outline: REPORT_CREDITS.outline,
              section: REPORT_CREDITS.section,
            })}
          </p>
        )}
        <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
          {create.isPending ? t('studio.starting') : t('studio.report.generate')}
        </button>
        {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.reports')} />}
      </form>

      {reports.isPending && <SkeletonRows rows={2} label={t('studio.report.loading')} />}

      {reportsList !== undefined && (
        <h2 className="font-medium">{t('studio.report.reportsHeading')}</h2>
      )}

      {reportsList !== undefined && reportsList.length === 0 && (
        <EmptyState
          title={t('studio.report.empty.title')}
          reason={t('studio.report.empty.reason')}
        />
      )}

      {reportsList !== undefined && reportsList.length > 0 && (
        <ul className="flex flex-col divide-y divide-border border-y border-border">
          {reportsList.map((report) => (
            <ReportRow key={report.id} report={report} />
          ))}
        </ul>
      )}

      <CheatsheetForm workspaceId={workspaceId} sources={readySources} />
    </div>
  )
}

/**
 * The cheatsheet half of the sub-tab.
 *
 * A cheatsheet is created and then read on its own screen, so this is a form and
 * nothing else: the panel navigates to the new document rather than listing every
 * sheet in the workspace, because the only one a student is looking at is the one
 * they just asked for, and `docs/API.md` has no list route to build a list from.
 *
 * The price is `report_section`, which is what the plan matrix charges a
 * cheatsheet, and it is shown before the button that spends it.
 */
function CheatsheetForm({
  workspaceId,
  sources,
}: {
  workspaceId: string
  sources: { id: string; title: string }[]
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const create = useCreateCheatsheet(workspaceId)
  const [selected, setSelected] = useState<string[]>([])
  const [scope, setScope] = useState('')
  const [examDate, setExamDate] = useState('')
  const [depth, setDepth] = useState<CheatsheetDepth>('normal')
  const error = asApiError(create.error)
  const ready = scope.trim().length > 0 && selected.length > 0 && !create.isPending

  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const depthLabel: Record<CheatsheetDepth, string> = {
    brief: t('studio.cheatsheet.depth.brief'),
    normal: t('studio.cheatsheet.depth.normal'),
    thorough: t('studio.cheatsheet.depth.thorough'),
  }

  return (
    <form
      className="panel flex flex-col gap-3 px-3 py-3"
      onSubmit={(event) => {
        event.preventDefault()
        if (!ready) return
        create.mutate(
          {
            scope: scope.trim(),
            // Empty means "no date", which is a real state: a cheatsheet for a
            // course rather than for one exam.
            examDate: examDate.length > 0 ? examDate : null,
            depth,
            sourceIds: selected,
          },
          {
            onSuccess: (response) => {
              setScope('')
              setSelected([])
              void navigate(`/cheatsheets/${response.cheatsheet.id}`)
            },
          },
        )
      }}
    >
      <h2 className="font-medium">{t('studio.cheatsheet.heading')}</h2>
      <Field
        label={t('studio.cheatsheet.scopeLabel')}
        name="cheatsheet-scope"
        value={scope}
        hint={t('studio.cheatsheet.scopeHint')}
        onChange={(event) => {
          setScope(event.target.value)
        }}
      />
      <div className="flex flex-col gap-1">
        <label className="label" htmlFor="cheatsheet-exam-date">
          {t('studio.cheatsheet.examDateLabel')}
        </label>
        <input
          id="cheatsheet-exam-date"
          className="field"
          type="date"
          value={examDate}
          onChange={(event) => {
            setExamDate(event.target.value)
          }}
        />
        <span className="text-xs text-muted">{t('studio.cheatsheet.examDateHint')}</span>
      </div>
      <div className="flex flex-col gap-1">
        <label className="label" htmlFor="cheatsheet-depth">
          {t('studio.cheatsheet.depthLabel')}
        </label>
        <select
          id="cheatsheet-depth"
          className="field"
          value={depth}
          onChange={(event) => {
            setDepth(event.target.value as CheatsheetDepth)
          }}
        >
          {(['brief', 'normal', 'thorough'] as const).map((value) => (
            <option key={value} value={value}>
              {depthLabel[value]}
            </option>
          ))}
        </select>
      </div>
      <SourcePicker
        workspaceId={workspaceId}
        sources={sources}
        selected={selected}
        onChange={setSelected}
        disabled={create.isPending}
        legend={t('studio.generateFrom')}
        emptyHint={t('studio.noIndexedSources')}
      />
      {REPORT_CREDITS.section !== null && (
        <p className="text-xs text-muted">
          {t('studio.cheatsheet.cost', { credits: REPORT_CREDITS.section })}
        </p>
      )}
      <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
        {create.isPending ? t('studio.starting') : t('studio.cheatsheet.generate')}
      </button>
      {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.cheatsheets')} />}
    </form>
  )
}

/**
 * One report in the list.
 *
 * The section count is derived from what the server sent — the outline's sections
 * and the ones that already have a body — rather than from a stored number, so
 * the row cannot claim progress the report does not have. `0 of 5` before the
 * first section is generated is a real state and is shown as one.
 */
function ReportRow({ report }: { report: Report }) {
  const { t } = useTranslation()
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const statusLabel: Record<ReportStatus, string> = {
    draft: t('studio.report.status.draft'),
    generating: t('studio.status.generating'),
    ready: t('studio.status.ready'),
    failed: t('studio.status.failed'),
  }
  // The outline is passed through by the server, so it can be a row the
  // generator would not have written — see `readableOutline`. Without it this
  // read threw and took the whole Studio panel down with an error boundary,
  // which is also what made the report screen behind this link unreachable.
  const total = report.outline === null ? 0 : readableOutline(report.outline).sections.length
  const written = report.sections.filter((section) => section.body_md !== null).length

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {/* `w-full … @md:w-auto @md:flex-1` is load-bearing, not decoration, and it is
            the opposite of the `min-w-0` a fieldset needs. This row's other three
            children are all effectively fixed-width (a count, a status chip, a
            button), while `flex-1` gives this span a flex basis of `0`: the three
            fit on the line without it, so the title is left the remainder. Measured
            in Chromium at 390px, the title rendered **16px wide and 18 lines** — two
            characters per line, a report's own name unreadable on a phone — and at
            200% text scale 100px over 6 lines. Giving it the whole line at narrow
            widths pushes the other three to the next line and lets the title wrap at
            word boundaries; `@md:` restores the one-line row when the panel is wide
            enough for all four.

            `@md:` (28rem of *container*) rather than the viewport `sm:` this used to
            carry, because the width that decides this is the panel's: at 1365px a
            three-column grid leaves a panel ~157px of content while a 390px phone
            leaves one panel ~350px, so `sm:` fired on the desktop window and handed
            the title the same squeeze the phone measurement had just fixed — and the
            container breakpoint is rem-based, so its threshold doubles with the
            reader's text size, which is when the room is actually needed. The
            `@container` is on the panel content box in `WorkspaceScreen`. */}
        <span className="w-full min-w-0 wrap-anywhere font-medium @md:w-auto @md:flex-1">
          {report.title.length > 0 ? report.title : t('studio.report.untitled')}
        </span>
        {total > 0 && (
          <span className="meta">
            {t('studio.report.written', { written, total })}
          </span>
        )}
        <span className={STATUS_CHIP[report.status]} role="status">
          {report.status === 'generating' && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {statusLabel[report.status]}
        </span>
        <Link to={`/reports/${report.id}`} className="btn btn-secondary">
          {t('studio.report.open')}
        </Link>
      </div>

      {report.error !== null && report.error.length > 0 && (
        <p className="note note-danger text-sm">{report.error}</p>
      )}
    </li>
  )
}
