import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useSearchParams } from 'react-router'

import { useSettings } from '@/api/queries/settings'
import { useUsage } from '@/api/queries/usage'
import { PageFooter, PageHeader } from '@/components/layout/Section'
import { Icon, type IconName } from '@/components/ui/Icon'
import { DEFAULT_TIMEZONE, formatInZone, zoneOffsetLabel } from '@/lib/time'
import { useFormFactor } from '@/lib/useFormFactor'
import { CalendarScreen } from '@/features/calendar/CalendarScreen'
import { LayoutSwitch, TasksScreen } from '@/features/tasks/TasksScreen'

/**
 * The planner, as a **host with two views**.
 *
 * The navigation was already changed to `Today · Workspaces · Study · Planner ·
 * Settings`, and `/calendar` and `/tasks` are redirects into this screen. That is
 * what `SPEC.md:664-665` asked for and what the design draws: *"Planner (Calendar
 * | Tasks)"* on a desktop, and *"Tasks live inside Planner as a segmented control
 * (Plan | Tasks)"* on a phone. So the two former screens are panels this host
 * renders, and the switcher is the segmented control.
 *
 * **The view is in the URL, not in component state.** `?view=calendar|tasks`. A
 * view held in `useState` cannot be deep-linked, cannot be bookmarked, and cannot
 * be measured — the accessibility walk and `scripts/measure-mobile.mjs` navigate
 * by URL, and a screen that only changes on a click is a screen no harness can
 * reach. It is also what keeps the two redirects meaningful: `/tasks` resolves to
 * `/planner?view=tasks`, which is a real address rather than a hint.
 *
 * **`?view=plan` is an alias for the calendar view, and that is the design's own
 * structure rather than a shortcut.** Every Planner frame draws the plan *on the
 * calendar*: the `Suggested study plan` banner sits above the week grid, the
 * proposed blocks are drawn in the grid as `Suggested · not accepted`, and the
 * prep-progress card is the sidebar beside them. There is no separate plan screen
 * in the design, so `plan` and `calendar` name one view — and the older URL keeps
 * working instead of becoming a 404. `SPEC.md:665`'s phone label for that view is
 * `Plan`, which is a label difference by form factor exactly like `nav.work`
 * ("Work" on a phone, "Workspaces" on a desktop), not a second view.
 *
 * **`?layout=list|board` applies to the tasks view only** and is ignored
 * elsewhere, so switching to the calendar and back does not lose the student's
 * board. It is in the URL for the same reason `view` is: the board is a real
 * place to come back to, and a link to it has to be a link.
 *
 * `now` is captured once, here, and passed down. The calendar's current-time line,
 * the grid's "Today" cell, the board's overdue marks and the plan's day counts all
 * read it, so they cannot disagree about which day it is or drift under the
 * student's hands while they look at the screen.
 *
 * **The eyebrow carries the date and nothing else.** The design's eyebrow is
 * `MONDAY, 12 OCTOBER 2026 / AUTUMN 2026`, and `AUTUMN 2026` has no data source —
 * no migration carries a semester or a term, and inventing one is the one thing
 * this app may not do with data (design record § 7). The account's real timezone
 * is the honest replacement for that half and it already appears in the page
 * footer, so the eyebrow keeps the half that is real: the student's own date.
 */
export function PlannerScreen() {
  const { t } = useTranslation()
  const [params] = useSearchParams()
  const usage = useUsage()
  const settings = useSettings()
  const isPhone = useFormFactor() === 'phone'

  const [now] = useState(() => new Date())

  // Anything that is not `tasks` is the plan, including an absent parameter and the
  // older `plan` value. One comparison rather than a switch, so a URL a
  // notification or a bookmark already carries cannot land on nothing.
  const view = params.get('view') === 'tasks' ? 'tasks' : 'plan'
  const layout = params.get('layout') === 'board' ? 'board' : 'list'

  const timeZone =
    usage.data === undefined ? DEFAULT_TIMEZONE : (settings.data?.timezone || DEFAULT_TIMEZONE)

  const views: { id: 'plan' | 'tasks'; to: string; label: string; icon: IconName }[] = [
    {
      id: 'plan',
      to: '/planner?view=plan',
      // The phone's label for this view is the spec's own word (`SPEC.md:665`,
      // "Plan | Tasks"); the desktop's is the design's frame label.
      label: isPhone ? t('planner.views.plan') : t('planner.views.calendar'),
      icon: 'calendar',
    },
    { id: 'tasks', to: '/planner?view=tasks', label: t('planner.views.tasks'), icon: 'list-checks' },
  ]

  const subtitle =
    view === 'tasks' ? t('planner.tasks.subtitle') : t('planner.calendar.subtitle')

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-6 px-5 py-8">
      <PageHeader
        eyebrow={formatInZone(now, timeZone, { dateStyle: 'full' })}
        title={t('nav.planner')}
        subtitle={subtitle}
        action={
          view === 'tasks' ? (
            // A real destination rather than a decorative button: the tasks view's
            // create form carries `id="new-task"`, and `tabIndex={-1}` on that
            // section makes the browser move focus there, so this both scrolls and
            // lands the keyboard where the work starts. No sheet exists in this
            // app, and a `+ New task` that opened nothing would be a control whose
            // only outcome is nothing.
            <a href="#new-task" className="btn btn-primary">
              <Icon name="plus" size={20} />
              {t('tasks.new.action')}
            </a>
          ) : undefined
        }
      />

      {/* The view switcher: the design's segmented control, as two links so each
          view is an address. `aria-current` marks the active one and the label
          weight goes 500 -> 600 as well, so the state is not carried by colour
          alone (gate 5d). */}
      <nav
        aria-label={t('planner.views.label')}
        className="flex flex-wrap items-center justify-between gap-3 border-b border-border"
      >
        <div className="flex flex-wrap gap-7">
          {views.map((item) => {
            const active = item.id === view
            return (
              <Link
                key={item.id}
                to={item.to}
                aria-current={active ? 'page' : undefined}
                className={`flex min-h-12 min-w-0 items-center gap-2 border-b-2 px-1 text-label ${
                  active
                    ? 'border-primary font-semibold text-primary-text'
                    : 'border-transparent text-muted hover:text-text'
                }`}
              >
                <Icon name={item.icon} size={20} />
                {item.label}
              </Link>
            )
          })}
        </div>

        {/* The tasks view's own List | Board switch, in this row rather than inside
            the panel, which is where the frame draws it. It is absent on a phone
            because both layouts render the same card list there, and a switch
            between two identical views is a control whose only outcome is nothing. */}
        {view === 'tasks' && !isPhone ? <LayoutSwitch layout={layout} /> : null}
      </nav>

      {view === 'tasks' ? (
        <TasksScreen now={now} timeZone={timeZone} layout={layout} />
      ) : (
        <CalendarScreen now={now} timeZone={timeZone} />
      )}

      {/* The footer's left half only. The design puts a saved state on the right
          (`All changes saved · 10:38`), and that line is two claims this screen
          cannot make: the clock is a render-time read, and "all changes saved" is
          the sync control's answer rather than this screen's. The shell's
          `SyncStatus` already states it, with the real code when a write was
          refused, so a second copy here would be a second live region announcing
          the same fact — and a worse one, because it would have no source. */}
      <PageFooter left={`${timeZone.toUpperCase()} · ${zoneOffsetLabel(now, timeZone)}`} />
    </div>
  )
}
