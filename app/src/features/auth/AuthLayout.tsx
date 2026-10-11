import type { ReactNode } from 'react'

import { Wordmark } from '@/components/ui/BrandIllustration'

/**
 * The chrome the auth screens share.
 *
 * The design puts the auth page on the tinted ground with a white card floating
 * on it, so that is what this is: one card, centred, with the wordmark at the top
 * as on the design's auth frame. The card is the page's only elevation, and this
 * is the one surface where the entry illustration's place is the wordmark rather
 * than the book, which belongs to the empty first-run state.
 *
 * Fields come from `components/ui/Field`, so a form here is wired exactly like a
 * form anywhere else in the app.
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
  return (
    <div className="flex min-h-[100dvh] flex-col items-center justify-center gap-5 bg-bg px-5 py-10 pt-safe-top pb-safe-bottom">
      <div className="card w-full max-w-md px-6 py-7">
        <Wordmark className="mb-6" />
        <h1>{title}</h1>
        <p className="mt-1 text-sm text-muted">{subtitle}</p>
        <div className="mt-6">{children}</div>
      </div>
      {footer && <div className="flex flex-wrap items-center justify-center gap-2">{footer}</div>}
    </div>
  )
}
