import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

import { SyncStatus } from '@/features/sync/SyncStatus'

/**
 * Bottom navigation for phones.
 *
 * Each item extends the shared `.nav-item` treatment via `.nav-item-tab` rather
 * than restating it, so the two navs cannot drift apart. The active marker is a
 * top rule instead of a left bar, because this bar sits at the bottom edge and
 * the marker points into the content from the edge it is on.
 *
 * `min-height: 2.75rem` from the base rule gives the 44px touch target (R-03),
 * and `pb-safe-bottom` clears the home indicator.
 *
 * **The band is `sticky bottom-0`, not `fixed`.** It is a flow sibling below
 * `main` (the shell root is a column on a phone, `.shell-phone`), so it takes the
 * height it needs and the content above it can never be painted over at the end of
 * the scroll — which is what the Phase 8 gate found when this was `fixed` and the
 * content reserved a hardcoded `3rem` for it. Both rows stay opaque, so content
 * scrolling *under* the band during a scroll is still hidden rather than showing
 * through.
 *
 * The short labels differ from the sidebar's long ones, which is why they are
 * listed here rather than derived from it. They are resolved inside the component
 * because copy cannot live at module scope, where there is no language yet.
 *
 * **The sync status chip sits in this band, above the five labels, and not among
 * them** (A189). Devices is reached from the control rather than from a sixth nav
 * item: the sidebar and this bar are built from the same five labels, and the
 * measured widths are exactly 5 x 78px on a 390px phone. A sixth item would take
 * width from every one of them, and the 200%-text-scale check found this bar
 * scrolling horizontally already. The chip is therefore a sibling of the `nav`,
 * which is also why `within(bar).getAllByRole('link')` still counts five.
 */
export function TabBar() {
  const { t } = useTranslation()
  const items = [
    { to: '/', short: t('nav.today') },
    { to: '/workspaces', short: t('nav.work') },
    { to: '/calendar', short: t('nav.calendar') },
    { to: '/tasks', short: t('nav.tasks') },
    { to: '/settings', short: t('nav.more') },
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
        // shrink below its label; five labels at 0.75rem fit 390px (5 x 78px measured)
        // but at 200% type scale they need ~435px. The bar is `position: fixed` with
        // `overflow: visible`, so the fifth item was painted 45px past the right edge of
        // the viewport with nothing to scroll: "More" (Settings and Billing) was simply
        // unreachable, and it was the reader who enlarged their type who lost it. The bar
        // now scrolls horizontally instead of overflowing, and at 100% nothing scrolls,
        // so the visual is unchanged. A focused item is scrolled into view by the browser,
        // which keeps it reachable by `Tab` as well as by swipe.
        className="flex overflow-x-auto overscroll-x-contain border-t border-border bg-surface pb-safe-bottom"
      >
        {items.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.to === '/'}
            className="nav-item nav-item-tab"
          >
            {item.short}
          </NavLink>
        ))}
      </nav>
    </div>
  )
}
