import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { useSaveSettings, useSettings, type ReminderChannel } from '@/api/queries/settings'
import { useUsage } from '@/api/queries/usage'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { planForReminderChannel } from '@/features/billing/plans'
import { LANGUAGES, LANGUAGE_LABELS, type Language } from '@/i18n'
import { platform } from '@/platform'

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
 * **The sections this phase does not own are named.** Calendar connections
 * (Phase 10), data export and deletion (Phase 7/15) and BYOK (Phase 13) each say
 * which phase brings them, rather than being an absent control a student would
 * look for and not find.
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
              const minPlan = planForReminderChannel(channel)
              const on = row.reminder_channels.includes(channel)
              return (
                <div key={channel} className="flex min-h-11 flex-wrap items-center gap-2">
                  <input
                    id={`channel-${channel}`}
                    type="checkbox"
                    checked={on}
                    // A locked channel is not choosable at all: the remedy is the
                    // plan, not a refusal after the fact.
                    disabled={!granted}
                    onChange={(event) =>
                      write({
                        reminder_channels: event.target.checked
                          ? [...row.reminder_channels, channel]
                          : row.reminder_channels.filter((kept) => kept !== channel),
                      })
                    }
                  />
                  <label htmlFor={`channel-${channel}`} className="text-body">
                    {t(`settings.channel.${channel}`)}
                  </label>
                  {!granted && minPlan !== null ? (
                    // The word carries the state; the tone repeats it, so the row
                    // reads in monochrome.
                    <span className="chip chip-due">
                      {t('settings.notifications.locked', { plan: minPlan })}
                    </span>
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

      <Section title={t('settings.later.title')}>
        <ul className="card flex flex-col gap-3 p-4">
          <li className="flex flex-col gap-1">
            <span className="text-body">{t('settings.later.calendar')}</span>
            <span className="meta">{t('settings.later.calendarPhase')}</span>
          </li>
          <li className="flex flex-col gap-1">
            <span className="text-body">{t('settings.later.data')}</span>
            <span className="meta">{t('settings.later.dataPhase')}</span>
          </li>
          <li className="flex flex-col gap-1">
            <span className="text-body">{t('settings.later.byok')}</span>
            <span className="meta">{t('settings.later.byokPhase')}</span>
          </li>
        </ul>
      </Section>
    </div>
  )
}
