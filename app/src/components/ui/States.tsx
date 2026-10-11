import type { ReactNode } from 'react'
import { Link } from 'react-router'

import {
  apiErrorReason,
  apiErrorTitle,
  planLimitMessage,
  type ApiError,
} from '@/api/errors'
import { platform } from '@/platform'

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
  padded = true,
}: {
  title: string
  /** Why it is empty. Required, so no state can ship without a cause. */
  reason: string
  /** The one thing that fills it. */
  action?: ReactNode
  /**
   * Whether to supply its own padding. The default suits a bare `.panel`, which
   * has none. Set `false` when the caller already pads the container, or the two
   * paddings stack and the text no longer lines up with a sibling element.
   */
  padded?: boolean
}) {
  return (
    <div className={`flex flex-col items-start gap-2 ${padded ? 'px-4 py-6' : ''}`}>
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
 *
 * `level` exists because this is also used inside a panel, where the page already
 * has an `h1` and a second one would flatten the document outline that a screen
 * reader user navigates by.
 */
export function NotBuiltYet({
  title,
  what,
  level = 1,
}: {
  title: string
  what: string
  level?: 1 | 2 | 3
}) {
  return (
    <section className="mx-auto flex max-w-2xl flex-col gap-3 px-5 py-10">
      {level === 1 ? <h1>{title}</h1> : level === 2 ? <h2>{title}</h2> : <h3>{title}</h3>}
      <div className="note">
        <p className="text-sm text-ink-muted">
          Not built yet. This screen will hold {what}.
        </p>
      </div>
    </section>
  )
}

/**
 * Any failure that came back from the API, rendered.
 *
 * One component rather than a reason string per call site, because the codes are
 * a closed set and the copy for each one is a decision that should be made once.
 * The server's `message` is not shown: it is written for logs, and a user cannot
 * act on "limit_reached". What is shown is what happened, in this product's
 * words, plus the numbers when the server sent them.
 *
 * An entitlement refusal gets a different treatment from a failure, because it is
 * not a failure: nothing is broken, the account is simply at its ceiling, and the
 * prompt says which ceiling, by how much, and what lifts it. That is the "inline
 * upgrade prompt exactly where a limit is hit" the spec asks for, and it is why
 * the prompt lives next to the control that hit the limit rather than in a
 * notification.
 */
export function ApiErrorNotice({
  error,
  noun,
  action,
  title,
  reason,
}: {
  error: ApiError
  /** What the limit counts, so the sentence can name it: "workspaces", "MB per file". */
  noun?: string
  action?: ReactNode
  /**
   * Overrides for the code's default wording, for the cases where a code means
   * something narrower at one call site than it does in general. `unauthorized`
   * on `/auth/login` is a wrong password, not an expired session; `conflict` on
   * signup is a taken address, not a lost update. The default copy stays the
   * general one, and only the screen that knows the narrower meaning overrides it.
   */
  title?: string
  reason?: string
}) {
  const kind = platform.kind()
  const isMobile = kind === 'ios' || kind === 'android'

  if (error.isPlanLimit) {
    return (
      <div className="note flex flex-col items-start gap-2">
        {/* `role="status"` is on the text, not on the wrapper: a live region is
            for content that changes on its own, and putting the link inside it
            would announce a control as part of a status message. */}
        <p className="font-medium" role="status">
          {title ?? apiErrorTitle(error)}
        </p>
        <p className="text-sm text-ink-muted">{planLimitMessage(error, noun)}</p>
        {/* Mobile builds carry no purchase UI (SPEC § Pricing), so they get the
            statement and no route to a checkout. */}
        {!isMobile && (
          <Link to="/billing" className="link inline-flex min-h-11 items-center text-sm">
            See plans
          </Link>
        )}
      </div>
    )
  }

  // A validation failure is the one case where the server's sentence is the
  // actionable content: it is the only statement of what was rejected. The form
  // shows it on the field when the code implies one, and here at the top when it
  // does not.
  const fallback =
    error.code === 'validation' && error.message.length > 0 ? error.message : apiErrorReason(error)

  return (
    <ErrorState
      title={title ?? apiErrorTitle(error)}
      reason={reason ?? fallback}
      action={action}
    />
  )
}
