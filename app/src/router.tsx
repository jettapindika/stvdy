import { useTranslation } from 'react-i18next'
import { createBrowserRouter, useParams } from 'react-router'

import { App } from './App'
import { NotBuiltYet } from './components/ui/States'
import { DeepLinkHandler } from './features/auth/DeepLinkHandler'
import { GoogleCallbackRoute } from './features/auth/GoogleCallbackScreen'
import { SignInScreen } from './features/auth/SignInScreen'
import { SignUpScreen } from './features/auth/SignUpScreen'
import { VerifyEmailScreen } from './features/auth/VerifyEmailScreen'
import { MobilePlanScreen } from './features/billing/MobilePlanScreen'
import { CheckoutScreen } from './features/billing/CheckoutScreen'
import { PlanUsageScreen } from './features/billing/PlanUsageScreen'
import { PricingScreen } from './features/billing/PricingScreen'
import { CalendarScreen } from './features/calendar/CalendarScreen'
import { DevicesScreen } from './features/devices/DevicesScreen'
import { DashboardScreen } from './features/dashboard/DashboardScreen'
import { StyleguideScreen } from './features/styleguide/StyleguideScreen'
import { SettingsScreen } from './features/settings/SettingsScreen'
import { TasksScreen } from './features/tasks/TasksScreen'
import { CheatsheetScreen } from './features/reports/CheatsheetScreen'
import { ReportScreen } from './features/reports/ReportScreen'
import { QuizScreen } from './features/quiz/QuizScreen'
import { PlannerScreen } from './features/planner/PlannerScreen'
import { StudyScreen } from './features/study/StudyScreen'
import { SyllabusReviewScreen } from './features/syllabus/SyllabusReviewScreen'
import { SyllabusScreen } from './features/syllabus/SyllabusPanel'
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
 * `/syllabus/:draftId`, with the id read from the path.
 *
 * The route table is evaluated at module scope, so the param cannot be read there:
 * this wrapper is what turns the path segment into the prop the review screen
 * takes, the same shape `BillingRoute` uses for the platform.
 */
function SyllabusReviewRoute() {
  const { draftId } = useParams()
  return <SyllabusReviewScreen draftId={draftId ?? ''} />
}

/**
 * `/billing/checkout/:paymentId`, with the id read from the path.
 *
 * The route table is evaluated at module scope, so the param cannot be read there:
 * this wrapper is what turns the path segment into the prop the screen takes, the
 * same shape `SyllabusReviewRoute` and `BillingRoute` use.
 */
function CheckoutRoute() {
  const { paymentId } = useParams()
  return <CheckoutScreen paymentId={paymentId ?? ''} />
}

/**
 * The routes that only exist where purchase UI is allowed.
 *
 * **The guard is a build-time constant, and that is the whole mechanism** (A322).
 * `__PURCHASE_UI__` is a `define` in `vite.config.ts`, so in the store build this
 * expression is `false ? [...] : []`, the minifier drops the dead branch, and the
 * references to `PricingScreen` and `CheckoutScreen` disappear with it. Those two
 * modules then have no remaining reference and are dropped, and with them their
 * transitive dependencies — `api/queries/billing.ts`, which holds the
 * `POST /billing/checkout` call, and `features/billing/checkout.ts`.
 *
 * A `platform.kind()` check here would not do any of that. It is a run-time
 * condition, so the branch is dead for a phone at run time while every module it
 * names is still in the file — which is exactly the state this replaces, and why
 * `scripts/check-no-purchase-ui.mjs` was red on the shipped bundle. The claim in
 * A322 ("the checkout component is tree-shaken out of the mobile chunk") is true
 * only from here on.
 *
 * The array is spread into the route children rather than the entries being
 * written inline, so the mobile build's route table simply has two fewer entries
 * and no conditional element remains to be resolved at run time.
 */
const purchaseRoutes = __PURCHASE_UI__
  ? [
      // The pricing page (A305). It renders on web and desktop only, and it
      // says so itself rather than being hidden: on a phone the policy forbids
      // purchase UI, and a route that vanished would be a dead link from
      // whatever named it. In the store build the route is not registered at
      // all, so there is no link to be dead.
      { path: 'pricing', element: <PricingScreen /> },
      // The QRIS checkout screen, keyed by the payment it is about. A route
      // rather than a panel, because a student who reloads, or opens the link
      // on another device, must land back on the same live invoice — and the
      // poll is what recovers a webhook the gateway never delivered (A301).
      { path: 'billing/checkout/:paymentId', element: <CheckoutRoute /> },
    ]
  : []

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
          // The calendar: the week and month views, and the connections that feed
          // them. It is a real screen as of Phase 10, so the `NotBuiltRoute` that
          // named the phase is gone with its copy.
          { path: 'calendar', element: <CalendarScreen /> },
          {
            path: 'tasks',
            element: <TasksScreen />,
          },
          // The progress dashboard. It is reached from the plan section of Settings
          // rather than from the nav: the rail and the tab bar share five labels
          // (A189), and a sixth would take width from all of them.
          {
            path: 'dashboard',
            element: <DashboardScreen />,
          },
          // The planner and the syllabus import (Phase 11). The review screen is a
          // route rather than a panel because it is the phase's honesty gate: the
          // student reads the extraction, edits it, and confirms — and a URL they
          // can come back to is what makes a long review survivable.
          {
            path: 'planner',
            element: <PlannerScreen />,
          },
          {
            path: 'syllabus',
            element: <SyllabusScreen />,
          },
          {
            path: 'syllabus/:draftId',
            element: <SyllabusReviewRoute />,
          },
          {
            path: 'settings',
            element: <SettingsScreen />,
          },
          {
            path: 'billing',
            element: <BillingRoute />,
          },
          // The pricing page and the checkout screen, where the build allows
          // purchase UI at all (A322). Empty in the store build.
          ...purchaseRoutes,
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
