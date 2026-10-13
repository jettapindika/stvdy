import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, NavLink, useParams } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { EXPORT_FORMATS, useDeleteAccount, useExportAccount } from '@/api/queries/account'
import { useSaveSettings, useSettings, type ReminderChannel } from '@/api/queries/settings'
import { useUsage } from '@/api/queries/usage'
import { PageFooter, PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { planForReminderChannel } from '@/features/billing/plans'
import { ConnectionsPanel } from '@/features/calendar/ConnectionsPanel'
import { DevicesPanel } from '@/features/devices/DevicesScreen'
import { LANGUAGES, LANGUAGE_LABELS, type Language } from '@/i18n'
import { saveBlob } from '@/lib/download'
import { DEFAULT_TIMEZONE, zoneOffsetLabel } from '@/lib/time'
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
 * it is saved through the same `PUT /settings` but read from `GET /auth/me`. The
 * page's footer states that same zone and its offset for today, because "when is
 * now" is the question every deadline on the other screens is answered in.
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

/**
 * The screen's sections, in the order the sub-nav lists them.
 *
 * The design's Settings frames are a two-column screen: a vertical sub-nav on the
 * left and one section's content on the right, which is a change from the earlier
 * single long scroll. The section is part of the URL (`/settings/devices`) rather
 * than component state, because a student who is told "revoke it in Settings →
 * Devices" should be able to be sent a link to it, and because the sync control in
 * the shell links straight to `/devices` and a browser Back from there should land
 * somewhere sensible.
 *
 * The vocabulary is the design's five, and the three the earlier screen had that
 * are not among them (Notifications, Language, Data) are folded into the section
 * they belong to rather than dropped: notifications and language are study
 * preferences in the sense the design uses the phrase, and the data controls are
 * facts about the account.
 */
const SECTIONS = ['account', 'study', 'calendar', 'devices', 'appearance'] as const
type SettingsSection = (typeof SECTIONS)[number]

/** The section a path names, or the first one when it names nothing usable. */
function sectionFromParam(value: string | undefined): SettingsSection {
  const found = SECTIONS.find((section) => section === value)
  return found ?? 'account'
}

export function SettingsScreen() {
  const { t, i18n } = useTranslation()
  const session = useSession()
  const settings = useSettings()
  const usage = useUsage()
  const { section: sectionParam } = useParams()
  const section = sectionFromParam(sectionParam)
  const [error, setError] = useState<ApiError | null>(null)

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

  // The session is part of what this screen needs: the settings row's mutation id
  // *is* the user's id (A200), and the timezone lives on the account (A203). So a
  // form rendered before the account is known would be a form whose saves carry an
  // empty id — a write the server would reject as another row's.
  if (settings.isPending || session.status === 'loading') {
    return (
      <div className="flex w-full min-w-0 flex-col gap-6 px-5 md:px-[32px] py-8">
        <PageHeader title={t('nav.settings')} subtitle={t('settings.subtitle')} />
        <LoadingState label={t('settings.loading')} />
      </div>
    )
  }

  if (row === undefined) {
    return (
      <div className="flex w-full min-w-0 flex-col gap-6 px-5 md:px-[32px] py-8">
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

  // The footer's two facts. The zone is the account's, and it falls back to the
  // app's documented default only because `Intl` throws on an empty zone — a
  // footer that took the screen down would be worse than one naming the product's
  // own default. Both are read from one clock read, so the date and the offset
  // cannot come from two different instants, and the date is read *in that zone*
  // rather than on the machine (lib/time.ts's rule): a student in Jakarta at 00:30
  // must not read yesterday's date off a laptop still set to UTC.
  const now = new Date()
  const footerZone = timeZone === '' ? DEFAULT_TIMEZONE : timeZone
  const today = new Intl.DateTimeFormat(i18n.language, {
    day: 'numeric',
    month: 'long',
    year: 'numeric',
    timeZone: footerZone,
  }).format(now)
  const footerZoneLabel = `${footerZone} · ${zoneOffsetLabel(now, footerZone)}`

  return (
    <div className="flex w-full min-w-0 flex-col gap-6 px-5 md:px-[32px] py-8">
      <PageHeader
        eyebrow={t('shell.settingsEyebrow')}
        title={t('nav.settings')}
        subtitle={t('settings.subtitle')}
      />

      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('settings.noun')} action={t('actions.retry')} />
      ) : null}

      {/* The two-column body. `lg` rather than `md` because the sub-nav is a column
          of words and the content beside it is a form; below that width the nav
          wraps into a row above the content, which is what a phone gets — five
          full-width rows would push the section the student came for off the
          first screen. */}
      <div className="flex flex-col gap-6 lg:flex-row lg:gap-8">
        <nav
          aria-label={t('settings.sections')}
          className="flex shrink-0 flex-wrap gap-1 lg:w-56 lg:flex-col"
        >
          {SECTIONS.map((item) => (
            <NavLink key={item} to={`/settings/${item}`} className="nav-item">
              {t(`settings.section.${item}`)}
            </NavLink>
          ))}
        </nav>

        <div className="flex min-w-0 flex-1 flex-col gap-6">
          {section === 'account' && (
            <>
              <AccountIdentity />

              <Section title={t('settings.plan.title')}>
                <p className="text-label text-muted">{t('settings.plan.hint')}</p>
                <div className="card flex flex-col gap-4 p-4">
                  <Link to="/billing" className="link inline-flex min-h-11 items-center self-start">
                    {t('settings.plan.open')}
                  </Link>
                  <hr className="border-border" />
                  {/* The dashboard's only route in, on every form factor. It is not
                      a sixth nav label (A189), and this section is where a student
                      is already looking at what their plan gives them. */}
                  <p className="text-small text-muted">{t('settings.plan.progressHint')}</p>
                  <Link
                    to="/dashboard"
                    className="link inline-flex min-h-11 items-center self-start"
                  >
                    {t('settings.plan.progress')}
                  </Link>
                </div>
              </Section>

              {/* An update is a fact about this installation, which is what this
                  section is about. The component renders nothing on web and mobile
                  (A335), so the section is absent there rather than present and
                  disabled. */}
              <UpdatesPanel />

              <DataSection />

              {/* The operator console, and it is the only way in.
                  The route is registered for every account and the *server* is the
                  gate (`AdminUser` answers 404 to a non-admin, A7) — but a route
                  nothing links to is a screen an operator has to be told about by
                  hand, which is the same reason the plan section above carries the
                  dashboard's only route in. `is_admin` is the account's own flag, so
                  rendering it here tells a student nothing they do not already know,
                  and it is a display fact only: opening `/admin` as a non-admin gets
                  the API's 404, not a screen. */}
              {session.status === 'signed-in' && session.user.is_admin && (
                <Section title={t('settings.admin.title')}>
                  <p className="text-label text-muted">{t('settings.admin.hint')}</p>
                  <div className="card flex flex-col gap-4 p-4">
                    <Link to="/admin" className="link inline-flex min-h-11 items-center self-start">
                      {t('settings.admin.open')}
                    </Link>
                  </div>
                </Section>
              )}
            </>
          )}

          {section === 'study' && (
            <>
              <Section title={t('settings.study.title')}>
                <p className="text-label text-muted">{t('settings.study.intro')}</p>
                <div className="card flex flex-col gap-6 p-6">
                  <label className="flex flex-col gap-1">
                    <span className="label">{t('settings.study.timezone')}</span>
                    <input
                      className="field"
                      value={timeZone}
                      onChange={(event) => write({ timezone: event.target.value })}
                    />
                    <span className="text-caption text-muted">
                      {t('settings.study.timezoneHint')}
                    </span>
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
                      onChange={(event) =>
                        write({ max_study_min_per_day: Number(event.target.value) })
                      }
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
                <p className="text-label text-muted">{t('settings.notifications.intro')}</p>
                <div className="card flex flex-col gap-6 p-6">
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
                            // A locked channel is not choosable at all, and neither is
                            // one this deployment cannot deliver on: in both cases the
                            // remedy is elsewhere, and a control that refuses after the
                            // fact is worse than one that is visibly unavailable (A288).
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
                              `min-w-11` for the same reason `LockedFeature`'s link
                              does. Measured in Chromium at 390px: the label was
                              **65.9x22**, so a thumb aimed at the channel name and
                              landing a few pixels high hit the row `<div>` and toggled
                              nothing (Phase 14); and with the height fixed,
                              `input#channel-email` was **35.3x44** at 100% text in both
                              themes, because "Email" is a short word and the label is
                              only as wide as its text. The width is on **every**
                              channel label rather than the email row: the next short
                              name ("Web", "Push") has the same defect, and a per-row
                              fix is one that gets forgotten. It costs a 9px sliver on
                              the narrow rows and buys the 44px target the gate
                              requires. The checkbox itself stays 13x13 — the label is
                              what a finger reaches. */}
                          <label
                            htmlFor={`channel-${channel}`}
                            className="text-body flex min-h-11 min-w-11 items-center"
                          >
                            {t(`settings.channel.${channel}`)}
                          </label>
                          {!granted && minPlan !== null ? (
                            // The word carries the state; the tone repeats it, so the
                            // row reads in monochrome.
                            <span className="chip chip-due">
                              {t('settings.notifications.locked', { plan: minPlan })}
                            </span>
                          ) : !available ? (
                            // Granted by the plan and not deliverable yet. The sentence
                            // names which of the two it is, because "not available"
                            // alone reads as a bug rather than as a channel that is
                            // coming.
                            <span className="chip chip-due">
                              {t('settings.notifications.notYet')}
                            </span>
                          ) : null}
                        </div>
                      )
                    })}
                  </fieldset>

                  <fieldset className="flex flex-col gap-2">
                    <legend className="label">{t('settings.notifications.quiet')}</legend>
                    <p className="text-caption text-muted">
                      {t('settings.notifications.quietHint')}
                    </p>
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
                            // clearing either clears both. A window with one end would
                            // be a row the migration's own check constraint refuses.
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
                              quiet_end_hour:
                                event.target.value === '' ? null : Number(event.target.value),
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
                <p className="text-label text-muted">{t('settings.language.intro')}</p>
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
            </>
          )}

          {section === 'calendar' && <ConnectionsPanel />}

          {section === 'devices' && <DevicesPanel />}

          {section === 'appearance' && (
            <>
              <Section title={t('settings.appearance.title')}>
                <p className="text-label text-muted">{t('settings.appearance.intro')}</p>
                <div className="card flex flex-col gap-2 p-4">
                  <label className="label" htmlFor="settings-theme">
                    {t('settings.appearance.label')}
                  </label>
                  <select
                    id="settings-theme"
                    className="field max-w-64"
                    value={row.theme}
                    onChange={(event) =>
                      write({ theme: event.target.value as 'system' | 'light' | 'dark' })
                    }
                  >
                    <option value="system">{t('settings.appearance.system')}</option>
                    <option value="light">{t('settings.appearance.light')}</option>
                    <option value="dark">{t('settings.appearance.dark')}</option>
                  </select>
                </div>
              </Section>

              {/* The one section this build does not own. It is named rather than
                  omitted, because a student looking for it would otherwise conclude
                  it does not exist. */}
              <Section title={t('settings.later.title')}>
                <p className="text-label text-muted">{t('settings.later.intro')}</p>
                <ul className="card flex flex-col gap-3 p-4">
                  <li className="flex flex-col gap-1">
                    <span className="text-body">{t('settings.later.byok')}</span>
                    <span className="meta">{t('settings.later.byokPhase')}</span>
                  </li>
                </ul>
              </Section>
            </>
          )}
        </div>
      </div>

      <PageFooter
        left={<span className="uppercase">{footerZoneLabel}</span>}
        right={<span className="uppercase">{today}</span>}
      />
    </div>
  )
}

