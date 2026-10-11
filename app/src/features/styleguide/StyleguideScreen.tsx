import { useTranslation } from 'react-i18next'

import { CitationList } from '@/components/ui/Citation'
import { EmptyState, ErrorState, LoadingState, SkeletonRows } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { PageHeader, Section } from '@/components/layout/Section'
import { COURSE_NAMES, courseVars } from '@/tokens/tokens'

/**
 * A reference page for the design system, at `/styleguide`.
 *
 * It exists so the tokens and components can be reviewed in one place while the
 * product screens are still empty, and it is labelled as a reference so nobody
 * mistakes it for a product surface.
 *
 * Everything below uses obviously generic placeholders ("Source title", "page
 * 12") rather than plausible-looking sample data. A style guide full of invented
 * course names and student counts is exactly the fabrication R-23 and R-38
 * forbid, and it would be the easiest place to let it in unnoticed.
 */

export function StyleguideScreen() {
  const { t } = useTranslation()
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  //
  // The token names are the CSS variable names, and they must match
  // `app/src/tokens/tokens.ts`: the contrast test parses this file's stylesheet and
  // fails when the two disagree, so a swatch labelled with the wrong token is a
  // build failure rather than a page that lies to whoever reads it.
  const swatches: { token: string; className: string; role: string }[] = [
    { token: 'bg', className: 'bg-bg', role: t('styleguide.swatch.bg') },
    { token: 'surface', className: 'bg-surface', role: t('styleguide.swatch.surface') },
    { token: 'surface-raised', className: 'bg-surface-raised', role: t('styleguide.swatch.raised') },
    { token: 'border', className: 'bg-border', role: t('styleguide.swatch.border') },
    { token: 'border-strong', className: 'bg-border-strong', role: t('styleguide.swatch.borderStrong') },
    { token: 'text', className: 'bg-text', role: t('styleguide.swatch.text') },
    { token: 'text-muted', className: 'bg-muted', role: t('styleguide.swatch.textMuted') },
    { token: 'text-subtle', className: 'bg-subtle', role: t('styleguide.swatch.textSubtle') },
    { token: 'primary', className: 'bg-primary', role: t('styleguide.swatch.primary') },
    { token: 'primary-text', className: 'bg-primary-text', role: t('styleguide.swatch.primaryText') },
    { token: 'primary-tint', className: 'bg-primary-tint', role: t('styleguide.swatch.primaryTint') },
    { token: 'source', className: 'bg-source', role: t('styleguide.swatch.source') },
    { token: 'source-tint', className: 'bg-source-tint', role: t('styleguide.swatch.sourceTint') },
    { token: 'success', className: 'bg-success', role: t('styleguide.swatch.success') },
    { token: 'success-tint', className: 'bg-success-tint', role: t('styleguide.swatch.successTint') },
    { token: 'warning', className: 'bg-warning', role: t('styleguide.swatch.warning') },
    { token: 'warning-tint', className: 'bg-warning-tint', role: t('styleguide.swatch.warningTint') },
    { token: 'danger', className: 'bg-danger', role: t('styleguide.swatch.danger') },
    { token: 'danger-tint', className: 'bg-danger-tint', role: t('styleguide.swatch.dangerTint') },
    { token: 'ring', className: 'bg-ring', role: t('styleguide.swatch.ring') },
  ]

  // The six course hues. Shown as the marker plus the chip plus the initial,
  // which is the whole point: the colour is never the only signal, so the swatch
  // has to demonstrate the redundancy rather than just the hue.
  const courses = COURSE_NAMES.map((name) => ({
    name,
    vars: courseVars(name),
    label: t(`styleguide.course.${name}`),
  }))

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-10 px-5 py-8">
      <PageHeader title={t('styleguide.title')} subtitle={t('styleguide.subtitle')} />

      <Section title={t('styleguide.palette.title')}>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.palette.description')}</p>
        <ul className="flex flex-col divide-y divide-border border-y border-border">
          {swatches.map((swatch) => (
            <li key={swatch.token} className="flex items-center gap-3 py-2">
              <span
                className={`h-6 w-6 shrink-0 rounded-sm border border-border ${swatch.className}`}
                aria-hidden="true"
              />
              <span className="font-mono text-xs">{swatch.token}</span>
              <span className="ml-auto text-right text-xs text-muted">{swatch.role}</span>
            </li>
          ))}
        </ul>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.palette.contrastNote')}</p>
      </Section>

      <Section title={t('styleguide.course.title')}>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.course.description')}</p>
        <ul className="flex flex-wrap gap-2">
          {courses.map((course) => (
            <li key={course.name} className="flex items-center gap-2" style={course.vars}>
              <span className="course-marker h-6" aria-hidden="true" />
              <span
                className="chip"
                style={{ backgroundColor: 'var(--course-tint)', color: 'var(--course-marker)' }}
              >
                {course.label.charAt(0)}
              </span>
              <span className="text-sm">{course.label}</span>
            </li>
          ))}
        </ul>
      </Section>

      <Section title={t('styleguide.type.title')}>
        <div className="flex flex-col gap-3">
          <p className="text-2xl font-semibold">{t('styleguide.type.headingOne')}</p>
          <p className="text-lg font-semibold">{t('styleguide.type.headingTwo')}</p>
          <p className="font-semibold">{t('styleguide.type.headingThree')}</p>
          <p>{t('styleguide.type.body')}</p>
          <p className="meta">{t('styleguide.type.meta')}</p>
          <p>
            {t('styleguide.type.linkBefore')}{' '}
            <a className="link" href="#palette">
              {t('styleguide.type.linkText')}
            </a>{' '}
            {t('styleguide.type.linkAfter')}
          </p>
        </div>
      </Section>

      <Section title={t('styleguide.citations.title')}>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.citations.description')}</p>
        <div className="flex flex-col gap-3 border-y border-border py-3">
          <p className="text-sm">
            {t('styleguide.citations.exampleLayer')}{' '}
            <CitationList
              citations={[
                {
                  type: 'chunk',
                  label: 1,
                  sourceTitle: t('styleguide.citations.sampleSource'),
                  page: 12,
                },
              ]}
            />
            {t('styleguide.citations.exampleOverhead')}{' '}
            <CitationList
              citations={[
                {
                  type: 'chunk',
                  label: 2,
                  sourceTitle: t('styleguide.citations.sampleSource'),
                  page: 4,
                },
                {
                  type: 'chunk',
                  label: 3,
                  sourceTitle: t('styleguide.citations.sampleAnotherSource'),
                  page: null,
                },
              ]}
            />
            {t('styleguide.citations.exampleSeparately')}{' '}
            <CitationList
              citations={[
                {
                  type: 'web',
                  url: 'https://example.com/paper',
                  title: t('styleguide.citations.sampleWebResult'),
                },
              ]}
            />
            {t('styleguide.citations.exampleEnd')}
          </p>
          <p className="text-sm">{t('styleguide.citations.interactive')}</p>
        </div>
      </Section>

      <Section title={t('styleguide.controls.title')}>
        <p className="max-w-prose text-sm text-muted">
          {t('styleguide.controls.descriptionBefore')}{' '}
          <code>{t('styleguide.controls.descriptionCode')}</code>{' '}
          {t('styleguide.controls.descriptionAfter')}
        </p>
        <div className="flex flex-wrap items-center gap-2" aria-hidden="true">
          <button type="button" className="btn btn-primary" aria-disabled="true" tabIndex={-1}>
            {t('styleguide.controls.primary')}
          </button>
          <button type="button" className="btn btn-secondary" aria-disabled="true" tabIndex={-1}>
            {t('styleguide.controls.secondary')}
          </button>
          <button type="button" className="btn btn-quiet" aria-disabled="true" tabIndex={-1}>
            {t('styleguide.controls.quiet')}
          </button>
          <button type="button" className="btn btn-primary" disabled>
            {t('styleguide.controls.disabled')}
          </button>
        </div>
        <label className="flex max-w-sm flex-col gap-1" aria-hidden="true">
          <span className="label">{t('styleguide.controls.fieldLabel')}</span>
          <input
            className="field"
            placeholder={t('styleguide.controls.placeholder')}
            tabIndex={-1}
            readOnly
          />
        </label>
      </Section>

      <Section title={t('styleguide.states.title')}>
        <div className="flex flex-col gap-3">
          <div className="panel">
            <EmptyState
              title={t('styleguide.states.emptyTitle')}
              reason={t('styleguide.states.emptyReason')}
            />
          </div>
          <ErrorState
            title={t('styleguide.states.errorTitle')}
            reason={t('styleguide.states.errorReason')}
          />
          <div className="panel">
            <LoadingState label={t('styleguide.states.loading')} />
          </div>
          <div className="panel">
            <SkeletonRows label={t('styleguide.states.loadingRows')} />
          </div>
        </div>
      </Section>

      <Section title={t('styleguide.chips.title')}>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.chips.description')}</p>
        <ul className="flex flex-wrap items-center gap-2">
          <li className="chip">{t('styleguide.chips.neutral')}</li>
          <li className="chip chip-working">{t('styleguide.chips.working')}</li>
          <li className="chip chip-ready">{t('styleguide.chips.ready')}</li>
          <li className="chip chip-due">{t('styleguide.chips.due')}</li>
          <li className="chip chip-failed">{t('styleguide.chips.failed')}</li>
          <li className="chip chip-source">{t('styleguide.chips.source')}</li>
        </ul>
        <ul className="flex flex-wrap items-center gap-4">
          {(['lock', 'streak', 'check', 'clock', 'cards', 'chevron-right', 'external', 'search', 'plus', 'close', 'alert'] as const).map(
            (name) => (
              <li key={name} className="flex items-center gap-2 text-muted">
                <Icon name={name} />
                <span className="font-mono text-xs">{name}</span>
              </li>
            ),
          )}
        </ul>
      </Section>

      <Section title={t('styleguide.locked.title')}>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.locked.description')}</p>
        <div className="flex flex-col gap-3">
          <LockedFeature whatKey="planner.rebalance" minPlan="Power" />
          <LockedFeature
            whatKey="planner.rebalance"
            minPlan="Power"
            reasonKey="planner.rebalanceReason"
          />
          {/* The plan-gated export this phase adds: the format is in Pro's
              `export_formats` and not in Free's, so the locked state is the one a
              free account sees beside the export menu. */}
          <LockedFeature
            whatKey="reports.locked.pdf"
            minPlan="Pro"
            reasonKey="reports.locked.pdfReason"
          />
          {/* The same gate as the plan screen states it: no reason line, because
              the screen's own heading already says what the section is. */}
          <LockedFeature whatKey="plan.formats.pdf" minPlan="Pro" />
        </div>
      </Section>

      <Section title={t('styleguide.composer.title')}>
        <p className="max-w-prose text-sm text-muted">{t('styleguide.composer.description')}</p>
        <div className="flex flex-col gap-2 border-y border-border py-3">
          <label className="flex min-h-11 items-center gap-2 text-sm text-muted">
            <input type="checkbox" checked readOnly tabIndex={-1} />
            {t('honesty.sourcesOnly')}
          </label>
          <p className="composer-hint">{t('honesty.composerHint')}</p>
        </div>
      </Section>
    </div>
  )
}
