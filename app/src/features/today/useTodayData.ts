import { useQuery } from '@tanstack/react-query'

import { api } from '@/api/client'
import { useEvents } from '@/api/queries/events'
import { hasLocalCache } from '@/api/queries/synced'
import { useActivity, useSessions, type SessionRow } from '@/api/queries/study'
import { useTasks } from '@/api/queries/tasks'
import type { EventRow } from '@/api/queries/events'
import type { TaskRow } from '@/features/tasks/order'
import { platform } from '@/platform'

/**
 * Everything the Today view renders, read the way this build can read it.
 *
 * The roadmap's gate for this phase is that the Today view works **offline**, and
 * that is why there is no `GET /today`: a server route cannot (A205). So the
 * screen's input is "the rows this device already has", and the buckets are
 * computed from them by `day.ts`, which is pure.
 *
 * There is exactly one asymmetry between the builds, and it is stated rather than
 * hidden: the web build has no local cache and no outbox (A14/A183), so its reads
 * are the REST routes. That is not a second write path — a write still goes
 * through `saveTask`/`logSession`'s one decision — it is the read a build without
 * a database has to make. The shell reads its cache and needs no network at all.
 *
 * Two reads here are more than one round trip on the web build, and both are
 * bounded rather than paginated: the due-card count walks workspaces → decks →
 * each deck's queue, and the setup counts walk workspaces → decks. Both walk with
 * the *same* queue limit the study screen itself reads, so the number the Today
 * view shows is the number of cards a review would actually put in front of the
 * student — not a claim about rows the device never fetched.
 */

/**
 * The review queue's size, the deck it starts in, and the names that go with it.
 *
 * The title and the course are carried here rather than looked up again by the
 * screen because this read is already walking to the deck: a second lookup would
 * be a second walk to the same row, and the two could disagree about which deck
 * "Review N cards" opens.
 */
interface ReviewQueue {
  deckId: string | null
  dueCount: number
  deckTitle: string | null
  courseName: string | null
}

/** Live workspaces, sources and decks, for the new-user checklist. */
interface SetupCounts {
  workspaces: number
  sources: number
  decks: number
}

/** A course, as the timeline's and the deadlines' tags name it. */
export interface CourseRef {
  id: string
  name: string
  /** The student's own course code (`BIO 201`), or null when they gave none. */
  code: string | null
  /** The course's hue index, 0-5, as the server assigned it. */
  color: number
}

/** The checklist's counts, plus the workspace "continue where you left off" opens. */
interface SetupRead {
  counts: SetupCounts
  lastWorkspace: { id: string; name: string; updatedAt: string | null } | null
  courses: CourseRef[]
}

/** The page size the study screen uses, so the count matches what a review shows. */
const QUEUE_LIMIT = 100

async function readReviewQueue(): Promise<ReviewQueue> {
  if (hasLocalCache()) {
    // The cache holds cards, and `due_at` is a card field the pull ships, so
    // due-ness is computed here rather than fetched: a device that is offline has
    // to answer this question, and `features/study/due.ts` is the one rule for it.
    const [cards, decks, workspaces] = await Promise.all([
      platform.sync.read<{ deck_id: string; due_at: string; deleted_at?: string | null }>('flashcard'),
      platform.sync.read<{ id: string; workspace_id: string; title: string }>('deck'),
      platform.sync.read<{ id: string; name: string }>('workspace'),
    ])
    const now = Date.now()
    const due = cards.filter(
      (card) => card.deleted_at == null && Date.parse(card.due_at) <= now,
    )
    const deckId = due[0]?.deck_id ?? null
    const deck = decks.find((row) => row.id === deckId)
    return {
      deckId,
      dueCount: due.length,
      deckTitle: deck?.title ?? null,
      courseName:
        workspaces.find((row) => row.id === deck?.workspace_id)?.name ?? null,
    }
  }

  const { workspaces } = await api.get<{ workspaces: { id: string; name: string }[] }>(
    '/workspaces',
  )
  let deckId: string | null = null
  let dueCount = 0
  let deckTitle: string | null = null
  let courseName: string | null = null
  for (const workspace of workspaces) {
    const { decks } = await api.get<{ decks: { id: string; title: string; status: string }[] }>(
      `/workspaces/${workspace.id}/decks`,
    )
    for (const deck of decks.filter((row) => row.status === 'ready')) {
      const { cards } = await api.get<{ cards: unknown[] }>(
        `/decks/${deck.id}/due?limit=${QUEUE_LIMIT}`,
      )
      dueCount += cards.length
      if (deckId === null && cards.length > 0) {
        deckId = deck.id
        deckTitle = deck.title
        courseName = workspace.name
      }
    }
  }
  return { deckId, dueCount, deckTitle, courseName }
}

