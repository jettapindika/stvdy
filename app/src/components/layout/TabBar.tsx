import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

import { Icon, type IconName } from '@/components/ui/Icon'
import { SyncStatus } from '@/features/sync/SyncStatus'

/**
 * Bottom navigation for phones.
 *
 * Each item extends the shared `.nav-item` treatment via `.nav-item-tab` rather
 * than restating it, so the two navs cannot drift apart.
 *
 * **The five destinations are the spec's** (`SPEC.md:664`): Today, Workspaces,
 * Study, Planner, More. Tasks and Calendar are views inside Planner, so they are
 * not tabs; Devices, the plan screen and Help are behind More, which is Settings.
 * The earlier bar read Today / Work / Calendar / Tasks / More and had no route to
 * Study at all.
 *
 * **The design's tabs are a 24px icon over a 10px label**, and that is what this
 * renders. The icon is additive rather than a replacement: the words stay, because
 * a bare glyph is not an accessible name and because the five labels are what a
 * student learns first. The 44px `min-height` from `.nav-item-tab` is kept, so the
 * design's visual density does not cost the touch target (R-03).
 *
 * **The band is `sticky bottom-0`, not `fixed`.** It is a flow sibling below
 * `main` (the shell root is a column on a phone, `.shell-phone`), so it takes the
 * height it needs and the content above it can never be painted over at the end of
 * the scroll — which is what the Phase 8 gate found when this was `fixed` and the
 * content reserved a hardcoded `3rem` for it. Both rows stay opaque, so content
 * scrolling *under* the band during a scroll is still hidden rather than showing
 * through.
 *
 * **The sync status chip sits in this band, above the five tabs, and not among
 * them** (A189): the sidebar and this bar are built from the same five labels, and
 * a sixth item would take width from every one of them. The chip is therefore a
 * sibling of the `nav`, which is also why `within(bar).getAllByRole('link')` still
 * counts five.
 */
export function TabBar() {
  const { t } = useTranslation()
  // Resolved inside the component because copy cannot live at module scope, where
  // there is no language yet.
  const items: { to: string; label: string; icon: IconName }[] = [
    { to: '/', label: t('nav.today'), icon: 'today' },
    { to: '/workspaces', label: t('nav.work'), icon: 'workspaces' },
    { to: '/study', label: t('nav.study'), icon: 'study' },
    { to: '/planner', label: t('nav.planner'), icon: 'planner' },
    { to: '/settings', label: t('nav.more'), icon: 'settings' },
  ]

  return (
    <div className="sticky bottom-0 z-40 flex flex-col md:hidden">
      {/* Its own row, opaque like the bar below it, so a line of content scrolling
          under the band is hidden rather than showing through the chip's row. */}
      <div className="flex justify-end bg-surface px-3 pt-2">
        <SyncStatus variant="chip" />
      </div>

      <nav
        aria-label={t('shell.nav.main')}
        // `overflow-x-auto` is load-bearing, and it is what the 200%-text-scale check
        // found. Each item is `flex: 1` with the UA's `min-width: auto`, so it cannot
        // shrink below its label; five labels fit 390px but at 200% type scale they
        // need more than the viewport. Without this the fifth item is painted past the
        // right edge with nothing to scroll, and it was the reader who enlarged their
        // type who lost it. At 100% nothing scrolls, so the visual is unchanged. A
        // focused item is scrolled into view by the browser, which keeps it reachable
        // by `Tab` as well as by swipe.
        className="flex overflow-x-auto overscroll-x-contain border-t border-border bg-surface pb-safe-bottom"
      >
        {items.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === '/'}
            className="nav-item nav-item-tab"
          >
            <Icon name={item.icon} size={24} className="nav-icon" />
            {item.label}
          </NavLink>
        ))}
      </nav>
    </div>
  )
}
