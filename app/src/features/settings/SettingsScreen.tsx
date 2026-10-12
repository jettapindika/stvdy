import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { EXPORT_FORMATS, useDeleteAccount, useExportAccount } from '@/api/queries/account'
import { useSaveSettings, useSettings, type ReminderChannel } from '@/api/queries/settings'
import { useUsage } from '@/api/queries/usage'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { planForReminderChannel } from '@/features/billing/plans'
import { LANGUAGES, LANGUAGE_LABELS, type Language } from '@/i18n'
import { saveBlob } from '@/lib/download'
import { platform } from '@/platform'
import { UpdatesPanel } from './UpdatesPanel'

/**
 * Settings: what the student chooses, and what another phase owns.
 *
 * **Quiet hours are a window, and the window may wrap midnight.** `22 → 7` is the
 * normal case, so the form says which window it means ("22:00 to 07:00, next day")
 * rather than presenting two numbers and leaving the student to work out whether
 * the pair is valid. The rule is membership, not `start < end` (A202).
 *
 * **A channel the plan does not grant is locked, and the plan that has it is
 * named.** The fact comes from two places that are each authoritative for their
 * half: `GET /usage` says what *this account* may choose (that is the enforcement,
 * and the server refuses a channel the plan does not grant per channel with 403
 * `plan_required`), and `config/plans.toml` says which plan first has it — because
 * a screen that derived the name from the account's own plan could only ever say
 * "not yours". Choosing a locked channel is impossible rather than a 403 after the
 * fact.
 *
 * **The timezone is on the account, not on the settings row** (A203), which is why
 * it is saved through the same `PUT /settings` but read from `GET /auth/me`.
 *
 * **The sections this phase does not own are named.** BYOK (Phase 13) says which
 * phase brings it, rather than being an absent control a student would look for and
 * not find. Data export and deletion used to be in that list and is now a section of
 * its own (A355) — copy that outlives its phase is worse than no copy, because it
 * tells a student a working feature does not exist.
 *
 * **Deleting the account is behind a type-to-confirm, and the copy states the
 * consequences before the button is reachable.** A destructive action behind one tap
 * is the wrong default on a phone, where a mis-tap is easy and the account is not
 * recoverable; and the forfeiture of remaining prepaid time is what `docs/EULA.md`
 * § 5 promises will be said before confirmation (A350).
 *
 * **This screen is the phone's only route to the plan.** The credit meter lives in
 * the sidebar, which a phone does not render, and both `LockedFeature` and the
 * plan-limit notice deliberately carry no link on iOS and Android (A143) — so
 * without the link in the plan section below, the read-only plan screen Phase 14
 * builds (A322) would be reachable on a phone only by typing a URL into a webview
 * that has no address bar. The section is a link for the same reason Devices is
 * one: a sub-screen of settings is reached from here, not from a sixth tab item
 * (A189).
 */

/** The channels the settings screen offers, in the order the matrix lists them. */
const CHANNELS: ReminderChannel[] = ['web', 'push', 'email', 'telegram', 'discord']

