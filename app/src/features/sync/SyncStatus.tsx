import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { useSyncStatus } from '@/api/queries/sync'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { platform, type SyncStatus as SyncState } from '@/platform'

/**
 * What sync is doing, in one control, rendered by the shell in both form factors.
 *
 * The placement is A189's, and it is not a style choice: Devices is reached from
 * here rather than from a sixth nav item, because the sidebar and the phone tab bar
 * are both built from the same five labels, and a sixth would cost every item its
 * 44px target at 200% text scale. So the control is one component with two layouts
 * — a row at the foot of the sidebar and a chip above the tab bar — and both link
 * to `/devices`, which carries the device list, the "Last synced" detail and
 * "Reset local data and resync".
 *
 * **The honesty rules are enforced here, at the last place before the user reads
 * them** (roadmap gate 7, A187). Three of them, and each is a real defect if it
 * fails:
 *
 * 1. **A pending write can never be rendered as "Synced".** The shell already
 *    derives the status that way, and this component derives it *again* — a shell
 *    that reported `synced` with a non-empty outbox (an older build, a bug) still
 *    cannot make this control claim the write was accepted. A rejection outranks a
 *    pending write, because a refused change is the more urgent fact.
 * 2. **"Last synced" is the stored timestamp**, formatted in the reader's own
 *    zone, never a clock read at render time: a control that said "Last synced
 *    just now" would be inventing a fact about work it did not do.
 * 3. **A rejection is shown with the server's own `code` and as "Not saved"**, not
 *    counted as synced. The code is the server's word for what it refused; the
 *    sentence around it is this product's, so it is translated.
 *
 * The state is carried by a word, never by colour alone, and the word is inside the
 * link so it is part of the control's accessible name.
 */
export function SyncStatus({
  variant = 'sidebar',
}: {
  variant?: 'sidebar' | 'chip' | 'compact'
}) {
  const { t } = useTranslation()
  const { report, failed } = useSyncStatus()
  const session = useSession()

  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const words: Record<SyncState, string> = {
    synced: t('sync.state.synced'),
    syncing: t('sync.state.syncing'),
    offline: t('sync.state.offline'),
    needs_attention: t('sync.state.needsAttention'),
    revoked: t('sync.state.revoked'),
  }
  // The tone repeats what the word already says, so the control reads correctly in
  // monochrome (gate 5d).
  const tones: Record<SyncState, string> = {
    synced: 'chip chip-ready',
    syncing: 'chip chip-working',
    offline: 'chip chip-due',
    needs_attention: 'chip chip-failed',
    revoked: 'chip chip-failed',
  }

  // Rule 1, and the reason a rejection outranks a pending write: with a refused
  // change in the last pass, "Needs attention" is the more urgent truth.
  const state: SyncState | null =
    report === null
      ? null
      : report.state === 'needs_attention' || report.state === 'revoked'
        ? report.state
        : report.outbox > 0
          ? 'syncing'
          : report.state

  const word = failed ? t('sync.unavailable') : state === null ? t('states.loading') : words[state]
  const tone = failed || state === null ? 'chip' : tones[state]


  // Rules 2 and 3. The lines are built in the order of how much they matter: a
  // pending count first, then a refusal, then when this device last synced.
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  const details: string[] = []
  /**
   * The lines the **compact** control still carries.
   *
   * It drops "Last synced" and never the two facts a state word cannot imply. Gate 7(d)
   * requires a refusal to be shown *with the server's own code* — "Needs attention" is a
   * state, not the reason — and gate 7(b) requires a pending write never to read as
   * settled. Both are facts the reader acts on; the timestamp is context. So the top bar
   * stays one line in the ordinary case and grows for exactly the states that have
   * something to say.
   */
  const essentials: string[] = []
  if (report !== null) {
    if (report.outbox > 0) {
      const line = t('sync.pending', { count: report.outbox })
      details.push(line)
      essentials.push(line)
    }
    if (report.code !== null) {
      const line = t('sync.rejected', { code: report.code })
      details.push(line)
      essentials.push(line)
    }
    if (report.lastSyncedAt !== null) {
      details.push(
        t('sync.lastSynced', {
          when: formatInZone(new Date(report.lastSyncedAt), timezone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
        }),
      )
    } else {
      // A build with no local store has nothing that could have synced, so it says
      // that rather than claiming a device-side time it can never have (A14).
      details.push(platform.runtime() === 'web' ? t('sync.onlineOnly') : t('sync.neverSynced'))
    }
  }

  return (
    <Link
      to="/devices"
      className={
        variant === 'compact'
          ? // The top bar's: the state word, plus the two lines `essentials` keeps.
            // It is a row rather than a column so the ordinary case is one line tall.
            'flex min-h-11 flex-wrap items-center gap-x-2 focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring'
          : variant === 'chip'
            ? 'flex min-h-11 max-w-64 flex-col items-end justify-center gap-0.5 rounded-card border border-border bg-surface px-2 py-1 text-right hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring'
            : 'flex min-h-11 flex-col justify-center gap-1 rounded-field px-2 py-1 hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring'
      }
    >
      <span className={tone} role="status">
        {word}
      </span>
      {(variant === 'compact' ? essentials : details).map((detail, index) => (
        <span key={index} className="meta">
          {detail}
        </span>
      ))}
      {/* The control's destination, in the control's own accessible name: the
          visible text is a state, and a state alone does not say that this opens
          the device list. */}
      <span className="sr-only">{t('nav.devices')}</span>
    </Link>
  )
}
