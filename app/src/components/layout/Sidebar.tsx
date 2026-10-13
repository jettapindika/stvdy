import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

import { Wordmark } from '@/components/ui/BrandIllustration'
import { Icon, type IconName } from '@/components/ui/Icon'
import { useWorkspaces } from '@/api/queries/workspaces'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE } from '@/lib/time'
import { useFormFactor } from '@/lib/useFormFactor'

/**
 * Primary navigation, shared by the sidebar and the phone tab bar.
 *
 * **The items carry icons as of the redesign.** The earlier revision had none,
 * deliberately — five short labels were judged unambiguous and a thin-stroke icon
 * set was judged to be the tell of a generated interface. The design's navigation
 * item is an icon plus a label, and the design is the requirement, so the glyphs
 * are back. The `Icon` set is bundled and drawn for this product rather than
 * imported.
 *
 * The five destinations are the spec's (`SPEC.md:664-665`): Today, Workspaces,
 * Study, Planner, Settings — with Tasks and Calendar living *inside* Planner as
 * views rather than as items of their own. That is also why this rail no longer
 * lists Calendar and Tasks: they are `/planner?view=…`, and `/calendar` and
 * `/tasks` redirect there so an old link or a notification tap still lands.
 *
 * The two navs share `.nav-item` in styles.css (the tab bar extends it via
 * `.nav-item-tab`), so a change to the base treatment reaches both. That is what
 * keeps them from drifting apart.
 *
 * **This semester** is the course list under the rail's divider. It is built from
 * the real workspaces, and it renders nothing at all when there are none — the
 * alternative is a heading over an empty list, which reads as a loading failure.
 * A workspace's course code is shown only when it has one, because the field is
 * optional and inventing a code is the one thing this app may not do with data.
 *
 * The credit meter sits at the foot of the rail beside the sync control: it is the
 * one number on this rail, and the bottom is where a balance is looked for. Both
 * are links, so the number and the explanation of it are one `Tab` apart (R-32).
 */
export function Sidebar() {
  const { t } = useTranslation()
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const items: { to: string; label: string; icon: IconName }[] = [
    { to: '/', label: t('nav.today'), icon: 'today' },
    { to: '/workspaces', label: t('nav.workspaces'), icon: 'workspaces' },
    { to: '/study', label: t('nav.study'), icon: 'study' },
    { to: '/planner', label: t('nav.planner'), icon: 'planner' },
    { to: '/settings', label: t('nav.settings'), icon: 'settings' },
  ]

  return (
    <nav
      aria-label={t('shell.nav.main')}
      className="hidden w-54 shrink-0 flex-col gap-1 border-r border-border bg-surface px-3 py-4 pt-safe-top md:flex"
    >
      <Wordmark className="mb-4 px-2" />

      {items.map((item) => (
        <NavLink key={item.to} to={item.to} end={item.to === '/'} className="nav-item">
          <Icon name={item.icon} className="nav-icon" />
          {item.label}
        </NavLink>
      ))}

      <SemesterCourses />

      <SidebarFooter />
    </nav>
  )
}

/**
 * The course list under the rail's divider.
 *
 * The design draws "THIS SEMESTER" over three course links, each a code in mono
 * and a name in the UI face. Both come from the workspace row; the code is
 * optional (`workspaces.course_code` is nullable) and is omitted rather than
 * filled in when absent.
 *
 * **Nothing renders while the list is empty or still loading.** A heading over an
 * empty list is a promise the rail cannot keep, and the empty state for "no
 * workspaces yet" belongs on the Workspaces screen where the student can act on
 * it, not on a rail with no room to explain.
 */
function SemesterCourses() {
  const { t } = useTranslation()
  const isPhone = useFormFactor() === 'phone'
  const workspaces = useWorkspaces({ enabled: !isPhone })

  const rows = workspaces.data ?? []
  if (rows.length === 0) return null

  return (
    <div className="mt-2 flex flex-col gap-0.5 border-t border-border pt-4">
      <p className="eyebrow px-2 pb-2">{t('shell.semester')}</p>
      {rows.map((workspace) => (
        <NavLink
          key={workspace.id}
          to={`/workspaces/${workspace.id}`}
          className="flex min-h-10 flex-col justify-center gap-0.5 rounded-field px-2 py-1.5 hover:bg-surface-raised"
        >
          {workspace.course_code && (
            <span className="meta text-primary-text">{workspace.course_code}</span>
          )}
          {/* `title` is not decoration: the rail is 216px and a course name is longer
              than that, so the name truncates — and the harness exempts a `truncate`
              only when the full text is reachable another way. Without it the reader
              has no way to see which course they are looking at. */}
          <span className="truncate text-label text-text" title={workspace.name}>
            {workspace.name}
          </span>
        </NavLink>
      ))}
    </div>
  )
}

/**
 * The rail's closing note.
 *
 * The design puts a help line and a term label here. The term label ("AUTUMN
 * 2026") has no data source — no migration carries a semester or term, and the
 * product has no academic-calendar concept — so it is not invented. The line
 * shows the account's real timezone instead, which the app does have and which is
 * the more useful fact to have permanently on screen in a product whose whole
 * planner is timezone-sensitive.
 */
function SidebarFooter() {
  const { t } = useTranslation()
  const session = useSession()
  const zone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  return (
    <div className="mt-4 flex flex-col gap-2 border-t border-border pt-4">
      <p className="flex items-start gap-2 px-2 text-caption text-muted">
        <Icon name="help" size={16} className="mt-0.5 shrink-0" />
        {t('shell.railNote')}
      </p>
      <p className="meta px-2 uppercase">{zone}</p>
    </div>
  )
}
