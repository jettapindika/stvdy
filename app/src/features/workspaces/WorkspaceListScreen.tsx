import { useId, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, validationMessage } from '@/api/errors'
import { useDashboard } from '@/api/queries/dashboard'
import {
  useCreateWorkspace,
  useDeleteWorkspace,
  useRenameWorkspace,
  useWorkspaces,
  type Workspace,
} from '@/api/queries/workspaces'
import { PageFooter, PageHeader, Section } from '@/components/layout/Section'
import { BrandIllustration } from '@/components/ui/BrandIllustration'
import { Field } from '@/components/ui/Field'
import { Icon } from '@/components/ui/Icon'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { DEFAULT_TIMEZONE, formatInZone, localDateKey } from '@/lib/time'
import { COURSE_NAMES, courseVars } from '@/tokens/tokens'

/**
 * The workspace list, and the first-run entry surface.
 *
 * This is where the brand illustration belongs: the design uses it on the entry
 * page, and an empty first-run screen is the app's equivalent moment. It is not
 * repeated on every screen, which is what keeps it a brand asset rather than a
 * sticker.
 *
 * A workspace is one course or subject, and the point of it is that many sources
 * live together so retrieval can cross them (the product's first differentiator).
 * The empty state says that, because "no workspaces" alone would not tell a new
 * student what a workspace is for.
 *
 * Renaming and deleting happen **in the card**, not in a modal. A modal would need
 * focus trapping, a restore target, and an escape route to be correct, and the
 * card already has everything needed to ask the question in place. The delete
 * confirmation names what else goes with it, because deleting a workspace takes
 * its sources.
 *
 * **The card grid is why this screen is wider than the reading column.** The design
 * draws three 371px course cards across a 1160px directory; at `max-w-3xl` they
 * would be one column of 240px cards, which is not the design. The container takes
 * the frame's own column instead — the full width less 32px a side — and the prose
 * inside a card keeps its own measure.
 */

type Sort = 'recent' | 'name'

/**
 * The order the cards are shown in.
 *
 * `recent` is `updated_at`, **not** "recently opened": nothing in the app records
 * when a workspace was last opened, and a control labelled "opened" over a field
 * that changes on any edit would be a claim about the student's behaviour that no
 * row supports. The label says what the sort does.
 *
 * A row whose timestamp the server did not send sorts last rather than first: a
 * `NaN` comparison is false in both directions, so an unparsed date would otherwise
 * leave the order to the engine's sort implementation.
 */
function sortWorkspaces(rows: Workspace[], sort: Sort): Workspace[] {
  const copy = [...rows]
  if (sort === 'name') {
    return copy.sort((left, right) => left.name.localeCompare(right.name))
  }
  const updatedAt = (workspace: Workspace): number => {
    const at = Date.parse(workspace.updated_at)
    return Number.isNaN(at) ? 0 : at
  }
  return copy.sort(
    (left, right) => updatedAt(right) - updatedAt(left) || left.name.localeCompare(right.name),
  )
}

