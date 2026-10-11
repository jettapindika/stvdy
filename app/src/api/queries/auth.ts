import { useMutation, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'
import { platform, type PlatformKind } from '../../platform'
import { saveSession, setAccessToken } from '../../lib/tokens'

/**
 * Auth: the four calls a session is made of, plus the device identity they carry.
 *
 * A session is not just a token pair. The contract makes the device part of the
 * credential (`device: {name, platform, app_version?}` on signup and login), so
 * the same account on two machines is two sessions that can be revoked
 * independently. That is why the device name is collected at signup rather than
 * left to the server to guess, and why it is defaulted from `platform.kind()`
 * instead of being a required empty field.
 *
 * Establishing a session means three things at once, and they are done in one
 * place so no call site can do two of them: the access token goes into memory,
 * the rotating refresh token and device id go into the secure store, and the
 * `['auth','me']` cache entry is filled so the app can render the signed-in shell
 * without a second round trip.
 */

export interface User {
  id: string
  email: string
  email_verified: boolean
  email_verified_at: string | null
  plan_id: string
  timezone: string
  student_verified: boolean
  created_at: string
}

export interface AuthResponse {
  user: User
  access_token: string
  refresh_token: string
  expires_at: string
  device_id: string
}

export const AUTH_ME_KEY = ['auth', 'me'] as const

/** What a device calls itself in the device list before the user renames it. */
const DEVICE_NAME_BY_KIND: Record<PlatformKind, string> = {
  web: 'Web browser',
  macos: 'Mac',
  windows: 'Windows PC',
  ios: 'iPhone or iPad',
  android: 'Android device',
}

export function defaultDeviceName(kind: PlatformKind = platform.kind()): string {
  return DEVICE_NAME_BY_KIND[kind]
}

/**
 * The device half of a signup or login body.
 *
 * `app_version` is omitted rather than filled with a placeholder: the build does
 * not currently inject a version into the web bundle, and a made-up version
 * string in a device record is worse than an absent one (R-17).
 */
export function currentDevice(name: string = defaultDeviceName()) {
  return { name, platform: platform.kind() }
}

async function establishSession(response: AuthResponse): Promise<AuthResponse> {
  setAccessToken(response.access_token)
  await saveSession({ refreshToken: response.refresh_token, deviceId: response.device_id })
  return response
}

export function useSignUp() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (input: { email: string; password: string; deviceName: string }) => {
      const response = await api.post<AuthResponse>(
        '/auth/signup',
        {
          email: input.email,
          password: input.password,
          device: currentDevice(input.deviceName),
        },
        { auth: false },
      )
      return establishSession(response)
    },
    onSuccess: (response) => {
      queryClient.setQueryData(AUTH_ME_KEY, response.user)
    },
  })
}

export function useSignIn() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (input: { email: string; password: string }) => {
      const response = await api.post<AuthResponse>(
        '/auth/login',
        {
          email: input.email,
          password: input.password,
          // A returning user is not asked to name the machine again; the device
          // list is where it gets renamed.
          device: currentDevice(),
        },
        { auth: false },
      )
      return establishSession(response)
    },
    onSuccess: (response) => {
      queryClient.setQueryData(AUTH_ME_KEY, response.user)
    },
  })
}

/**
 * Completes a Google sign-in from a single-use ticket.
 *
 * The server's OAuth callback redirects with `?ticket=...` and never with
 * tokens, because a deep link lands in browser history and the OS URL handler
 * and a credential placed there could not be revoked. This consumes the ticket
 * and establishes the session the same way the password path does: the access
 * token goes to memory, the rotating refresh token and device id go to the
 * secure store. The device is created here, not in the callback, so a browser
 * round trip cannot spend a device slot on its own.
 */
export async function completeGoogleSignIn(ticket: string): Promise<AuthResponse> {
  const response = await api.post<AuthResponse>(
    '/auth/google/exchange',
    { ticket, device: currentDevice() },
    { auth: false },
  )
  return establishSession(response)
}

export function useGoogleSignIn() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: completeGoogleSignIn,
    onSuccess: (response) => {
      queryClient.setQueryData(AUTH_ME_KEY, response.user)
    },
  })
}

export function useVerifyEmail() {
  return useMutation({
    mutationFn: (token: string) =>
      api.post<{ verified: boolean }>('/auth/verify-email', { token }, { auth: false }),
  })
}

export function useResendVerification() {
  return useMutation({
    mutationFn: () => api.post<void>('/auth/resend-verification'),
  })
}
