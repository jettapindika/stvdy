import { useState } from 'react'
import { Link, useNavigate } from 'react-router'

import { asApiError, validationMessage } from '@/api/errors'
import { useSignIn } from '@/api/queries/auth'
import { ApiErrorNotice } from '@/components/ui/States'
import { Field } from '@/components/ui/Field'
import { AuthCard } from './AuthLayout'

/**
 * Sign in.
 *
 * The device is sent but not asked about: a returning user is naming a machine
 * they already own, and the device list is where a wrong name gets fixed. Asking
 * again would be a second chance to get it wrong for no benefit.
 */

const FIELDS = ['email', 'password'] as const

export function SignInScreen() {
  const navigate = useNavigate()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const signIn = useSignIn()

  const error = asApiError(signIn.error)
  const validation = error?.code === 'validation' ? validationMessage(error, FIELDS) : null
  const errorFor = (field: string) =>
    validation?.field === field ? validation.message : undefined
  const showNotice = error !== null && (validation === null || validation.field === null)

  return (
    <AuthCard
      title="Sign in"
      subtitle="Your workspaces, sources, and study material, on every device you sign in from."
      footer={
        <>
          <span className="text-sm text-ink-muted">New to Stvdy?</span>
          <Link to="/signup" className="btn btn-quiet">
            Create an account
          </Link>
        </>
      }
    >
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          signIn.mutate({ email, password }, { onSuccess: () => void navigate('/workspaces') })
        }}
      >
        {showNotice && error !== null && (
          <ApiErrorNotice
            error={error}
            // On this route `unauthorized` is a wrong password, not an ended
            // session, and saying "your session has ended" here would send the
            // user looking for a problem they do not have.
            {...(error.code === 'unauthorized'
              ? {
                  title: 'That did not match',
                  reason:
                    'Check the email and the password and try again. Neither is shown to anyone else.',
                }
              : {})}
          />
        )}

        <Field
          label="Email"
          type="email"
          name="email"
          autoComplete="email"
          required
          value={email}
          error={errorFor('email')}
          onChange={(event) => {
            setEmail(event.target.value)
          }}
        />

        <Field
          label="Password"
          type="password"
          name="password"
          autoComplete="current-password"
          required
          value={password}
          error={errorFor('password')}
          onChange={(event) => {
            setPassword(event.target.value)
          }}
        />

        <button type="submit" className="btn btn-primary w-full" disabled={signIn.isPending}>
          {signIn.isPending ? 'Signing in' : 'Sign in'}
        </button>
      </form>
    </AuthCard>
  )
}
