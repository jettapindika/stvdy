import { useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { api, ensureFreshSession } from '@/api/client'
import { AUTH_ME_KEY, type User } from '@/api/queries/auth'
import { getAccessToken, subscribeSession } from '@/lib/tokens'

/**
 * The session, as the app sees it.
 *
 * There are three states and they are deliberately distinct: `loading` while the
 * stored refresh token is being exchanged, `signed-out` when there is no session,
 * and `signed-in` with the user. Collapsing `loading` into `signed-out` is what
 * makes an app flash its sign-in screen for a moment on every cold start, which
 * reads as "I was logged out" to the person holding the phone.
 *
 * Bootstrap is the query's `queryFn`: read the access token from memory, and if
 * there is none, exchange the stored refresh token for a fresh pair. The access
 * token is never read from storage, because it is never written there.
 *
 * The subscription at the bottom is the other half of that contract. When the API
 * layer gives up on a session (a refresh the server refused), it clears the
 * token; this hook has to hear that and move to `signed-out`, or the app keeps
 * rendering a signed-in shell around requests that all fail. That is the
 * difference between "your session ended" and an app that appears broken.
 */

export type SessionState =
  | { status: 'loading' }
  | { status: 'signed-out' }
  | { status: 'signed-in'; user: User }

export function useSession(): SessionState {
  const queryClient = useQueryClient()

  const query = useQuery({
    queryKey: AUTH_ME_KEY,
    queryFn: async (): Promise<User | null> => {
      if (getAccessToken() === null && !(await ensureFreshSession())) return null
      // `GET /auth/me` answers a bare `UserView`, not a `{user}` envelope. The
      // envelope here used to be assumed from the client side alone, which made
      // every cold start that restored a stored refresh token report `undefined`
      // as the user and land on `/signin`.
      return await api.get<User>('/auth/me')
    },
    // A refused session is an answer, not a transient failure: retrying it would
    // only repeat the same 401.
    retry: false,
    staleTime: Infinity,
  })

  useEffect(
    () =>
      subscribeSession(() => {
        if (getAccessToken() === null) queryClient.setQueryData(AUTH_ME_KEY, null)
      }),
    [queryClient],
  )

  if (query.isPending) return { status: 'loading' }
  const user = query.data
  return user ? { status: 'signed-in', user } : { status: 'signed-out' }
}
