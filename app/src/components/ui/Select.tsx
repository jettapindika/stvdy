import { useId } from 'react'
import type { ReactNode, SelectHTMLAttributes } from 'react'

/**
 * A labelled `<select>`, with its error and its hint.
 *
 * Mirror of `Field`, built for `<select>` instead of `<input>`. The caption is
 * a real `<label for>` rather than a wrapping label: the hint and the error are
 * attached with `aria-describedby`, and `aria-invalid` marks the select itself,
 * so assistive technology announces the state on entry.
 *
 * The chevron comes from `select.field` in `styles.css`: `appearance: none`
 * removes the browser chrome, and `--select-chevron` (a per-theme SVG whose
 * stroke matches `--text-muted`) replaces it. Explicit theme overrides in
 * `appearance.ts` set the same variable, so the chevron tracks every path.
 */

export function Select({
  label,
  error,
  hint,
  children,
  className,
  ...select
}: {
  label: string
  /** Shown under the select, and announced as part of its description. */
  error?: string
  hint?: string
  children: ReactNode
} & Omit<SelectHTMLAttributes<HTMLSelectElement>, 'children'>) {
  const id = useId()
  const errorId = useId()
  const hintId = useId()
  const describedBy = [error ? errorId : null, hint ? hintId : null].filter(
    (value): value is string => value !== null,
  )

  return (
    <div className="flex flex-col gap-1">
      <label className="label" htmlFor={id}>
        {label}
      </label>
      <select
        id={id}
        className={className ? `field ${className}` : 'field'}
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy.length > 0 ? describedBy.join(' ') : undefined}
        {...select}
      >
        {children}
      </select>
      {hint && (
        <span id={hintId} className="text-xs text-muted">
          {hint}
        </span>
      )}
      {error && (
        <span id={errorId} className="text-sm text-danger">
          {error}
        </span>
      )}
    </div>
  )
}
