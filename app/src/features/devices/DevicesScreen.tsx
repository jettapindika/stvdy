import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { asApiError } from '@/api/errors'
import { useDevices, useRenameDevice, useRevokeDevice, type Device } from '@/api/queries/devices'
import { useSyncStatus } from '@/api/queries/sync'
import { PageHeader, Section, WidgetHeading } from '@/components/layout/Section'
import { Icon, type IconName } from '@/components/ui/Icon'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { platform } from '@/platform'

/**
 * Devices and sync: every machine this account is signed in on.
 *
 * Reached from the sync status control rather than from a sixth nav item (A189),
 * and filed under Settings as its "Devices & sync" section: the standalone
 * `/devices` route and the section render the same `DevicesPanel`, so the two
 * cannot come to disagree about which device is this one or what signing it out
 * does. The standalone page keeps its own `PageHeader`; the panel below it is the
 * body either way.
 *
 * **The design's row is a tile, an identity, an action and a sync line.** A 48px
 * raised tile carries the platform's glyph (a laptop for the two desktop
 * platforms and the browser, a phone for the two mobile ones), because the
 * platform is the first thing a student looks for and a wall of text rows is the
 * one list on this screen nobody reads. The glyph is decoration: the platform's
 * own name is always beside it, so nothing is carried by an icon alone.
 *
 * **Three honesty rules decide what this screen may say**, and all three are about
 * not inventing a fact:
 *
 * - **"Last synced" is `last_sync_at`**, the column `sync::touch_last_sync` writes
 *   when that device actually pulls or pushes (A180). It is deliberately *not*
 *   `last_seen_at`, which moves on any authenticated request: a device that signed
 *   up and never pulled would read as "synced just now", a time that does not
 *   trace to the work it names. A null renders "Not synced yet" in the timestamp's
 *   own slot rather than as a date, because there is no date to show.
 * - **The device count is the list's own length**, and it is omitted while the
 *   list is still unknown or empty: a zero in the heading of a card that says "No
 *   devices" is a number with nothing behind it.
 * - **"Reset local data and resync" says what it deletes** — this device's local
 *   copy *including changes it has not sent yet* — and says that the account is not
 *   touched. A reset button that did not name the unsent writes would be the one
 *   control on this screen that can destroy a student's work without warning.
 *
 * **There is no price and no upgrade control anywhere here** (gate 7f): a device
 * over the plan's `devices_max` is over it, the remedy is to sign one out, and the
 * plan screen is where a plan is discussed. On a phone build the rule is absolute,
 * because the stores forbid a route to a purchase.
 */
export function DevicesScreen() {
  const { t } = useTranslation()

  return (
    <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 py-8">
      <PageHeader
        eyebrow={t('devices.eyebrow')}
        title={t('devices.title')}
        subtitle={t('devices.subtitle')}
      />
      <DevicesPanel />
    </div>
  )
}

/**
 * The device list and the local reset, without a page header.
 *
 * Split out of the screen because the redesign files both under Settings as the
 * "Devices & Sync" section, and the standalone `/devices` route still has to work
 * — it is what the sync status control in the top bar opens (A189). One body, two
 * frames: a second copy inside Settings is how the two would come to disagree about
 * which device is this one or what revoking it does.
 */
