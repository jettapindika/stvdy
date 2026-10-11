import { createBrowserRouter } from 'react-router'

import { App } from './App'
import { NotBuiltYet } from './components/ui/States'
import { SignInScreen } from './features/auth/SignInScreen'
import { SignUpScreen } from './features/auth/SignUpScreen'
import { VerifyEmailScreen } from './features/auth/VerifyEmailScreen'
import { StyleguideScreen } from './features/styleguide/StyleguideScreen'
import { TodayScreen } from './features/today/TodayScreen'
import { WorkspaceListScreen } from './features/workspaces/WorkspaceListScreen'
import { WorkspaceScreen } from './features/workspaces/WorkspaceScreen'

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
  { path: '/signin', element: <SignInScreen /> },
  { path: '/signup', element: <SignUpScreen /> },
  { path: '/verify', element: <VerifyEmailScreen /> },
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <TodayScreen /> },
      { path: 'workspaces', element: <WorkspaceListScreen /> },
      { path: 'workspaces/:workspaceId', element: <WorkspaceScreen /> },
      {
        path: 'calendar',
        element: (
          <NotBuiltYet
            title="Calendar"
            what="a week and month view of classes, deadlines, and the study blocks the planner proposes, plus Google Calendar and ICS connections"
          />
        ),
      },
      {
        path: 'tasks',
        element: (
          <NotBuiltYet
            title="Tasks"
            what="tasks and subtasks across your workspaces, with quick capture and an assistant that proposes breakdowns you confirm before they save"
          />
        ),
      },
      {
        path: 'settings',
        element: (
          <NotBuiltYet
            title="Settings"
            what="timezone and study hours, daily limits, reminder channels, connected calendars, and your signed-in devices"
          />
        ),
      },
      {
        path: 'billing',
        element: (
          <NotBuiltYet
            title="Plan and usage"
            what="your current plan, when the prepaid period ends, and your remaining credits. Purchase screens live on the web and desktop builds only."
          />
        ),
      },
      {
        path: 'styleguide',
        element: <StyleguideScreen />,
      },
      {
        path: '*',
        element: (
          <NotBuiltYet title="Not found" what="nothing: this address does not match a screen" />
        ),
      },
    ],
  },
])