export function WorkspaceListScreen() {
  const { t } = useTranslation()
  const list = useWorkspaces()
  const create = useCreateWorkspace()
  const [sort, setSort] = useState<Sort>('recent')
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const [courseCode, setCourseCode] = useState('')
  const [examDate, setExamDate] = useState('')
  const formId = useId()

  // The dashboard's `cards_due` is "now" rather than a windowed figure (the server
  // says so), so the window only has to be a real local day. The account's own zone
  // is not read here: the due count is the same instant in any zone, and asking the
  // session for it would put a second request in front of the band.
  const today = localDateKey(new Date(), DEFAULT_TIMEZONE)

  const createError = asApiError(create.error)
  const listError = asApiError(list.error)
  const validation =
    createError?.code === 'validation' ? validationMessage(createError, t, ['name']) : null
  const showNotice = createError !== null && (validation === null || validation.field === null)

  const rows = list.data ?? []
  const ordered = useMemo(() => sortWorkspaces(rows, sort), [rows, sort])
  const sources = rows.reduce((total, workspace) => total + workspace.source_count, 0)
  // Absent, never zero: the band's count line is omitted until the server has
  // answered, and a workspace list the student can see is not held up by it.
  //
  // Asked for only once there is a workspace to describe, because the band is not
  // rendered at all without one: a first-run account should not fetch a summary for
  // a state it cannot draw.
  const dashboard = useDashboard(today, today, { enabled: rows.length > 0 })
  const due = dashboard.data?.totals.cards_due

  return (
    // `w-full` is load-bearing, exactly as it is on `TasksScreen` (Phase 14's fix-round-1
    // measured that one at 493px against a 390 viewport). Without it this div's width is
    // its *fit-content*, which can never go below its content's min-content, so the root
    // grows past the viewport as soon as anything inside it is wider than the phone at
    // 200% text. `w-full` makes the width definite (the container's content box) instead
    // of content-based, which is the property that stops a wide intrinsic child from
    // resizing the screen.
    //
    // **Measured, and it is behaviour-preserving at HEAD:** with and without the class,
    // this screen reports the same four combinations, 44 targets, 80 text pairs, 48 Tab
    // stops and `page 390/390` — the class only changes which of two ways the same width
    // is arrived at. It is here because the *latent* failure is real and was observed:
    // with a peer's in-flight workspace-course fields in the tree, this screen's content
    // pushed the root to **502px inside a 390px viewport** with 25 elements past the right
    // edge and the page scrolling sideways. The class is content-dependent, which is why it
    // is worth the paragraph: a screen that fits today is one field away from not fitting.
    <div className="flex w-full flex-col gap-6 px-5 md:px-[32px] py-8">
      <PageHeader
        eyebrow={t('workspaces.list.eyebrow')}
        title={t('nav.workspaces')}
        subtitle={t('workspaces.list.subtitle')}
        action={
          <button
            type="button"
            // One primary action per screen: while the form is open the *form's*
            // submit button is the primary one, so this becomes the way out of it.
            className={creating ? 'btn btn-secondary w-full sm:w-auto' : 'btn btn-primary w-full sm:w-auto'}
            aria-expanded={creating}
            aria-controls={creating ? formId : undefined}
            onClick={() => {
              setCreating(!creating)
            }}
          >
            <Icon name="plus" size={20} />
            {creating ? t('actions.cancel') : t('workspaces.list.newLabel')}
          </button>
        }
      />

      {creating && (
        <form
          id={formId}
          className="card flex flex-col gap-3 p-4 sm:flex-row sm:flex-wrap sm:items-end"
          onSubmit={(event) => {
            event.preventDefault()
            create.mutate(
              { name, courseCode, examDate },
              {
                onSuccess: () => {
                  setName('')
                  setCourseCode('')
                  setExamDate('')
                  setCreating(false)
                },
              },
            )
          }}
        >
          <div className="min-w-0 flex-1">
            <Field
              label={t('workspaces.list.newLabel')}
              name="name"
              value={name}
              required
              autoFocus
              placeholder={t('workspaces.list.namePlaceholder')}
              hint={t('workspaces.list.nameHint')}
              error={validation?.field === 'name' ? validation.message : undefined}
              onChange={(event) => {
                setName(event.target.value)
              }}
            />
          </div>
          <div className="min-w-0 flex-1">
            <Field
              label={t('workspaces.list.courseCodeLabel')}
              name="course_code"
              value={courseCode}
              // The server caps the code at the column's 16 characters. The cap is
              // repeated here because a longer one comes back as a 422 the form has
              // no field to attach the message to.
              maxLength={16}
              placeholder={t('workspaces.list.courseCodePlaceholder')}
              hint={t('workspaces.list.courseCodeHint')}
              onChange={(event) => {
                setCourseCode(event.target.value)
              }}
            />
          </div>
          <div className="min-w-0 flex-1">
            <Field
              label={t('workspaces.list.examDateLabel')}
              name="exam_date"
              type="date"
              value={examDate}
              hint={t('workspaces.list.examDateHint')}
              onChange={(event) => {
                setExamDate(event.target.value)
              }}
            />
          </div>
          <button type="submit" className="btn btn-primary" disabled={create.isPending}>
            {create.isPending ? t('workspaces.list.creating') : t('workspaces.list.create')}
          </button>
        </form>
      )}

      {showNotice && createError !== null && (
        <ApiErrorNotice error={createError} noun={t('workspaces.noun')} />
      )}

      {list.isPending && (
        <div className="card">
          <SkeletonRows rows={2} label={t('workspaces.list.loading')} />
        </div>
      )}

      {listError !== null && (
        <ApiErrorNotice
          error={listError}
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void list.refetch()}
            >
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {/* The review band renders only once the due count is known. Its one figure is
          the reason it exists, and a band that failed to read it would be a heading
          over nothing — the workspaces below are still complete without it. */}
      {rows.length > 0 && due !== undefined && (
        <div className="raised flex flex-wrap items-center justify-between gap-5 p-6">
          <div className="flex min-w-0 flex-col gap-2">
            <p className="eyebrow eyebrow-primary wrap-anywhere">
              {t('workspaces.list.reviewEyebrow', { count: rows.length })}
            </p>
            <p className="text-h2 wrap-anywhere">
              {due > 0
                ? t('workspaces.list.reviewTitle', { count: due })
                : t('workspaces.list.reviewNone')}
            </p>
            <p className="max-w-prose text-body text-muted wrap-anywhere">{t('workspaces.list.reviewReason')}</p>
          </div>
          {due > 0 && (
            <Link to="/study" className="btn btn-secondary">
              {t('workspaces.list.reviewAction')}
            </Link>
          )}
        </div>
      )}

      {rows.length === 0 && list.data !== undefined && (
        <div className="card flex flex-col items-start gap-6 p-6 sm:flex-row sm:items-center">
          <BrandIllustration className="w-44 max-w-full shrink-0 sm:w-52" />
          {/* `padded={false}`: the card supplies the padding, and the illustration
              sits beside the text, so a second set of padding would push the text
              off the illustration's left edge. */}
          <EmptyState
            padded={false}
            title={t('workspaces.list.emptyTitle')}
            reason={t('workspaces.list.emptyReason')}
            action={
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => {
                  setCreating(true)
                }}
              >
                {t('workspaces.list.newLabel')}
              </button>
            }
          />
        </div>
      )}

      {rows.length > 0 && (
        <Section title={t('workspaces.list.sectionTitle')} count={rows.length}>
          {/* The sort control sits on its own wrapping row rather than in `Section`'s
              `action`, which is a `justify-between` row with no `flex-wrap`: at 200%
              text a `<select>` whose longest option is "Recently updated" is wider
              than the space the h2 leaves, and `Section` is frozen. In a wrapping row
              it drops to its own line instead of pushing the page sideways.

              It is the sort alone, and that is a decision: a filter has to be
              computable from the data the app holds, and there is no per-workspace due
              count anywhere in the API — the only due figure is the account-wide
              `cards_due` the band above reads. A "has cards due" pill would either
              lie or do nothing, and a filter that does nothing is worse than none. */}
          <div className="flex flex-wrap items-center gap-3">
            <label className="eyebrow" htmlFor={`${formId}-sort`}>
              {t('workspaces.list.sortLabel')}
            </label>
            <select
              id={`${formId}-sort`}
              className="field w-auto py-0"
              value={sort}
              onChange={(event) => {
                setSort(event.target.value === 'name' ? 'name' : 'recent')
              }}
            >
              <option value="recent">{t('workspaces.list.sortRecent')}</option>
              <option value="name">{t('workspaces.list.sortName')}</option>
            </select>
          </div>
          {/* `gap-6` = 24px, which is the frame's own pitch. `411:43737 Workspace grid`
              is 1160 wide holding three cells at x=0, 394.667 and 789.333, each
              370.667 wide — so the gap is 394.667 - 370.667 = 24, and
              3 x 370.667 + 2 x 24 = 1160. At `gap-4` the cards were 376 wide, 5.3px
              too wide with the gaps 8px too narrow. The vertical pitch agrees: rows sit
              at y=0 and y=356 against a 332px card, which is the same 24. */}
          <ul className="grid list-none grid-cols-1 gap-6 md:grid-cols-2 xl:grid-cols-3">
            {ordered.map((workspace) => (
              <WorkspaceCard key={workspace.id} workspace={workspace} />
            ))}
          </ul>
        </Section>
      )}

      {rows.length > 0 && (
        <PageFooter
          left={t('workspaces.list.footerCounts', {
            // `count` is what selects `_one`/`_other`: without it i18next looks for the
            // bare key, does not find it, and **renders the key itself** — the footer
            // read `workspaces.list.footerCounts` on screen. The two other figures are
            // the ones the sentence interpolates.
            count: rows.length,
            workspaces: rows.length,
            sources,
          })}
        />
      )}
    </div>
  )
}

