import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'
import { useQueries } from '@tanstack/react-query'

import { api } from '@/api/client'
import { Icon, type IconName } from '@/components/ui/Icon'
import { useWorkspaces } from '@/api/queries/workspaces'
import { useTasks } from '@/api/queries/tasks'
import { sourceKeys, type Source } from '@/api/queries/sources'
import { flashcardKeys, type DecksResponse } from '@/api/queries/flashcards'
import { platform } from '@/platform'

/**
 * The command palette: `⌘K` / `Ctrl K`.
 *
 * It searches what the account actually holds — workspaces, sources, decks and
 * tasks — plus a fixed set of navigation commands, and it is the destination the
 * top bar's search field opens.
 *
 * **The index is built from the queries the screens already use, and it is built
 * only while the palette is open.** Sources and decks are per-workspace endpoints,
 * so indexing them means one request per workspace for each; that is a fan-out of
 * `2 x workspaces` requests, which is why it is gated on `open` rather than
 * mounted with the shell. A student with six courses issues twelve requests the
 * first time they press `⌘K` and none thereafter, because TanStack caches them and
 * the screens share the same keys.
 *
 * **Nothing is invented.** Every row is a real row the API returned, and a
 * category with no rows is omitted rather than shown empty — the palette's own
 * empty state says that nothing matched, which is a fact, rather than rendering
 * four empty headings.
 *
 * **What it does not do:** it does not search message text or chunk contents, and
 * it does not call a search endpoint. Both would need a server-side index; the
 * palette says what it searched so a student who does not find a phrase in a
 * source's body is not left thinking the app lost it.
 *
 * Accessibility: a labelled dialog with a focus trap by construction (the only
 * focusable thing is the input; Tab is intercepted to stay inside), Escape closes,
 * arrows move the active row, Enter activates it. The active row is marked with
 * `aria-activedescendant` rather than by moving focus, so a screen reader reads the
 * row without the input losing its place — which is the combobox pattern in full:
 * the input is `role="combobox"` owning the `role="listbox"`, and each row is a
 * `role="option"` carrying `aria-selected`. The roles are not decoration; without
 * them `aria-activedescendant` is not exposed and arrow-key movement is silent.
 * The active row is scrolled into the list's viewport by hand, because focus never
 * enters the list and so nothing else would.
 */

/** One result row, before it is rendered. */
interface Result {
  id: string
  group: string
  label: string
  /** The line under the label: a course, a due date, a status. */
  detail: string | null
  icon: IconName
  to: string
}

