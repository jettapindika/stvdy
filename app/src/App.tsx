import { Outlet } from 'react-router'

import { Shell } from './components/layout/Shell'

export function App() {
  return (
    <Shell>
      <Outlet />
    </Shell>
  )
}