async function readSetup(): Promise<SetupRead> {
  if (hasLocalCache()) {
    const [workspaces, sources, decks] = await Promise.all([
      platform.sync.read<{
        id: string
        name: string
        course_code?: string | null
        color?: number
        updated_at?: string
        deleted_at?: string | null
      }>('workspace'),
      platform.sync.read<{ deleted_at?: string | null }>('source'),
      platform.sync.read<{ deleted_at?: string | null }>('deck'),
    ])
    const live = <T extends { deleted_at?: string | null }>(rows: T[]) =>
      rows.filter((row) => row.deleted_at == null)
    const liveWorkspaces = live(workspaces)
    return {
      counts: {
        workspaces: liveWorkspaces.length,
        sources: live(sources).length,
        decks: live(decks).length,
      },
      // The most recently touched workspace, which is what "continue" means. An
      // unknown `updated_at` sorts last rather than first: a row this build cannot
      // date is not evidence of recent work.
      lastWorkspace:
        [...liveWorkspaces]
          .sort(
            (left, right) =>
              Date.parse(right.updated_at ?? '') - Date.parse(left.updated_at ?? '') ||
              left.id.localeCompare(right.id),
          )
          .map((row) => ({ id: row.id, name: row.name, updatedAt: row.updated_at ?? null }))[0] ??
        null,
      courses: liveWorkspaces.map((row) => ({
        id: row.id,
        name: row.name,
        code: row.course_code ?? null,
        // A row pulled before the colour column existed has none; indigo is the
        // documented fallback, and the tag always carries the code or the name
        // beside it, so the hue is never the only signal.
        color: row.color ?? 0,
      })),
    }
  }

  const { workspaces } = await api.get<{
    workspaces: {
      id: string
      name: string
      updated_at: string
      source_count: number
      course_code: string | null
      color: number
    }[]
  }>('/workspaces')
  let decks = 0
  for (const workspace of workspaces) {
    const body = await api.get<{ decks: unknown[] }>(`/workspaces/${workspace.id}/decks`)
    decks += body.decks.length
  }
  return {
    counts: {
      workspaces: workspaces.length,
      sources: workspaces.reduce((total, row) => total + row.source_count, 0),
      decks,
    },
    lastWorkspace:
      [...workspaces]
        .sort(
          (left, right) =>
            Date.parse(right.updated_at) - Date.parse(left.updated_at) ||
            left.id.localeCompare(right.id),
        )
        .map((row) => ({ id: row.id, name: row.name, updatedAt: row.updated_at }))[0] ?? null,
    courses: workspaces.map((row) => ({
      id: row.id,
      name: row.name,
      code: row.course_code,
      color: row.color,
    })),
  }
}

export interface TodayData {
  tasks: TaskRow[]
  events: EventRow[]
  sessions: SessionRow[]
  activity: { reviews: string[]; attempts: string[]; sessions: string[] }
  dueCards: number
  /** The deck a one-tap review opens, or null when nothing is due. */
  reviewDeckId: string | null
  /** The deck's title and its course, so the review block can name what it opens. */
  reviewDeck: { title: string | null; course: string | null }
  /** The live courses, so a timeline row or a deadline can be tagged with one. */
  courses: CourseRef[]
  setup: SetupCounts
  /** The most recently touched workspace, for "continue where you left off". */
  lastWorkspace: { id: string; name: string; updatedAt: string | null } | null
  /** The window the reads used, so the screen can say what "today" covers. */
  window: { from: string; to: string }
  loading: boolean
  failed: boolean
}

/**
 * `from` and `to` are ISO instants bounding the read, and the caller computes them
 * in the student's zone: the window is a *local* day, and a UTC window would ask
 * the server for the wrong one (A6).
 */
export function useTodayData(window: { from: string; to: string }): TodayData {
  const tasks = useTasks()
  const events = useEvents(window.from, window.to)
  const sessions = useSessions(window.from, window.to)
  const activity = useActivity(window.from, window.to)
  const queue = useQuery({ queryKey: ['review-queue'], queryFn: readReviewQueue })
  const setup = useQuery({ queryKey: ['setup-counts'], queryFn: readSetup })

  const queries = [tasks, events, sessions, activity, queue, setup]
  return {
    tasks: tasks.data ?? [],
    events: events.data ?? [],
    sessions: sessions.data ?? [],
    activity: activity.data ?? { reviews: [], attempts: [], sessions: [] },
    dueCards: queue.data?.dueCount ?? 0,
    reviewDeckId: queue.data?.deckId ?? null,
    reviewDeck: { title: queue.data?.deckTitle ?? null, course: queue.data?.courseName ?? null },
    courses: setup.data?.courses ?? [],
    setup: setup.data?.counts ?? { workspaces: 0, sources: 0, decks: 0 },
    lastWorkspace: setup.data?.lastWorkspace ?? null,
    window,
    loading: queries.some((query) => query.isPending),
    failed: queries.some((query) => query.isError),
  }
}
