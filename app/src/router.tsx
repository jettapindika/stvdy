import { useTranslation } from 'react-i18next'
import { createBrowserRouter } from 'react-router'

import { App } from './App'
import { NotBuiltYet } from './components/ui/States'
import { DeepLinkHandler } from './features/auth/DeepLinkHandler'
import { GoogleCallbackRoute } from './features/auth/GoogleCallbackScreen'
import { SignInScreen } from './features/auth/SignInScreen'
import { SignUpScreen } from './features/auth/SignUpScreen'
import { VerifyEmailScreen } from './features/auth/VerifyEmailScreen'
import { MobilePlanScreen } from './features/billing/MobilePlanScreen'
import { PlanUsageScreen } from './features/billing/PlanUsageScreen'
import { DevicesScreen } from './features/devices/DevicesScreen'
import { StyleguideScreen } from './features/styleguide/StyleguideScreen'
import { SettingsScreen } from './features/settings/SettingsScreen'
import { TasksScreen } from './features/tasks/TasksScreen'
import { CheatsheetScreen } from './features/reports/CheatsheetScreen'
import { ReportScreen } from './features/reports/ReportScreen'
import { QuizScreen } from './features/quiz/QuizScreen'
import { StudyScreen } from './features/study/StudyScreen'
import { TodayScreen } from './features/today/TodayScreen'
import { WorkspaceListScreen } from './features/workspaces/WorkspaceListScreen'
import { WorkspaceScreen } from './features/workspaces/WorkspaceScreen'
import { platform, type PlatformKind } from './platform'

/**
 * The element `/billing` renders, chosen by platform (A322).
 *
 * A phone gets the read-only plan screen and the web/desktop builds get theirs.
 * The choice is made here, at the route, rather than inside the screen, so that a
 * purchase surface added to the web/desktop screen is not reachable from a phone
 * build at all — `config/billing_policy.toml` is a second lock, not the only one.
 *
 * A pure function of the kind so the decision is testable without a browser, the
 * way `formFactorForWidth` is.
 */
export function billingElementFor(kind: PlatformKind) {
  return kind === 'ios' || kind === 'android' ? <MobilePlanScreen /> : <PlanUsageScreen />
}

/**
 * `/billing`, with the platform read when the route renders.
 *
 * **Not** `billingElementFor(platform.kind())` at module scope, which is the
 * version that looks right and is wrong: this file is evaluated while `main.tsx`
 * is still importing, before `installRuntimePlatform()` installs the native
 * adapter, so at module scope `platform.kind()` answers `web` on an iPhone and the
 * phone would be handed the desktop screen. The same trap the language table
 * documents below — a value read before the thing that decides it exists.
 */
function BillingRoute() {
  return billingElementFor(platform.kind())
}

/**
 * A route whose screen is planned but not built yet.
 *
 * This file is a route table, not a component: it is evaluated once at module
 * scope, before any language is known, so a `t` call in the table itself would
 * freeze whichever language happened to load first. The table passes the keys and
 * this component resolves them at render, where a language exists.
 */
function NotBuiltRoute({ titleKey, whatKey }: { titleKey: string; whatKey: string }) {
  const { t } = useTranslation()
  return <NotBuiltYet title={t(titleKey)} what={t(whatKey)} />
}

/**
 * Routes.
 *
 * Every destination in the navigation resolves to a real screen. The ones that
 * are not built yet say so plainly and name what will be there, rather than
 * pretending to work: a nav item that leads to a broken or empty promise is the
 * defect R-24 describes.
 *
 * The three auth routes sit outside the `App` element on purpose. `App` is the
 * signed-in shell, and it redirects to `/signin` when there is no session; a
 * sign-in screen behind that gate could never be reached. They also carry no
 * sidebar or tab bar, because none of their destinations work before there is an
 * account to own the data.
 *
 * `/verify` is public for the same reason: the emailed link is opened on whatever
 * device is to hand, which is usually not the one that signed up, and often a
 * browser with no session at all.
 */
export const router = createBrowserRouter([
  {
    // Every route sits under this so a deep link that arrives anywhere in the app
    // is handled in one place: a running instance, or the cold start the shell
    // launched. It renders the outlet unless an auth ticket is being spent, in
    // which case it renders the callback screen over the whole app.
    element: <DeepLinkHandler />,
    children: [
      // The web build's Google callback lands here; the native shell delivers the
      // same ticket as a deep-link intent instead.
      { path: '/auth/callback', element: <GoogleCallbackRoute /> },
      { path: '/signin', element: <SignInScreen /> },
      { path: '/signup', element: <SignUpScreen /> },
      // The server mails the web link at `/verify-email?token=...`
      // (`auth/email_verify.rs`), so that is the path this screen must answer on.
      // `/verify` is where signup lands in-app, and both render the same screen.
      { path: '/verify-email', element: <VerifyEmailScreen /> },
      { path: '/verify', element: <VerifyEmailScreen /> },
      {
        path: '/',
        element: <App />,
        children: [
          { index: true, element: <TodayScreen /> },
          { path: 'workspaces', element: <WorkspaceListScreen /> },
          { path: 'workspaces/:workspaceId', element: <WorkspaceScreen /> },
          { path: 'study/:deckId', element: <StudyScreen /> },
          { path: 'quiz/:quizId', element: <QuizScreen /> },
          // The two printable documents. A report is edited in two steps and a
          // cheatsheet is read in a sandboxed frame, so neither is a panel: each
          // needs the whole width and its own URL to come back to.
          { path: 'reports/:reportId', element: <ReportScreen /> },
          { path: 'cheatsheets/:cheatsheetId', element: <CheatsheetScreen /> },
          {
            path: 'calendar',
            element: <NotBuiltRoute titleKey="nav.calendar" whatKey="routes.calendar.what" />,
          },
          {
            path: 'tasks',
            element: <TasksScreen />,
          },
          {
            path: 'settings',
            element: <SettingsScreen />,
          },
          {
            path: 'billing',
            element: <BillingRoute />,
          },
          // Reached from the sync status control rather than from the nav (A189):
          // the sidebar and the phone tab bar are both built from the same five
          // labels, and Settings — which the spec nests Devices under — is still a
          // `NotBuiltRoute` because Phase 9 owns it.
          {
            path: 'devices',
            element: <DevicesScreen />,
          },
          {
            path: 'styleguide',
            element: <StyleguideScreen />,
          },
          {
            path: '*',
            element: <NotBuiltRoute titleKey="routes.notFound.title" whatKey="routes.notFound.what" />,
          },
        ],
      },
    ],
  },
])
