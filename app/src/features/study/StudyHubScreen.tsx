import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'
import { useQueries } from '@tanstack/react-query'

import { api } from '@/api/client'
import { asApiError } from '@/api/errors'
import { useDashboard } from '@/api/queries/dashboard'
import { flashcardKeys, useDeleteDeck, type Deck, type DecksResponse } from '@/api/queries/flashcards'
import { quizKeys, type QuizzesResponse } from '@/api/queries/quizzes'
import { useWorkspaces } from '@/api/queries/workspaces'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { formatCount } from '@/features/billing/CreditMeter'
import { DEFAULT_TIMEZONE, formatInZone, localDateKey } from '@/lib/time'
import { useSession } from '@/features/auth/useSession'

/**
 * The Study hub: every deck and every quiz the account holds, in one place.
 *
 * `SPEC.md:568` asks for it by name — "Study hub: decks across workspaces with due
 * badges, recent quizzes with scores, and a single 'Review all due' action" — and
 * the design gives it a top-level nav item and a phone tab, so the nav's third
 * destination needed a screen rather than a dead link.
 *
 * **The fan-out is the honest cost of the API's shape.** Decks and quizzes are
 * per-workspace endpoints (`/workspaces/:id/decks`, `/workspaces/:id/quizzes`),
 * and there is no cross-workspace list. Rather than invent one on the client or add
 * a server route that would duplicate two existing handlers, this screen asks once
 * per workspace for each — `2 x workspaces` requests, all of them cached under the
 * same keys the workspace screen uses, so arriving here and then opening a course
 * is warm.
 *
 * **The due figure is account-wide, and it is the one the API has.** There is no
 * cross-deck due endpoint to badge each row with, and `GET /decks/:id/due` per deck
 * would fetch every due card's front and back to render a count. `GET /dashboard`
 * already counts due cards across the account for exactly this question ("what
 * should I do next"), so the badge is that number and the copy says so; a deck row
 * shows how many cards it holds and whether it is still generating, both facts the
 * row carries, and the *queue* is what `StudyScreen` computes when it opens.
 *
 * **"Review a deck" opens the largest ready deck, and says which one.** There is no
 * cross-deck queue, so a merged session is not a thing this screen can start; the
 * deck with the most cards is the one that can teach the most, the choice is stable
 * across renders, and the line under the badge names the deck it will open. When
 * the account has nothing due the action is absent rather than pointing at a
 * session that would end immediately.
 */
