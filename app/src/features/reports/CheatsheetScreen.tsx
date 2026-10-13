import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useParams } from 'react-router'

import { asApiError } from '@/api/errors'
import {
  useCheatsheet,
  useCheatsheetHtml,
  useExportCheatsheetPdf,
  useRerenderCheatsheet,
} from '@/api/queries/cheatsheets'
import { PageFooter, PageHeader, Section } from '@/components/layout/Section'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { PLANS, planForFormat } from '@/features/billing/plans'
import { saveBlob } from '@/lib/download'

/**
 * The cheatsheet: one generated document, rendered through a fixed print
 * template, printable as A4.
 *
 * **The frame is `sandbox`ed, and that is the security property of this screen.**
 * The HTML is generated: the model wrote the document's text and the renderer
 * assembled the page from typed blocks. It is still untrusted input, and it is
 * untrusted input that the app would otherwise be *injecting into its own DOM* —
 * a `<script>` in a text field, a `<form>` pointing at somewhere else, a
 * top-level navigation. `sandbox` with no permissions gives the frame a unique
 * opaque origin, blocks scripts, forms and top-level navigation, and leaves the
 * document rendered. `srcDoc` rather than a URL because the route needs the
 * bearer token and a frame navigation cannot carry a header.
 *
 * **The page count is the server's.** `page_count` is read back from the renderer,
 * because the footer's `Halaman N dari M` is printed from that same number: a
 * count this screen computed, or guessed from the depth, would be a different
 * number from the one on the page. It is null until a PDF exists, and it is not
 * estimated in the meantime — the preview's heading carries it when it is there
 * and carries nothing when it is not.
 *
 * **Re-rendering is free and that is the point.** The blocks are the document, so
 * a template fix reaches a cheatsheet the student already has without a model
 * call, without credits, and without changing a word of the content.
 *
 * The screen is the design's reading column: a page header, the preview card, and
 * the actions. The one primary action is the PDF download, and it is the only
 * filled button here; re-rendering is a secondary action beside it because it is
 * free by contract, and the line under them says so rather than leaving a student
 * to guess which of the two costs credits.
 */

export function CheatsheetScreen() {
  const { t } = useTranslation()
  const { cheatsheetId = '' } = useParams()
  const cheatsheet = useCheatsheet(cheatsheetId)
  const error = asApiError(cheatsheet.error)

  if (cheatsheet.isPending) {
    return (
      <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 md:px-[32px] py-8">
        <LoadingState label={t('cheatsheet.loading')} />
      </div>
    )
  }

  if (error !== null) {
    return (
      <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 md:px-[32px] py-8">
        <ApiErrorNotice
          error={error}
          noun={t('errors.nouns.cheatsheet')}
          action={
            <Link to="/workspaces" className="btn btn-secondary">
              {t('cheatsheet.backToWorkspaces')}
            </Link>
          }
        />
      </div>
    )
  }

  const sheet = cheatsheet.data?.cheatsheet
  if (sheet === undefined) return null

  if (sheet.status === 'failed') {
    return (
      <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 md:px-[32px] py-8">
        <EmptyState
          title={t('cheatsheet.failed.title')}
          reason={sheet.error ?? t('cheatsheet.failed.reason')}
        />
      </div>
    )
  }

  const generating = sheet.status === 'pending' || sheet.status === 'generating'

  return (
    <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 md:px-[32px] py-8">
      <PageHeader
        // The scope and the date the student is revising for, which is the context
        // the title cannot carry. The date is omitted rather than filled with a
        // placeholder when the sheet has none.
        eyebrow={
          sheet.exam_date === null
            ? sheet.scope
            : t('cheatsheet.eyebrow', { scope: sheet.scope, date: sheet.exam_date })
        }
        title={sheet.title}
        subtitle={t('cheatsheet.subtitle', {
          depth: t(`cheatsheet.depth.${sheet.depth}`),
        })}
      />

      {generating && <LoadingState label={t('cheatsheet.generating')} />}

      {!generating && <CheatsheetPreview cheatsheetId={sheet.id} pageCount={sheet.page_count} />}

      <Section title={t('cheatsheet.actions.title')}>
        <p className="text-label text-muted">{t('cheatsheet.actions.hint')}</p>
        <CheatsheetActions cheatsheetId={sheet.id} pageCount={sheet.page_count} />
      </Section>

      {sheet.template_version !== null && (
        <PageFooter
          left={t('cheatsheet.template', { version: sheet.template_version })}
          right={sheet.scope}
        />
      )}
    </div>
  )
}

