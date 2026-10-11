import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SignInScreen } from './SignInScreen'
import { SignUpScreen } from './SignUpScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'
import type { FakeSecureStore } from '@/test/harness'

/**
 * The two forms.
 *
 * What is asserted here is the wiring the screens exist for: the device name is
 * proposed rather than left blank, the request carries the fields the contract
 * names, a validation failure lands on the field it is about, and a refusal is
 * reported instead of swallowed.
 */

const SESSION = {
  user: {
    id: 'u1',
    email: 'student@example.com',
    email_verified: false,
    email_verified_at: null,
    plan_id: 'free',
    timezone: 'Asia/Jakarta',
    student_verified: false,
    created_at: '2026-10-11T08:00:00Z',
  },
  access_token: 'access-1',
  refresh_token: 'refresh-1',
  expires_at: '2026-10-11T10:00:00Z',
  device_id: 'device-1',
}

let store: FakeSecureStore

beforeEach(() => {
  store = installFakePlatform('macos')
  setAccessToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('signing up', () => {
  it('proposes the device name from the platform', () => {
    installFakePlatform('ios')
    mockFetch(() => jsonResponse(200, {}))

    renderWithProviders(<SignUpScreen />)

    // The device is part of the credential, so the name it will appear under is
    // offered rather than left as an empty box the user has to invent.
    expect(screen.getByLabelText('This device')).toHaveValue('iPhone or iPad')
  })

  it('sends the email, the password, and the device', async () => {
    const calls = mockFetch(() => jsonResponse(201, SESSION))
    const user = userEvent.setup()
    renderWithProviders(<SignUpScreen />)

    await user.type(screen.getByLabelText('Email'), 'student@example.com')
    await user.type(screen.getByLabelText('Password'), 'correct horse battery staple')
    await user.click(screen.getByRole('button', { name: 'Create account' }))

    await vi.waitFor(() => {
      expect(calls.some((call) => call.url.endsWith('/auth/signup'))).toBe(true)
    })

    const signup = calls.find((call) => call.url.endsWith('/auth/signup'))
    expect(JSON.parse(String(signup?.init.body))).toEqual({
      email: 'student@example.com',
      password: 'correct horse battery staple',
      device: { name: 'Mac', platform: 'macos' },
    })

    // The session is established where the contract says it goes: refresh token
    // and device id in the secure store, nothing else stored anywhere.
    expect(store.values.get('stvdy.refresh_token')).toBe('refresh-1')
    expect(store.values.get('stvdy.device_id')).toBe('device-1')
  })

  it('puts a validation failure on the field it names', async () => {
    mockFetch(() =>
      jsonResponse(400, {
        code: 'validation',
        message: 'password must be at least 12 characters',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )
    const user = userEvent.setup()
    renderWithProviders(<SignUpScreen />)

    await user.type(screen.getByLabelText('Email'), 'student@example.com')
    await user.type(screen.getByLabelText('Password'), 'short')
    await user.click(screen.getByRole('button', { name: 'Create account' }))

    // The message is beside the input it is about, not at the top of the page.
    expect(await screen.findByText('password must be at least 12 characters')).toBeInTheDocument()
    expect(screen.getByLabelText('Password')).toHaveAttribute('aria-invalid', 'true')
    expect(screen.getByLabelText('Password')).toHaveAccessibleDescription(
      'password must be at least 12 characters',
    )
    // And it is not also announced as a form-level failure.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('keeps an unattributable validation failure at the form level', async () => {
    mockFetch(() =>
      jsonResponse(400, {
        code: 'validation',
        message: 'request rejected',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )
    const user = userEvent.setup()
    renderWithProviders(<SignUpScreen />)

    await user.type(screen.getByLabelText('Email'), 'student@example.com')
    await user.type(screen.getByLabelText('Password'), 'long enough to pass')
    await user.click(screen.getByRole('button', { name: 'Create account' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('request rejected')
    expect(screen.getByLabelText('Email')).not.toHaveAttribute('aria-invalid')
  })

  it('reports a taken email address without pretending it succeeded', async () => {
    mockFetch(() =>
      jsonResponse(409, {
        code: 'conflict',
        message: 'email already registered',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )
    const user = userEvent.setup()
    renderWithProviders(<SignUpScreen />)

    await user.type(screen.getByLabelText('Email'), 'student@example.com')
    await user.type(screen.getByLabelText('Password'), 'correct horse battery staple')
    await user.click(screen.getByRole('button', { name: 'Create account' }))

    expect(await screen.findByText('That address is already registered')).toBeInTheDocument()
    expect(store.values.size).toBe(0)
  })
})

describe('signing in', () => {
  it('signs in and stores the session', async () => {
    const calls = mockFetch(() => jsonResponse(200, SESSION))
    const user = userEvent.setup()
    renderWithProviders(<SignInScreen />)

    await user.type(screen.getByLabelText('Email'), 'student@example.com')
    await user.type(screen.getByLabelText('Password'), 'correct horse battery staple')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))

    await vi.waitFor(() => {
      expect(store.values.get('stvdy.refresh_token')).toBe('refresh-1')
    })

    const login = calls.find((call) => call.url.endsWith('/auth/login'))
    expect(JSON.parse(String(login?.init.body))).toMatchObject({
      email: 'student@example.com',
      device: { name: 'Mac', platform: 'macos' },
    })
  })

  it('shows a wrong password as a failure rather than as a broken app', async () => {
    mockFetch(() =>
      jsonResponse(401, {
        code: 'unauthorized',
        message: 'invalid credentials',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )
    const user = userEvent.setup()
    renderWithProviders(<SignInScreen />)

    await user.type(screen.getByLabelText('Email'), 'student@example.com')
    await user.type(screen.getByLabelText('Password'), 'wrong')
    await user.click(screen.getByRole('button', { name: 'Sign in' }))

    expect(await screen.findByText('That did not match')).toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Check the email and the password and try again.',
    )
  })
})
