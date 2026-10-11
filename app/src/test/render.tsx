import type { ReactNode } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render } from '@testing-library/react'
import { MemoryRouter } from 'react-router'

/**
 * A screen rendered the way the app renders it: inside a query client and a
 * router, with retries off.
 *
 * Retries are off because these tests assert on what a screen shows for a given
 * server answer. A retry would add a second, invisible request and make a failure
 * show up as a timeout instead of as the wrong text.
 *
 * The container is returned so an accessibility check can scope axe to the screen
 * under test. Running axe on the whole document instead would report the shell's
 * landmarks, which a component rendered on its own cannot provide.
 */
export function renderWithProviders(
  ui: ReactNode,
  options?: { route?: string; client?: QueryClient },
): { client: QueryClient; container: HTMLElement } {
  const client =
    options?.client ??
    new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    })

  const { container } = render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[options?.route ?? '/']}>{ui}</MemoryRouter>
    </QueryClientProvider>,
  )

  return { client, container }
}
