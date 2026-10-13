import type { ReactNode } from 'react'

/**
 * A titled region of a screen.
 *
 * The heading is a real `<h2>` so the page has a navigable outline, and it gets
 * an `id` derived from its title so sections are genuinely linkable. Without the
 * id, an in-page link like `#palette` is a dead anchor: it looks like a
 * destination and does nothing when clicked (R-24).
 *
 * The optional `count` renders in monospace next to the title because counts are
 * evidence, not prose. A count is only shown when it is real; callers omit it
 * rather than passing a zero to fill space.
 *
 * **The heading is 24/32 as of the redesign**, up from 20/28: the design's h2 is
 * 24/32 and its card headings are 20/26, so a section heading and a card heading
 * are no longer the same size. `Section` is the former and `WidgetHeading` is the
 * latter.
 */
export function sectionId(title: string): string {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
}

export function Section({
  title,
  count,
  action,
  children,
}: {
  title: string
  count?: number
  action?: ReactNode
  children: ReactNode
}) {
  const id = sectionId(title)
  return (
    <section aria-labelledby={id} className="flex flex-col gap-3">
      {/* `flex-wrap` and `min-w-0` are the 200%-text accommodation. The row is a
          heading plus an action, and at a 32px root the pair measured 313px in a 310px
          column — a 3px overflow that scrolls the whole page sideways. `min-w-0` lets
          the heading take the width it is given (a flex child's automatic minimum is
          its content), and `flex-wrap` puts the action on the next line when the two
          genuinely cannot share one. */}
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        {/* The anchor target IS the heading, so an in-page link lands on visible
            content rather than on an empty spacer, and `scroll-mt` applies to the
            element the browser actually scrolls to. */}
        {/* A **block**, not a flex row, and that is the fix for a 3px overflow at 200%
            text: as a flex container the title becomes an anonymous flex item whose
            automatic minimum is its content, so "Your workspaces" at a 48px h2 size
            could not break and overflowed its 310px column. A block wraps its text
            normally, and the count still sits on the same baseline because an inline
            span is what a baseline is. */}
        <h2 id={id} className="min-w-0 scroll-mt-4">
          {title}
          {count !== undefined && <span className="meta"> {count}</span>}
        </h2>
        {action}
      </div>
      {children}
    </section>
  )
}

/**
 * The heading at the top of a card.
 *
 * The design's widget heading: a 20/26 heading with its count or state on the
 * right, and an optional eyebrow above it. It is a separate component from
 * `Section` because the two are different sizes and different levels — a card
 * heading is inside a section, and rendering it as an `<h3>` is what keeps the
 * page's heading order honest.
 */
export function WidgetHeading({
  title,
  eyebrow,
  meta,
  action,
}: {
  title: string
  eyebrow?: string
  /** A count or a state, in mono. Omitted rather than zero-filled. */
  meta?: string
  action?: ReactNode
}) {
  return (
    <div className="flex flex-col gap-1">
      {eyebrow && <p className="eyebrow">{eyebrow}</p>}
      <div className="widget-heading">
        <h3>{title}</h3>
        {meta && <span className="meta">{meta}</span>}
        {action}
      </div>
    </div>
  )
}

/**
 * The page header.
 *
 * The design's pattern: an eyebrow naming the context (a date, a course, a
 * section), the title, and a description — with an optional control on the right
 * (a date picker, the page's one primary action).
 *
 * **The eyebrow is not a restatement of the heading.** The earlier revision had no
 * eyebrow at all, on the grounds that "a pill restating the heading adds a line of
 * reading and no fact" (R-09). That rule still holds: the eyebrow here carries the
 * context the heading cannot — `MONDAY, 12 OCTOBER 2026 / AUTUMN 2026`,
 * `MY COURSES / BIO 201` — and a caller that has nothing to say there omits it.
 */
export function PageHeader({
  title,
  subtitle,
  eyebrow,
  action,
}: {
  title: string
  subtitle?: string
  eyebrow?: string
  action?: ReactNode
}) {
  return (
    <header className="flex flex-wrap items-end justify-between gap-4">
      <div className="flex min-w-0 flex-col gap-2">
        {eyebrow && <p className="eyebrow">{eyebrow}</p>}
        <h1>{title}</h1>
        {subtitle && <p className="max-w-prose text-body text-muted">{subtitle}</p>}
      </div>
      {action}
    </header>
  )
}

/**
 * The foot of a page.
 *
 * The design puts the reader's timezone on the left and the saved state on the
 * right, in 11px mono above a rule. Both are facts the app has: the zone comes from
 * the account, and the saved state from the sync engine's own report. A caller that
 * has neither omits the component rather than rendering two empty spans.
 *
 * It is not a substitute for `SyncStatus` — that control reports pending writes and
 * refusals, and this line states where "now" is and whether the last pass landed.
 */
export function PageFooter({ left, right }: { left: ReactNode; right?: ReactNode }) {
  return (
    <footer className="page-footer">
      <span>{left}</span>
      {right && <span>{right}</span>}
    </footer>
  )
}