/**
 * Who this account is.
 *
 * The address and the plan's name, read from the session and from `GET /usage`.
 * There is no display name to show: the product never asks for one, and deriving one
 * from the address would be a fact the app invented. The plan is the account's own,
 * and it is the answer to the question a student arrives at this section with.
 *
 * Both facts are read-only, so their labels are the mono eyebrow layer rather than
 * `.label`: a control's label sits above a control, and a label over a value reads
 * as a field the student can type into.
 */
function AccountIdentity() {
  const { t } = useTranslation()
  const session = useSession()
  const usage = useUsage()

  return (
    <Section title={t('settings.section.account')}>
      <p className="text-label text-muted">{t('settings.account.intro')}</p>
      <div className="card flex flex-col gap-4 p-6">
        <div className="flex flex-col gap-1">
          <span className="eyebrow">{t('settings.account.email')}</span>
          <span className="text-body break-words">
            {session.status === 'signed-in' ? session.user.email : ''}
          </span>
        </div>
        <div className="flex flex-col gap-1">
          <span className="eyebrow">{t('settings.account.plan')}</span>
          {/* The plan's name as the server reports it (`usage.name` is the matrix's
              display name, e.g. "Free"), or a loading word while the answer is in
              flight — a placeholder name would be a wrong one. */}
          <span className="text-body">
            {usage.data?.name ?? (usage.isPending ? t('states.loading') : '')}
          </span>
        </div>
      </div>
    </Section>
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
 *  * **What happens next is the server's, not this screen's.** The request is the deletion:
 *    the server revokes every session including this one and releases the account's identity,
 *    so the screen clears the local token store rather than announcing a window it would then
 *    have to keep track of. Nothing here computes a date or describes a state the server has
 *    not already entered (A362).
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
      <p className="text-label text-muted">{t('settings.data.intro')}</p>
      <div className="card flex flex-col gap-4 p-6">
        <p className="text-small text-muted">{t('settings.data.exportHint')}</p>
        <div className="flex flex-wrap gap-2">
          {EXPORT_FORMATS.map((format) => (
            <button
              key={format}
              type="button"
              className="btn btn-secondary"
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
            className="btn btn-secondary self-start"
            onClick={() => {
              setError(null)
              setConfirming(true)
            }}
          >
            {t('settings.data.deleteStart')}
          </button>
        ) : (
          <div className="flex flex-col gap-3">
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
                className="btn btn-danger"
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
                className="btn btn-secondary"
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
