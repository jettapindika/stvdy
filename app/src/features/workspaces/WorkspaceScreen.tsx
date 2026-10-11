import { useRef, useState } from 'react'

import { EmptyState } from '@/components/ui/States'

/**
 * The workspace view: Sources, Chat, and Studio.
 *
 * Composition differs by form factor on purpose (RHYTHM 2). Wide screens get all
 * three panels side by side, because reading a source while asking about it is
 * the actual work. Phones get one panel at a time behind a tab strip.
 *
 * The panel strip is a complete ARIA tab pattern, not half of one. Announcing
 * `role="tab"` and `aria-selected` without `aria-controls`, `role="tabpanel"`, and
 * `aria-labelledby` tells a screen reader "selected tab" and then gives it no
 * panel to move to, which is worse than plain buttons. The keyboard contract is
 * part of the same pattern: roving tabindex (only the active tab is a tab stop)
 * and Left/Right/Home/End, per the WAI-ARIA authoring practices.
 *
 * The strip is `lg:hidden`: below `lg` the panels are one at a time, at and above
 * it all three are visible and there is no tab interface at all.
 *
 * The root fills the space the shell gives it (`min-h-0 flex-1`) rather than the
 * viewport, so panels scroll internally without an outer page scroll.
 */

type Panel = 'sources' | 'chat' | 'studio'

const PANELS: { id: Panel; label: string; empty: { title: string; reason: string } }[] = [
  {
    id: 'sources',
    label: 'Sources',
    empty: {
      title: 'No sources',
      reason:
        'Add a PDF, slide deck, document, or link. Each one is chunked and indexed, so a question can be answered from any of them and cite the page it came from.',
    },
  },
  {
    id: 'chat',
    label: 'Chat',
    empty: {
      title: 'Ask about this workspace',
      reason:
        'Answers come from your sources first, and every claim carries a citation back to the page it came from. Turn on web search to bring in outside results, cited separately.',
    },
  },
  {
    id: 'studio',
    label: 'Studio',
    empty: {
      title: 'Nothing generated yet',
      reason:
        'From the selected sources you can generate a flashcard deck, a quiz, a report, or a print-ready cheatsheet. Each one keeps the link back to the material it came from.',
    },
  },
]

export function WorkspaceScreen() {
  const [active, setActive] = useState<Panel>('chat')
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  // Arrow keys move between tabs and move focus with them, which is what the
  // tablist pattern expects. Without this, Tab walks all three and the arrows do
  // nothing, so the roles would be promising behaviour that is not there.
  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const index = PANELS.findIndex((p) => p.id === active)
    let next = index
    if (event.key === 'ArrowRight') next = (index + 1) % PANELS.length
    else if (event.key === 'ArrowLeft') next = (index - 1 + PANELS.length) % PANELS.length
    else if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = PANELS.length - 1
    else return

    const target = PANELS[next]
    if (!target) return
    event.preventDefault()
    setActive(target.id)
    tabRefs.current[target.id]?.focus()
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex items-center gap-3 px-5 py-3">
        <h1 className="truncate text-base font-semibold">Workspace</h1>
      </header>

      <div
        role="tablist"
        aria-label="Workspace panels"
        onKeyDown={onKeyDown}
        className="mx-3 mb-3 flex gap-1 rounded-pill bg-surface-sunk p-1 lg:hidden"
      >
        {PANELS.map((panel) => {
          const selected = active === panel.id
          return (
            <button
              key={panel.id}
              ref={(node) => {
                tabRefs.current[panel.id] = node
              }}
              id={`tab-${panel.id}`}
              role="tab"
              type="button"
              aria-selected={selected}
              aria-controls={`panel-${panel.id}`}
              // Roving tabindex: only the active tab is in the tab order.
              tabIndex={selected ? 0 : -1}
              onClick={() => setActive(panel.id)}
              className={`btn flex-1 ${selected ? 'btn-secondary' : 'btn-quiet'}`}
            >
              {panel.label}
            </button>
          )
        })}
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-1 lg:grid-cols-[minmax(200px,1fr)_minmax(320px,2fr)_minmax(220px,1fr)]">
        {PANELS.map((panel) => (
          <section
            key={panel.id}
            id={`panel-${panel.id}`}
            role="tabpanel"
            aria-labelledby={`tab-${panel.id}`}
            className={`min-h-0 flex-col border-rule lg:flex lg:border-r lg:last:border-r-0 ${
              active === panel.id ? 'flex' : 'hidden'
            }`}
          >
            <div className="border-b border-rule px-4 py-2">
              <span className="label">{panel.label}</span>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto">
              <EmptyState title={panel.empty.title} reason={panel.empty.reason} />
            </div>
          </section>
        ))}
      </div>
    </div>
  )
}