/**
 * The rendered document, in a frame it cannot escape.
 *
 * The frame carries a `title` because it is a document a screen reader user has
 * to be able to name, and `srcdoc` because the HTML arrived as text. It is not
 * sized to its content: the document is a print layout with its own page breaks,
 * so the frame scrolls inside a fixed height rather than stretching the app's
 * page around a document that is meant to be printed, not read in a window.
 *
 * The count the renderer reported sits in the section's heading, in the mono
 * metadata slot the design puts a count in — not under the button, where it was a
 * second reading of the same number.
 */
function CheatsheetPreview({
  cheatsheetId,
  pageCount,
}: {
  cheatsheetId: string
  pageCount: number | null
}) {
  const { t } = useTranslation()
  const html = useCheatsheetHtml(cheatsheetId, true)
  const error = asApiError(html.error)
  const frame = useRef<HTMLIFrameElement>(null)
  const [frameFocused, setFrameFocused] = useState(false)

  // **The frame's focus ring has to be driven from JavaScript, and this is why.**
  // The frame is a scrollable region: it is a `Tab` stop (measured in Chromium at
  // 390px, the walk goes `… Download PDF > Re-render` through it), and a keyboard
  // user needs to land in it to scroll the document with the arrow keys. But a
  // focused `<iframe>` matches **no** focus pseudo-class — Chromium puts the focus
  // on the frame's inner document, so `frame.matches(':focus')`,
  // `:focus-visible` and even `:focus-within` on the frame and on its ancestors are
  // all false, and the frame's `focus`/`focusin`/`blur` events do not fire either.
  // The global `:focus-visible` rule in `styles.css` therefore cannot reach it, and
  // the measured result was a `Tab` stop with no visible ring at all.
  //
  // The one signal the browser does give is the window's own blur/focus pair: focus
  // entering the frame blurs the window (`document.activeElement` is then the frame,
  // and `document.hasFocus()` is still true), and focus leaving the frame fires the
  // window's focus event. `hasFocus()` is what keeps an app switch — which also
  // blurs the window — from drawing a ring on a frame nobody is using.
  useEffect(() => {
    function onWindowBlur() {
      if (document.activeElement === frame.current && document.hasFocus()) setFrameFocused(true)
    }
    function onWindowFocus() {
      setFrameFocused(false)
    }
    window.addEventListener('blur', onWindowBlur)
    window.addEventListener('focus', onWindowFocus)
    return () => {
      window.removeEventListener('blur', onWindowBlur)
      window.removeEventListener('focus', onWindowFocus)
    }
  }, [])

  return (
    <Section
      title={t('cheatsheet.preview.title')}
      action={
        // The renderer's own count, or nothing. Never an estimate.
        pageCount === null ? undefined : (
          <span className="meta min-w-0 break-words">
            {t('cheatsheet.preview.pages', { count: pageCount })}
          </span>
        )
      }
    >
      <div className="card flex flex-col gap-3 p-4">
        {html.isPending && <LoadingState label={t('cheatsheet.preview.loading')} />}

        {error !== null && (
          <ApiErrorNotice
            error={error}
            noun={t('errors.nouns.cheatsheet')}
            action={
              <button
                type="button"
                className="btn btn-secondary"
                onClick={() => void html.refetch()}
              >
                {t('actions.retry')}
              </button>
            }
          />
        )}

        {html.data !== undefined && (
          <iframe
            ref={frame}
            title={t('cheatsheet.preview.title')}
            // Empty, so the document gets an opaque origin and none of the
            // capabilities sandbox can withhold: no scripts, no forms, no
            // top-level navigation, no same-origin access to this app.
            sandbox=""
            srcDoc={html.data}
            // The ring is the same 2px `--ring` treatment the global rule applies to
            // every other control; only the way it is switched on differs (see the
            // effect above).
            className={`h-[32rem] w-full rounded-card border border-border bg-surface ${
              frameFocused ? 'outline outline-2 outline-offset-2 outline-ring' : ''
            }`}
          />
        )}
      </div>
    </Section>
  )
}