export function DevicesPanel() {
  const { t } = useTranslation()
  const devices = useDevices()
  const session = useSession()
  const error = asApiError(devices.error)
  const { report } = useSyncStatus()

  // Why the list and the row actions cannot reach the server. The shell's own
  // report rather than `navigator.onLine`, which is a guess about the server made
  // from a fact about the radio — the same choice `TodayScreen` makes, and the same
  // component-level reason: a browser tab with a working radio and a dead endpoint
  // is not offline, and saying it is would be the app inventing a cause.
  const offline = report?.state === 'offline'

  // The user's zone, as everywhere else an instant is shown: a device that synced
  // at 23:40 in Jakarta must not read as the next day.
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  // The five platforms the shell registers, plus a fallback: `devices.platform` is
  // free text on the wire, and a value this build does not know is shown as it
  // arrived rather than mapped to a wrong name.
  const platformLabel = (value: string): string => {
    switch (value) {
      case 'web':
        return t('devices.platform.web')
      case 'macos':
        return t('devices.platform.macos')
      case 'windows':
        return t('devices.platform.windows')
      case 'ios':
        return t('devices.platform.ios')
      case 'android':
        return t('devices.platform.android')
      default:
        return value
    }
  }

  const rows = devices.data ?? []
  // The count the server's own list gives, and the card's own heading. Omitted
  // rather than zero-filled: an empty list already says so in the empty state
  // below, and a `0` beside it would be a figure with nothing behind it.
  const count =
    devices.data === undefined || devices.data.length === 0
      ? undefined
      : t('devices.count', { count: devices.data.length })

  return (
    <>
      <Section title={t('settings.section.devices')}>
        <p className="text-label text-muted">{t('devices.intro')}</p>

        <div className="card flex flex-col gap-4 p-6">
          <WidgetHeading title={t('devices.heading')} meta={count} />

          {/* The reason, before the thing it explains. A list that failed to load
              and a list that is genuinely empty look the same without it, and the
              student's next move is different in each case. */}
          {offline ? (
            <div className="note flex flex-wrap items-center gap-3" role="status">
              <Icon name="alert" className="shrink-0 text-warning" />
              <p className="min-w-0 flex-1 text-label">
                {t('states.offline')} — {t('states.offlineReason')}
              </p>
            </div>
          ) : null}

          {devices.isPending && <SkeletonRows rows={2} label={t('devices.loading')} />}
          {error !== null && <ApiErrorNotice error={error} />}

          {devices.data !== undefined &&
            (rows.length === 0 ? (
              // `padded={false}`: the card already carries the padding, and two
              // paddings stack into a state that does not line up with the heading
              // above it.
              <EmptyState
                title={t('devices.empty.title')}
                reason={t('devices.empty.reason')}
                padded={false}
              />
            ) : (
              <ul className="flex flex-col divide-y divide-border">
                {rows.map((device) => (
                  <li key={device.id} className="py-4">
                    <DeviceRow
                      device={device}
                      timezone={timezone}
                      platformLabel={platformLabel(device.platform)}
                    />
                  </li>
                ))}
              </ul>
            ))}

          {/* The design's in-card strip, and the half a student would not guess:
              signing a device out ends its session, and nothing they studied
              disappears with it. A destructive action that does not say what it
              leaves alone reads as one that takes everything. */}
          {rows.length > 0 ? (
            <div className="raised px-4 py-3">
              <p className="text-label text-muted">{t('devices.signOutNote')}</p>
            </div>
          ) : null}
        </div>

        {/* The design's "About device sync" block: a raised tone rather than a card,
            because it is not a control and carries no action. */}
        <div className="raised flex flex-wrap items-start gap-4 p-6">
          <Icon name="refresh-cw" size={24} className="shrink-0 text-primary-text" />
          <div className="flex min-w-0 flex-1 flex-col gap-2">
            <p className="text-label font-medium">{t('devices.sync.title')}</p>
            <p className="text-label text-muted">{t('devices.sync.body')}</p>
          </div>
        </div>
      </Section>

      <Section title={t('devices.reset.title')}>
        <ResetLocalData />
      </Section>
    </>
  )
}

/**
 * One device: what it is, when it last synced, and the two things that can be done
 * to it.
 *
 * Both actions confirm before they act, in the row rather than in a dialog: the
 * copy that says what will happen sits beside the button that does it, and there
 * is no focus trap to get wrong. Signing out this device ends the session here,
 * and the confirmation says so — the same button must not mean two different
 * things silently.
 */
