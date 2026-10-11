import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router'

import { asApiError, validationMessage } from '@/api/errors'
import { defaultDeviceName, useSignUp } from '@/api/queries/auth'
import { ApiErrorNotice } from '@/components/ui/States'
import { Field } from '@/components/ui/Field'
import { AuthCard } from './AuthLayout'

/**
 * Sign up.
 *
 * The device name is a real field with a real default, not an invisible detail.
 * The contract makes the device part of the credential, and the device list is
 * where a student sees and revokes their machines, so the name they will read
 * later is chosen here rather than derived by the server.
 *
 * A `validation` failure is attributed to the field it names when the server's
 * sentence identifies one, and shown at the top of the form when it does not.
 * That is the whole reason the notice and the field errors are two things: a
 * message pinned to the wrong input is worse than a message at the top.
 */

const FIELDS = ['email', 'password', 'device'] as const

export function SignUpScreen() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [deviceName, setDeviceName] = useState(defaultDeviceName)
  const signUp = useSignUp()

  const error = asApiError(signUp.error)
  const validation = error?.code === 'validation' ? validationMessage(error, t, FIELDS) : null
  const errorFor = (field: string) =>
    validation?.field === field ? validation.message : undefined
  const showNotice = error !== null && (validation === null || validation.field === null)

  return (
    <AuthCard
      title={t('auth.signUp.title')}
      subtitle={t('auth.signUp.subtitle')}
      footer={
        <>
          <span className="text-sm text-muted">{t('auth.signUp.haveAccount')}</span>
          <Link to="/signin" className="btn btn-quiet">
            {t('auth.signIn.title')}
          </Link>
        </>
      }
    >
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          signUp.mutate(
            { email, password, deviceName },
            {
              onSuccess: (response) =>
                void navigate(response.user.email_verified ? '/workspaces' : '/verify'),
            },
          )
        }}
      >
        {showNotice && error !== null && (
          <ApiErrorNotice
            error={error}
            // `conflict` on signup is a taken address, which is a normal thing to
            // hit and has an obvious next step. The general wording ("something
            // else changed this first") describes a lost update instead.
            {...(error.code === 'conflict'
              ? {
                  title: t('auth.signUp.takenTitle'),
                  reason: t('auth.signUp.takenReason'),
                }
              : {})}
          />
        )}

        <Field
          label={t('auth.field.email')}
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
          label={t('auth.field.password')}
          type="password"
          name="password"
          autoComplete="new-password"
          required
          value={password}
          error={errorFor('password')}
          onChange={(event) => {
            setPassword(event.target.value)
          }}
        />

        <Field
          label={t('auth.signUp.deviceLabel')}
          name="device"
          autoComplete="off"
          required
          value={deviceName}
          hint={t('auth.signUp.deviceHint')}
          error={errorFor('device')}
          onChange={(event) => {
            setDeviceName(event.target.value)
          }}
        />

        <button type="submit" className="btn btn-primary w-full" disabled={signUp.isPending}>
          {signUp.isPending ? t('auth.signUp.pending') : t('auth.signUp.submit')}
        </button>
      </form>
    </AuthCard>
  )
}
