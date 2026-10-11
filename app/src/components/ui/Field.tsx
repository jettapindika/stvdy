import { useId } from 'react'
import type { InputHTMLAttributes } from 'react'

/**
 * A labelled text input, with its error and its hint.
 *
 * The caption is a real `<label for>` rather than a wrapping label. That matters
 * once a field has a hint or an error: a wrapping label's accessible name is its
 * whole text content, so the description would be read out as part of the field's
 * name ("New workspace The course or subject this material belongs to"). Here the
 * caption is the name, and the hint and the error are attached with
 * `aria-describedby`, which is what they are.
 *
 * This is the field pattern the styleguide documents, with the three details that
 * get forgotten by hand wired in one place:
 *
 * - the error is **text**, not a red border, so the state is not carried by
 *   colour alone and a screen reader announces it;
 * - `aria-invalid` marks the field itself, which is what assistive technology
 *   reports on entering it;
 * - the description is announced with the field rather than only seen beside it.
 */

export function Field({
  label,
  error,
  hint,
  ...input
}: {
  label: string
  /** Shown under the field, and announced as part of its description. */
  error?: string
  hint?: string
} & InputHTMLAttributes<HTMLInputElement>) {
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
      <input
        id={id}
        className="field"
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy.length > 0 ? describedBy.join(' ') : undefined}
        {...input}
      />
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
