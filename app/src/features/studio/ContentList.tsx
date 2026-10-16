import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { ApiError } from '@/api/errors'
import { Icon } from '@/components/ui/Icon'
import { BulkToolbar, useReorder, useSelection } from '@/features/studio/ContentManagement'

/**
 * The list half of a Studio panel: the `<ul>` of rows, the Select toggle, the
 * bulk-delete toolbar and the reorder wiring.
 *
 * Every panel lists the same kind of thing, so this behaviour lives here once. A
 * panel supplies its items and a render function for its own row; a second copy
 * per content type is how one panel's reorder ends up without the debounce or its
 * bulk delete without the confirm step.
 *
 * The reorder arrows only appear when `onReorder` is given: a list that cannot
 * reorder should not offer arrows that do nothing.
 */

/** What a row needs from the list to participate in selection and reorder. */
export interface RowBindings {
  selectMode: boolean
  selected: boolean
  onToggleSelect: () => void
  onMoveUp: () => void
  onMoveDown: () => void
  canMoveUp: boolean
  canMoveDown: boolean
}

export interface ContentListProps<T extends { id: string }> {
  items: T[]
  /** The `t()` noun for a limit or validation notice: "decks", "quizzes". */
  noun: string
  /** Renders one row. `bindings` carries this row's selection and reorder props. */
  row: (item: T, index: number, bindings: RowBindings) => ReactNode
  onBulkDelete: (ids: string[]) => void
  bulkPending: boolean
  bulkError: ApiError | null
  /** Given a full order, persists it. Absent means no reorder arrows. */
  onReorder?: (ids: string[]) => Promise<unknown>
  /** Rendered after the rows but before the toolbar, e.g. the cheatsheet form. */
  children?: ReactNode
}

export function ContentList<T extends { id: string }>({
  items,
  noun,
  row,
  onBulkDelete,
  bulkPending,
  bulkError,
  onReorder,
  children,
}: ContentListProps<T>) {
  const { t } = useTranslation()
  const selection = useSelection()
  const ids = items.map((item) => item.id)
  const reorder = useReorder(ids, onReorder)

  // The rows render in the local order while a move is in flight. Every id the
  // server sent is present, so a lookup by id keeps each item's own data with it.
  const ordered = reorder.order
    .map((id) => items.find((item) => item.id === id))
    .filter((item): item is T => item !== undefined)

  const selectedIds = ordered
    .map((item) => item.id)
    .filter((id) => selection.selected.has(id))

  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className="btn btn-quiet text-sm"
          aria-pressed={selection.selectMode}
          onClick={() => {
            if (selection.selectMode) selection.exit()
            else selection.enter()
          }}
        >
          <Icon name="check" size={16} />
          {t('studio.bulk.select')}
        </button>
      </div>

      <ul className="flex flex-col divide-y divide-border border-y border-border">
        {ordered.map((item, index) =>
          row(item, index, {
            selectMode: selection.selectMode,
            selected: selection.selected.has(item.id),
            onToggleSelect: () => {
              selection.toggle(item.id)
            },
            onMoveUp: () => {
              reorder.move(index, -1)
            },
            onMoveDown: () => {
              reorder.move(index, 1)
            },
            canMoveUp: index > 0,
            canMoveDown: index < ordered.length - 1,
          }),
        )}
      </ul>

      {children}

      {selection.selectMode && selectedIds.length > 0 && (
        <BulkToolbar
          count={selectedIds.length}
          pending={bulkPending}
          error={bulkError}
          noun={noun}
          onConfirm={() => {
            onBulkDelete(selectedIds)
          }}
          onDeselectAll={selection.clear}
        />
      )}
    </>
  )
}
