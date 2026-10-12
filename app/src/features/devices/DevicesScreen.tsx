import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { asApiError } from '@/api/errors'
import { useDevices, useRenameDevice, useRevokeDevice, type Device } from '@/api/queries/devices'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { platform } from '@/platform'

/**
 * Devices and sync: every machine this account is signed in on.
 *
 * Reached from the sync status control rather than from a sixth nav item (A189),
 * which is why it also carries the two things the spec puts beside the device
 * list: when each device last synced, and "Reset local data and resync".
 *
 * **Two honesty rules decide what this screen may say**, and both are about not
 * inventing a fact:
 *
 * - **"Last synced" is `last_sync_at`**, the column `sync::touch_last_sync` writes
 *   when that device actually pulls or pushes (A180). It is deliberately *not*
 *   `last_seen_at`, which moves on any authenticated request: a device that signed
 *   up and never pulled would read as "synced just now", a time that does not
 *   trace to the work it names. A null is rendered as "Not synced yet" rather than
 *   as a date, because there is no date to show.
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
  const devices = useDevices()
  const session = useSession()
  const error = asApiError(devices.error)

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

  return (
    <div className="flex flex-col gap-6 px-5 py-6">
      <PageHeader title={t('devices.title')} subtitle={t('devices.subtitle')} />

      {devices.isPending && <SkeletonRows rows={2} label={t('devices.loading')} />}
      {error !== null && <ApiErrorNotice error={error} />}

      {devices.data !== undefined && (
        <Section title={t('devices.signedIn')}>
          {devices.data.length === 0 ? (
            <EmptyState title={t('devices.empty.title')} reason={t('devices.empty.reason')} />
          ) : (
            <ul className="flex flex-col divide-y divide-border border-y border-border">
              {devices.data.map((device) => (
                <li key={device.id} className="py-3">
                  <DeviceRow
                    device={device}
                    timezone={timezone}
                    platformLabel={platformLabel(device.platform)}
                  />
                </li>
              ))}
            </ul>
          )}
        </Section>
      )}

      <Section title={t('devices.reset.title')}>
        <ResetLocalData />
      </Section>
    </div>
  )
}

/**
 * One device: what it is, when it last synced, and the two things that can be done
 * to it.
 *
 * Both actions confirm before they act, in the row rather than in a dialog: the
 * copy that says what will happen sits beside the button that does it, and there
 * is no focus trap to get wrong. Revoking this device signs the user out here, and
 * the confirmation says so — the same button must not mean two different things
 * silently.
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

  const lastSynced =
    device.last_sync_at === null
      ? t('sync.neverSynced')
      : t('sync.lastSynced', {
          when: formatInZone(new Date(device.last_sync_at), timezone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
        })

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 flex-1 break-words font-medium">{device.name}</span>
        {device.current && <span className="chip">{t('devices.thisDevice')}</span>}
        <span className="meta">{platformLabel}</span>
        <span className="meta">{lastSynced}</span>
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
            className="btn btn-quiet"
            onClick={() => {
              setConfirming(true)
            }}
          >
            {t('devices.signOut')}
          </button>
        )}
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
          <label className="flex min-w-40 flex-1 flex-col gap-1">
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

      {confirming && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm text-muted">
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
 * student would not guess, which is why the copy names it.
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
    return <p className="max-w-prose text-sm text-muted">{t('devices.reset.noneLocal')}</p>
  }

  return (
    <div className="flex flex-col gap-2">
      <p className="max-w-prose text-sm text-muted">{t('devices.reset.what')}</p>

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
          <span className="text-sm text-muted">{t('devices.reset.confirm')}</span>
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
        <p className="text-sm text-muted" role="status">
          {t('devices.reset.done')}
        </p>
      )}
      {failed && (
        <p className="note note-danger text-sm" role="alert">
          {t('devices.reset.failed')}
        </p>
      )}
    </div>
  )
}
