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
 */
export function renderWithProviders(
  ui: ReactNode,
  options?: { route?: string; client?: QueryClient },
): { client: QueryClient } {
  const client =
    options?.client ??
    new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    })

  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[options?.route ?? '/']}>{ui}</MemoryRouter>
    </QueryClientProvider>,
  )

  return { client }
}
