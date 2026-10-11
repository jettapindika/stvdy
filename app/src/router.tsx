import { createBrowserRouter } from 'react-router'

import { App } from './App'
import { Placeholder } from './components/Placeholder'

/** Phase 0 routes are placeholders; later phases replace each element. */
export const router = createBrowserRouter([
  {
    path: '/',
    element: <App />,
    children: [
      { index: true, element: <Placeholder title="Today" phase={9} /> },
      { path: 'workspaces', element: <Placeholder title="Workspaces" phase={1} /> },
      { path: 'calendar', element: <Placeholder title="Calendar" phase={10} /> },
      { path: 'tasks', element: <Placeholder title="Tasks" phase={9} /> },
      { path: 'settings', element: <Placeholder title="Settings" phase={9} /> },
      { path: 'billing', element: <Placeholder title="Plan and usage" phase={13} /> },
      { path: '*', element: <Placeholder title="Not found" phase={0} /> },
    ],
  },
])
