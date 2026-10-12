import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useEvents } from '@/api/queries/events'
import { useReminders } from '@/api/queries/reminders'
import { useSettings } from '@/api/queries/settings'
import { useTasks } from '@/api/queries/tasks'
import { useSession } from '@/features/auth/useSession'
import { platform } from '@/platform'
import {
  LEAD_EVENT_MIN,
  LEAD_TASK_MIN,
  MAX_LOOKAHEAD_DAYS,
  planLocalNotifications,
  type LocalReminder,
  type LocalReminderKind,
} from './localSchedule'

/**
 * The local notifications this device schedules from its own synced rows.
 *
 * **Mobile only, and the gate is not an optimisation.** `Platform.notifications`
 * says it in `platform/types.ts`: mobile honours `at` and fires while the app is
 * closed, while desktop and web *cannot schedule* and deliver immediately. So an
 * ungated mount on the web build is a burst of notifications on every page load —
 * one per upcoming task and event, fired at the moment of the load rather than at
 * the time the reminder is about. The gate is on `Platform.kind()` rather than on
 * a capability probe because the capability is not what varies: it is the
 * *meaning* of `schedule()`, and only the mobile adapters give it the one a
 * schedule needs.
 *
 * The desktop shell's own native notification is the shell's (A5, A32): it is
 * delivered from the shell's process, not from a webview that may not be running.
 * Web and desktop get their reminders over the server's channels.
 *
 * **What it reads is what the device already has**, which is what makes reminders
 * work offline: `useTasks`, `useEvents` and `useSettings` read the local cache on
 * a build with one (A14), so a phone in aeroplane mode still schedules today's
 * deadlines. The plan itself is `localSchedule.ts`'s, which is pure and takes
 * `now` as an argument.
 *
 * **The `seen` list is the cross-device fallback** (A285). It holds the reminder
 * keys this device has already shown, under one key in `Platform.localState`, and
 * it is unioned with the keys the server's own feed reports for this device: a
 * reminder the server has already delivered — by push, or into the in-app feed
 * this device acknowledged — must not also arrive as a local notification, which
 * is the double-notification the cross-device dedupe exists to prevent. The key
 * both sides compute is `reminders::dedupe_key`'s, `"<kind>:<subject>:<local
 * date>"`, rebuilt from the feed row's own `kind`, `subject_id` and `local_date`
 * so the two producers cannot disagree about the format.
 */

/**
 * The one key this hook writes to local state. The web adapter prefixes it, the
 * way `useFocusTimer`'s `focus_timer` is prefixed.
 */
export const SEEN_KEY = 'reminder_seen'

/** How many keys the record keeps. A fortnight of reminders is far fewer. */
const SEEN_LIMIT = 200

/** The keys this device has already shown. */
export async function readSeenReminderIds(): Promise<string[]> {
  const stored = await platform.localState.get<string[]>(SEEN_KEY)
  return Array.isArray(stored) ? stored : []
}

/**
 * Records the keys this device has now shown.
 *
 * Appended rather than replaced, because the list is the record of what was
 * shown and a device that dropped it would re-notify about everything it had
 * already said. Capped, so the key cannot grow without bound on a phone that
 * never clears its storage.
 */
export async function markRemindersSeen(ids: string[]): Promise<void> {
  if (ids.length === 0) return
  const merged = [...new Set([...(await readSeenReminderIds()), ...ids])]
  await platform.localState.set(SEEN_KEY, merged.slice(Math.max(0, merged.length - SEEN_LIMIT)))
}

/**
 * The reminder key a server feed row stands for.
 *
 * `reminders::dedupe_key(kind, subject_key, local_date)`, rebuilt from the row's
 * own columns. The client cannot read the server's `dedupe_key` column — the feed
 * does not carry it — and it must not invent a second format, so this and
 * `localSchedule.ts` derive the same string from the same three facts.
 */
function feedKey(row: { kind: string; subject_id: string | null; local_date: string }): string | null {
  if (row.subject_id === null) return null
  return `${row.kind}:${row.subject_id}:${row.local_date}`
}

export interface LocalNotificationsState {
  /** The notifications this device scheduled, in fire order. */
  scheduled: LocalReminder[]
  /** Records one key as shown, for a caller that showed one outside this hook. */
  markSeen(id: string): Promise<void>
}

