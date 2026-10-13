import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { Icon } from '@/components/ui/Icon'
import { Wordmark } from '@/components/ui/BrandIllustration'
import { CreditMeter } from '@/features/billing/CreditMeter'
import { SyncStatus } from '@/features/sync/SyncStatus'
import { useSession } from '@/features/auth/useSession'
import { platform } from '@/platform'

/**
 * The desktop top bar.
 *
 * The design puts a global search field on the left and the account group on the
 * right, and every desktop frame carries it. This is the one piece of chrome the
 * app did not have at all before the redesign: credits lived at the foot of the
 * sidebar, and there was no search anywhere.
 *
 * **The search field is a button, not an input.** Typing happens in the command
 * palette, which is the same control the `⌘K` badge advertises. Two places to type
 * would mean two result lists, two focus traps and two sets of keyboard bindings,
 * and the palette is the one that can show grouped results and keyboard guidance.
 * The field is therefore a labelled button that opens it, and it carries the
 * shortcut hint rather than pretending to accept text it would not keep.
 *
 * **The shortcut is shown per platform.** `⌘K` on macOS, `Ctrl K` elsewhere —
 * `platform.shortcutLabel` owns that, so this file never decides which modifier a
 * reader has.
 *
 * The credit meter and the sync control sit here rather than at the foot of the
 * sidebar. Both are links, so the number and its explanation are one `Tab` apart,
 * and both are in the bar's tab order before the page content.
 *
 * **The drag strip is present but inert.** A desktop native window leaves a drag
 * region (`SPEC.md:89`), and the top bar is where it lives: `.drag-region` sits
 * above the row. `src-tauri/tauri.conf.json` sets no `decorations: false`, so the
 * window is decorated today and the OS title bar is what moves it — the strip is
 * zero-height in every current build, and the web build and a phone pay nothing
 * for it. The rule and the class are in place so that turning on a frameless
 * window is a config change plus adding `.window-frameless` to the shell root, not
 * a redesign of the bar.
 */
export function TopBar({ onOpenPalette }: { onOpenPalette: () => void }) {
  const { t } = useTranslation()

  return (
    <header className="topbar hidden md:flex">
      <div className="drag-region" aria-hidden="true" />
      {/* `flex-wrap` is the 200%-text accommodation: at a 32px root the search field
          and the account group together need more than the bar's width, and the
          alternatives are all worse — squeezing the field clipped its own label
          (measured: 71px against 197px of content), and letting the row overflow
          scrolled the whole page sideways at both viewports. Wrapping puts the
          account group on a second line, which is a taller bar and no lost control. */}
      <div className="flex w-full flex-wrap items-center gap-4">
        {/* The wordmark appears here only on a phone, where there is no sidebar to
            carry it. On the desktop the sidebar's own wordmark is the brand, and a
            second one 32px away would be noise. */}
        <button type="button" onClick={onOpenPalette} className="search-field max-w-xl">
          <Icon name="search" className="shrink-0" />
          <span className="min-w-0 flex-1 truncate text-muted">{t('palette.searchPlaceholder')}</span>
          <kbd className="kbd shrink-0">{platform.shortcutLabel('K')}</kbd>
        </button>

        <div className="ml-auto flex flex-wrap items-center gap-4">
          {/* The chip variant is the phone's, where it sits above the tab bar with
              a row to itself. In the bar it is rendered compact: the full control
              carries "Last synced …" and a pending count, which is three lines in a
              80px bar and made the whole header twice its height. */}
          <SyncStatus variant="compact" />
          <CreditMeter variant="pill" />
          <AccountMenu />
        </div>
      </div>
    </header>
  )
}

/**
 * The phone's app header.
 *
 * The design's phone frames carry a 72px bar with the wordmark, a search action
 * and the avatar, and a separate 54px row under it holding the credit meter. The
 * two are separate rows because the credit pill is wide: at 390px it takes most of
 * a row, and squeezing it beside the wordmark is what pushed the header past the
 * viewport in the earlier layout.
 *
 * The status bar the design draws above this ("10:30", signal, battery) is OS
 * chrome and is not built.
 */
