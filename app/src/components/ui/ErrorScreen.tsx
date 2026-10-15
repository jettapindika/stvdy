import { useTranslation } from 'react-i18next'
import { isRouteErrorResponse, useRouteError } from 'react-router'

/**
 * Full-page error boundary for the router. Catches render crashes that
 * bubble past every nested route and shows a user-friendly message
 * instead of react-router's raw stack trace.
 *
 * In dev mode the actual error is printed below the message so the
 * developer doesn't have to open devtools.
 */
export function ErrorScreen() {
  const { t } = useTranslation()
  const error = useRouteError()

  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : String(error)

  return (
    <div className="bg-bg flex min-h-screen items-center justify-center p-6">
      <div className="flex max-w-md flex-col items-start gap-4">
        <p className="text-muted text-label">Stvdy</p>

        <div
          className="note note-danger flex flex-col items-start gap-2"
          role="alert"
        >
          <p className="text-h4">{t('states.error')}</p>
          <p className="text-body">{t('states.crashReason')}</p>
        </div>

        <button
          type="button"
          className="btn btn-primary"
          onClick={() => window.location.reload()}
        >
          {t('states.reload')}
        </button>

        {import.meta.env.DEV && (
          <pre className="text-danger w-full overflow-auto rounded border p-3 text-xs">
            {message}
          </pre>
        )}
      </div>
    </div>
  )
}