/**
 * Printing, re-rendering, and the one number the student can act on.
 *
 * "Download PDF" is plan-gated by `export_formats`: the plan that does not include
 * `pdf` gets no button and no price — on a phone, nothing but the statement that
 * it is not on the plan. Re-rendering is offered to everyone, because it is free
 * by contract: it reads the stored blocks and runs the renderer again, so it can
 * neither cost credits nor change the document's content. The line under the two
 * buttons says that in words, because a free button beside a paid one is exactly
 * where a student would otherwise assume the worst.
 */
function CheatsheetActions({
  cheatsheetId,
  pageCount,
}: {
  cheatsheetId: string
  pageCount: number | null
}) {
  const { t } = useTranslation()
  const session = useSession()
  const rerender = useRerenderCheatsheet(cheatsheetId)
  const exportPdf = useExportCheatsheetPdf(cheatsheetId)
  // Null until the account is known, so this screen never states a plan fact it
  // has not been told: "not available on your plan" is a claim about the plan, and
  // it must not appear while the plan is still being read.
  const allowed =
    session.status === 'signed-in' ? (PLANS[session.user.plan_id]?.exportFormats ?? []) : null
  const pdfAllowed = allowed?.includes('pdf') ?? false
  const exportError = asApiError(exportPdf.error)
  const rerenderError = asApiError(rerender.error)

  return (
    <>
      <div className="flex flex-wrap gap-2">
        {pdfAllowed && (
          <button
            type="button"
            className="btn btn-primary"
            disabled={exportPdf.isPending}
            onClick={() => {
              exportPdf.mutate(undefined, {
                onSuccess: (blob) => {
                  saveBlob(blob, `${t('cheatsheet.pdfFilename')}.pdf`)
                },
              })
            }}
          >
            {exportPdf.isPending ? t('cheatsheet.pdf.preparing') : t('cheatsheet.pdf.download')}
          </button>
        )}
        <button
          type="button"
          className="btn btn-secondary"
          disabled={rerender.isPending}
          onClick={() => {
            rerender.mutate()
          }}
        >
          {rerender.isPending ? t('cheatsheet.rerender.pending') : t('cheatsheet.rerender.action')}
        </button>
      </div>

      {/* The renderer's own count again, here where the file it describes is
          downloaded: the same number the preview's heading carries, because it is
          the same server figure and neither is an estimate. */}
      {pdfAllowed && pageCount !== null && (
        <p className="text-caption text-muted">{t('cheatsheet.pdf.pages', { pages: pageCount })}</p>
      )}

      {allowed !== null && !pdfAllowed && (
        <LockedFeature
          whatKey="reports.locked.pdf"
          minPlan={planForFormat('pdf') ?? 'Pro'}
          reasonKey="reports.locked.pdfReason"
        />
      )}

      {/* Re-rendering is free, and saying so is the honesty rule applied to a
          button that would otherwise look like the paid one beside it. */}
      <p className="text-caption text-muted">{t('cheatsheet.rerender.free')}</p>

      {exportError !== null && (
        <ApiErrorNotice error={exportError} noun={t('errors.nouns.export')} />
      )}
      {rerenderError !== null && (
        <ApiErrorNotice error={rerenderError} noun={t('errors.nouns.cheatsheet')} />
      )}
    </>
  )
}
