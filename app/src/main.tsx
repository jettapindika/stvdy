import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { RouterProvider } from 'react-router'

import { installPlatform, isTauri } from './platform'
import { currentOsPlatform, desktopPlatform } from './platform/desktop'
import { mobilePlatform } from './platform/mobile'
import { router } from './router'
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
  installPlatform(kind === 'ios' || kind === 'android' ? mobilePlatform(kind) : desktopPlatform(kind))
}

const container = document.getElementById('root')
if (!container) throw new Error('#root is missing from index.html')

// Install the adapter before the first render so `platform.*` never falls back
// to the web implementation on a native build.
installRuntimePlatform()

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