function WorkspaceCard({ workspace }: { workspace: Workspace }) {
  const { t } = useTranslation()
  const rename = useRenameWorkspace()
  const remove = useDeleteWorkspace()
  const [mode, setMode] = useState<'idle' | 'renaming' | 'deleting'>('idle')
  const [name, setName] = useState(workspace.name)
  const [courseCode, setCourseCode] = useState(workspace.course_code ?? '')
  const [examDate, setExamDate] = useState(workspace.exam_date ?? '')

  const renameError = asApiError(rename.error)
  const removeError = asApiError(remove.error)
  // A date the API did not send is absent, not empty: `new Date('')` is an Invalid
  // Date and formatting one throws, which would take the whole list down over a
  // field the server is allowed to omit.
  const exam = workspace.exam_date

  return (
    <li
      className="card flex overflow-hidden"
      style={courseVars(COURSE_NAMES[workspace.color] ?? 'indigo')}
    >
      {/* The course hue as a 4px bar. The code and the name are always present, so
          the colour is never the only thing that identifies the course. */}
      <span className="course-marker" aria-hidden="true" />

      <div className="flex min-w-0 flex-1 flex-col gap-3 p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          {/* A real count from the list response, never a placeholder. Sky is the
              source pair, and this is a fact about the workspace's sources. */}
          <span className="chip chip-source">
            {t('workspaces.list.sourceCount', { count: workspace.source_count })}
          </span>
          {mode === 'idle' && (
            // `flex-wrap`: at 200% text `Rename` and `Delete` together are wider than
            // the card's line, and without this the pair overflows the viewport —
            // measured at 336px of content in a 240px row, with `Delete` painted 23px
            // past the right edge. Wrapping stacks them instead.
            <div className="flex flex-wrap gap-1">
              <button
                type="button"
                className="btn btn-quiet"
                onClick={() => {
                  setMode('renaming')
                }}
              >
                {t('workspaces.list.rename')}
              </button>
              <button
                type="button"
                className="btn btn-quiet"
                onClick={() => {
                  setMode('deleting')
                }}
              >
                {t('actions.delete')}
              </button>
            </div>
          )}
        </div>

        {mode === 'renaming' ? (
          <form
            className="flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault()
              rename.mutate(
                { workspaceId: workspace.id, name, courseCode, examDate },
                {
                  onSuccess: () => {
                    setMode('idle')
                  },
                },
              )
            }}
          >
            <Field
              label={t('workspaces.list.renameLabel')}
              value={name}
              required
              autoFocus
              onChange={(event) => {
                setName(event.target.value)
              }}
            />
            <Field
              label={t('workspaces.list.courseCodeLabel')}
              value={courseCode}
              maxLength={16}
              onChange={(event) => {
                setCourseCode(event.target.value)
              }}
            />
            <Field
              label={t('workspaces.list.examDateLabel')}
              type="date"
              value={examDate}
              onChange={(event) => {
                setExamDate(event.target.value)
              }}
            />
            <div className="flex flex-wrap gap-2">
              <button type="submit" className="btn btn-primary" disabled={rename.isPending}>
                {rename.isPending ? t('workspaces.list.saving') : t('actions.save')}
              </button>
              <button
                type="button"
                className="btn btn-quiet"
                onClick={() => {
                  setName(workspace.name)
                  setCourseCode(workspace.course_code ?? '')
                  setExamDate(workspace.exam_date ?? '')
                  setMode('idle')
                  rename.reset()
                }}
              >
                {t('actions.cancel')}
              </button>
            </div>
          </form>
        ) : (
          <div className="flex min-w-0 flex-col gap-2">
            {workspace.course_code && (
              <p className="meta uppercase wrap-anywhere text-primary-text">{workspace.course_code}</p>
            )}
            <h3 className="min-w-0 wrap-anywhere text-h4">
              <Link to={`/workspaces/${workspace.id}`} className="inline-flex min-h-11 items-center hover:underline">
                {workspace.name}
              </Link>
            </h3>
            {/* A link that is a control needs its own target, which is why this is
                `inline-flex min-h-11` and not a bare `.link` (see styles.css). */}
            <Link
              to={`/workspaces/${workspace.id}`}
              className="link inline-flex min-h-11 items-center self-start"
            >
              {t('workspaces.list.open')}
            </Link>
          </div>
        )}

        {mode === 'deleting' && (
          <div className="note note-danger flex flex-col items-start gap-2">
            <p className="font-medium">
              {t('workspaces.list.deleteConfirmTitle', { name: workspace.name })}
            </p>
            <p className="text-small">
              {workspace.source_count === 0
                ? t('workspaces.list.deleteNoSources')
                : t('workspaces.list.deleteSources', { count: workspace.source_count })}
            </p>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                className="btn btn-secondary"
                autoFocus
                disabled={remove.isPending}
                onClick={() => {
                  remove.mutate(workspace.id, {
                    onSuccess: () => {
                      setMode('idle')
                    },
                  })
                }}
              >
                {remove.isPending
                  ? t('workspaces.list.deleting')
                  : t('workspaces.list.deleteConfirm')}
              </button>
              <button
                type="button"
                className="btn btn-quiet"
                onClick={() => {
                  remove.reset()
                  setMode('idle')
                }}
              >
                {t('workspaces.keepIt')}
              </button>
            </div>
          </div>
        )}

        {/* The date the student set, formatted in the reader's own calendar and
            never as a countdown: nothing here knows how many days are left, and a
            countdown that disagreed with the student's own calendar would be
            wrong on exactly the day it mattered. A workspace with no date omits
            the line rather than showing a dash. */}
        {typeof exam === 'string' && exam.length > 0 && mode !== 'renaming' && (
          <p className="meta uppercase wrap-anywhere border-t border-border pt-3">
            {t('workspaces.list.examLine', {
              date: formatInZone(new Date(`${exam}T00:00:00Z`), 'UTC', {
                day: '2-digit',
                month: 'short',
                year: 'numeric',
              }),
            })}
          </p>
        )}

        {renameError !== null && (
          <ApiErrorNotice error={renameError} noun={t('workspaces.noun')} />
        )}
        {removeError !== null && (
          <ApiErrorNotice error={removeError} noun={t('workspaces.noun')} />
        )}
      </div>
    </li>
  )
}
