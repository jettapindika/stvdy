import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useEvents, type EventRow } from '@/api/queries/events'
import { useWorkspaces } from '@/api/queries/workspaces'
import { PageHeader, Section } from '@/components/layout/Section'
import { EmptyState, LoadingState } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone, localDateKey, zoneOffsetLabel } from '@/lib/time'
import { courseVars } from '@/tokens/tokens'
import { ConnectionsPanel } from './ConnectionsPanel'
import { MonthGrid } from './MonthGrid'
import { WeekGrid, eventCellId } from './WeekGrid'
import { courseHue, localMonth, localWeek, placeEvents } from './calendar'

/**
 * The calendar: a week, a month, and the connections that feed them.
 *
 * **The window is the local one.** `localWeek`/`localMonth` are pure functions of
 * a fixed instant and the student's own IANA zone (never the machine's), so the
 * week starts on the student's Monday and the month holds the student's days —
 * which is the whole reason those functions are unit-tested separately from this
 * screen.
 *
 * **"Today" is the local today**, and it is the same instant the window was
 * computed from: `now` is captured once on mount, so the heading, the grid and the
 * "Today" cell cannot disagree about which day it is, and a window that changed on
 * every render would be a new query on every render.
 *
 * **Every number on this screen traces to a row** (gate 7). The month cell's count
 * is that day's bucket; the day's list is the rows `GET /events?from=&to=` (or the
 * sync cache) returned for the window; the connections' "last synced" is
 * `calendar_connections.last_synced_at` rendered. There is no "you have 3 hours
 * free" anywhere: nothing in this app computes free time, so nothing here claims
 * it. The one thing this screen says about the future is what the rows say.
 *
 * **Nothing here is generative and nothing spends credits**, so there is no cost
 * on any control — a badge that always reads `0` is noise, and the honesty gate
 * asks for the absence rather than the zero.
 *
 * The event detail is the one overlay, and it is a `<dialog>`-shaped inline panel
 * rather than a modal: it renders inside the day's own row, `Escape` closes it,
 * and focus returns to the cell that opened it. A trap-free panel is one a
 * keyboard user cannot lose their place in.
 */
