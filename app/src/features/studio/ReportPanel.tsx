import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCreateCheatsheet,
  useCheatsheets,
  useDeleteCheatsheet,
  useRenameCheatsheet,
  type Cheatsheet,
  type CheatsheetDepth,
  type CheatsheetStatus,
} from '@/api/queries/cheatsheets'
import {
  readableOutline,
  useCreateOutline,
  useDeleteReport,
  useReports,
  type Report,
  type ReportStatus,
} from '@/api/queries/reports'
import { useSources } from '@/api/queries/sources'
import { useBulkDelete, useReorderContent } from '@/api/queries/workspaces'
import { Field } from '@/components/ui/Field'
import { Select } from '@/components/ui/Select'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { REPORT_CREDITS } from '@/features/billing/plans'
import { ContentList, type RowBindings } from '@/features/studio/ContentList'
import { ContentRow } from '@/features/studio/ContentRow'
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
  const cheatsheets = useCheatsheets(workspaceId)
  const cheatsheetList = cheatsheets.data?.cheatsheets
  const bulk = useBulkDelete(workspaceId)
  const bulkError = asApiError(bulk.error)
  const cheatsheetBulk = useBulkDelete(workspaceId)
  const cheatsheetBulkError = asApiError(cheatsheetBulk.error)
  const reorder = useReorderContent(workspaceId)

  return (
    <div className="flex flex-col gap-5 px-5 py-5 md:px-4 md:py-4">
      <form
        className="panel flex flex-col gap-3 px-4 py-4"
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
        <ContentList
          items={reportsList}
          noun={t('errors.nouns.reports')}
          onBulkDelete={(ids) => {
            bulk.mutate({ reportIds: ids })
          }}
          bulkPending={bulk.isPending}
          bulkError={bulkError}
          onReorder={async (ids) => {
            await reorder.mutateAsync({ reportIds: ids })
          }}
          row={(report, _index, bindings) => (
            <ReportRow
              key={report.id}
              report={report}
              workspaceId={workspaceId}
              bindings={bindings}
            />
          )}
        />
      )}

      <CheatsheetForm workspaceId={workspaceId} sources={readySources} />

      {cheatsheetList !== undefined && cheatsheetList.length > 0 && (
        <ContentList
          items={cheatsheetList}
          noun={t('errors.nouns.cheatsheets')}
          onBulkDelete={(ids) => {
            cheatsheetBulk.mutate({ cheatsheetIds: ids })
          }}
          bulkPending={cheatsheetBulk.isPending}
          bulkError={cheatsheetBulkError}
          onReorder={async (ids) => {
            await reorder.mutateAsync({ cheatsheetIds: ids })
          }}
          row={(sheet, _index, bindings) => (
            <CheatsheetRow
              key={sheet.id}
              sheet={sheet}
              workspaceId={workspaceId}
              bindings={bindings}
            />
          )}
        />
      )}
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
      className="panel flex flex-col gap-3 px-4 py-4"
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
      <Select
        label={t('studio.cheatsheet.depthLabel')}
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
      </Select>
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
function ReportRow({
  report,
  workspaceId,
  bindings,
}: {
  report: Report
  workspaceId: string
  bindings: RowBindings
}) {
  const { t } = useTranslation()
  const remove = useDeleteReport(workspaceId)
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
  const displayName = report.title.length > 0 ? report.title : t('studio.report.untitled')

  return (
    <ContentRow
      title={displayName}
      meta={
        total > 0 ? (
          <span className="meta">{t('studio.report.written', { written, total })}</span>
        ) : undefined
      }
      statusChip={STATUS_CHIP[report.status]}
      statusLabel={statusLabel[report.status]}
      working={report.status === 'generating'}
      action={
        <Link to={`/reports/${report.id}`} className="btn btn-secondary">
          {t('studio.report.open')}
        </Link>
      }
      error={report.error}
      // No inline rename: a report's title travels with its outline through
      // `PATCH /reports/:id`, and the outline editor on the report's own screen
      // is where both are edited together.
      deleteConfirm={t('studio.report.deleteConfirm', { name: displayName })}
      deleteBody={
        written === 0
          ? t('studio.report.deleteNoSections')
          : t('studio.report.deleteSections', { count: written })
      }
      deleteIt={t('studio.report.deleteIt')}
      deleting={t('studio.report.deleting')}
      deleteNoun={t('errors.nouns.report')}
      onDelete={(onSuccess) => {
        remove.mutate(report.id, { onSuccess })
      }}
      deletePending={remove.isPending}
      deleteError={asApiError(remove.error)}
      resetDelete={remove.reset}
      bindings={bindings}
    />
  )
}

/**
 * One cheatsheet in the list.
 *
 * Listed rather than only created: without a row there is no way back to a sheet
 * after a reload, and no way to delete one. The status word is the cheatsheet's
 * own vocabulary (`pending | generating | ready | failed`), which is why it is
 * not the deck's `STATUS_CHIP` map.
 */
function CheatsheetRow({
  sheet,
  workspaceId,
  bindings,
}: {
  sheet: Cheatsheet
  workspaceId: string
  bindings: RowBindings
}) {
  const { t } = useTranslation()
  const remove = useDeleteCheatsheet(workspaceId)
  const rename = useRenameCheatsheet(workspaceId)
  const statusLabel: Record<CheatsheetStatus, string> = {
    pending: t('studio.status.generating'),
    generating: t('studio.status.generating'),
    ready: t('studio.status.ready'),
    failed: t('studio.status.failed'),
  }
  const displayName = sheet.title.length > 0 ? sheet.title : t('studio.cheatsheet.heading')

  return (
    <ContentRow
      title={displayName}
      meta={<span className="meta">{sheet.scope}</span>}
      statusChip={STATUS_CHIP[sheet.status === 'pending' ? 'generating' : sheet.status]}
      statusLabel={statusLabel[sheet.status]}
      working={sheet.status === 'pending' || sheet.status === 'generating'}
      action={
        <Link to={`/cheatsheets/${sheet.id}`} className="btn btn-secondary">
          {t('studio.report.open')}
        </Link>
      }
      error={sheet.error}
      rename={{
        label: t('studio.cheatsheet.rename'),
        onSubmit: (title) => {
          rename.mutate({ cheatsheetId: sheet.id, title })
        },
        pending: rename.isPending,
        error: asApiError(rename.error),
        reset: rename.reset,
      }}
      deleteConfirm={t('studio.cheatsheet.deleteConfirm', { name: displayName })}
      deleteBody={t('studio.cheatsheet.deleteBody')}
      deleteIt={t('studio.cheatsheet.deleteIt')}
      deleting={t('studio.cheatsheet.deleting')}
      deleteNoun={t('errors.nouns.cheatsheet')}
      onDelete={(onSuccess) => {
        remove.mutate(sheet.id, { onSuccess })
      }}
      deletePending={remove.isPending}
      deleteError={asApiError(remove.error)}
      resetDelete={remove.reset}
      bindings={bindings}
    />
  )
}