/**
 * Schedules this device's local notifications.
 *
 * `signedIn` is the caller's statement that there is an account — without one there
 * is no timezone, no quiet hours and no rows, and firing the four reads anyway
 * would be four 401s behind the sign-in screen.
 *
 * `now` is a parameter with a clock read only as its default, so a test can put the
 * schedule on either side of a quiet-hours boundary or a DST transition. The
 * instant is frozen at mount: a value that changed identity every render would make
 * the effect's dependency list unstable, and the plan would be recomputed for no
 * reason on every paint.
 */
export function useLocalNotifications(signedIn: boolean, now?: Date): LocalNotificationsState {
  const { t } = useTranslation()
  const [instant] = useState(() => now ?? new Date())
  const enabled = signedIn && (platform.kind() === 'ios' || platform.kind() === 'android')

  const session = useSession()
  const settings = useSettings()
  const tasks = useTasks()
  const events = useEvents(
    instant.toISOString(),
    new Date(instant.getTime() + MAX_LOOKAHEAD_DAYS * 24 * 60 * 60 * 1000).toISOString(),
  )
  // The server's own feed for this device. Absent while offline, which is why the
  // locally recorded keys are the authoritative half and this is the union.
  const feed = useReminders(enabled)

  const row = settings.data?.settings
  // The account's zone, not the device's (A203): a phone set to UTC must still
  // schedule at the Jakarta hour the server would use. `useSettings`' cached path
  // cannot answer it, so the session is the fallback there — the same pair
  // `SettingsScreen` reads.
  const timeZone =
    settings.data?.timezone ||
    (session.status === 'signed-in' ? session.user.timezone : '')
  // The two primitives, not a pair object: an object rebuilt every render would be
  // a new dependency identity on every render, and the effect below would re-run
  // and re-schedule the whole plan on every paint.
  const quietStart = row?.quiet_start_hour ?? null
  const quietEnd = row?.quiet_end_hour ?? null

  const [scheduled, setScheduled] = useState<LocalReminder[]>([])

  const copy = useCallback(
    (reminder: { kind: LocalReminderKind; subjectId: string }): { title: string; body: string } => {
      const subject =
        reminder.kind === 'task_due'
          ? tasks.data?.find((row) => row.id === reminder.subjectId)?.title
          : events.data?.find((event) => event.id === reminder.subjectId)?.title
      const title = subject ?? t('reminders.local.unknownSubject')
      // The two sentences are spelled out rather than selected by a table, because
      // every string has to go through `t()` as a literal key: the i18n contract
      // test reads `t('…')` out of the source, and a key reached through a variable
      // is copy nothing can see is rendered — it looks unused and it fails the
      // "no unused key" check while being the only thing that renders it.
      if (reminder.kind === 'task_due') {
        return {
          title: t('reminders.local.taskTitle'),
          body: t('reminders.local.taskBody', { title, minutes: LEAD_TASK_MIN }),
        }
      }
      return {
        title: t('reminders.local.eventTitle'),
        body: t('reminders.local.eventBody', { title, minutes: LEAD_EVENT_MIN }),
      }
    },
    [events.data, t, tasks.data],
  )

  useEffect(() => {
    if (!enabled) return
    if (tasks.data === undefined) return

    let cancelled = false

    void (async () => {
      const delivered = (feed.data ?? []).flatMap((row) => {
        const key = feedKey(row)
        return key === null ? [] : [key]
      })
      const plan = planLocalNotifications({
        now: instant,
        timeZone,
        quiet: quietStart === null || quietEnd === null ? null : { start: quietStart, end: quietEnd },
        tasks: tasks.data ?? [],
        events: events.data ?? [],
        seen: [...(await readSeenReminderIds()), ...delivered],
        copy,
      })

      for (const reminder of plan) {
        // Best-effort by contract (`NotificationsApi.schedule`): desktop and web
        // deliver immediately, and a refused permission throws. Neither is a reason
        // to lose the rest of the plan.
        try {
          await platform.notifications.schedule(reminder.title, reminder.body, reminder.at)
        } catch {
          // A schedule that failed is not a reminder that fired, so it is not
          // recorded as shown and the next mount tries it again.
          continue
        }
        await markRemindersSeen([reminder.id])
      }
      if (!cancelled) setScheduled(plan)
    })()

    return () => {
      cancelled = true
    }
  }, [copy, enabled, events.data, feed.data, instant, quietEnd, quietStart, tasks.data, timeZone])

  const markSeen = useCallback(async (id: string) => {
    await markRemindersSeen([id])
  }, [])

  return { scheduled, markSeen }
}
