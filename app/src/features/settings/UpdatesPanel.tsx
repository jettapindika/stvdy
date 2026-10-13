import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Section } from '@/components/layout/Section'
import { platform } from '@/platform'
import type { UpdateProgress, UpdateStatus } from '@/platform/types'

/**
 * Settings → Updates: the one screen this phase adds, and the one place the
 * product tells the student that a new build exists.
 *
 * **It says only what the manifest said** (A336, roadmap gate 7a). The version and
 * the notes are the two facts the manifest carries, and they are rendered as it
 * carried them: no "what's new" line composed here, no download size, no
 * percentage, and no "security update" label unless the release notes say so. The
 * tempting additions are all claims about data this app does not have — the
 * plugin's own download events make `contentLength` optional, so a progress bar
 * would be real on some downloads and invented on others.
 *
 * **The update is offered, never applied.** Nothing here downloads on mount and
 * nothing installs without a click. An update that installs itself is a restart
 * the student did not agree to, and this phase's whole subject is that the
 * decision to run new code is the user's.
 *
 * **A failed check is not "up to date".** `error` is a state of its own with its
 * own words and a retry, because a check that silently failed while the screen
 * read "You are up to date" is the one bug in this panel a student cannot detect.
 *
 * **`unsupported` renders nothing at all.** The web build has no installer and
 * mobile updates ship through TestFlight and Play (A335), so the section is absent
 * rather than present-and-disabled: a greyed-out "Check for updates" on a phone is
 * a promise this platform cannot keep, and a student who sees it will look for the
 * reason it is off.
 */
export function UpdatesPanel() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<UpdateStatus | null>(null)
  const [progress, setProgress] = useState<UpdateProgress | null>(null)
  const [checking, setChecking] = useState(false)
  const [installing, setInstalling] = useState(false)

  const check = async () => {
    setChecking(true)
    setProgress(null)
    // `checkForUpdate` never rejects: a failure is `{kind: 'error'}`, so this
    // screen cannot render "up to date" over a check that did not run.
    setStatus(await platform.checkForUpdate())
    setChecking(false)
  }

  const install = async () => {
    setInstalling(true)
    setProgress({ phase: 'downloading' })
    try {
      await platform.installUpdate(setProgress)
    } catch (failure) {
      // An install that failed has to say so. The update stays offered, so the
      // retry is the same button rather than a second one.
      setStatus({
        kind: 'error',
        message: failure instanceof Error ? failure.message : String(failure),
      })
      setProgress(null)
    } finally {
      setInstalling(false)
    }
  }

  // Nothing to offer on a platform with no update channel. Checked before the
  // section renders, so the whole section is absent rather than empty.
  if (platform.kind() === 'web' || platform.kind() === 'ios' || platform.kind() === 'android') {
    return null
  }

  const available = status?.kind === 'available' ? status : null

  return (
    <Section title={t('settings.updates.title')}>
      {/* The one line that says what this section is for, and it stays whether or
          not a check has run: the section's own description rather than a card
          paragraph that disappears the moment there is a version to report. */}
      <p className="text-label text-muted">{t('settings.updates.hint')}</p>

      <div className="card flex flex-col gap-3 p-4">
        {/* The state word first, so the state is never carried by colour alone and
            a screen reader reaches the fact before the control. */}
        {status?.kind === 'up-to-date' ? (
          <p className="text-body">
            {t('settings.updates.upToDate', { version: status.current })}
          </p>
        ) : null}

        {available !== null ? (
          <>
            <p className="text-body">
              {t('settings.updates.available', {
                version: available.version,
                current: available.current,
              })}
            </p>
            {available.notes !== null ? (
              // The manifest's notes, verbatim: `whitespace-pre-wrap` so the line
              // breaks the release author wrote survive, and no markdown renderer,
              // because notes are text and not markup from an endpoint.
              <p className="text-small text-muted whitespace-pre-wrap">{available.notes}</p>
            ) : null}
          </>
        ) : null}

        {status?.kind === 'error' ? (
          <p className="text-small text-danger" role="alert">
            {t('settings.updates.failed', { message: status.message })}
          </p>
        ) : null}

        {progress !== null ? (
          <p className="text-small text-muted" aria-live="polite">
            {progress.phase === 'downloading'
              ? t('settings.updates.downloading')
              : progress.phase === 'installing'
                ? t('settings.updates.installing')
                : t('settings.updates.restartRequired')}
          </p>
        ) : null}

        <div className="flex flex-wrap gap-2">
          {available === null ? (
            <button
              type="button"
              className="btn btn-secondary"
              disabled={checking || installing}
              onClick={() => void check()}
            >
              {checking ? t('settings.updates.checking') : t('settings.updates.check')}
            </button>
          ) : (
            <button
              type="button"
              className="btn btn-primary"
              disabled={installing}
              onClick={() => void install()}
            >
              {/* Labelled as the update it is (gate 7e): never a "fix" and never a
                  "security patch", because the manifest did not say that. */}
              {installing ? t('settings.updates.installing') : t('settings.updates.install')}
            </button>
          )}
        </div>
      </div>
    </Section>
  )
}
