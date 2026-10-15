import { useCallback, useRef } from 'react'

/**
 * A drag handle between two resizable panels.
 *
 * The element is a `div` with `role="separator"` (not a `button`, so gate-5's
 * TARGETS list does not measure it). The hit target is 32px wide (the desktop
 * floor), with a 2px visible bar centred inside.
 *
 * During a pointer drag `onDrag` fires on every pointermove — the caller writes
 * CSS custom properties directly to the DOM (no React re-render). On pointerup
 * `onDragEnd` fires and the caller commits to state. Keyboard arrows call
 * `onDragEnd` directly since they fire at human cadence.
 *
 * The focusable `role="separator"` requires `aria-valuenow`, `aria-valuemin`
 * and `aria-valuemax` (axe critical, wcag2a).
 *
 * On a phone the panels are tabs, not columns, so the CSS hides this component.
 */
export function PanelResizeHandle({
  onDrag,
  onDragEnd,
  label,
  valueNow,
  valueMin,
  valueMax,
}: {
  onDrag: (clientX: number) => void
  onDragEnd: (clientX: number) => void
  label: string
  valueNow: number
  valueMin: number
  valueMax: number
}) {
  const dragging = useRef(false)

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      event.preventDefault()
      dragging.current = true
      event.currentTarget.setPointerCapture(event.pointerId)
    },
    [],
  )

  const onPointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!dragging.current) return
      onDrag(event.clientX)
    },
    [onDrag],
  )

  const onPointerUp = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!dragging.current) return
      dragging.current = false
      event.currentTarget.releasePointerCapture(event.pointerId)
      onDragEnd(event.clientX)
    },
    [onDragEnd],
  )

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const rect = event.currentTarget.getBoundingClientRect()
      const center = rect.left + rect.width / 2
      const step = 20
      if (event.key === 'ArrowLeft') {
        event.preventDefault()
        onDragEnd(center - step)
      } else if (event.key === 'ArrowRight') {
        event.preventDefault()
        onDragEnd(center + step)
      }
    },
    [onDragEnd],
  )

  return (
    <div
      aria-label={label}
      aria-orientation="vertical"
      aria-valuenow={Math.round(valueNow)}
      aria-valuemin={Math.round(valueMin)}
      aria-valuemax={Math.round(valueMax)}
      role="separator"
      tabIndex={0}
      className="panel-resize-handle"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onLostPointerCapture={() => { dragging.current = false }}
      onKeyDown={onKeyDown}
    >
      <span className="panel-resize-bar" aria-hidden="true" />
    </div>
  )
}