export function SettingsScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const settings = useSettings()
  const usage = useUsage()
  const [error, setError] = useState<ApiError | null>(null)
  const [resetting, setResetting] = useState(false)

  const userId = session.status === 'signed-in' ? session.user.id : ''
  const save = useSaveSettings(userId)
  const row = settings.data?.settings
  // The account's own zone, which the settings row does not carry (A203). The
  // cached path cannot answer it either — `users.timezone` is not a synced
  // entity — so the session's own value is the fallback rather than an empty
  // string, which would render as a field the student could save blank.
  const sessionZone = session.status === 'signed-in' ? session.user.timezone : ''
  const timeZone = settings.data?.timezone || sessionZone

  const write = (patch: Parameters<typeof save.mutate>[0]) => {
    setError(null)
    save.mutate(patch, { onError: (failure) => setError(asApiError(failure)) })
  }

  /**
   * "Reset local data and resync" — the cache **and** the outbox (A186).
   *
   * A pending offline write from this device must not be replayed by the next
   * account that signs in on it, so the wipe is total rather than a refresh. The
   * copy above the button says what it deletes, because a control that silently
   * removes queued work is one a student cannot consent to.
   */
  const reset = async () => {
    setResetting(true)
    setError(null)
    try {
      await platform.sync.reset()
    } catch (failure) {
      setError(asApiError(failure))
    } finally {
      setResetting(false)
    }
  }

  // The session is part of what this screen needs: the settings row's mutation id
  // *is* the user's id (A200), and the timezone lives on the account (A203). So a
  // form rendered before the account is known would be a form whose saves carry an
  // empty id — a write the server would reject as another row's.
  if (settings.isPending || session.status === 'loading') {
    return (
      <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
        <PageHeader title={t('nav.settings')} subtitle={t('settings.subtitle')} />
        <LoadingState label={t('settings.loading')} />
      </div>
    )
  }

  if (row === undefined) {
    return (
      <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
        <PageHeader title={t('nav.settings')} subtitle={t('settings.subtitle')} />
        {error !== null ? <ApiErrorNotice error={error} noun={t('settings.noun')} /> : null}
      </div>
    )
  }

  const grantedChannels = usage.data?.features.reminder_channels ?? []
  // What this deployment can actually deliver on (A288), which is a different
  // question from what the plan grants. `telegram` and `discord` are granted to
  // Power by the matrix and have no transport, so a row that offered them would be
  // granted, selectable and silently never fire — and the student's only evidence
  // would be a reminder that never arrived.
  const availableChannels = settings.data?.channels_available ?? []
  // Guarded for the same reason as the field above, and the failure it prevents is
  // worse: the shell and the server are versioned independently (A353), so a client
  // newer than the server it talks to is a supported deployment. A `/settings`
  // response from a server that predates this field has no `reminder_channels`, and
  // `undefined.includes` throws inside the render — which takes the whole screen
  // down behind the router's error boundary rather than degrading one section. An
  // absent field means "no channel is on", which is what the server's own default
  // would have said.
  const selectedChannels = row.reminder_channels ?? []

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader title={t('nav.settings')} subtitle={t('settings.subtitle')} />

      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('settings.noun')} action={t('actions.retry')} />
      ) : null}

      <Section title={t('settings.study.title')}>
        <div className="card flex flex-col gap-4 p-4">
          <label className="flex flex-col gap-1">
            <span className="label">{t('settings.study.timezone')}</span>
            <input
              className="field"
              value={timeZone}
              onChange={(event) => write({ timezone: event.target.value })}
            />
            <span className="meta">{t('settings.study.timezoneHint')}</span>
          </label>

          <div className="flex flex-wrap gap-4">
            <label className="flex flex-col gap-1">
              <span className="label">{t('settings.study.startHour')}</span>
              <input
                className="field max-w-24"
                type="number"
                min={0}
                max={23}
                value={row.study_start_hour}
                onChange={(event) => write({ study_start_hour: Number(event.target.value) })}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="label">{t('settings.study.endHour')}</span>
              <input
                className="field max-w-24"
                type="number"
                min={1}
                max={24}
                value={row.study_end_hour}
                onChange={(event) => write({ study_end_hour: Number(event.target.value) })}
              />
            </label>
          </div>

          <label className="flex flex-col gap-1">
            <span className="label">{t('settings.study.maxMinutes')}</span>
            <input
              className="field max-w-32"
              type="number"
              min={15}
              max={1440}
              value={row.max_study_min_per_day}
              onChange={(event) => write({ max_study_min_per_day: Number(event.target.value) })}
            />
          </label>

          <div className="flex flex-wrap gap-4">
            <label className="flex flex-col gap-1">
              <span className="label">{t('settings.study.blockLength')}</span>
              <input
                className="field max-w-24"
                type="number"
                min={5}
                max={180}
                value={row.block_len_min}
                onChange={(event) => write({ block_len_min: Number(event.target.value) })}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="label">{t('settings.study.breakLength')}</span>
              <input
                className="field max-w-24"
                type="number"
                min={0}
                max={60}
                value={row.break_min}
                onChange={(event) => write({ break_min: Number(event.target.value) })}
              />
            </label>
          </div>
        </div>
      </Section>

      <Section title={t('settings.notifications.title')}>
        <div className="card flex flex-col gap-4 p-4">
          <fieldset className="flex flex-col gap-2">
            <legend className="label">{t('settings.notifications.channels')}</legend>
            {CHANNELS.map((channel) => {
              const granted = grantedChannels.includes(channel)
              const available = availableChannels.includes(channel)
              const minPlan = planForReminderChannel(channel)
              const on = selectedChannels.includes(channel)
              return (
                <div key={channel} className="flex min-h-11 flex-wrap items-center gap-2">
                  <input
                    id={`channel-${channel}`}
                    type="checkbox"
                    checked={on}
                    // A locked channel is not choosable at all, and neither is one
                    // this deployment cannot deliver on: in both cases the remedy is
                    // elsewhere, and a control that refuses after the fact is worse
                    // than one that is visibly unavailable (A288).
                    disabled={!granted || !available}
                    onChange={(event) =>
                      write({
                        reminder_channels: event.target.checked
                          ? [...selectedChannels, channel]
                          : selectedChannels.filter((kept) => kept !== channel),
                      })
                    }
                  />
                  {/* The label is the hit area, and it carries `min-h-11` and
                      `min-w-11` for the same reason `LockedFeature`'s link does.
                      Measured in Chromium at 390px: the label was **65.9x22**, so
                      a thumb aimed at the channel name and landing a few pixels
                      high hit the row `<div>` and toggled nothing (Phase 14);
                      and with the height fixed, `input#channel-email` was
                      **35.3x44** at 100% text in both themes, because "Email" is a
                      short word and the label is only as wide as its text. The
                      width is on **every** channel label rather than the email row:
                      the next short name ("Web", "Push") has the same defect, and a
                      per-row fix is one that gets forgotten. It costs a 9px sliver
                      on the narrow rows and buys the 44px target the gate requires.
                      The checkbox itself stays 13x13 — the label is what a finger
                      reaches. */}
                  <label
                    htmlFor={`channel-${channel}`}
                    className="text-body flex min-h-11 min-w-11 items-center"
                  >
                    {t(`settings.channel.${channel}`)}
                  </label>
                  {!granted && minPlan !== null ? (
                    // The word carries the state; the tone repeats it, so the row
                    // reads in monochrome.
                    <span className="chip chip-due">
                      {t('settings.notifications.locked', { plan: minPlan })}
                    </span>
                  ) : !available ? (
                    // Granted by the plan and not deliverable yet. The sentence names
                    // which of the two it is, because "not available" alone reads as
                    // a bug rather than as a channel that is coming.
                    <span className="chip chip-due">{t('settings.notifications.notYet')}</span>
                  ) : null}
                </div>
              )
            })}
          </fieldset>

          <fieldset className="flex flex-col gap-2">
            <legend className="label">{t('settings.notifications.quiet')}</legend>
            <p className="meta">{t('settings.notifications.quietHint')}</p>
            <div className="flex flex-wrap gap-4">
              <label className="flex flex-col gap-1">
                <span className="label">{t('settings.notifications.quietFrom')}</span>
                <input
                  className="field max-w-24"
                  type="number"
                  min={0}
                  max={23}
                  value={row.quiet_start_hour ?? ''}
                  onChange={(event) =>
                    // The pair is both-or-neither: setting one sets both, and
                    // clearing either clears both. A window with one end would be
                    // a row the migration's own check constraint refuses.
                    write({
                      quiet_start_hour:
                        event.target.value === '' ? null : Number(event.target.value),
                      quiet_end_hour: row.quiet_end_hour ?? 7,
                    })
                  }
                />
              </label>
              <label className="flex flex-col gap-1">
                <span className="label">{t('settings.notifications.quietTo')}</span>
                <input
                  className="field max-w-24"
                  type="number"
                  min={0}
                  max={23}
                  value={row.quiet_end_hour ?? ''}
                  onChange={(event) =>
                    write({
                      quiet_start_hour: row.quiet_start_hour ?? 22,
                      quiet_end_hour: event.target.value === '' ? null : Number(event.target.value),
                    })
                  }
                />
              </label>
            </div>
            {/* Which window it means, in words. `22 → 7` wraps midnight, and a
                student reading two numbers has no way to know that is allowed. */}
            {row.quiet_start_hour !== null && row.quiet_end_hour !== null ? (
              <p className="text-small text-muted">
                {row.quiet_start_hour < row.quiet_end_hour
                  ? t('settings.notifications.quietSameDay', {
                      from: row.quiet_start_hour,
                      to: row.quiet_end_hour,
                    })
                  : t('settings.notifications.quietNextDay', {
                      from: row.quiet_start_hour,
                      to: row.quiet_end_hour,
                    })}
              </p>
            ) : (
              <p className="text-small text-muted">{t('settings.notifications.quietNone')}</p>
            )}
          </fieldset>
        </div>
      </Section>

      <Section title={t('settings.language.title')}>
        <div className="card flex flex-col gap-2 p-4">
          <label className="label" htmlFor="settings-language">
            {t('settings.language.label')}
          </label>
          <select
            id="settings-language"
            className="field max-w-64"
            value={row.language}
            onChange={(event) => write({ language: event.target.value as Language })}
          >
            {LANGUAGES.map((language) => (
              <option key={language} value={language}>
                {LANGUAGE_LABELS[language]}
              </option>
            ))}
          </select>
        </div>
      </Section>

      <Section title={t('settings.appearance.title')}>
        <div className="card flex flex-col gap-2 p-4">
          <label className="label" htmlFor="settings-theme">
            {t('settings.appearance.label')}
          </label>
          <select
            id="settings-theme"
            className="field max-w-64"
            value={row.theme}
            onChange={(event) => write({ theme: event.target.value as 'system' | 'light' | 'dark' })}
          >
            <option value="system">{t('settings.appearance.system')}</option>
            <option value="light">{t('settings.appearance.light')}</option>
            <option value="dark">{t('settings.appearance.dark')}</option>
          </select>
        </div>
      </Section>

      <Section title={t('settings.plan.title')}>
        <div className="card flex flex-col gap-3 p-4">
          <p className="text-small text-muted">{t('settings.plan.hint')}</p>
          <Link to="/billing" className="link inline-flex min-h-11 items-center self-start">
            {t('settings.plan.open')}
          </Link>
          <hr className="border-border" />
          {/* The dashboard's only route in, on every form factor. It is not a sixth
              nav label (A189), and this section is where a student is already
              looking at what their plan gives them. */}
          <p className="text-small text-muted">{t('settings.plan.progressHint')}</p>
          <Link to="/dashboard" className="link inline-flex min-h-11 items-center self-start">
            {t('settings.plan.progress')}
          </Link>
        </div>
      </Section>

      <Section title={t('settings.devices.title')}>
        <div className="card flex flex-col gap-3 p-4">
          <p className="text-small text-muted">{t('settings.devices.hint')}</p>
          <Link to="/devices" className="link inline-flex min-h-11 items-center self-start">
            {t('settings.devices.open')}
          </Link>
          <hr className="border-border" />
          <p className="text-small text-muted">{t('settings.devices.resetHint')}</p>
          <button
            type="button"
            className="btn btn-secondary self-start"
            disabled={resetting}
            onClick={() => void reset()}
          >
            {t('settings.devices.reset')}
          </button>
        </div>
      </Section>

      {/* Between the account's own devices and the "not built yet" list: an update
          is a fact about this installation, which is what the section above is
          about too, and it is not a feature another phase owes. The component
          renders nothing on web and mobile (A335), so the section is absent there
          rather than present and disabled. */}
      <UpdatesPanel />

      {/* The data controls (A355). Between the account's own devices and the "not built
          yet" list, because both are facts about this account: what we hold and how to
          take it or leave. The section used to be a line under "Not built yet", and the
          copy that said so is gone rather than left to contradict the code. */}
      <DataSection />

      <Section title={t('settings.later.title')}>
        <ul className="card flex flex-col gap-3 p-4">
          <li className="flex flex-col gap-1">
            <span className="text-body">{t('settings.later.byok')}</span>
            <span className="meta">{t('settings.later.byokPhase')}</span>
          </li>
        </ul>
      </Section>
    </div>
  )
}

