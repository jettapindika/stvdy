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
    <section aria-labelledby={`${id}-heading`} className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <h2 id={`${id}-heading`} className="flex items-baseline gap-2 scroll-mt-4">
          {/* The id lives on an inner span rather than on the h2 in a way that
              would break the heading outline, so an anchor lands here. */}
          <span id={id} className="scroll-mt-20" />
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
