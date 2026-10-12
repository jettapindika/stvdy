import { useId, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { usePendingActions, useResolveAction, type ActionRow } from '@/api/queries/actions'
import { ApiErrorNotice } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { asApiError } from '@/api/errors'
import { AGENT_CREDITS } from '@/features/billing/plans'
import { formatInZone } from '@/lib/time'

/**
 * The confirmation card: what the assistant wants to do, what it costs, and the
 * student's answer.
 *
 * **A WRITE tool can only ever reach `GateDecision::Propose`** (A271) — the gate
 * has no arm that executes one — so this card is always rendering a *proposal*,
 * and the honesty rules bind it in three places:
 *
 *  1. the label is `honesty.suggested` until the proposal is confirmed, so the
 *     card can never read as though the thing had happened;
 *  2. `honesty.nothingAddedYet` is on the card, because the spec requires that
 *     exact sentence beside Confirm / Edit / Dismiss;
 *  3. the credit cost is shown **before** the confirm, read from `plans.ts` — the
 *     same file the server charges from — and never a literal (gate 7(d)).
 *
 * **The three states, and why the third one matters.** A `pending` proposal shows
 * its arguments, its cost and the controls. A `confirmed` one shows what the
 * execution produced and no controls. A `rejected` or `expired` one is **not
 * rendered at all**: an expired card that lingered would be a student tapping
 * confirm on something the server will refuse with a `409`, which is the one
 * failure this screen can cause by itself.
 *
 * **What it reads.** `GET /actions/pending`, never the synced `pending_actions`
 * cache — a proposal the student confirmed on their other phone must not still
 * look confirmable here, and the server's lazy expiry (A274) and single-winner
 * claim (A275) both live on that route. The card confirmed on another device
 * therefore updates in place: the list is invalidated on the stream's `action`
 * event and after every resolution, so a second device's tap lands here too.
 *
 * The edit is a JSON textarea rather than a per-tool form, and that is the honest
 * shape rather than the lazy one: the confirm re-parses an edit through the same
 * `parse_args` the proposal went through, so the edit's contract *is* the tool's
 * argument schema. A form with six hand-written fields per tool would be a second
 * copy of that schema, and the two would diverge the first time a tool's bounds
 * changed. An invalid edit comes back as a `400` naming the field, which is what
 * the notice under the field shows.
 */

/** The fields each write tool's card shows, in order.
 *
 * The keys are the tools' own argument names (`crates/core/src/agent/tools.rs`), so
 * a field the model set and this table does not name is still visible in the JSON
 * the edit field shows — nothing is hidden, and nothing is invented for a tool the
 * card does not know. The label each one carries is resolved as
 * `` t(`agent.field.${field}`) ``, which is the dynamic-prefix form the i18n
 * contract test understands: a static table of label keys would be copy nothing
 * could see was rendered.
 */
const WRITE_TOOL_FIELDS: Record<string, string[]> = {
  create_task: ['title', 'notes', 'due_at', 'estimate_min', 'priority'],
  update_task: ['title', 'notes', 'due_at', 'estimate_min', 'priority', 'status'],
  create_event: ['title', 'starts_at', 'ends_at'],
  schedule_study_block: ['title', 'starts_at', 'duration_min'],
  create_flashcards: ['title', 'count', 'source_ids'],
  create_quiz: ['title', 'count', 'difficulty'],
}

/**
 * The price of confirming this tool, or null when the matrix states none.
 *
 * Read from the exported table rather than written here, and read from the *same*
 * table for both generative writes: `flashcard_batch_10` and `quiz_10` are the
 * keys the deck and quiz routes spend under, so a change to `config/plans.toml`
 * reaches this button and the server together.
 */
export function actionCost(tool: string): number | null {
  if (tool === 'create_flashcards') return AGENT_CREDITS.flashcards
  if (tool === 'create_quiz') return AGENT_CREDITS.quiz
  return null
}

/** The instants and counts a proposal carries, rendered as a reader can check them. */
function FieldValue({ field, value, timeZone }: { field: string; value: unknown; timeZone: string }) {
  if (value === null || value === undefined) return null
  if (field === 'due_at' || field === 'starts_at' || field === 'ends_at') {
    const instant = new Date(String(value))
    if (Number.isNaN(instant.getTime())) return <>{String(value)}</>
    return <>{formatInZone(instant, timeZone)}</>
  }
  if (Array.isArray(value)) return <>{value.length}</>
  if (typeof value === 'object') return <>{JSON.stringify(value)}</>
  return <>{String(value)}</>
}

/**
 * One proposal.
 *
 * `timeZone` is the account's, so a due instant the model resolved in Jakarta reads
 * as the Jakarta evening the student typed rather than as a UTC instant.
 */
export function ActionCard({ action, timeZone }: { action: ActionRow; timeZone: string }) {
  const { t } = useTranslation()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(() => JSON.stringify(action.args, null, 2))
  const [editError, setEditError] = useState<string | null>(null)
  const resolve = useResolveAction()
  const editId = useId()
  const editErrorId = `${editId}-error`

  // A tool this build does not know renders its own name rather than nothing: the
  // registry is a closed enum and the server is the one that chooses, so a name
  // that arrives here is a server newer than this client, and showing it is the
  // honest fallback.
  const knownTool = Object.prototype.hasOwnProperty.call(WRITE_TOOL_FIELDS, action.tool)
  const what = knownTool ? t(`agent.tool.${action.tool}`) : action.tool
  const cost = actionCost(action.tool)
  const pending = action.status === 'pending'
  const failure = asApiError(resolve.confirm.error ?? resolve.reject.error)

  // The result of a confirmed execution, in whichever shape the executor recorded
  // it (`{"task": …}`, `{"event": …}`, `{"deck": …}`, `{"quiz": …}`). The row's own
  // title is what the student recognises; a result this cannot read shows no line
  // rather than a wrong one.
  let createdTitle: string | null = null
  if (action.result !== null) {
    for (const value of Object.values(action.result)) {
      if (typeof value !== 'object' || value === null) continue
      if ('title' in value && typeof value.title === 'string') {
        createdTitle = value.title
        break
      }
    }
  }

  /**
   * Confirms what is proposed, or the edit when one is open.
   *
   * An edit that is not a JSON object is refused **here** rather than sent: the
   * server would answer the same way, but silently confirming the original after
   * a typo would be the app doing something the student did not ask for. The
   * field keeps its text and the sentence says which of the two problems it is.
   */
  function confirm() {
    if (!editing) {
      setEditError(null)
      resolve.confirm.mutate({ id: action.id })
      return
    }
    let parsed: unknown
    try {
      parsed = JSON.parse(draft) as unknown
    } catch {
      setEditError(t('agent.card.editInvalidJson'))
      return
    }
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      setEditError(t('agent.card.editNotObject'))
      return
    }
    setEditError(null)
    resolve.confirm.mutate({ id: action.id, args: parsed as Record<string, unknown> })
  }

  return (
    <section
      aria-label={t('agent.card.label')}
      className="panel flex flex-col gap-3 px-4 py-4"
    >
      <div className="flex items-start gap-3">
        {/* The state is an icon **and** a word, never a colour alone: `clock` while
            the proposal is waiting, `check` once it is confirmed. Both carry their
            sentence beside them. */}
        <Icon name={pending ? 'clock' : 'check'} className="mt-0.5 shrink-0 text-muted" />
        <div className="flex min-w-0 flex-col gap-1">
          <p className="text-body font-medium">
            {pending
              ? t('honesty.suggested', { what })
              : t('agent.card.confirmed')}
          </p>
          {createdTitle !== null && (
            <p className="text-small text-muted">{t('agent.card.created', { title: createdTitle })}</p>
          )}
          {/* The server recorded why a confirmed action did not finish (the enqueue
              failed after the claim committed). It is shown rather than hidden: the
              row *was* created and the student needs to know the work did not start. */}
          {action.error !== null && (
            <p className="note note-danger text-small" role="status">
              {t('agent.card.failed', { error: action.error })}
            </p>
          )}
        </div>
      </div>

      {pending && (
        <dl className="flex flex-col gap-1">
          {(WRITE_TOOL_FIELDS[action.tool] ?? []).map((field) => {
            const value = action.args[field]
            if (value === null || value === undefined) return null
            return (
              <div key={field} className="flex flex-wrap gap-x-2">
                <dt className="text-small text-muted">{t(`agent.field.${field}`)}</dt>
                <dd className="min-w-0 text-small break-words">
                  <FieldValue field={field} value={value} timeZone={timeZone} />
                </dd>
              </div>
            )
          })}
        </dl>
      )}

      {/* The cost, before the action runs (gate 7(d)). A tool the matrix does not
          price says so, because "the matrix charges nothing" and "this is free
          today" are different claims. */}
      {pending && (
        <p className="meta">
          {cost === null ? t('agent.card.noCost') : t('agent.card.cost', { credits: cost })}
        </p>
      )}

      {pending && <p className="text-small text-muted">{t('honesty.nothingAddedYet')}</p>}

      {/* The card is time-limited and says so: a proposal the student finds after
          the TTL has passed is one the server will refuse, and the honest thing is
          to say when that happens rather than to let them find out. */}
      {pending && (
        <p className="meta">
          {t('agent.card.expires', {
            when: formatInZone(new Date(action.expires_at), timeZone, {
              dateStyle: 'medium',
              timeStyle: 'short',
            }),
          })}
        </p>
      )}

      {failure !== null && <ApiErrorNotice error={failure} noun={t('agent.card.noun')} />}

      {pending && editing && (
        <div className="flex flex-col gap-1">
          <label className="label" htmlFor={editId}>
            {t('agent.card.editLabel')}
          </label>
          <textarea
            id={editId}
            className="field min-h-32 font-mono text-small"
            value={draft}
            aria-describedby={editError === null ? `${editId}-hint` : `${editId}-hint ${editErrorId}`}
            aria-invalid={editError !== null}
            onChange={(event) => {
              setDraft(event.target.value)
              setEditError(null)
            }}
          />
          <span id={`${editId}-hint`} className="text-xs text-muted">
            {t('agent.card.editHint')}
          </span>
          {editError !== null && (
            <span id={editErrorId} className="note note-danger text-small" role="alert">
              {editError}
            </span>
          )}
        </div>
      )}

      {pending && (
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            className="btn btn-primary min-h-11"
            disabled={resolve.confirm.isPending}
            onClick={confirm}
          >
            {t('actions.confirm')}
          </button>
          <button
            type="button"
            className="btn btn-secondary min-h-11"
            onClick={() => {
              setDraft(JSON.stringify(action.args, null, 2))
              setEditError(null)
              setEditing((open) => !open)
            }}
          >
            {editing ? t('actions.cancel') : t('actions.edit')}
          </button>
          <button
            type="button"
            className="btn btn-quiet min-h-11"
            disabled={resolve.reject.isPending}
            onClick={() => {
              resolve.reject.mutate(action.id)
            }}
          >
            {t('actions.dismiss')}
          </button>
        </div>
      )}
    </section>
  )
}

