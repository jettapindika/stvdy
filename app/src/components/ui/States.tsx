import type { ReactNode } from 'react'

/**
 * Empty, loading, and error states that say something useful (R-27).
 *
 * The rule these enforce is that a state names its *cause* and the *next action*.
 * "No data" tells the user neither, so it is not allowed here: every state takes
 * a required `reason`, and an optional `action`. Making `reason` required is the
 * mechanism, not a convention: a caller cannot ship a blank state by forgetting.
 *
 * This matters more than usual for this product. A student who sees an empty
 * screen needs to know whether it is empty because they have not added material
 * yet, because a filter excluded everything, or because something failed. Those
 * are three different situations with three different next steps, and they must
 * not look the same.
 */

export function EmptyState({
  title,
  reason,
  action,
}: {
  title: string
  /** Why it is empty. Required, so no state can ship without a cause. */
  reason: string
  /** The one thing that fills it. */
  action?: ReactNode
}) {
  return (
    <div className="flex flex-col items-start gap-2 px-4 py-6">
      <p className="font-medium text-ink">{title}</p>
      <p className="max-w-prose text-sm text-ink-muted">{reason}</p>
      {action && <div className="mt-1">{action}</div>}
    </div>
  )
}

export function ErrorState({
  title,
  reason,
  action,
}: {
  title: string
  /** What failed, in plain words. */
  reason: string
  /** What to do about it. */
  action?: ReactNode
}) {
  return (
    <div className="note note-danger flex flex-col items-start gap-2" role="alert">
      <p className="font-medium">{title}</p>
      <p className="text-sm">{reason}</p>
      {action && <div>{action}</div>}
    </div>
  )
}

export function LoadingState({
  /** What is loading, named. "Loading" alone does not say what. */
  label,
}: {
  label: string
}) {
  return (
    <div className="flex items-center gap-2 px-4 py-6" role="status" aria-live="polite">
      <span className="h-2 w-2 animate-working rounded-full bg-ink-muted" aria-hidden="true" />
      <span className="text-sm text-ink-muted">{label}</span>
    </div>
  )
}

/**
 * A skeleton stands in for content whose shape is known (a list of rows, a
 * paragraph). It is not used as a decorative "product shot": a skeleton that
 * mimics a layout the real data never fills is a lie about what is coming, and
 * R-17/R-38 forbid presenting invented shape as real.
 */
export function SkeletonRows({ rows = 3, label }: { rows?: number; label: string }) {
  return (
    <div className="flex flex-col gap-3 px-4 py-4" role="status" aria-live="polite">
      <span className="sr-only">{label}</span>
      {Array.from({ length: rows }, (_, index) => (
        <div key={index} className="flex flex-col gap-1.5" aria-hidden="true">
          <div className="h-3 w-2/5 rounded-sm bg-surface-sunk" />
          <div className="h-3 w-4/5 rounded-sm bg-surface-sunk" />
        </div>
      ))}
    </div>
  )
}

/**
 * For a screen that is planned but not built. Says which phase builds it and what
 * it will contain, rather than pretending to be a working screen (R-24, R-26).
 */
export function NotBuiltYet({ title, what }: { title: string; what: string }) {
  return (
    <section className="mx-auto flex max-w-2xl flex-col gap-3 px-5 py-10">
      <h1>{title}</h1>
      <div className="note">
        <p className="text-sm text-ink-muted">
          Not built yet. This screen will hold {what}.
        </p>
      </div>
    </section>
  )
}
