import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, validationMessage } from '@/api/errors'
import {
  useCreateWorkspace,
  useDeleteWorkspace,
  useRenameWorkspace,
  useWorkspaces,
  type Workspace,
} from '@/api/queries/workspaces'
import { PageHeader, Section } from '@/components/layout/Section'
import { BrandIllustration } from '@/components/ui/BrandIllustration'
import { Field } from '@/components/ui/Field'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'

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
 * Renaming and deleting happen **in the row**, not in a modal. A modal would need
 * focus trapping, a restore target, and an escape route to be correct, and the
 * row already has everything needed to ask the question in place. The delete
 * confirmation names what else goes with it, because deleting a workspace takes
 * its sources.
 */
export function WorkspaceListScreen() {
  const { t } = useTranslation()
  const list = useWorkspaces()
  const create = useCreateWorkspace()
  const [name, setName] = useState('')

  const createError = asApiError(create.error)
  const listError = asApiError(list.error)
  const validation =
    createError?.code === 'validation' ? validationMessage(createError, ['name']) : null
  const showNotice = createError !== null && (validation === null || validation.field === null)

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader
        title={t('nav.workspaces')}
        subtitle={t('workspaces.list.subtitle')}
      />

      <Section title={t('workspaces.list.sectionTitle')} count={list.data?.length}>
        <div className="card">
          <form
            className="flex flex-col gap-3 border-b border-border px-4 py-4 sm:flex-row sm:items-end"
            onSubmit={(event) => {
              event.preventDefault()
              create.mutate(name, {
                onSuccess: () => {
                  setName('')
                },
              })
            }}
          >
            <div className="flex-1">
              <Field
                label={t('workspaces.list.newLabel')}
                name="name"
                value={name}
                required
                placeholder={t('workspaces.list.namePlaceholder')}
                hint={t('workspaces.list.nameHint')}
                error={validation?.field === 'name' ? validation.message : undefined}
                onChange={(event) => {
                  setName(event.target.value)
                }}
              />
            </div>
            <button type="submit" className="btn btn-primary" disabled={create.isPending}>
              {create.isPending ? t('workspaces.list.creating') : t('workspaces.list.create')}
            </button>
          </form>

          {showNotice && createError !== null && (
            <div className="px-4 py-4">
              <ApiErrorNotice error={createError} noun={t('workspaces.noun')} />
            </div>
          )}

          {list.isPending && (
            <SkeletonRows rows={2} label={t('workspaces.list.loading')} />
          )}

          {listError !== null && (
            <div className="px-4 py-4">
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
            </div>
          )}

          {list.data !== undefined && list.data.length === 0 && (
            <div className="flex flex-col items-start gap-6 px-4 py-6 sm:flex-row sm:items-center">
              <BrandIllustration className="w-44 shrink-0 sm:w-52" />
              {/* `padded={false}`: the card supplies the padding, and the
                  illustration sits beside the text, so a second set of padding
                  would push the text off the illustration's left edge. */}
              <EmptyState
                padded={false}
                title={t('workspaces.list.emptyTitle')}
                reason={t('workspaces.list.emptyReason')}
              />
            </div>
          )}

          {list.data !== undefined && list.data.length > 0 && (
            <ul>
              {list.data.map((workspace) => (
                <WorkspaceRow key={workspace.id} workspace={workspace} />
              ))}
            </ul>
          )}
        </div>
      </Section>
    </div>
  )
}

function WorkspaceRow({ workspace }: { workspace: Workspace }) {
  const { t } = useTranslation()
  const rename = useRenameWorkspace()
  const remove = useDeleteWorkspace()
  const [mode, setMode] = useState<'idle' | 'renaming' | 'deleting'>('idle')
  const [name, setName] = useState(workspace.name)

  const renameError = asApiError(rename.error)
  const removeError = asApiError(remove.error)

  return (
    <li className="flex flex-col gap-3 border-b border-border px-4 py-3 last:border-b-0">
      {mode === 'renaming' ? (
        <form
          className="flex flex-col gap-3 sm:flex-row sm:items-end"
          onSubmit={(event) => {
            event.preventDefault()
            rename.mutate(
              { workspaceId: workspace.id, name },
              {
                onSuccess: () => {
                  setMode('idle')
                },
              },
            )
          }}
        >
          <div className="flex-1">
            <Field
              label={t('workspaces.list.renameLabel')}
              value={name}
              required
              autoFocus
              onChange={(event) => {
                setName(event.target.value)
              }}
            />
          </div>
          <div className="flex gap-2">
            <button type="submit" className="btn btn-primary" disabled={rename.isPending}>
              {rename.isPending ? t('workspaces.list.saving') : t('actions.save')}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                setName(workspace.name)
                setMode('idle')
                rename.reset()
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
        </form>
      ) : (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <Link
            to={`/workspaces/${workspace.id}`}
            className="flex min-h-11 min-w-0 flex-1 items-center break-words font-medium hover:underline"
          >
            {workspace.name}
          </Link>
          {/* A real count from the list response, never a placeholder. */}
          <span className="meta">
            {t('workspaces.list.sourceCount', { count: workspace.source_count })}
          </span>
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

      {mode === 'deleting' && (
        <div className="note note-danger flex flex-col items-start gap-2">
          <p className="font-medium">
            {t('workspaces.list.deleteConfirmTitle', { name: workspace.name })}
          </p>
          <p className="text-sm">
            {workspace.source_count === 0
              ? t('workspaces.list.deleteNoSources')
              : t('workspaces.list.deleteSources', { count: workspace.source_count })}
          </p>
          <div className="flex gap-2">
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

      {renameError !== null && (
        <ApiErrorNotice error={renameError} noun={t('workspaces.noun')} />
      )}
      {removeError !== null && (
        <ApiErrorNotice error={removeError} noun={t('workspaces.noun')} />
      )}
    </li>
  )
}
