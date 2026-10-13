import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { uploadSyllabus } from '@/api/queries/syllabus'
import { useUsage } from '@/api/queries/usage'
import { useWorkspaces } from '@/api/queries/workspaces'
import { PageHeader, Section } from '@/components/layout/Section'
import { EmptyState, LoadingState, ApiErrorNotice } from '@/components/ui/States'
import { PLANNER_CREDITS } from '@/features/billing/plans'

/**
 * The syllabus upload, and the door to the review screen.
 *
 * **The cost is on the button, before the action runs** (honesty gate 7(d)), and it
 * is the matrix's own number from `PLANNER_CREDITS` — never a literal. The eight
 * credits are charged by the route, and the extraction is a whole-document model
 * call, so this is the one place the student is told the price of a syllabus.
 *
 * **There is no locked card here, and that is the matrix's shape rather than an
 * omission.** `Feature::SyllabusImport` is granted by every plan: free is capped by
 * *count* (`syllabus_imports_total = 1`) and refuses the second one with 422
 * `limit_reached`, which `ApiErrorNotice` already renders as the plan-limit note
 * with the count. A `LockedFeature` naming a plan would be wrong twice — the free
 * plan does grant it, and no upgrade would change the answer for a pro account that
 * had simply run out.
 *
 * **The upload writes nothing but a draft.** The route answers 202 with a draft id;
 * the extraction rides the ingest job (A250) and lands in `syllabus_drafts`; and the
 * review screen is where the student reads it and decides. That is why this panel
 * links to the review screen rather than rendering items itself.
 */
export function SyllabusPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const usage = useUsage()
  const [text, setText] = useState('')
  const [title, setTitle] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)
  const [draftId, setDraftId] = useState<string | null>(null)
  const file = useRef<HTMLInputElement>(null)

  const cost = PLANNER_CREDITS.syllabusImport
  const allowance = usage.data?.limits.syllabus_imports_total

  const upload = async () => {
    setError(null)
    setBusy(true)
    try {
      const chosen = file.current?.files?.[0]
      const body = await uploadSyllabus(workspaceId, {
        file: chosen,
        text: chosen === undefined ? text : undefined,
        title: title === '' ? undefined : title,
      })
      setDraftId(body.draft_id)
    } catch (thrown) {
      setError(asApiError(thrown))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Section title={t('syllabus.upload.title')}>
      <div className="card flex flex-col gap-3 p-4">
        <p className="text-small text-muted wrap-anywhere">{t('syllabus.upload.hint')}</p>

        {/* The count against the plan's ceiling, when the plan has one. `null` is
            "unlimited" rather than zero, which is the shape `GET /usage` uses. */}
        {allowance !== undefined && allowance.limit !== null ? (
          <p className="meta">
            {t('syllabus.upload.allowance', {
              used: allowance.used ?? 0,
              limit: allowance.limit,
            })}
          </p>
        ) : null}

        <div className="flex flex-col gap-1">
          <label className="label" htmlFor="syllabus-file">
            {t('syllabus.upload.fileLabel')}
          </label>
          <input
            ref={file}
            id="syllabus-file"
            className="field"
            type="file"
            accept="application/pdf,text/plain"
          />
        </div>

        <div className="flex flex-col gap-1">
          <label className="label" htmlFor="syllabus-text">
            {t('syllabus.upload.textLabel')}
          </label>
          <textarea
            id="syllabus-text"
            className="field"
            rows={5}
            value={text}
            onChange={(event) => {
              setText(event.target.value)
            }}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label className="label" htmlFor="syllabus-title">
            {t('syllabus.upload.nameLabel')}
          </label>
          <input
            id="syllabus-title"
            className="field"
            type="text"
            value={title}
            onChange={(event) => {
              setTitle(event.target.value)
            }}
          />
        </div>

        <button
          type="button"
          className="btn btn-primary self-start"
          disabled={busy}
          onClick={() => void upload()}
        >
          {cost === null
            ? t('syllabus.upload.action')
            : t('syllabus.upload.actionWithCost', { count: cost })}
        </button>

        {error !== null ? (
          <ApiErrorNotice error={error} noun={t('syllabus.noun')} action={t('actions.retry')} />
        ) : null}

        {draftId !== null ? (
          <Link
            to={`/syllabus/${draftId}`}
            className="link inline-flex min-h-11 items-center self-start"
          >
            {t('syllabus.upload.review')}
          </Link>
        ) : null}
      </div>
    </Section>
  )
}

/**
 * `/syllabus`: choose a workspace, then upload.
 *
 * The upload route is `/workspaces/:id/syllabus`, because a source belongs to a
 * workspace and the syllabus becomes one (`sources.kind='syllabus'`). So the screen
 * asks which workspace first rather than inventing a default: a source filed in the
 * wrong course is a goal whose topics come from the wrong material, and the
 * student's own choice is the only honest answer to "which course is this".
 *
 * A workspace with no sources yet is a legal state and the panel is still shown —
 * the syllabus *becomes* the source, so requiring one first would be a rule the
 * server does not have.
 */
export function SyllabusScreen() {
  const { t } = useTranslation()
  const workspaces = useWorkspaces()
  const rows = workspaces.data ?? []

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-8 px-5 md:px-[32px] py-8">
      <PageHeader title={t('syllabus.title')} subtitle={t('syllabus.subtitle')} />

      {workspaces.isPending ? <LoadingState label={t('syllabus.loading')} /> : null}

      {!workspaces.isPending && rows.length === 0 ? (
        <EmptyState
          title={t('syllabus.noWorkspace.title')}
          reason={t('syllabus.noWorkspace.reason')}
          action={
            <Link to="/workspaces" className="link inline-flex min-h-11 items-center">
              {t('syllabus.noWorkspace.action')}
            </Link>
          }
        />
      ) : null}

      {rows.map((workspace) => (
        <SyllabusPanel key={workspace.id} workspaceId={workspace.id} />
      ))}
    </div>
  )
}
