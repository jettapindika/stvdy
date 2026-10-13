import { useId, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { API_BASE_URL } from '@/api/client'
import { asApiError, validationMessage, type ApiError } from '@/api/errors'
import {
  feedUrl,
  googleConnectUrl,
  useAddIcs,
  useConnections,
  useFeedToken,
  useManualSync,
  useRegenerateFeedToken,
  useRemoveConnection,
  type Connection,
} from '@/api/queries/calendar'
import { useUsage } from '@/api/queries/usage'
import { Section } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { planForFeature } from '@/features/billing/plans'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { platform } from '@/platform'
import { useSession } from '@/features/auth/useSession'

/**
 * The calendar connections: the feed the student subscribes to, the ICS feeds they
 * subscribe to, and the Google account.
 *
 * **The plan split is the thing to get right here, and it is easy to get
 * backwards.** The spec's matrix gives the ICS feed and the ICS import to every
 * plan (`config/plans.toml`'s free block has them), and the Google connection is
 * pro-only (`google_calendar = false` on free, `google_accounts_max = 0`). So the
 * feed link, its Copy and Regenerate, and the "Add an ICS URL" field are
 * **ungated** — a `LockedFeature` around any of them would refuse a feature every
 * plan has — and the only lock on this panel is around "Connect Google Calendar",
 * whose plan comes from the matrix rather than from a literal here.
 *
 * **Gate 7 is why every status line is rendered rather than summarised.** A
 * connection holding an `error` says what the error is, beside the connection, in
 * words: the app must not report a connection as healthy while it holds a reason
 * it is not. "Last synced" is `last_synced_at` rendered — never a client-side
 * guess, and "never" rather than a date when it is null.
 *
 * **The Google URL is opened by the shell.** The route answers `{ url }` and this
 * panel hands it to `platform.openExternal`, i.e. the system browser. The spec
 * forbids an embedded webview for an OAuth consent screen by name, and it is also
 * the honest thing: the student can see the address bar of the page asking for
 * their calendar.
 *
 * Nothing here spends credits and nothing here is generative, so there is no cost
 * badge: a badge that always reads `0` is noise, and the honesty gate asks for the
 * absence rather than for a zero.
 */
export function ConnectionsPanel() {
  const { t } = useTranslation()
  const connections = useConnections()
  const error = asApiError(connections.error)

  const google = (connections.data ?? []).filter((row) => row.provider === 'google')
  const ics = (connections.data ?? []).filter((row) => row.provider === 'ics')

  return (
    <div className="flex flex-col gap-6">
      {/* The connected service first, then the two ways of feeding the calendar,
          then the evidence list — the order the design reads in. */}
      <Section title={t('calendar.connections.google.title')}>
        <GoogleSection existing={google} />
      </Section>

      <Section title={t('calendar.connections.feed.title')}>
        <FeedLink />
      </Section>

      <Section title={t('calendar.connections.ics.title')}>
        <IcsSection connections={ics} />
      </Section>

      <Section title={t('calendar.connections.status.title')}>
        {connections.isPending && (
          <SkeletonRows rows={1} label={t('calendar.connections.loading')} />
        )}
        {error !== null && <ApiErrorNotice error={error} />}
        {connections.data !== undefined && connections.data.length === 0 && (
          <EmptyState
            title={t('calendar.connections.empty.title')}
            reason={t('calendar.connections.empty.reason')}
          />
        )}
        {connections.data !== undefined && connections.data.length > 0 && (
          <ul className="card flex flex-col divide-y divide-border">
            {connections.data.map((connection) => (
              <li key={connection.id} className="flex flex-col gap-2 p-4">
                <ConnectionRow connection={connection} />
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  )
}

/**
 * A connection's status, as a word and the tone that repeats it.
 *
 * The status is a **word**, not a colour: `status` arrives as a string from the
 * server and is rendered as itself when this build does not recognise it, rather
 * than being mapped to a wrong label or hidden. An unrecognised value is a newer
 * server or a new provider, and showing it is more useful than showing nothing.
 * The tone repeats the word rather than replacing it, so the state survives a
 * monochrome screen and a screen reader.
 */
function useStatusChip(status: string): { word: string; className: string } {
  const { t } = useTranslation()
  if (status === 'connected') {
    return { word: t('calendar.connections.status.connected'), className: 'chip chip-ready' }
  }
  if (status === 'error') {
    return { word: t('calendar.connections.status.error'), className: 'chip chip-failed' }
  }
  if (status === 'revoked') {
    return { word: t('calendar.connections.status.revoked'), className: 'chip chip-due' }
  }
  return { word: status, className: 'chip' }
}

/**
 * `last_synced_at` rendered in the account's own zone, or the word for never.
 *
 * The instant is the server's and the zone is the student's: a connection that
 * has never run says "Never synced" rather than being given today's date, which
 * is the difference between a fact and a guess (A247).
 */
function useLastSynced(connection: Connection): string {
  const { t } = useTranslation()
  const session = useSession()
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  if (connection.last_synced_at === null) return t('calendar.connections.neverSynced')
  return formatInZone(new Date(connection.last_synced_at), timezone, {
    dateStyle: 'medium',
    timeStyle: 'short',
  })
}

/**
 * One connection's line: what it is, whether it is healthy, and when it last ran.
 */
function ConnectionRow({ connection }: { connection: Connection }) {
  const { t } = useTranslation()
  const remove = useRemoveConnection()
  const removeError = asApiError(remove.error)
  const status = useStatusChip(connection.status)
  const syncedAt = useLastSynced(connection)
  const lastSynced = t('calendar.connections.lastSynced', { when: syncedAt })

  const provider =
    connection.provider === 'google'
      ? t('calendar.connections.provider.google')
      : connection.provider === 'ics'
        ? t('calendar.connections.provider.ics')
        : connection.provider

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 break-words text-label font-medium">{provider}</span>
        {connection.account_email !== null && (
          <span className="meta min-w-0 break-words">{connection.account_email}</span>
        )}
        <span className={status.className}>{status.word}</span>
        <span className="meta min-w-0 break-words">{lastSynced}</span>
        <button
          type="button"
          className="btn btn-quiet"
          disabled={remove.isPending}
          onClick={() => {
            remove.mutate(connection.id)
          }}
        >
          {t('calendar.connections.disconnect')}
        </button>
      </div>
      {/* The stored reason, in the server's own words. A connection that holds an
          error is not healthy, and saying so is the honesty gate's whole demand
          here (A247). */}
      {connection.error !== null && (
        <p className="note note-warning break-words">{connection.error}</p>
      )}
      {removeError !== null && <ApiErrorNotice error={removeError} />}
    </div>
  )
}

/**
 * "Your calendar feed": the link, Copy, and Regenerate.
 *
 * **Ungated on purpose.** The feed is the matrix's `ICS feed + import` row, which
 * every plan has; a lock here would be the defect the brief warns about.
 *
 * The token is read rather than minted on mount: minting on read would rotate a
 * student's feed every time they opened this panel and break every client they had
 * already subscribed. So `null` is a real state and is rendered as one.
 *
 * The URL is a read-only `.field` with a real `<label>` rather than a paragraph of
 * monospace: it is the one secret this screen is required to show, and a field is
 * what a student can select, copy and paste into a calendar client without
 * knowing a keyboard shortcut. The Copy button reads `navigator.clipboard` and
 * reports what happened in words — a clipboard write can be refused (an insecure
 * context, a browser permission) and a button that silently did nothing is the
 * failure this avoids.
 *
 * Copy is the panel's **one primary action**, which is the design's own
 * assignment: its frame fills this button and outlines Regenerate, and the
 * connect and import controls below are secondary for that reason.
 */
function FeedLink() {
  const { t } = useTranslation()
  const token = useFeedToken()
  const regenerate = useRegenerateFeedToken()
  const [copied, setCopied] = useState(false)
  const error = asApiError(token.error) ?? asApiError(regenerate.error)

  const url = token.data == null ? null : feedUrl(API_BASE_URL, token.data)

  return (
    <div className="card flex flex-col gap-4 p-6">
      {/* The card's mark and its format, on one line. The name itself is the
          `Section`'s h2 directly above, so repeating it here would put two
          headings with the same words in the outline. */}
      <div className="flex flex-wrap items-center gap-3">
        <Icon name="link" size={24} className="shrink-0 text-muted" />
        <span className="eyebrow ml-auto">{t('calendar.connections.feed.format')}</span>
      </div>

      <p className="max-w-prose text-small text-muted">{t('calendar.connections.feed.hint')}</p>

      {token.isPending && <SkeletonRows rows={1} label={t('calendar.connections.loading')} />}
      {error !== null && <ApiErrorNotice error={error} />}

      {token.data === null && !token.isPending && (
        <p className="text-small text-muted">{t('calendar.connections.feed.none')}</p>
      )}

      {url !== null && (
        <>
          <div className="flex flex-col gap-2">
            <label className="label" htmlFor="feed-url">
              {t('calendar.connections.feed.label')}
            </label>
            <input
              id="feed-url"
              className="field font-mono"
              type="url"
              readOnly
              value={url}
              aria-describedby="feed-url-helper"
            />
            <p className="text-caption text-muted" id="feed-url-helper">
              {t('calendar.connections.feed.helper')}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                void navigator.clipboard
                  .writeText(url)
                  .then(() => {
                    setCopied(true)
                  })
                  .catch(() => {
                    setCopied(false)
                  })
              }}
            >
              {t('calendar.connections.feed.copy')}
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              disabled={regenerate.isPending}
              onClick={() => {
                setCopied(false)
                regenerate.mutate()
              }}
            >
              {t('calendar.connections.feed.regenerate')}
            </button>
            {copied && (
              <span className="meta" role="status">
                {t('calendar.connections.feed.copied')}
              </span>
            )}
          </div>
          {/* The two consequences of the link being a secret, said once, where
              both buttons are: it is the only key to the schedule, and rotating
              it strands every client already subscribed to the old one. */}
          <p className="flex items-start gap-2 border-t border-border pt-4">
            <Icon name="lock" size={16} className="mt-0.5 shrink-0 text-subtle" />
            <span className="min-w-0 text-caption text-muted">
              {t('calendar.connections.feed.private')}{' '}
              {t('calendar.connections.feed.regenerateHint')}
            </span>
          </p>
        </>
      )}

      {token.data === null && !token.isPending && (
        <button
          type="button"
          className="btn btn-primary self-start"
          disabled={regenerate.isPending}
          onClick={() => {
            regenerate.mutate()
          }}
        >
          {t('calendar.connections.feed.create')}
        </button>
      )}
    </div>
  )
}

/**
 * "Connect Google Calendar" — the one locked feature on this panel.
 *
 * The lock is decided by the plan matrix, not by a literal: `planForFeature`
 * reads `config/plans.toml`, so a retuned matrix moves this with it, and the
 * server refuses the same thing through the entitlement engine (A245). Until the
 * plan is known, nothing is offered — offering the button and then locking it
 * would be a control that changed its mind.
 *
 * **The connected account is stated as the server reports it, and the two sync
 * preferences are shown rather than offered.** `GET /calendar/connections` is the
 * only route here that carries them: it reports each connection's `status`, its
 * `account_email`, its `last_synced_at`, and — through `study_calendar_id` —
 * whether a Study Plan calendar is receiving the student's blocks. No route
 * writes either preference; the study calendar is created and named by the server
 * when the connection is made. A switch here would be a control whose only
 * outcome is a refusal, so the rows state the facts instead, with the word beside
 * the mark.
 *
 * **Every account gets its own block, and the status is not summarised across
 * them.** `google_accounts_max` is more than one on the paid plans, so a card
 * with a single status chip would report one account's health as all of them.
 */
function GoogleSection({ existing }: { existing: Connection[] }) {
  const { t } = useTranslation()
  const usage = useUsage()
  const sync = useManualSync()
  const [error, setError] = useState<ApiError | null>(null)
  const [opening, setOpening] = useState(false)
  const minPlan = planForFeature('google_calendar')

  const granted = usage.data?.features.google_calendar

  if (usage.data !== undefined && granted !== true) {
    if (minPlan === null) return null
    return (
      <LockedFeature
        whatKey="calendar.connections.google.title"
        minPlan={minPlan}
        reasonKey="calendar.connections.google.lockedReason"
      />
    )
  }

  const syncError = asApiError(sync.error)
  // The heading row carries the status when there is exactly one account to have
  // one. With more than one, a single chip in the heading would report one
  // account's health as all of them, so each account states its own instead.
  const soleAccount = existing.length === 1 ? existing : []

  return (
    <div className="card flex flex-col gap-4 p-6">
      {/* The service's own mark and status beside its description. The name is the
          `Section`'s h2 directly above, so repeating it here would put two
          headings with the same words in the outline. */}
      <div className="flex flex-wrap items-center gap-3">
        <span className="raised flex size-10 shrink-0 items-center justify-center text-primary-text">
          <Icon name="calendar" size={24} />
        </span>
        <p className="min-w-0 flex-1 text-small text-muted">
          {t('calendar.connections.google.hint')}
        </p>
        {soleAccount.map((connection) => (
          <GoogleStatus key={connection.id} connection={connection} />
        ))}
      </div>

      {existing.map((connection) => (
        <GoogleAccount
          key={connection.id}
          connection={connection}
          showStatus={existing.length > 1}
        />
      ))}

      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className="btn btn-secondary"
          disabled={opening}
          onClick={() => {
            setOpening(true)
            setError(null)
            void googleConnectUrl()
              .then((url) => platform.openExternal(url))
              .catch((failure: unknown) => {
                setError(asApiError(failure))
              })
              .finally(() => {
                setOpening(false)
              })
          }}
        >
          {t('calendar.connections.google.connect')}
        </button>
        {existing.length > 0 && (
          <button
            type="button"
            className="btn btn-quiet"
            disabled={sync.isPending}
            onClick={() => {
              sync.mutate()
            }}
          >
            {t('calendar.connections.syncNow')}
          </button>
        )}
      </div>
      {sync.data !== undefined && (
        // The honest report of a 202: jobs were queued, not a sync that ran. The
        // number is what the server inserted, so a second press inside the same
        // minute says 0 rather than claiming a second pass.
        <p className="text-small text-muted" role="status">
          {t('calendar.connections.syncQueued', { count: sync.data.queued })}
        </p>
      )}
      {error !== null && <ApiErrorNotice error={error} />}
      {syncError !== null && <ApiErrorNotice error={syncError} />}
    </div>
  )
}

/**
 * The status of a Google connection, as the chip the design puts in the card's
 * heading row when there is one account to have a status.
 */
function GoogleStatus({ connection }: { connection: Connection }) {
  const status = useStatusChip(connection.status)
  return <span className={status.className}>{status.word}</span>
}

/**
 * One connected Google account: which address it is, whether it is healthy, when
 * it last ran, and what it moves.
 *
 * An absent `account_email` is rendered as its own line rather than left blank: a
 * connection the server reports without an address is a fact the student should
 * see, and an empty inset would read as a rendering fault.
 *
 * `showStatus` is on when the card holds more than one account: the heading row
 * can carry a single status, and with several accounts the state belongs to the
 * account it is about.
 */
function GoogleAccount({
  connection,
  showStatus,
}: {
  connection: Connection
  showStatus: boolean
}) {
  const { t } = useTranslation()
  const status = useStatusChip(connection.status)
  const syncedAt = useLastSynced(connection)

  return (
    <div className="flex flex-col gap-3">
      <div className="raised flex flex-wrap items-start justify-between gap-4 rounded-field p-4">
        <div className="flex min-w-0 flex-col gap-1">
          <p className="eyebrow">{t('calendar.connections.google.account')}</p>
          <p className="min-w-0 break-words text-label font-medium">
            {connection.account_email ?? t('calendar.connections.google.noAccount')}
          </p>
        </div>
        <div className="flex min-w-0 flex-col gap-1">
          <p className="eyebrow">{t('calendar.connections.google.lastSynced')}</p>
          <p className="meta min-w-0 break-words">{syncedAt}</p>
        </div>
        {showStatus && <span className={status.className}>{status.word}</span>}
      </div>

      {/* The connection's two preferences, as the server's own facts: a connected
          account imports its events, and a non-null `study_calendar_id` is the
          Study Plan calendar the server made for this account. */}
      <SyncPreference
        title={t('calendar.connections.google.importTitle')}
        hint={t('calendar.connections.google.importHint')}
        on={connection.status === 'connected'}
      />
      <SyncPreference
        title={t('calendar.connections.google.studyTitle')}
        hint={t('calendar.connections.google.studyHint')}
        on={connection.study_calendar_id !== null}
      />

      {/* The stored reason, in the server's own words (A247) — the same rule the
          connections list keeps, stated here because this is where the account's
          health is read. */}
      {connection.error !== null && (
        <p className="note note-warning break-words">{connection.error}</p>
      )}
    </div>
  )
}

/**
 * One sync preference: what it does, and whether it is on.
 *
 * **Read-only, and that is the honest presentation.** The state is the server's
 * and no route writes it, so the mark is a disabled checkbox rather than a
 * switch: a control that refuses every press is worse than a state that says what
 * it is. The word beside it is what carries the state on screen — the mark alone
 * would be shape doing a word's job.
 *
 * The mark's **accessible name is the preference**, not that word: a screen
 * reader told "On, checkbox, checked, unavailable" learns nothing about what is
 * on, so the title is the name and the word stays visible beside the mark.
 */
function SyncPreference({ title, hint, on }: { title: string; hint: string; on: boolean }) {
  const { t } = useTranslation()
  const titleId = useId()
  return (
    <div className="flex min-h-11 flex-wrap items-center justify-between gap-3 border-t border-border pt-3">
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <p className="text-label font-medium" id={titleId}>
          {title}
        </p>
        <p className="text-small text-muted">{hint}</p>
      </div>
      <span className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={on}
          disabled
          readOnly
          tabIndex={-1}
          aria-labelledby={titleId}
        />
        <span className="text-label text-muted">
          {on ? t('calendar.connections.google.on') : t('calendar.connections.google.off')}
        </span>
      </span>
    </div>
  )
}

/**
 * "Add an ICS URL" — ungated, because every plan has the import.
 *
 * The field is a `<form>` with a `<label for>`, so the failure the server returns
 * (`validation`, with a readable reason) is shown on the field rather than at the
 * top of the page: the server's sentence is the actionable content for this code,
 * and it belongs next to the box the student just typed in.
 */
function IcsSection({ connections }: { connections: Connection[] }) {
  const { t } = useTranslation()
  const add = useAddIcs()
  const [url, setUrl] = useState('')
  const [notice, setNotice] = useState<string | null>(null)
  const error = asApiError(add.error)

  const shown =
    error === null
      ? null
      : error.code === 'validation'
        ? validationMessage(error, t, ['url']).message
        : null

  return (
    <div className="card flex flex-col gap-4 p-6">
      <p className="max-w-prose text-small text-muted">{t('calendar.connections.ics.hint')}</p>

      {connections.length > 0 && (
        <ul className="flex flex-col gap-1">
          {connections.map((connection) => (
            <li key={connection.id} className="meta min-w-0 break-words">
              {t('calendar.connections.ics.connected', { id: connection.id.slice(0, 8) })}
            </li>
          ))}
        </ul>
      )}

      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          setNotice(null)
          add.mutate(url.trim(), {
            onSuccess: () => {
              setUrl('')
              setNotice(t('calendar.connections.ics.queued'))
            },
          })
        }}
      >
        {/* `min-w-0` and not a rem minimum. `min-w-60` is `15rem`, which is 240px
            at a 16px root and **480px at a 32px root** — wider than a 390px phone,
            so at 200% text the field alone made the whole page scroll sideways
            (measured: 626px of `scrollWidth` against a 390px `clientWidth`, both
            themes, and no overflow at all at 100%). `flex-1` already gives the
            field the row; a minimum width is what a phone cannot afford. */}
        <label className="flex min-w-0 flex-1 flex-col gap-1" htmlFor="ics-url">
          <span className="label">{t('calendar.connections.ics.label')}</span>
          <input
            id="ics-url"
            className="field"
            type="url"
            inputMode="url"
            value={url}
            placeholder="https://"
            onChange={(event) => {
              setUrl(event.target.value)
            }}
          />
        </label>
        <button
          type="submit"
          className="btn btn-secondary"
          disabled={add.isPending || url.trim() === ''}
        >
          {t('calendar.connections.ics.add')}
        </button>
      </form>

      {shown !== null && (
        <p className="note note-danger" role="alert">
          {shown}
        </p>
      )}
      {error !== null && shown === null && <ApiErrorNotice error={error} />}
      {notice !== null && (
        <p className="text-small text-muted" role="status">
          {notice}
        </p>
      )}
    </div>
  )
}