export function CommandPalette({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  /** The rendered option per result id, for the one thing a ref is needed for: scrolling. */
  const optionRefs = useRef(new Map<string, HTMLLIElement>())
  const listId = useId()

  // Reset on each open, so the palette never reopens on a stale query with a stale
  // active row pointing past the end of a shorter list.
  useEffect(() => {
    if (!open) return
    setQuery('')
    setActive(0)
    // Focus after the dialog is in the DOM, or the input does not exist yet.
    const frame = requestAnimationFrame(() => inputRef.current?.focus())
    return () => cancelAnimationFrame(frame)
  }, [open])

  const workspaces = useWorkspaces({ enabled: open })
  const tasks = useTasks({ enabled: open })
  const rows = workspaces.data ?? []

  /**
   * The per-workspace reads, as plain option objects.
   *
   * **`useSources`/`useDecks` are deliberately not called here.** A hook inside a
   * `.map()` is two defects at once: the hook count then varies with the number of
   * workspaces, so the render where the workspace list resolves throws "Rendered
   * more hooks than during the previous render"; and a hook returns a *result*
   * (`{data, status, isPending…}`), which carries neither `queryKey` nor `queryFn`,
   * so `useQueries` would be handed options it cannot fetch with. The keys and the
   * fetch are therefore built here from the same key factories the screens use —
   * `sourceKeys.list` / `flashcardKeys.decks` — which is what keeps the cache
   * shared, so opening a workspace after a search is already warm.
   *
   * `enabled: open` is the fan-out gate: this is `2 x workspaces` requests, and
   * none of them is worth making until a student actually opens the palette.
   */
  const sourceQueries = useQueries({
    queries: rows.map((workspace) => ({
      queryKey: sourceKeys.list(workspace.id),
      queryFn: async ({ signal }: { signal: AbortSignal }) => {
        const body = await api.get<{ sources: Source[] }>(
          `/workspaces/${workspace.id}/sources`,
          { signal },
        )
        return body.sources
      },
      enabled: open,
    })),
  })
  const deckQueries = useQueries({
    queries: rows.map((workspace) => ({
      queryKey: flashcardKeys.decks(workspace.id),
      queryFn: async ({ signal }: { signal: AbortSignal }) =>
        api.get<DecksResponse>(`/workspaces/${workspace.id}/decks`, { signal }),
      enabled: open,
    })),
  })

  const results = useMemo(() => {
    const nameOf = new Map(rows.map((workspace) => [workspace.id, workspace.name]))
    const out: Result[] = []

    // Navigation commands. These are the palette's own vocabulary, not data, and
    // they come first when the query is empty because "go somewhere" is what a
    // student opens it for most often.
    const commands: Result[] = [
      { id: 'nav:today', group: 'go', label: t('nav.today'), detail: null, icon: 'today', to: '/' },
      {
        id: 'nav:workspaces',
        group: 'go',
        label: t('nav.workspaces'),
        detail: null,
        icon: 'workspaces',
        to: '/workspaces',
      },
      { id: 'nav:study', group: 'go', label: t('nav.study'), detail: null, icon: 'study', to: '/study' },
      {
        id: 'nav:planner',
        group: 'go',
        label: t('nav.planner'),
        detail: null,
        icon: 'planner',
        to: '/planner',
      },
      {
        id: 'nav:settings',
        group: 'go',
        label: t('nav.settings'),
        detail: null,
        icon: 'settings',
        to: '/settings',
      },
    ]
    out.push(...commands)

    for (const workspace of rows) {
      out.push({
        id: `workspace:${workspace.id}`,
        group: 'workspaces',
        label: workspace.name,
        // The course code when there is one, the source count otherwise. Both are
        // facts the row carries; neither is derived from the other.
        detail: workspace.course_code ?? t('palette.sourceCount', { count: workspace.source_count }),
        icon: 'workspaces',
        to: `/workspaces/${workspace.id}`,
      })
    }

    for (const queryResult of sourceQueries) {
      for (const source of queryResult.data ?? []) {
        out.push({
          id: `source:${source.id}`,
          group: 'sources',
          label: source.title,
          detail: nameOf.get(source.workspace_id) ?? null,
          icon: 'file-text',
          to: `/workspaces/${source.workspace_id}`,
        })
      }
    }

    for (const queryResult of deckQueries) {
      for (const deck of queryResult.data?.decks ?? []) {
        out.push({
          id: `deck:${deck.id}`,
          group: 'decks',
          label: deck.title,
          detail: nameOf.get(deck.workspace_id) ?? null,
          icon: 'cards',
          to: `/study/${deck.id}`,
        })
      }
    }

    for (const task of tasks.data ?? []) {
      out.push({
        id: `task:${task.id}`,
        group: 'tasks',
        label: task.title,
        // A task with no workspace is a quick capture, and it says so rather than
        // rendering an empty second line.
        detail: task.workspace_id ? (nameOf.get(task.workspace_id) ?? null) : t('palette.noCourse'),
        icon: 'list-checks',
        to: '/planner?view=tasks',
      })
    }

    const needle = query.trim().toLowerCase()
    if (needle.length === 0) return out
    return out.filter(
      (result) =>
        result.label.toLowerCase().includes(needle) ||
        (result.detail?.toLowerCase().includes(needle) ?? false),
    )
  }, [rows, sourceQueries, deckQueries, tasks.data, query, t])

  // Grouped in the design's order, and only groups that have rows survive.
  const grouped = useMemo(() => {
    const order = ['go', 'workspaces', 'sources', 'decks', 'tasks'] as const
    return order
      .map((group) => ({ group, rows: results.filter((result) => result.group === group) }))
      .filter((entry) => entry.rows.length > 0)
  }, [results])

  /**
   * Bring the active row into the list's viewport.
   *
   * **The browser does not do this for us.** The active row is marked with
   * `aria-activedescendant` and focus stays in the input, so nothing scrolls: the
   * list is `overflow-y-auto` inside a `70vh` popup and a six-course account fans
   * out to a dozen source and deck rows, so arrow-key movement below the fold moves
   * a highlight the student cannot see. `block: 'nearest'` is the scroll that does
   * nothing when the row is already visible, which keeps the list still while the
   * highlight is inside it.
   *
   * The id is read from `results[active]` rather than from the ref map's own order,
   * because the map is filled in render order (grouped) while `active` indexes the
   * flat list.
   */
  const activeResult = results[active]
  useEffect(() => {
    if (!open || activeResult === undefined) return
    optionRefs.current.get(activeResult.id)?.scrollIntoView({ block: 'nearest' })
  }, [open, activeResult])

  if (!open) return null

  const activate = (result: Result | undefined) => {
    if (!result) return
    onClose()
    navigate(result.to)
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-text/30 p-4 pt-[10vh]"
      // A click on the scrim closes; a click inside must not, so the dialog stops
      // the event rather than the scrim guessing from coordinates.
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t('palette.title')}
        className="float flex max-h-[70vh] w-full max-w-2xl flex-col overflow-hidden"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onClose()
            return
          }
          if (event.key === 'ArrowDown') {
            event.preventDefault()
            setActive((current) => (results.length === 0 ? 0 : (current + 1) % results.length))
            return
          }
          if (event.key === 'ArrowUp') {
            event.preventDefault()
            setActive((current) =>
              results.length === 0 ? 0 : (current - 1 + results.length) % results.length,
            )
            return
          }
          if (event.key === 'Enter') {
            event.preventDefault()
            activate(results[active])
            return
          }
          // Tab is intercepted rather than trapped with a sentinel element: the
          // dialog has exactly one focusable control plus its rows, and cycling
          // between them is the whole of what a trap would do.
          if (event.key === 'Tab') event.preventDefault()
        }}
      >
        <div className="flex items-center gap-3 border-b border-border px-4 py-3">
          <Icon name="search" className="shrink-0 text-muted" />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value)
              // The list shrinks as the query narrows, so the active index has to
              // go back to the top or it would point past the end.
              setActive(0)
            }}
            aria-label={t('palette.title')}
            // The combobox wiring, which is what makes `aria-activedescendant`
            // mean anything: the attribute is only honoured on a combobox whose
            // listbox is named by `aria-controls`, and its value must be an id in
            // that listbox. Without the roles the input's arrow-key movement is
            // silent to a screen reader — the highlight moves and nothing is read.
            role="combobox"
            aria-expanded={true}
            aria-controls={listId}
            aria-activedescendant={
              results[active] ? `${listId}-${results[active].id}` : undefined
            }
            placeholder={t('palette.searchPlaceholder')}
            className="min-w-0 flex-1 bg-transparent text-body text-text outline-none placeholder:text-subtle"
            autoComplete="off"
          />
          <kbd className="kbd shrink-0">{t('palette.escape')}</kbd>
        </div>

        <ul id={listId} role="listbox" aria-label={t('palette.title')} className="min-h-0 flex-1 overflow-y-auto py-2">
          {grouped.length === 0 && (
            <li role="presentation" className="px-4 py-6 text-center text-label text-muted">
              {/* Names what was searched, so "not found" is not read as "the app
                  lost it": the palette never looks inside a source's text. */}
              {t('palette.noResults', { query })}
            </li>
          )}
          {grouped.map((entry) => (
            <li key={entry.group} role="presentation">
              <p className="eyebrow px-4 py-1" role="presentation">
                {t(`palette.group.${entry.group}`)}
              </p>
              <ul role="group" aria-label={t(`palette.group.${entry.group}`)}>
                {entry.rows.map((result) => {
                  const index = results.indexOf(result)
                  return (
                    // A listbox option rather than a button: the keyboard is the
                    // input's — arrows move `aria-activedescendant` and Enter
                    // activates — so the row is a selected *option*, and a button
                    // inside a listbox is neither reachable by Tab nor announced as
                    // the active row. `aria-selected` is what a screen reader reads
                    // as the highlight moves; `aria-current` on a button meant
                    // nothing outside a navigation list.
                    <li
                      key={result.id}
                      ref={(element) => {
                        if (element === null) optionRefs.current.delete(result.id)
                        else optionRefs.current.set(result.id, element)
                      }}
                      id={`${listId}-${result.id}`}
                      role="option"
                      aria-selected={index === active}
                      onMouseEnter={() => setActive(index)}
                      onClick={() => activate(result)}
                      className={`flex w-full cursor-pointer items-center gap-3 px-4 py-2 text-left ${
                        index === active ? 'bg-primary-tint text-primary-text' : 'text-text'
                      }`}
                    >
                      <Icon name={result.icon} className="shrink-0" />
                      {/* `title` on both clipped spans: a label ellipsised with
                          nothing carrying the full string is a row the student
                          cannot tell from its neighbour, and the palette is opened
                          from the phone header, where there is no hover to reveal
                          one. The layout harness exempts a `truncate` only when a
                          `title` holds the text. */}
                      <span className="min-w-0 flex-1 truncate text-label" title={result.label}>
                        {result.label}
                      </span>
                      {result.detail && (
                        <span className="meta shrink-0 truncate" title={result.detail}>
                          {result.detail}
                        </span>
                      )}
                    </li>
                  )
                })}
              </ul>
            </li>
          ))}
        </ul>

        {/* The keyboard guidance the design puts at the foot of the palette. */}
        <div className="flex items-center gap-4 border-t border-border px-4 py-2">
          <span className="meta flex items-center gap-1">
            <kbd className="kbd">↑</kbd>
            <kbd className="kbd">↓</kbd>
            {t('palette.move')}
          </span>
          <span className="meta flex items-center gap-1">
            <kbd className="kbd">↵</kbd>
            {t('palette.open')}
          </span>
          <span className="meta ml-auto">
            {t('palette.shortcut', { shortcut: platform.shortcutLabel('K') })}
          </span>
        </div>
      </div>
    </div>
  )
}
