import { useState } from 'react'
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
      <Section title={t('calendar.connections.feed.title')}>
        <FeedLink />
      </Section>

      <Section title={t('calendar.connections.google.title')}>
        <GoogleSection existing={google} />
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
          <ul className="flex flex-col divide-y divide-border border-y border-border">
            {connections.data.map((connection) => (
              <li key={connection.id} className="flex flex-col gap-2 py-3">
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
 * One connection's line: what it is, whether it is healthy, and when it last ran.
 *
 * The status is a **word**, not a colour: `status` arrives as a string from the
 * server and is rendered as itself when this build does not recognise it, rather
 * than being mapped to a wrong label or hidden. An unrecognised value is a newer
 * server or a new provider, and showing it is more useful than showing nothing.
 */
function ConnectionRow({ connection }: { connection: Connection }) {
  const { t } = useTranslation()
  const session = useSession()
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  const remove = useRemoveConnection()
  const removeError = asApiError(remove.error)

  const provider =
    connection.provider === 'google'
      ? t('calendar.connections.provider.google')
      : connection.provider === 'ics'
        ? t('calendar.connections.provider.ics')
        : connection.provider

  const status =
    connection.status === 'connected'
      ? t('calendar.connections.status.connected')
      : connection.status === 'error'
        ? t('calendar.connections.status.error')
        : connection.status === 'revoked'
          ? t('calendar.connections.status.revoked')
          : connection.status

  const lastSynced =
    connection.last_synced_at === null
      ? t('calendar.connections.neverSynced')
      : t('calendar.connections.lastSynced', {
          when: formatInZone(new Date(connection.last_synced_at), timezone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
        })

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 break-words font-medium">{provider}</span>
        {connection.account_email !== null && (
          <span className="meta min-w-0 break-words">{connection.account_email}</span>
        )}
        <span className="chip">{status}</span>
        <span className="meta">{lastSynced}</span>
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
 * The Copy button reads `navigator.clipboard` and reports what happened in words.
 * A clipboard write can be refused (an insecure context, a browser permission) and
 * a button that silently did nothing is the failure this avoids; the URL is also
 * rendered as selectable text, so the copy is a convenience rather than the only
 * way to get it.
 */
function FeedLink() {
  const { t } = useTranslation()
  const token = useFeedToken()
  const regenerate = useRegenerateFeedToken()
  const [copied, setCopied] = useState(false)
  const error = asApiError(token.error) ?? asApiError(regenerate.error)

  const url = token.data == null ? null : feedUrl(API_BASE_URL, token.data)

  return (
    <div className="card flex flex-col gap-3 p-4">
      <p className="max-w-prose text-small text-muted">{t('calendar.connections.feed.hint')}</p>

      {token.isPending && <SkeletonRows rows={1} label={t('calendar.connections.loading')} />}
      {error !== null && <ApiErrorNotice error={error} />}

      {token.data === null && !token.isPending && (
        <p className="text-small text-muted">{t('calendar.connections.feed.none')}</p>
      )}

      {url !== null && (
        <>
          <p className="min-w-0 break-all font-mono text-caption text-muted" id="feed-url">
            {url}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              className="btn btn-secondary"
              aria-describedby="feed-url"
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
              className="btn btn-quiet"
              disabled={regenerate.isPending}
              onClick={() => {
                setCopied(false)
                regenerate.mutate()
              }}
            >
              {t('calendar.connections.feed.regenerate')}
            </button>
            {copied && <span role="status">{t('calendar.connections.feed.copied')}</span>}
          </div>
          {/* Rotation's consequence, stated where the button is: the old URL
              stops working, so every client subscribed with it must be given the
              new one. */}
          <p className="max-w-prose text-caption text-subtle">
            {t('calendar.connections.feed.regenerateHint')}
          </p>
        </>
      )}

      {token.data === null && !token.isPending && (
        <button
          type="button"
          className="btn btn-secondary self-start"
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

  return (
    <div className="card flex flex-col gap-3 p-4">
      <p className="max-w-prose text-small text-muted">
        {t('calendar.connections.google.hint')}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className="btn btn-primary"
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
    <div className="card flex flex-col gap-3 p-4">
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
        <button type="submit" className="btn btn-primary" disabled={add.isPending || url.trim() === ''}>
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
