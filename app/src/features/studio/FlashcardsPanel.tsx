import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCreateDeck,
  useDecks,
  useDeleteDeck,
  useRenameDeck,
  type Deck,
  type DeckStatus,
} from '@/api/queries/flashcards'
import { useSources } from '@/api/queries/sources'
import { useBulkDelete, useReorderContent } from '@/api/queries/workspaces'
import { Field } from '@/components/ui/Field'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { ContentList, type RowBindings } from '@/features/studio/ContentList'
import { ContentRow } from '@/features/studio/ContentRow'
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

const STATUS_CHIP: Record<DeckStatus, string> = {
  generating: 'chip chip-working',
  ready: 'chip chip-ready',
  failed: 'chip chip-failed',
}

export function FlashcardsPanel({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
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
  const bulk = useBulkDelete(workspaceId)
  const bulkError = asApiError(bulk.error)
  const reorder = useReorderContent(workspaceId)
  // The cost and the request cap are the server's: the panel renders what
  // `GET /workspaces/:id/decks` serves rather than a literal that can drift from
  // the plan matrix.
  const cost = decks.data?.cost

  return (
    <div className="flex flex-col gap-5 px-5 py-5 md:px-4 md:py-4">
      <form
        className="panel flex flex-col gap-3 px-4 py-4"
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
          label={t('studio.deck.nameLabel')}
          name="deck-title"
          value={title}
          hint={t('studio.deck.nameHint')}
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
          legend={t('studio.generateFrom')}
          emptyHint={t('studio.noIndexedSources')}
        />
        <label className="label" htmlFor="deck-count">
          {t('studio.deck.countLabel')}
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
          <p className="text-xs text-muted">
            {t('studio.deck.cost', { credits: cost.per_batch, count: cost.batch_size })}
          </p>
        )}
        <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
          {create.isPending ? t('studio.starting') : t('studio.deck.generate')}
        </button>
        {error !== null && <ApiErrorNotice error={error} noun={t('errors.nouns.decks')} />}
      </form>

      {decks.isPending && <SkeletonRows rows={2} label={t('studio.deck.loading')} />}

      {decksList !== undefined && decksList.length === 0 && (
        <EmptyState title={t('studio.deck.empty.title')} reason={t('studio.deck.empty.reason')} />
      )}

      {decksList !== undefined && decksList.length > 0 && (
        <ContentList
          items={decksList}
          noun={t('errors.nouns.decks')}
          onBulkDelete={(ids) => {
            bulk.mutate({ deckIds: ids })
          }}
          bulkPending={bulk.isPending}
          bulkError={bulkError}
          onReorder={async (ids) => {
            await reorder.mutateAsync({ deckIds: ids })
          }}
          row={(deck, _index, bindings) => (
            <DeckRow key={deck.id} deck={deck} workspaceId={workspaceId} bindings={bindings} />
          )}
        />
      )}
    </div>
  )
}

/**
 * One generated deck in the list.
 *
 * The chip carries its state in a word, not only in a tone — the same rule the
 * source rows follow — so "Generating" and "Failed" are still readable in
 * monochrome and to a screen reader. The row's own layout and its rename/delete
 * modes live in `ContentRow`, which the four panels share.
 */
function DeckRow({
  deck,
  workspaceId,
  bindings,
}: {
  deck: Deck
  workspaceId: string
  bindings: RowBindings
}) {
  const { t } = useTranslation()
  const remove = useDeleteDeck(workspaceId)
  const rename = useRenameDeck(workspaceId)
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const statusLabel: Record<DeckStatus, string> = {
    generating: t('studio.status.generating'),
    ready: t('studio.status.ready'),
    failed: t('studio.status.failed'),
  }
  const displayName = deck.title.length > 0 ? deck.title : t('studio.deck.untitled')
  return (
    <ContentRow
      title={displayName}
      meta={<span className="meta">{t('studio.deck.cards', { count: deck.card_count })}</span>}
      statusChip={STATUS_CHIP[deck.status]}
      statusLabel={statusLabel[deck.status]}
      working={deck.status === 'generating'}
      action={
        deck.status === 'ready' ? (
          <Link to={`/study/${deck.id}`} className="btn btn-secondary">
            {t('nav.study')}
          </Link>
        ) : undefined
      }
      error={deck.error}
      rename={{
        label: t('studio.deck.rename'),
        onSubmit: (title) => {
          rename.mutate({ deckId: deck.id, title })
        },
        pending: rename.isPending,
        error: asApiError(rename.error),
        reset: rename.reset,
      }}
      deleteConfirm={t('studio.deck.deleteConfirm', { name: displayName })}
      deleteBody={
        deck.card_count === 0
          ? t('studio.deck.deleteNoCards')
          : t('studio.deck.deleteCards', { count: deck.card_count })
      }
      deleteIt={t('studio.deck.deleteIt')}
      deleting={t('studio.deck.deleting')}
      deleteNoun={t('errors.nouns.deck')}
      onDelete={(onSuccess) => {
        remove.mutate(deck.id, { onSuccess })
      }}
      deletePending={remove.isPending}
      deleteError={asApiError(remove.error)}
      resetDelete={remove.reset}
      bindings={bindings}
    />
  )
}
