import { useState } from 'react'
import { Link } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCreateDeck,
  useDecks,
  type Deck,
  type DeckStatus,
} from '@/api/queries/flashcards'
import { useSources } from '@/api/queries/sources'
import { Field } from '@/components/ui/Field'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { SourcePicker } from '@/features/workspaces/SourcePicker'

/**
 * Studio → Flashcards.
 *
 * Generating is asynchronous and costs credits, so the panel says both things
 * plainly: the button names the cost before it is pressed, and a deck in flight
 * shows its status rather than a spinner that never resolves. The count is a
 * number input rather than a preset because the plan's monthly allowance is the
 * real limit, and the refusal comes from the server with the numbers in it.
 */

const STATUS_LABEL: Record<DeckStatus, string> = {
  generating: 'Generating',
  ready: 'Ready',
  failed: 'Failed',
}

const STATUS_CHIP: Record<DeckStatus, string> = {
  generating: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

export function FlashcardsPanel({ workspaceId }: { workspaceId: string }) {
  const sources = useSources(workspaceId)
  const decks = useDecks(workspaceId)
  const create = useCreateDeck(workspaceId)
  const [selected, setSelected] = useState<string[]>([])
  const [title, setTitle] = useState('')
  const [count, setCount] = useState(10)

  const readySources = (sources.data ?? []).filter((source) => source.status === 'ready')
  const error = asApiError(create.error)
  const ready = title.trim().length > 0 && selected.length > 0 && !create.isPending
  const decksList = decks.data?.decks
  // The cost and the request cap are the server's: the panel renders what
  // `GET /workspaces/:id/decks` serves rather than a literal that can drift from
  // the plan matrix.
  const cost = decks.data?.cost

  return (
    <div className="flex flex-col gap-4 px-4 py-4">
      <form
        className="panel flex flex-col gap-3 px-3 py-3"
        onSubmit={(event) => {
          event.preventDefault()
          if (!ready) return
          create.mutate(
            { title: title.trim(), sourceIds: selected, count },
            {
              onSuccess: () => {
                setTitle('')
                setSelected([])
              },
            },
          )
        }}
      >
        <Field
          label="Deck name"
          name="deck-title"
          value={title}
          hint="What this deck is for."
          onChange={(event) => {
            setTitle(event.target.value)
          }}
        />
        <SourcePicker
          workspaceId={workspaceId}
          sources={readySources}
          selected={selected}
          onChange={setSelected}
          disabled={create.isPending}
          legend="Generate from"
          emptyHint="No indexed sources yet. Add one in the Sources panel, then come back."
        />
        <label className="label" htmlFor="deck-count">
          How many cards
        </label>
        <input
          id="deck-count"
          className="field"
          type="number"
          min={1}
          max={cost?.max_cards}
          value={count}
          onChange={(event) => {
            const next = event.target.valueAsNumber
            setCount(Number.isNaN(next) ? 0 : next)
          }}
        />
        {cost !== undefined && (
          <p className="text-xs text-ink-muted">
            Costs {cost.per_batch} credits per {cost.batch_size} cards. The deck appears here while
            it is generated, usually within a minute.
          </p>
        )}
        <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
          {create.isPending ? 'Starting' : 'Generate deck'}
        </button>
        {error !== null && <ApiErrorNotice error={error} noun="decks" />}
      </form>

      {decks.isPending && <SkeletonRows rows={2} label="Loading this workspace's decks" />}

      {decksList !== undefined && decksList.length === 0 && (
        <EmptyState
          title="No decks yet"
          reason="Pick the sources above and generate one. Every card is linked to the passage it came from."
        />
      )}

      {decksList !== undefined && decksList.length > 0 && (
        <ul className="flex flex-col divide-y divide-rule border-y border-rule">
          {decksList.map((deck) => (
            <DeckRow key={deck.id} deck={deck} />
          ))}
        </ul>
      )}
    </div>
  )
}

/**
 * One generated deck in the list.
 *
 * The chip carries its state in a word, not only in a tone — the same rule the
 * source rows follow — so "Generating" and "Failed" are still readable in
 * monochrome and to a screen reader.
 */
function DeckRow({ deck }: { deck: Deck }) {
  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 flex-1 break-words font-medium">
          {deck.title.length > 0 ? deck.title : 'Untitled deck'}
        </span>
        <span className="meta">
          {deck.card_count} {deck.card_count === 1 ? 'card' : 'cards'}
        </span>
        <span className={STATUS_CHIP[deck.status]} role="status">
          {deck.status === 'generating' && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {STATUS_LABEL[deck.status]}
        </span>
        {deck.status === 'ready' && (
          <Link to={`/study/${deck.id}`} className="btn btn-secondary">
            Study
          </Link>
        )}
      </div>

      {deck.error !== null && deck.error.length > 0 && (
        <p className="note note-danger text-sm">{deck.error}</p>
      )}
    </li>
  )
}