function DeviceRow({
  device,
  timezone,
  platformLabel,
}: {
  device: Device
  timezone: string
  platformLabel: string
}) {
  const { t } = useTranslation()
  const revoke = useRevokeDevice()
  const rename = useRenameDevice()
  const [confirming, setConfirming] = useState(false)
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(device.name)
  const revokeError = asApiError(revoke.error)
  const renameError = asApiError(rename.error)

  // The two mobile platforms get the phone glyph and everything else the laptop:
  // `web` is a browser on a machine, and a platform string this build does not
  // know is a desktop machine far more often than it is a phone.
  const glyph: IconName =
    device.platform === 'ios' || device.platform === 'android' ? 'smartphone' : 'laptop'

  const lastSynced =
    device.last_sync_at === null
      ? null
      : formatInZone(new Date(device.last_sync_at), timezone, {
          dateStyle: 'medium',
          timeStyle: 'short',
        })

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-4">
          <span className="flex h-12 w-12 shrink-0 items-center justify-center rounded-field bg-surface-raised">
            <Icon name={glyph} size={24} className="text-primary-text" />
          </span>

          <div className="flex min-w-0 flex-1 flex-col gap-2">
            {/* `w-full … sm:w-auto sm:flex-1`, the shape `ReportPanel`'s row uses and
                load-bearing for the same reason. `flex-1` gives the name a flex basis of
                `0` and this row's other four children are all effectively fixed-width, so
                at 200% text they take the whole line and leave the name **20px** of a
                390px phone — measured in Chromium, `scrollWidth` 29 against a
                `clientWidth` of 20, a device name wrapping a character per line. Taking
                the whole line at phone width pushes the rest to the next line; `sm:`
                restores the one-line row where there is room. */}
            <span className="min-w-0 w-full break-words font-medium sm:w-auto sm:flex-1">
              {device.name}
            </span>
            <div className="flex flex-wrap items-center gap-3">
              {/* Every row the server lists is signed in: `GET /devices` excludes
                  revoked ones, so this is the list's state rather than a guess about
                  a row. The word is on the chip and the tone repeats it, so the row
                  reads in monochrome. */}
              <span className="chip chip-ready">{t('devices.signedIn')}</span>
              {device.current && <span className="chip">{t('devices.thisDevice')}</span>}
              <span className="meta">{platformLabel}</span>
            </div>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {!editing && (
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                setName(device.name)
                setEditing(true)
              }}
            >
              {t('devices.rename')}
            </button>
          )}
          {!confirming && (
            <button
              type="button"
              className="btn btn-danger"
              onClick={() => {
                setConfirming(true)
              }}
            >
              {t('devices.signOutDevice')}
            </button>
          )}
        </div>
      </div>

      {editing && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            rename.mutate(
              { deviceId: device.id, name },
              {
                onSuccess: () => {
                  setEditing(false)
                },
              },
            )
          }}
        >
          {/* `min-w-0` and not `min-w-40`. This form is a column at phone width and
             a row from `sm` up, so the minimum only ever applied where the layout
             does not need it — and it is `min-width: 10rem`, 320px at a 32px root,
             which is wider than the 310px content box it sat in: measured in
             Chromium at 390px with 200% text, the form's `scrollWidth` was 320
             against a `clientWidth` of 310. `flex-1` is what takes the row, which is
             the same pairing `ConnectionsPanel`'s ICS field uses. */}
          <label className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="label">{t('devices.renameLabel')}</span>
            <input
              className="field"
              value={name}
              autoFocus
              onChange={(event) => {
                setName(event.target.value)
              }}
            />
          </label>
          <button type="submit" className="btn btn-primary" disabled={rename.isPending}>
            {t('actions.save')}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => {
              rename.reset()
              setEditing(false)
            }}
          >
            {t('actions.cancel')}
          </button>
        </form>
      )}

      {/* The design's sync line: the label in the reading face and the instant in
          mono, because the instant is evidence and the label is prose. A device
          that has never pulled has no instant, so the words that say so stand in
          its place rather than an invented date (A180). */}
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-label text-muted">{t('devices.lastSyncedLabel')}</span>
        <span className="meta">{lastSynced ?? t('sync.neverSynced')}</span>
      </div>

      {confirming && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-small text-muted">
            {device.current
              ? t('devices.revokeConfirmCurrent')
              : t('devices.revokeConfirm', { name: device.name })}
          </span>
          <button
            type="button"
            className="btn btn-danger"
            autoFocus
            disabled={revoke.isPending}
            onClick={() => {
              revoke.mutate(device.id, {
                onSuccess: () => {
                  setConfirming(false)
                },
              })
            }}
          >
            {t('devices.signOut')}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => {
              revoke.reset()
              setConfirming(false)
            }}
          >
            {t('devices.keep')}
          </button>
        </div>
      )}

      {revokeError !== null && <ApiErrorNotice error={revokeError} />}
      {renameError !== null && <ApiErrorNotice error={renameError} />}
    </div>
  )
}

/**
 * "Reset local data and resync", and the sentence that says what it deletes.
 *
 * The reset is the shell's: it wipes the SQLite cache **and the outbox** and
 * resyncs from zero (A186). The outbox is the half that matters and the half a
 * student would not guess, which is why the copy names it — and it is the
 * section's description rather than a line inside the card, so it is read before
 * the button that acts on it.
 *
 * On the web build the control is replaced by the fact instead of being offered:
 * there is no local database there (A14), so a button that deletes nothing would
 * be a promise with nothing behind it.
 */
function ResetLocalData() {
  const { t } = useTranslation()
  const [confirming, setConfirming] = useState(false)
  const [done, setDone] = useState(false)
  const [failed, setFailed] = useState(false)

  if (platform.runtime() === 'web') {
    return <p className="max-w-prose text-small text-muted">{t('devices.reset.noneLocal')}</p>
  }

  return (
    <div className="card flex flex-col gap-3 p-4">
      <p className="max-w-prose text-small text-muted">{t('devices.reset.what')}</p>

      {!confirming && (
        <button
          type="button"
          className="btn btn-secondary self-start"
          onClick={() => {
            setDone(false)
            setConfirming(true)
          }}
        >
          {t('devices.reset.action')}
        </button>
      )}

      {confirming && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-small text-muted">{t('devices.reset.confirm')}</span>
          <button
            type="button"
            className="btn btn-danger"
            autoFocus
            onClick={() => {
              // The shell's wipe, not a screen-level cache clear: only it can
              // delete the database and the outbox together.
              void platform.sync
                .reset()
                .then(() => {
                  setDone(true)
                  setConfirming(false)
                })
                .catch(() => {
                  setFailed(true)
                  setConfirming(false)
                })
            }}
          >
            {t('devices.reset.confirmAction')}
          </button>
          <button
            type="button"
            className="btn btn-quiet"
            onClick={() => {
              setConfirming(false)
            }}
          >
            {t('actions.cancel')}
          </button>
        </div>
      )}

      {done && (
        <p className="text-small text-muted" role="status">
          {t('devices.reset.done')}
        </p>
      )}
      {failed && (
        <p className="note note-danger text-small" role="alert">
          {t('devices.reset.failed')}
        </p>
      )}
    </div>
  )
}
