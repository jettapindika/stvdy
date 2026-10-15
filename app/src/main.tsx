import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { RouterProvider } from 'react-router'
import { MotionConfig } from 'motion/react'

import { installPlatform, isTauri } from './platform'
import { currentOsPlatform, desktopPlatform } from './platform/desktop'
import { mobilePlatform } from './platform/mobile'
import { router } from './router'
import { initializeAppearance } from './lib/appearance'
// Initialises i18next before the first render, so no screen ever paints a key
// instead of a sentence. Imported for its side effect, and first among the app
// imports so the resources are registered before any component module runs.
import './i18n'
import './styles.css'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
      // The sync engine owns freshness; refetching on every focus change would
      // fight it and cause redundant pulls.
      refetchOnWindowFocus: false,
    },
  },
})

function installRuntimePlatform() {
  if (!isTauri()) return
  const kind = currentOsPlatform()
  const adapter =
    kind === 'ios' || kind === 'android' ? mobilePlatform(kind) : desktopPlatform(kind)
  // The shell's sync commands need the session's credentials handed to them
  // (A186); they are the one capability that rides beside the adapter rather than
  // inside `Platform.sync`, whose seven members are fixed.
  installPlatform(adapter, { configureSync: adapter.configureSync })
}

const container = document.getElementById('root')
if (!container) throw new Error('#root is missing from index.html')

// Install the adapter before the first render so `platform.*` never falls back
// to the web implementation on a native build.
installRuntimePlatform()
initializeAppearance()

createRoot(container).render(
  <StrictMode>
    <MotionConfig reducedMotion="user">
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </MotionConfig>
  </StrictMode>,
)