export function CalendarScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const zone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  // Fixed for the life of the mount, and the single source of "now" for the
  // window, the heading and the Today cell. Navigating away and back recomputes
  // it, which is what a student expects from a calendar they just re-opened.
  const [now] = useState(() => new Date())
  const [view, setView] = useState<'week' | 'month'>('week')
  const [selected, setSelected] = useState<string | null>(null)
  // The cell that opened the detail, so `Escape` can hand focus back to it. A
  // detail that closed into `document.body` would leave the keyboard user at the
  // top of the page with no idea where they were.
  const opener = useRef<string | null>(null)

  const week = useMemo(() => localWeek(now, zone), [now, zone])
  const month = useMemo(() => localMonth(now, zone), [now, zone])
  const todayKey = localDateKey(now, zone)
  const monthKey = todayKey.slice(0, 7)

  const days = view === 'week' ? week : month
  const first = days[0]?.key ?? todayKey
  const last = days[days.length - 1]?.key ?? todayKey
  // A **superset** of the local window, deliberately: the exact local edges need
  // the zone's offset arithmetic here, and a bug in it would silently ask for a
  // window the rows are not in — the one failure this screen cannot see, because
  // it renders whatever comes back. So the window is the grid's own days widened
  // by a day on each side (no zone is more than 14 hours from UTC), and which day
  // a row is *on* is decided by `placeEvents`, in the student's zone. This is
  // `TodayScreen`'s rule and the same reasoning.
  const from = new Date(Date.parse(`${first}T00:00:00Z`) - 24 * 60 * 60 * 1000).toISOString()
  const to = new Date(Date.parse(`${last}T23:59:59Z`) + 24 * 60 * 60 * 1000).toISOString()
  const events = useEvents(from, to)
  const workspaces = useWorkspaces()

  const buckets = useMemo(
    () => placeEvents(events.data ?? [], days, zone),
    [events.data, days, zone],
  )

  const workspaceNames = useMemo(() => {
    const names: Record<string, string> = {}
    for (const workspace of workspaces.data ?? []) names[workspace.id] = workspace.name
    return names
  }, [workspaces.data])

  useEffect(() => {
    if (selected === null) return undefined
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setSelected(null)
      const id = opener.current
      if (id !== null) document.getElementById(eventCellId(id))?.focus()
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [selected])

  const total = events.data?.length ?? 0

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader
        title={t('calendar.title')}
        subtitle={t('calendar.subtitle', { zone: zoneOffsetLabel(now, zone) })}
      />

      <Section title={t('calendar.view.title')}>
        <div className="flex flex-wrap items-center gap-2" role="group" aria-label={t('calendar.view.title')}>
          {(['week', 'month'] as const).map((name) => (
            <button
              key={name}
              type="button"
              className={view === name ? 'btn btn-primary' : 'btn btn-secondary'}
              aria-pressed={view === name}
              onClick={() => {
                setView(name)
                setSelected(null)
              }}
            >
              {name === 'week' ? t('calendar.view.week') : t('calendar.view.month')}
            </button>
          ))}
          {/* The count is the rows in the window, and it is rendered as the
              number it is. A pending read says nothing rather than `0`. */}
          {events.data !== undefined && (
            <span className="meta">{t('calendar.eventCount', { count: total })}</span>
          )}
        </div>
      </Section>

      <Section title={view === 'week' ? t('calendar.week.title') : t('calendar.month.title')}>
        {events.isPending && <LoadingState label={t('calendar.loading')} />}
        {events.data !== undefined && total === 0 && (
          <EmptyState
            title={t('calendar.empty.title')}
            reason={t('calendar.empty.reason')}
          />
        )}
        {events.data !== undefined &&
          total > 0 &&
          (view === 'week' ? (
            <WeekGrid
              buckets={buckets}
              zone={zone}
              todayKey={todayKey}
              selected={selected}
              onSelect={(id) => {
                opener.current = id
                setSelected((current) => (current === id ? null : id))
              }}
              workspaceNames={workspaceNames}
              detail={(event) => <EventDetail event={event} zone={zone} />}
            />
          ) : (
            <MonthGrid
              buckets={buckets}
              zone={zone}
              todayKey={todayKey}
              month={monthKey}
              onOpenWeek={() => {
                setView('week')
                setSelected(null)
              }}
            />
          ))}
      </Section>

      <ConnectionsPanel />
    </div>
  )
}

/**
 * One event, in full: what it is, when it is, and whether it can be edited.
 *
 * The read-only sentence is **words**, not only the lock icon, and it says which
 * of the two reasons applies: a Google or ICS row came from somewhere else and
 * this app will not let it be edited here, and the next sync would revert an edit
 * that landed anyway (A198/A240). "Read-only" alone would leave the student
 * wondering whether it was a permission problem on their account.
 *
 * Nothing here edits, deletes or creates. An event editor is Phase 9's Tasks
 * screen and the Today view's capture, and a second write path for `events` on
 * this screen would be the thing Phase 9's A198 exists to prevent.
 */
function EventDetail({ event, zone }: { event: EventRow; zone: string }) {
  const { t } = useTranslation()
  const hue = courseHue(event.workspace_id)
  const external = event.source !== 'app'

  return (
    <div className="panel flex flex-col gap-2 p-4" style={hue === null ? undefined : courseVars(hue)}>
      <div className="flex flex-wrap items-center gap-2">
        {hue === null ? null : <span className="course-marker h-6" aria-hidden="true" />}
        <h3 className="min-w-0 break-words">{event.title}</h3>
        {external && (
          <span className="chip flex items-center gap-1">
            <Icon name="lock" size={16} />
            {t('calendar.event.readOnly')}
          </span>
        )}
      </div>
      <p className="text-small text-muted">
        {t('calendar.event.when', {
          start: formatInZone(new Date(event.starts_at), zone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
          end: formatInZone(new Date(event.ends_at), zone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
        })}
      </p>
      <p className="text-small text-muted">
        {t('calendar.event.kind', { kind: t(`calendar.kind.${event.kind}`) })}
      </p>
      {external && (
        <p className="max-w-prose text-small text-subtle">
          {event.source === 'google'
            ? t('calendar.event.readOnlyGoogle')
            : t('calendar.event.readOnlyIcs')}
        </p>
      )}
    </div>
  )
}
