import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { PageFooter } from '@/components/layout/Section'
import { Wordmark } from '@/components/ui/BrandIllustration'

/**
 * The chrome the auth screens share.
 *
 * The design's auth frame is a centred card on the page ground, with the wordmark
 * in a page header above it and a metadata line at the foot. The card is the
 * page's only surface: the ground is `--bg`, the card is `--surface` with a 1px
 * border, and there is no shadow, because elevation is reserved for a layer that
 * genuinely floats above the page.
 *
 * The heading is `text-h2` up to the `sm` breakpoint and `text-h1` from there,
 * which is the frame's own measurement: 24/32 on a phone, 32/42 on a desktop. A
 * 32px heading is not wrong on a phone, but the frame reduces it, and the design's
 * values are the requirement.
 *
 * Fields come from `components/ui/Field`, so a form here is wired exactly like a
 * form anywhere else in the app.
 *
 * The frame's "Need help?" and "Privacy" links are deliberately not rendered:
 * neither has a route in this app, and a link with nowhere to go is a control
 * that lies about being one (R-26).
 */

export function AuthCard({
  title,
  subtitle,
  children,
  footer,
}: {
  title: string
  subtitle: string
  children: ReactNode
  footer?: ReactNode
}) {
  const { t } = useTranslation()

  return (
    <div className="auth-layout bg-bg text-text">
      <header className="auth-header">
        <Wordmark />
      </header>

      <main className="auth-main">
        <div className="auth-card card motion-entrance flex w-full max-w-[27.5rem] flex-col gap-5 p-6 sm:gap-6 sm:p-8">
          <div className="flex flex-col gap-2">
            <h1 className="text-h2 sm:text-h1">{title}</h1>
            <p className="text-body text-muted">{subtitle}</p>
          </div>

          {children}

          {/* The alternate account action sits inside the card, below the rule,
              which is where the frame puts it: it is a second way into the same
              product, not page chrome. */}
          {footer !== undefined && (
            <>
              <div className="border-t border-border" />
              <div className="flex flex-wrap items-center justify-center gap-2">{footer}</div>
            </>
          )}
        </div>
      </main>

      <div className="auth-footer">
        <PageFooter left={t('auth.metadata')} />
      </div>
    </div>
  )
}
