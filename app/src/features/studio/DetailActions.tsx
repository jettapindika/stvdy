import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import type { ApiError } from '@/api/errors'
import { ApiErrorNotice } from '@/components/ui/States'

/**
 * Rename and delete for a detail screen's header.
 *
 * A detail screen's title lives in `PageHeader`, so the rename form is rendered by
 * the screen in the title slot while these are the two buttons that sit in the
 * header's action slot. Keeping them here means the confirm copy, the `autoFocus`
 * on the destructive button and the pending labels are the same ones the list
 * rows use, rather than a second implementation per screen.
 */

/**
 * The title, replaced by an input while the reader is renaming.
 *
 * Escape cancels and Enter submits, which is what the codebase's other inline
 * forms do — a form that only submits on a button press is a form a keyboard user
 * has to hunt through.
 */
export function InlineTitleForm({
  defaultValue,
  label,
  pending,
  onSubmit,
  onCancel,
}: {
  defaultValue: string
  /** The input's accessible name, e.g. "New name". */
  label: string
  pending: boolean
  onSubmit: (title: string) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const [value, setValue] = useState(defaultValue)

  return (
    <form
      className="flex min-w-0 flex-wrap items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault()
        const trimmed = value.trim()
        if (trimmed.length === 0 || pending) return
        onSubmit(trimmed)
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          onCancel()
        }
      }}
    >
      <input
        className="field min-w-0 flex-1"
        value={value}
        autoFocus
        aria-label={label}
        onChange={(event) => {
          setValue(event.target.value)
        }}
      />
      <button type="submit" className="btn btn-primary text-sm" disabled={pending}>
        {t('actions.save')}
      </button>
      <button type="button" className="btn btn-quiet text-sm" onClick={onCancel}>
        {t('actions.cancel')}
      </button>
    </form>
  )
}

/**
 * The delete control in a detail screen's header: a trigger, then an inline
 * confirm. `onDeleted` runs after the tombstone lands — the screen navigates away
 * from a document that no longer exists.
 */
export function DetailDelete({
  confirm,
  body,
  deleteIt,
  deleting,
  noun,
  error,
  pending,
  reset,
  onDelete,
  onDeleted,
}: {
  confirm: string
  body: string
  deleteIt: string
  deleting: string
  noun: string
  error: ApiError | null
  pending: boolean
  reset: () => void
  onDelete: (onSuccess: () => void) => void
  onDeleted: () => void
}) {
  const { t } = useTranslation()
  const [confirming, setConfirming] = useState(false)

  return (
    <div className="flex flex-col gap-2">
      {!confirming && (
        <button
          type="button"
          className="btn btn-quiet text-danger text-sm"
          onClick={() => {
            setConfirming(true)
          }}
        >
          {t('actions.delete')}
        </button>
      )}

      {confirming && (
        <div className="note note-danger flex flex-col items-start gap-2">
          <p className="font-medium">{confirm}</p>
          <p className="text-small">{body}</p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-secondary"
              autoFocus
              disabled={pending}
              onClick={() => {
                onDelete(onDeleted)
              }}
            >
              {pending ? deleting : deleteIt}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                reset()
                setConfirming(false)
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
        </div>
      )}

      {error !== null && <ApiErrorNotice error={error} noun={noun} />}
    </div>
  )
}