/**
 * Export and deletion.
 *
 * Three decisions the copy and the structure both carry:
 *
 *  * **The export is three documents, not one.** The spec names JSON + Markdown + ICS, and
 *    they answer different questions: the JSON is for a machine, the Markdown is for the
 *    student reading what we hold, and the ICS is what a calendar can import. Offering one
 *    would be choosing for the student.
 *  * **The deletion is behind a type-to-confirm.** The student types their own email
 *    address, which is the one string that cannot be produced by a mis-tap, and the final
 *    button stays disabled until it matches. The consequences are stated *above* that field,
 *    because `docs/EULA.md` § 5 promises the forfeiture is said before confirmation (A350).
 *  * **What happens next is the server's answer, not this screen's.** The response carries
 *    the purge date; the copy below renders it rather than computing one, so the sentence on
 *    screen cannot disagree with the row in the database.
 */
function DataSection() {
  const { t } = useTranslation()
  const session = useSession()
  const exportAccount = useExportAccount()
  const deleteAccount = useDeleteAccount()
  const [confirming, setConfirming] = useState(false)
  const [typed, setTyped] = useState('')
  const [error, setError] = useState<ApiError | null>(null)

  const email = session.status === 'signed-in' ? session.user.email : ''
  // Case-insensitive and trimmed: the address is the confirmation, not a password, and a
  // student who types it with a trailing space has still confirmed what they meant.
  const matches = email !== '' && typed.trim().toLowerCase() === email.toLowerCase()
  const exportError = asApiError(exportAccount.error)
  const deleteError = asApiError(deleteAccount.error)

  return (
    <Section title={t('settings.data.title')}>
      <div className="card flex flex-col gap-3 p-4">
        <p className="text-small text-muted">{t('settings.data.exportHint')}</p>
        <div className="flex flex-wrap gap-2">
          {EXPORT_FORMATS.map((format) => (
            <button
              key={format}
              type="button"
              className="btn btn-secondary min-h-11"
              disabled={exportAccount.isPending}
              onClick={() => {
                setError(null)
                exportAccount.mutate(format, {
                  onSuccess: ({ blob }) => {
                    saveBlob(blob, `stvdy-export.${format}`)
                  },
                  onError: (failure) => setError(asApiError(failure)),
                })
              }}
            >
              {t(`settings.data.export.${format}`)}
            </button>
          ))}
        </div>
        {exportError !== null ? (
          <p role="alert" className="text-small text-danger">
            {t('settings.data.exportFailed', { message: exportError.message })}
          </p>
        ) : null}

        <hr className="border-border" />

        {/* The consequence is stated before the control that acts on it. The wording is the
            EULA's: deletion is immediate, remaining prepaid time is forfeited, and the
            records we must keep for tax or accounting survive in anonymised form. */}
        <p className="text-small text-muted">{t('settings.data.deleteConsequences')}</p>
        <p className="text-small text-muted">{t('settings.data.deleteForfeit')}</p>

        {!confirming ? (
          <button
            type="button"
            className="btn btn-secondary min-h-11 self-start"
            onClick={() => {
              setError(null)
              setConfirming(true)
            }}
          >
            {t('settings.data.deleteStart')}
          </button>
        ) : (
          <div className="flex flex-col gap-2">
            <label className="flex flex-col gap-1">
              <span className="label">{t('settings.data.deleteConfirmLabel', { email })}</span>
              <input
                className="field max-w-72"
                value={typed}
                autoComplete="off"
                onChange={(event) => setTyped(event.target.value)}
              />
            </label>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                className="btn btn-danger min-h-11"
                // Disabled until the address matches: the confirmation has to be something a
                // mis-tap cannot produce.
                disabled={!matches || deleteAccount.isPending}
                onClick={() => {
                  setError(null)
                  deleteAccount.mutate(undefined, {
                    onError: (failure) => setError(asApiError(failure)),
                  })
                }}
              >
                {deleteAccount.isPending
                  ? t('settings.data.deleting')
                  : t('settings.data.deleteConfirm')}
              </button>
              <button
                type="button"
                className="btn btn-secondary min-h-11"
                onClick={() => {
                  setTyped('')
                  setConfirming(false)
                }}
              >
                {t('settings.data.cancel')}
              </button>
            </div>
          </div>
        )}

        {error ?? deleteError ? (
          <p role="alert" className="text-small text-danger">
            {t('settings.data.deleteFailed', { message: (error ?? deleteError)?.message ?? '' })}
          </p>
        ) : null}
      </div>
    </Section>
  )
}