export function PhoneHeader({ onOpenPalette }: { onOpenPalette: () => void }) {
  const { t } = useTranslation()

  return (
    <div className="flex flex-col border-b border-border bg-surface md:hidden">
      {/* `flex-wrap` is the 200%-text accommodation, and it is the same one the desktop
          bar makes. At a 32px root the wordmark, the search button and the avatar
          together measure 414px against a 390px phone, and the overflow is reported by
          *this* row rather than by any one child — the avatar is 88px of it because
          `.avatar` is a `2.75rem` circle and rems grow with the root size. Wrapping puts
          the controls on a second line: a taller header, and every control still
          reachable, which is the trade the alternatives do not offer (shrinking the
          avatar clips its initials; letting it overflow scrolls the page sideways). */}
      <div className="flex flex-wrap items-center gap-3 px-5 py-3 pt-safe-top">
        <Wordmark />
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={onOpenPalette}
            aria-label={t('palette.open')}
            className="flex size-11 items-center justify-center rounded-field text-muted hover:bg-surface-raised"
          >
            <Icon name="search" size={20} />
          </button>
          <AccountMenu compact />
        </div>
      </div>
      {/* Its own row, left-aligned, so the pill does not compete with the header
          controls for width on a narrow phone. It is the **pill** variant, which is
          what the phone frame draws here (`Account and timezone` → `Credit meter`,
          153x34): the same capsule as the desktop bar, with the level bar in it. */}
      <div className="px-5 pb-3">
        <CreditMeter variant="pill" />
      </div>
    </div>
  )
}

/**
 * The account group: the reader's name and their initials in a circle.
 *
 * The design draws a name, a 44px avatar and a menu affordance. The product never
 * asks for a display name, so the account group shows the address's local part
 * tidied into a name — `design.pass` becomes `Design Pass`. That is a *reading* of
 * a fact the account holds rather than a new fact, which is the line the honesty
 * rules draw: inventing a name would be the latter. The full address is the link's
 * accessible name, so nothing is lost to a screen reader.
 *
 * The group links to Settings' account section rather than opening a menu of its
 * own: there is exactly one destination behind it, and a one-item menu is a click
 * for nothing.
 */
function AccountMenu({ compact = false }: { compact?: boolean }) {
  const { t } = useTranslation()
  const session = useSession()
  const email = session.status === 'signed-in' ? session.user.email : ''
  const display = displayNameOf(email)

  return (
    <Link
      to="/settings/account"
      className="flex min-h-11 items-center gap-3 rounded-field px-1 hover:bg-surface-raised"
    >
      {!compact && display.length > 0 && (
        <span className="hidden max-w-40 truncate text-label text-text lg:block" title={display}>
          {display}
        </span>
      )}
      <span className="avatar" aria-hidden="true">
        {initialsOf(email)}
      </span>
      <span className="sr-only">
        {email.length > 0 ? t('settings.account.forAccount', { email }) : t('settings.section.account')}
      </span>
    </Link>
  )
}

/**
 * A readable name from an email address, or an empty string.
 *
 * The local part with its separators turned into spaces and each word
 * capitalised: `rani.putri@...` reads `Rani Putri`. A local part that is already
 * one word is capitalised as it stands. Empty for an empty address, which is the
 * state before a session resolves — the bar then shows the avatar alone rather
 * than a name that is not the reader's.
 *
 * Exported because the Today greeting addresses the student by the same reading:
 * two derivations of one name would eventually greet them differently from the
 * name in the top bar, and the second one is the one nobody re-checks.
 */
export function displayNameOf(email: string): string {
  const local = email.split('@')[0] ?? ''
  return local
    .split(/[._-]+/)
    .filter((part) => part.length > 0)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ')
}

/**
 * Up to two initials from an email address.
 *
 * A local part is often `first.last` or `first_last`; those separators are where
 * the two initials actually are, so they are preferred over the first two
 * characters of the whole thing.
 */
function initialsOf(email: string): string {
  const local = email.split('@')[0] ?? ''
  const parts = local.split(/[._-]+/).filter((part) => part.length > 0)
  if (parts.length >= 2) {
    return ((parts[0]?.[0] ?? '') + (parts[1]?.[0] ?? '')).toUpperCase()
  }
  return local.slice(0, 2).toUpperCase()
}
