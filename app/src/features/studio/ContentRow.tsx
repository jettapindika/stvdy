import { useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { ApiError } from '@/api/errors'
import { Icon } from '@/components/ui/Icon'
import { ApiErrorNotice } from '@/components/ui/States'
import type { RowBindings } from '@/features/studio/ContentList'

/**
 * One row of content in a Studio panel — a deck, a quiz, a report or a cheatsheet.
 *
 * The four kinds differ only in what they render (a count, a status word, a link,
 * a cascade sentence) and in which mutations they call, so the row's behaviour
 * lives here once: the `idle | renaming | deleting` mode machine, the bulk
 * selection checkbox, the reorder arrows and the inline rename form. Each panel
 * supplies its own copy and callbacks; a second copy of this logic per content
 * type is how one of them ends up without an `autoFocus` on its destructive
 * button.
 *
 * Mutations arrive as callbacks rather than as `useMutation` results, so this
 * component never has to know a deck id from a quiz id — the panel closes over
 * its own item.
 */

type Mode = 'idle' | 'renaming' | 'deleting'

export interface ContentRowProps {
  /** The row's title, with the panel's own empty-title fallback already applied. */
  title: string
  /** Secondary text after the title: "12 cards", "5 questions", "2 of 5 written". */
  meta?: ReactNode
  /** The chip's class, from the panel's own status map. */
  statusChip: string
  /** The status word, so the chip reads in monochrome and to a screen reader. */
  statusLabel: string
  /** Whether the chip carries the animated dot (generating, or pending). */
  working: boolean
  /** The primary action link — "Study", "Take quiz", "Open report". */
  action?: ReactNode
  /**
   * The row's own error, e.g. a failed generation.
   */
  error?: string | null

  /**
   * Inline rename, when the kind supports it. Absent means the row offers delete
   * only — a report renames through its outline editor on its own screen, where
   * the title and the outline are edited together.
   */
  rename?: {
    /** The button's text, e.g. `studio.deck.rename`. */
    label: string
    /** Receives the form's trimmed, non-empty value. */
    onSubmit: (title: string) => void
    pending: boolean
    error: ApiError | null
    reset: () => void
  }

  /** The confirm block's copy, from the panel's own i18n group. */
  deleteConfirm: string
  deleteBody: string
  deleteIt: string
  deleting: string
  /** The domain noun `ApiErrorNotice` names in a limit or validation message. */
  deleteNoun: string

  /** Runs the delete; `onSuccess` returns the row to `idle`. */
  onDelete: (onSuccess: () => void) => void
  deletePending: boolean
  deleteError: ApiError | null
  resetDelete: () => void

  /** Selection and reorder, from the list that owns them. */
  bindings: RowBindings
}

export function ContentRow(props: ContentRowProps) {
  const { t } = useTranslation()
  const [mode, setMode] = useState<Mode>('idle')
  const [draft, setDraft] = useState(props.title)
  // Hoisted so the closures below keep the non-null narrowing: `props.rename` is
  // read inside callbacks, where a property access would be re-widened.
  const rename = props.rename

  const idle = mode === 'idle'
  const renaming = mode === 'renaming'
  const deleting = mode === 'deleting'
  // The reorder arrows are meaningless while the row is showing a checkbox or a
  // form: the row's order is not what the reader is looking at.
  const chrome = idle && !props.bindings.selectMode

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {props.bindings.selectMode && (
          <input
            type="checkbox"
            checked={props.bindings.selected}
            aria-label={props.title}
            onChange={props.bindings.onToggleSelect}
          />
        )}

        {renaming && rename !== undefined && (
          <form
            className="flex w-full min-w-0 flex-wrap items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault()
              const trimmed = draft.trim()
              if (trimmed.length === 0 || rename === undefined || rename.pending) return
              rename.onSubmit(trimmed)
              // Leave rename mode as soon as the request is accepted; the panel's
              // `onSuccess` invalidates the list, so the row re-renders from the
              // server's copy rather than from this draft.
              setMode('idle')
            }}
          >
            <input
              className="field min-w-0 flex-1"
              value={draft}
              autoFocus
              aria-label={t('studio.row.renameLabel')}
              onChange={(event) => {
                setDraft(event.target.value)
              }}
            />
            <button
              type="submit"
              className="btn btn-primary text-sm"
              disabled={rename.pending}
            >
              {t('actions.save')}
            </button>
            <button
              type="button"
              className="btn btn-quiet text-sm"
              onClick={() => {
                rename.reset()
                setDraft(props.title)
                setMode('idle')
              }}
            >
              {t('actions.cancel')}
            </button>
          </form>
        )}

        {!renaming && (
          <>
            {/* `w-full … @md:w-auto @md:flex-1` is load-bearing, not decoration.
                The row's other children are all effectively fixed-width (a count,
                a status chip, a link), while `flex-1` gives this span a flex basis
                of `0` — and with `overflow-wrap: anywhere` its min-content is one
                character, so nothing stops the row from squeezing the title there.
                Measured in Chromium on this screen's own fixture, a deck title
                rendered 24px wide and 308px tall at 100% text. The condition is the
                *panel's* width, not the window's, so `@container` on the panel
                content box plus `@md:` (28rem of container) asks the question that
                matters, and because container breakpoints are rem-based the
                threshold doubles with the reader's text size. */}
            <span className="w-full min-w-0 wrap-anywhere font-medium @md:w-auto @md:flex-1">
              {props.title}
            </span>
            {props.meta}
            <span className={props.statusChip} role="status">
              {props.working && (
                <span
                  className="h-2 w-2 animate-working rounded-full bg-current"
                  aria-hidden="true"
                />
              )}
              {props.statusLabel}
            </span>
            {props.action}
          </>
        )}

        {chrome && (
          <div className="flex flex-wrap items-center gap-1">
            <button
              type="button"
              className="btn btn-quiet p-1"
              disabled={!props.bindings.canMoveUp}
              aria-label={t('studio.row.moveUp')}
              onClick={props.bindings.onMoveUp}
            >
              <Icon name="chevron-up" />
            </button>
            <button
              type="button"
              className="btn btn-quiet p-1"
              disabled={!props.bindings.canMoveDown}
              aria-label={t('studio.row.moveDown')}
              onClick={props.bindings.onMoveDown}
            >
              <Icon name="chevron-down" />
            </button>
            {rename !== undefined && (
              <button
                type="button"
                className="btn btn-quiet text-sm"
                onClick={() => {
                  setDraft(props.title)
                  setMode('renaming')
                }}
              >
                {rename.label}
              </button>
            )}
            <button
              type="button"
              className="btn btn-quiet text-danger text-sm"
              onClick={() => {
                setMode('deleting')
              }}
            >
              {t('actions.delete')}
            </button>
          </div>
        )}
      </div>

      {deleting && (
        <div className="note note-danger flex flex-col items-start gap-2">
          <p className="font-medium">{props.deleteConfirm}</p>
          <p className="text-small">{props.deleteBody}</p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-secondary"
              autoFocus
              disabled={props.deletePending}
              onClick={() => {
                props.onDelete(() => {
                  setMode('idle')
                })
              }}
            >
              {props.deletePending ? props.deleting : props.deleteIt}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                props.resetDelete()
                setMode('idle')
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
          {props.deleteError !== null && (
            <ApiErrorNotice error={props.deleteError} noun={props.deleteNoun} />
          )}
        </div>
      )}

      {rename !== undefined && rename.error !== null && (
        <ApiErrorNotice error={rename.error} />
      )}

      {props.error !== null && props.error !== undefined && props.error.length > 0 && (
        <p className="note note-danger text-sm">{props.error}</p>
      )}
    </li>
  )
}