/**
 * The proposals worth showing, mounted under the chat thread.
 *
 * Renders **nothing** when there is nothing pending — not an empty box, not a
 * heading — because an empty container on every chat screen would read as a
 * feature that is broken rather than one with nothing to say.
 *
 * `rejected` and `expired` proposals are filtered out here rather than rendered
 * with a state, and that is deliberate: the route returns the last day's
 * resolutions so a card confirmed on another device can show that it was, but a
 * rejected or expired one has no outcome to report and no control to offer. A
 * `rejected` card that stayed would be a "Dismiss" button on something already
 * dismissed.
 */
export function ActionCardList({ timeZone }: { timeZone: string }) {
  const { t } = useTranslation()
  const actions = usePendingActions()
  const failure = asApiError(actions.error)

  const shown = (actions.data ?? []).filter(
    (action) => action.status === 'pending' || action.status === 'confirmed',
  )

  if (shown.length === 0 && failure === null) return null

  return (
    <div className="flex flex-col gap-3 px-4 py-3" aria-label={t('agent.list.label')}>
      {failure !== null && <ApiErrorNotice error={failure} noun={t('agent.card.noun')} />}
      {shown.map((action) => (
        <ActionCard key={action.id} action={action} timeZone={timeZone} />
      ))}
    </div>
  )
}
