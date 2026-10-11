import { useState } from 'react'

import { EmptyState } from '@/components/ui/States'

/**
 * The workspace view: Sources, Chat, and Studio.
 *
 * Composition differs by form factor on purpose (RHYTHM 2). Wide screens get all
 * three panels side by side, because reading a source while asking about it is
 * the actual work. Phones get one panel at a time behind a switcher, because
 * three columns on a 390px screen would be three unusable columns.
 *
 * The panels are separated by hairlines rather than by cards with shadows: they
 * are regions of one surface, not three floating objects (R-12).
 */

type Panel = 'sources' | 'chat' | 'studio'

const PANEL_LABELS: Record<Panel, string> = {
  sources: 'Sources',
  chat: 'Chat',
  studio: 'Studio',
}

export function WorkspaceScreen() {
  const [panel, setPanel] = useState<Panel>('chat')

  return (
    <div className="flex h-[100dvh] flex-col">
      <header className="flex items-center gap-3 border-b border-rule px-5 py-3">
        <h1 className="truncate text-base font-semibold">Workspace</h1>
      </header>

      {/* Phone: one panel at a time. These are real tabs, so the control is a
          real control and switching works (R-26). */}
      <div
        role="tablist"
        aria-label="Workspace panels"
        className="flex gap-1 border-b border-rule px-3 py-2 lg:hidden"
      >
        {(Object.keys(PANEL_LABELS) as Panel[]).map((key) => (
          <button
            key={key}
            role="tab"
            type="button"
            aria-selected={panel === key}
            onClick={() => setPanel(key)}
            className={`btn ${panel === key ? 'btn-secondary' : 'btn-quiet'} flex-1`}
          >
            {PANEL_LABELS[key]}
          </button>
        ))}
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-1 lg:grid-cols-[minmax(200px,1fr)_minmax(320px,2fr)_minmax(220px,1fr)]">
        <Panel region="sources" active={panel} title="Sources">
          <EmptyState
            title="No sources"
            reason="Add a PDF, slide deck, document, or link. Each one is chunked and indexed, so a question can be answered from any of them and cite the page it came from."
          />
        </Panel>

        <Panel region="chat" active={panel} title="Chat">
          <EmptyState
            title="Ask about this workspace"
            reason="Answers come from your sources first, and every claim carries a citation back to the page it came from. Turn on web search to bring in outside results, cited separately."
          />
        </Panel>

        <Panel region="studio" active={panel} title="Studio">
          <EmptyState
            title="Nothing generated yet"
            reason="From the selected sources you can generate a flashcard deck, a quiz, a report, or a print-ready cheatsheet. Each one keeps the link back to the material it came from."
          />
        </Panel>
      </div>
    </div>
  )
}

function Panel({
  region,
  active,
  title,
  children,
}: {
  region: Panel
  active: Panel
  title: string
  children: React.ReactNode
}) {
  return (
    <section
      aria-label={title}
      // Hidden below `lg` unless it is the active tab. `hidden` rather than
      // unmounting keeps the panel's state alive across switches.
      className={`min-h-0 flex-col border-rule lg:flex lg:border-r lg:last:border-r-0 ${
        active === region ? 'flex' : 'hidden'
      }`}
    >
      <div className="border-b border-rule px-4 py-2">
        <span className="label">{title}</span>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
    </section>
  )
}