export function StudyHubScreen() {
  const { t, i18n } = useTranslation()
  const session = useSession()
  const workspaces = useWorkspaces()
  const rows = workspaces.data ?? []
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  // The dashboard's window is the student's own local day. Only its due count is
  // read here, and that one is "now" rather than a window's, but the route takes a
  // window and a date computed in the account's zone is what keeps the request
  // honest about which day it is asking about.
  const today = localDateKey(new Date(), timezone)
  const dashboard = useDashboard(today, today)

  const deckQueries = useQueries({
    queries: rows.map((workspace) => ({
      queryKey: flashcardKeys.decks(workspace.id),
      queryFn: async ({ signal }: { signal: AbortSignal }) =>
        api.get<DecksResponse>(`/workspaces/${workspace.id}/decks`, { signal }),
    })),
  })
  const quizQueries = useQueries({
    queries: rows.map((workspace) => ({
      queryKey: quizKeys.quizzes(workspace.id),
      queryFn: async ({ signal }: { signal: AbortSignal }) =>
        api.get<QuizzesResponse>(`/workspaces/${workspace.id}/quizzes`, { signal }),
    })),
  })

  const courseOf = new Map(rows.map((workspace) => [workspace.id, workspace.name]))

  // Every deck the account holds, flattened. `due_count` is not on the row, so the
  // count shown is the deck's card count and the badge is its status — both facts
  // the row carries.
  const decks = deckQueries.flatMap((result) =>
    (result.data?.decks ?? []).map((deck) => ({ deck, course: courseOf.get(deck.workspace_id) })),
  )
  const quizzes = quizQueries.flatMap((result) =>
    (result.data?.quizzes ?? []).map((quiz) => ({
      quiz,
      course: courseOf.get(quiz.workspace_id),
    })),
  )

  // Every query this screen owns counts towards loading, including the quizzes:
  // gating on the decks alone would render the empty state for a second while the
  // quizzes were still arriving, and then replace it with a list.
  const loading =
    workspaces.isPending ||
    dashboard.isPending ||
    deckQueries.some((result) => result.isPending) ||
    quizQueries.some((result) => result.isPending)

  // One notice for the screen's reads. A workspace whose decks failed leaves the
  // rest of the list standing — the failure is reported above it rather than
  // replacing it, and the retry asks every failed read again.
  const failure =
    asApiError(workspaces.error) ??
    asApiError(dashboard.error) ??
    deckQueries.map((result) => asApiError(result.error)).find((error) => error !== null) ??
    quizQueries.map((result) => asApiError(result.error)).find((error) => error !== null) ??
    null
  const empty = !loading && failure === null && decks.length === 0 && quizzes.length === 0

  // The deck to open for "Review a deck": the largest one that is ready. Ties go
  // to the earlier row, so the choice is stable across renders. A ready deck with
  // no cards is not a session.
  const reviewTarget = decks
    .filter((entry) => entry.deck.status === 'ready' && entry.deck.card_count > 0)
    .sort((left, right) => right.deck.card_count - left.deck.card_count)[0]

  // `null` until the count has arrived, so an absent figure is never rendered as a
  // zero. A real zero is a fact, and it is what stops the primary action offering a
  // session with nothing in it.
  const dueCount = dashboard.data?.totals.cards_due ?? null

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-8 px-5 md:px-[32px] py-8">
      <PageHeader
        eyebrow={t('study.hub.eyebrow')}
        title={t('nav.study')}
        subtitle={t('study.hub.subtitle')}
        action={
          reviewTarget !== undefined && dueCount !== 0 ? (
            <Link to={`/study/${reviewTarget.deck.id}`} className="btn btn-primary">
              {t('study.hub.reviewAll')}
            </Link>
          ) : undefined
        }
      />

      {loading && <SkeletonRows rows={3} label={t('study.hub.loading')} />}

      {failure !== null && (
        <ApiErrorNotice
          error={failure}
          noun={t('errors.nouns.decks')}
          action={
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => {
                void workspaces.refetch()
                void dashboard.refetch()
                for (const result of [...deckQueries, ...quizQueries]) void result.refetch()
              }}
            >
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {/* The priority block: what is due across the account, and the deck the one
          action opens. It is `.raised` because it is the block that decides whether
          the student studies now or later. */}
      {!loading && failure === null && dueCount !== null && (
        <div className="raised flex flex-wrap items-baseline gap-x-3 gap-y-1 p-4">
          <p className="text-h4">
            {dueCount === 0
              ? t('study.hub.nothingDue')
              : t('study.hub.dueNow', { count: dueCount })}
          </p>
          {reviewTarget !== undefined && dueCount > 0 && (
            <p className="meta">{t('study.hub.reviewOpens', { deck: reviewTarget.deck.title })}</p>
          )}
        </div>
      )}

      {empty && (
        <EmptyState
          title={t('study.hub.empty.title')}
          reason={t('study.hub.empty.reason')}
          action={
            <Link to="/workspaces" className="btn btn-primary">
              {t('nav.workspaces')}
            </Link>
          }
        />
      )}

      {decks.length > 0 && (
        <Section title={t('study.hub.decks')} count={decks.length}>
          <div className="card">
            <ul>
              {decks.map(({ deck, course }) => (
                <HubDeckRow key={deck.id} deck={deck} course={course} />
              ))}
            </ul>
          </div>
        </Section>
      )}

      {quizzes.length > 0 && (
        <Section title={t('study.hub.quizzes')} count={quizzes.length}>
          <div className="card">
            <ul>
              {quizzes.map(({ quiz, course }) => (
                <li key={quiz.id} className="border-b border-border last:border-b-0">
                  <Link
                    to={`/quiz/${quiz.id}`}
                    className="flex min-h-[4.625rem] flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 hover:bg-surface-raised"
                  >
                    <span className="flex size-10 shrink-0 items-center justify-center rounded-field bg-surface-raised">
                      <Icon name="list-checks" className="text-primary-text" />
                    </span>
                    {/* The same `min-w-28` as the deck row above, for the same reason. */}
                    <span className="flex min-w-[min(7rem,100%)] flex-1 flex-col gap-1">
                      <span className="truncate text-label font-medium" title={quiz.title}>
                        {quiz.title}
                      </span>
                      <span className="meta truncate" title={course ?? undefined}>
                        {course ? `${course} · ` : ''}
                        {formatInZone(new Date(quiz.created_at), timezone, { dateStyle: 'medium' })}
                      </span>
                    </span>
                    {/* The difficulty is nullable on the wire (a quiz created before
                        the column carried a value has none), so it is rendered only
                        when present rather than resolving a key that would read
                        `difficulty.null`. */}
                    {quiz.difficulty !== null && (
                      <span className="chip">{t(`studio.quiz.difficulty.${quiz.difficulty}`)}</span>
                    )}
                  </Link>
                </li>
              ))}
            </ul>
          </div>
        </Section>
      )}

      {/* The count of what is in the account, which is the one number this screen
          can state without a per-deck request. */}
      {!loading && (decks.length > 0 || quizzes.length > 0) && (
        <p className="meta">
          {t('study.hub.totals', {
            decks: formatCount(decks.length, i18n.language),
            quizzes: formatCount(quizzes.length, i18n.language),
          })}
        </p>
      )}
    </div>
  )
}

/**
 * One deck row in the Study hub, with an inline delete action.
 *
 * The link covers the icon, title and chip; the delete button sits beside it
 * so a click on it does not navigate. The confirm block replaces the row
 * content, following the same pattern as `WorkspaceCard`'s delete.
 */
function HubDeckRow({ deck, course }: { deck: Deck; course: string | undefined }) {
  const { t } = useTranslation()
  const remove = useDeleteDeck(deck.workspace_id)
  const removeError = asApiError(remove.error)
  const [confirming, setConfirming] = useState(false)
  const displayName = deck.title.length > 0 ? deck.title : t('studio.deck.untitled')

  if (confirming) {
    return (
      <li className="border-b border-border px-4 py-3 last:border-b-0">
        <div className="note note-danger flex flex-col items-start gap-2">
          <p className="font-medium">
            {t('studio.deck.deleteConfirm', { name: displayName })}
          </p>
          <p className="text-small">
            {deck.card_count === 0
              ? t('studio.deck.deleteNoCards')
              : t('studio.deck.deleteCards', { count: deck.card_count })}
          </p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-secondary"
              autoFocus
              disabled={remove.isPending}
              onClick={() => {
                remove.mutate(deck.id, {
                  onSuccess: () => { setConfirming(false) },
                })
              }}
            >
              {remove.isPending
                ? t('studio.deck.deleting')
                : t('studio.deck.deleteIt')}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                remove.reset()
                setConfirming(false)
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
        </div>
        {removeError !== null && (
          <ApiErrorNotice error={removeError} noun={t('errors.nouns.deck')} />
        )}
      </li>
    )
  }

  return (
    <li className="flex items-center border-b border-border last:border-b-0">
      <Link
        to={`/study/${deck.id}`}
        className="flex min-h-[4.625rem] min-w-0 flex-1 flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 hover:bg-surface-raised"
      >
        <span className="flex size-10 shrink-0 items-center justify-center rounded-field bg-surface-raised">
          <Icon name="cards" className="text-primary-text" />
        </span>
        <span className="flex min-w-[min(7rem,100%)] flex-1 flex-col gap-1">
          <span className="truncate text-label font-medium" title={deck.title}>
            {deck.title}
          </span>
          {course && (
            <span className="meta truncate" title={course}>
              {course}
            </span>
          )}
        </span>
        <span className={deck.status === 'ready' ? 'chip' : 'chip chip-working'}>
          {deck.status === 'ready'
            ? t('study.hub.cardCount', { count: deck.card_count })
            : t('study.hub.generating')}
        </span>
      </Link>
      <button
        type="button"
        className="btn btn-quiet mr-2 text-danger text-sm"
        onClick={() => { setConfirming(true) }}
      >
        {t('actions.delete')}
      </button>
    </li>
  )
}
