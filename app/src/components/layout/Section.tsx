import type { ReactNode } from 'react'

/**
 * A titled region of a screen.
 *
 * The heading is a real `<h2>` so the page has a navigable outline, and the
 * optional `count` renders in monospace next to it because counts are evidence,
 * not prose. A count is only shown when it is real; callers omit it otherwise
 * rather than passing a zero to fill space.
 */
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
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <h2 className="flex items-baseline gap-2">
          {title}
          {count !== undefined && <span className="meta">{count}</span>}
        </h2>
        {action}
      </div>
      {children}
    </section>
  )
}

/**
 * Page header. No eyebrow badge above the title: a pill restating the heading
 * adds a line of reading and no fact (R-09).
 */
export function PageHeader({ title, subtitle }: { title: string; subtitle?: string }) {
  return (
    <header className="flex flex-col gap-1">
      <h1>{title}</h1>
      {subtitle && <p className="max-w-prose text-sm text-ink-muted">{subtitle}</p>}
    </header>
  )
}
