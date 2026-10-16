import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import type { ApiError } from '@/api/errors'
import { ApiErrorNotice } from '@/components/ui/States'

/**
 * The Studio panels' bulk-selection state.
 *
 * Lifted to the panel rather than kept per row, because "3 selected" and "Delete
 * all" are panel-level facts and a row cannot know what its siblings have
 * checked. The set is a `Set` and not an array so a checkbox toggles in O(1)
 * without an `includes` scan per render.
 */
export interface Selection {
  selectMode: boolean
  selected: Set<string>
  toggle: (id: string) => void
  clear: () => void
  exit: () => void
  enter: () => void
}

export function useSelection(): Selection {
  const [selectMode, setSelectMode] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(() => new Set())

  const toggle = useCallback((id: string) => {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  const clear = useCallback(() => {
    setSelected(new Set())
  }, [])

  /** Leaving select mode discards the selection: a stale checkmark that deletes
   * something is worse than making the reader tick it again. */
  const exit = useCallback(() => {
    setSelectMode(false)
    setSelected(new Set())
  }, [])

  const enter = useCallback(() => {
    setSelectMode(true)
  }, [])

  return { selectMode, selected, toggle, clear, exit, enter }
}

/**
 * The sticky bulk-delete toolbar a panel shows while anything is selected.
 *
 * Two states, not one: the first click on "Delete" swaps the toolbar for the
 * confirm block, so the destructive action is never the one under the pointer
 * when the toolbar appears. The confirm names the count, because "3 items" is the
 * only thing that makes the reader check what is ticked.
 */
export function BulkToolbar({
  count,
  pending,
  error,
  noun,
  onConfirm,
  onDeselectAll,
}: {
  count: number
  pending: boolean
  error: ApiError | null
  noun: string
  onConfirm: () => void
  onDeselectAll: () => void
}) {
  const { t } = useTranslation()
  const [confirming, setConfirming] = useState(false)

  // A selection change invalidates a confirmation that was about a different
  // set: the reader ticked another row, so the count in the block would be a lie.
  useEffect(() => {
    setConfirming(false)
  }, [count])

  return (
    <div className="sticky bottom-0 flex flex-col gap-2 border-t border-border bg-surface px-4 py-3">
      {confirming ? (
        <>
          <p className="font-medium">{t('studio.bulk.deleteConfirm', { count })}</p>
          <p className="text-small">{t('studio.bulk.deleteBody')}</p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-danger text-sm"
              autoFocus
              disabled={pending}
              onClick={onConfirm}
            >
              {pending ? t('studio.bulk.deleting') : t('studio.bulk.deleteIt')}
            </button>
            <button
              type="button"
              className="btn btn-quiet text-sm"
              onClick={() => {
                setConfirming(false)
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
        </>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <span className="meta">{t('studio.bulk.selected', { count })}</span>
          <button
            type="button"
            className="btn btn-danger text-sm"
            onClick={() => {
              setConfirming(true)
            }}
          >
            {t('actions.delete')}
          </button>
          <button type="button" className="btn btn-quiet text-sm" onClick={onDeselectAll}>
            {t('studio.bulk.deselectAll')}
          </button>
        </div>
      )}

      {error !== null && <ApiErrorNotice error={error} noun={noun} />}
    </div>
  )
}

/**
 * Arrow-button reordering, debounced.
 *
 * A move swaps two entries in a local copy and schedules one `PATCH` 300ms later,
 * so a reader who taps "up" three times sends one request describing the final
 * order rather than three that each describe an intermediate one. The local copy
 * is what the rows render, so the swap is immediate and does not wait on a
 * refetch — the codebase's `OutlineEditor` reorders the same way (local draft,
 * buttons, no drag library).
 *
 * `reorder` returns the panel's mutation promise, so `pending` clears when the
 * request settles rather than after a guessed delay, and a second move cannot
 * start while the first is still in flight — otherwise two overlapping orders
 * would land in whichever order the network chose.
 */
export interface Reorder {
  order: string[]
  move: (index: number, by: number) => void
  pending: boolean
}

export function useReorder(
  ids: string[],
  reorder: ((ids: string[]) => Promise<unknown>) | undefined,
): Reorder {
  const [order, setOrder] = useState<string[]>(ids)
  const [pending, setPending] = useState(false)
  const timer = useRef<number | undefined>(undefined)
  const signature = ids.join(',')

  // Re-seed from the server whenever its order changes, but not while a reorder
  // of our own is in flight: the refetch we are waiting for still carries the
  // pre-swap order and would clobber the swap the reader just made.
  const inFlight = useRef(false)
  const seeded = useRef(signature)
  useEffect(() => {
    if (inFlight.current) return
    if (seeded.current === signature) return
    seeded.current = signature
    setOrder(ids)
  }, [signature, ids])

  useEffect(
    () => () => {
      window.clearTimeout(timer.current)
    },
    [],
  )

  const move = useCallback(
    (index: number, by: number) => {
      if (reorder === undefined) return
      setOrder((current) => {
        const next = [...current]
        const target = index + by
        const moved = next[index]
        const displaced = next[target]
        if (moved === undefined || displaced === undefined) return current
        next[index] = displaced
        next[target] = moved

        window.clearTimeout(timer.current)
        timer.current = window.setTimeout(() => {
          inFlight.current = true
          setPending(true)
          const settled = next.join(',')
          void reorder(next).finally(() => {
            inFlight.current = false
            setPending(false)
            seeded.current = settled
          })
        }, 300)

        return next
      })
    },
    [reorder],
  )

  return { order, move, pending }
}
