import { useTranslation } from 'react-i18next'

import { PageHeader, Section, WidgetHeading, sectionId } from '@/components/layout/Section'
import { Citation, CitationList } from '@/components/ui/Citation'
import { Field } from '@/components/ui/Field'
import { Select } from '@/components/ui/Select'
import { Icon } from '@/components/ui/Icon'
import { COURSE_NAMES, contrast, courseVars, tokenColor } from '@/tokens/tokens'
import type { IconName } from '@/components/ui/Icon'

/**
 * The design system's spec page, at `/styleguide`.
 *
 * It is rebuilt from the design's own three sheets — Foundations, UI controls and
 * Academic patterns — so the redesign is legible as a system rather than as twenty
 * screens. Every specimen is a real component rendered from the live tokens: the
 * swatches read `COLORS` through `tokenColor`, so a token that changes changes the
 * page, and a colour is never copied from a sheet into a literal.
 *
 * **The sheets' document chrome is not reproduced.** Their 48px section titles and
 * `DESIGN SYSTEM / V1.0 / 01` labels are the *sheets'* scale, not the product's, so
 * the page title is the ordinary `h1` (32/42) and the sheet's own scale — the one
 * that matters — is the specimen in `Type scale`.
 *
 * **Specimen copy is obviously generic.** A gallery full of plausible course names
 * and student counts is the easiest place for a fabricated figure to slip in, so
 * the specimens name what they show ("Session name", "Course · duration") rather
 * than inventing a number.
 */

/** A colour token with the class that paints it and the key naming its role. */
const SWATCHES: { token: string; className: string; role: string }[] = [
  { token: 'bg', className: 'bg-bg', role: 'bg' },
  { token: 'surface', className: 'bg-surface', role: 'surface' },
  { token: 'surface-raised', className: 'bg-surface-raised', role: 'raised' },
  { token: 'border', className: 'bg-border', role: 'border' },
  { token: 'border-strong', className: 'bg-border-strong', role: 'borderStrong' },
  { token: 'text', className: 'bg-text', role: 'text' },
  { token: 'text-muted', className: 'bg-muted', role: 'textMuted' },
  { token: 'text-subtle', className: 'bg-subtle', role: 'textSubtle' },
  { token: 'primary', className: 'bg-primary', role: 'primary' },
  { token: 'on-primary', className: 'bg-on-primary', role: 'onPrimary' },
  { token: 'primary-text', className: 'bg-primary-text', role: 'primaryText' },
  { token: 'primary-tint', className: 'bg-primary-tint', role: 'primaryTint' },
  { token: 'source', className: 'bg-source', role: 'source' },
  { token: 'source-tint', className: 'bg-source-tint', role: 'sourceTint' },
  { token: 'success', className: 'bg-success', role: 'success' },
  { token: 'success-tint', className: 'bg-success-tint', role: 'successTint' },
  { token: 'warning', className: 'bg-warning', role: 'warning' },
  { token: 'warning-tint', className: 'bg-warning-tint', role: 'warningTint' },
  { token: 'danger', className: 'bg-danger', role: 'danger' },
  { token: 'danger-tint', className: 'bg-danger-tint', role: 'dangerTint' },
  { token: 'ring', className: 'bg-ring', role: 'ring' },
]

/** The four roles the dark theme is built from, as the design's sheet lists them. */
const DARK_CORE = ['bg', 'surface', 'border', 'text']

/**
 * The product's type scale, in order.
 *
 * `family` is set explicitly because only the `h1`–`h4` elements take Plus Jakarta
 * Sans from the stylesheet; a specimen rendered as a `p` would otherwise show the
 * size in the wrong family and teach the wrong thing.
 */
const TYPE_SCALE: { utility: string; family: string; spec: string; sample: string }[] = [
  { utility: 'text-display', family: 'font-heading', spec: '48 / 56', sample: 'display' },
  { utility: 'text-h1', family: 'font-heading', spec: '40 / 52', sample: 'headingOne' },
  { utility: 'text-h2', family: 'font-heading', spec: '24 / 32', sample: 'headingTwo' },
  { utility: 'text-h3', family: 'font-heading', spec: '20 / 26', sample: 'headingThree' },
  { utility: 'text-h4', family: 'font-heading', spec: '18 / 23', sample: 'h4' },
  { utility: 'text-body', family: '', spec: '16 / 24', sample: 'body' },
  { utility: 'text-label', family: '', spec: '14 / 20', sample: 'label' },
  { utility: 'text-small', family: '', spec: '13 / 20', sample: 'small' },
  { utility: 'text-caption', family: '', spec: '12 / 18', sample: 'caption' },
  { utility: 'text-meta', family: 'font-mono', spec: '11 / 18', sample: 'meta' },
]

/** The 4px grid. `bar` is the width utility that draws the step. */
const SPACING = [
  { bar: 'w-1', label: '4px' },
  { bar: 'w-2', label: '8px' },
  { bar: 'w-3', label: '12px' },
  { bar: 'w-4', label: '16px' },
  { bar: 'w-6', label: '24px' },
  { bar: 'w-8', label: '32px' },
]

const RADIUS = [
  { shape: 'rounded-field', name: 'controls' },
  { shape: 'rounded-card', name: 'cards' },
  { shape: 'rounded-sheet', name: 'sheets' },
  { shape: 'rounded-pill', name: 'chips' },
]

const ICONS: IconName[] = [
  'search',
  'plus',
  'check',
  'clock',
  'cards',
  'lock',
  'alert',
  'chevron-right',
  'external',
  'close',
]

const CHIPS = ['neutral', 'working', 'ready', 'due', 'failed', 'source'] as const

const ALERTS = [
  { tone: 'note-warning', title: 'warningTitle', reason: 'warningReason' },
  { tone: 'note-danger', title: 'dangerTitle', reason: 'dangerReason' },
  { tone: 'note-success', title: 'successTitle', reason: 'successReason' },
  { tone: '', title: 'neutralTitle', reason: 'neutralReason' },
] as const

export function StyleguideScreen() {
  const { t } = useTranslation()

  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const courses = COURSE_NAMES.map((name) => ({
    name,
    vars: courseVars(name),
    label: t(`styleguide.course.${name}`),
  }))

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-10 px-5 md:px-[32px] py-8">
      <PageHeader
        eyebrow={t('styleguide.eyebrow')}
        title={t('styleguide.title')}
        subtitle={t('styleguide.subtitle')}
      />

      <Section title={t('styleguide.foundations.title')}>
        <div className="flex flex-col gap-8">
          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="01.1"
              title={t('styleguide.palette.title')}
              meta={t('styleguide.palette.meta')}
            />
            <p className="max-w-prose text-body text-muted">{t('styleguide.palette.description')}</p>
            <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {SWATCHES.map((swatch) => (
                <li
                  key={swatch.token}
                  className="flex flex-col gap-2 rounded-field border border-border p-3"
                >
                  <div className="flex items-center gap-3">
                    {/* The swatch is painted by the token's own utility, so it is
                        the colour the app renders right now; the hex row beneath
                        names both themes' values, which is what the reader needs
                        in a theme whose swatch cannot show both. */}
                    <span
                      className={`h-8 w-8 shrink-0 rounded-field border border-border ${swatch.className}`}
                      aria-hidden="true"
                    />
                    <div className="flex min-w-0 flex-col">
                      <span className="text-label text-text">
                        {t(`styleguide.swatch.${swatch.role}`)}
                      </span>
                      <span className="meta">{swatch.token}</span>
                    </div>
                  </div>
                  <p className="meta">
                    <span className="text-subtle">{t('styleguide.palette.light')}</span>{' '}
                    {tokenColor(swatch.token, 'light')}
                    {' · '}
                    <span className="text-subtle">{t('styleguide.palette.dark')}</span>{' '}
                    {tokenColor(swatch.token, 'dark')}
                  </p>
                </li>
              ))}
            </ul>
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.palette.contrastNote')}
            </p>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="01.2"
              title={t('styleguide.dark.title')}
              meta={t('styleguide.dark.meta')}
            />
            <p className="max-w-prose text-body text-muted">{t('styleguide.dark.description')}</p>
            <ul className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {DARK_CORE.map((token) => (
                <li key={token} className="flex flex-col gap-2">
                  {/* The dark core is painted with the dark theme's value whatever
                      theme the reader is in: the specimen is *about* those four
                      values, so a swatch that followed the reader's theme would
                      show the light palette under a dark heading. */}
                  <span
                    className="h-12 w-full rounded-field border border-border"
                    style={{ backgroundColor: tokenColor(token, 'dark') }}
                    aria-hidden="true"
                  />
                  <span className="meta">{tokenColor(token, 'dark')}</span>
                </li>
              ))}
            </ul>
            <p className="max-w-prose text-body text-muted">{t('styleguide.dark.rule')}</p>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading eyebrow="01.3" title={t('styleguide.type.title')} />
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
              <div className="flex flex-col gap-1 rounded-field border border-border p-3">
                <span className="font-heading text-h2">Plus Jakarta Sans</span>
                <span className="meta">{t('styleguide.type.familyHeadings')}</span>
                <span className="text-caption text-muted">{t('styleguide.type.familyHeadingsNote')}</span>
              </div>
              <div className="flex flex-col gap-1 rounded-field border border-border p-3">
                <span className="text-h2">Inter</span>
                <span className="meta">{t('styleguide.type.familyBody')}</span>
                <span className="text-caption text-muted">{t('styleguide.type.familyBodyNote')}</span>
              </div>
              <div className="flex flex-col gap-1 rounded-field border border-border p-3">
                <span className="font-mono text-h2">JetBrains Mono</span>
                <span className="meta">{t('styleguide.type.familyMono')}</span>
                <span className="text-caption text-muted">{t('styleguide.type.familyMonoNote')}</span>
              </div>
            </div>

            <WidgetHeading title={t('styleguide.type.scale')} meta={t('styleguide.type.scaleMeta')} />
            <ul className="flex flex-col divide-y divide-border border-y border-border">
              {TYPE_SCALE.map((step) => (
                <li key={step.utility} className="flex flex-col gap-1 py-3">
                  <span className="meta">
                    {step.utility} · {step.spec} px
                  </span>
                  <span className={`${step.utility} ${step.family} break-words text-text`}>
                    {t(`styleguide.type.${step.sample}`)}
                  </span>
                </li>
              ))}
            </ul>
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.type.linkBefore')}{' '}
              {/* The anchor is derived from the section's own title, because
                  `Section` derives its id from the translated title: a hardcoded
                  `#foundations` is a live link in English and a dead one in
                  Indonesian, where the section is called "Fondasi". */}
              <a className="link" href={`#${sectionId(t('styleguide.foundations.title'))}`}>
                {t('styleguide.type.linkText')}
              </a>{' '}
              {t('styleguide.type.linkAfter')}
            </p>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="01.4"
              title={t('styleguide.spacing.title')}
              meta={t('styleguide.spacing.meta')}
            />
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.spacing.description')}
            </p>
            <ul className="flex flex-col gap-2">
              {SPACING.map((step) => (
                <li key={step.label} className="flex items-center gap-3">
                  <span className={`h-3 shrink-0 rounded-pill bg-primary ${step.bar}`} aria-hidden="true" />
                  <span className="meta">{step.label}</span>
                </li>
              ))}
            </ul>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="01.5"
              title={t('styleguide.radius.title')}
              meta={t('styleguide.radius.meta')}
            />
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.radius.description')}
            </p>
            <ul className="flex flex-wrap items-end gap-4">
              {RADIUS.map((corner) => (
                <li key={corner.shape} className="flex flex-col items-center gap-2">
                  <span
                    className={`h-16 w-16 border border-border-strong bg-surface-raised ${corner.shape}`}
                    aria-hidden="true"
                  />
                  <span className="meta">{t(`styleguide.radius.${corner.name}`)}</span>
                </li>
              ))}
            </ul>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="01.6"
              title={t('styleguide.a11y.title')}
              meta={t('styleguide.a11y.meta')}
            />
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
              <div className="flex flex-col gap-2 rounded-field border border-border p-3">
                <span className="text-h4 text-text">{t('styleguide.a11y.focusTitle')}</span>
                <span className="text-small text-muted">{t('styleguide.a11y.focusDescription')}</span>
                <span className="meta">{t('styleguide.a11y.focusKeys')}</span>
              </div>
              <div className="flex flex-col gap-2 rounded-field border border-border p-3">
                <span className="text-h4 text-text">{t('styleguide.a11y.contrastTitle')}</span>
                <span className="text-small text-muted">
                  {t('styleguide.a11y.contrastDescription')}
                </span>
                {/* The ratio is computed from the tokens rather than quoted from
                    the sheet, so a palette change cannot leave a stale figure on
                    the page. The pair is the same in both themes. */}
                <span className="meta">
                  {t('styleguide.a11y.contrastSample', {
                    ratio: contrast(tokenColor('primary', 'light'), tokenColor('on-primary', 'light')),
                  })}
                </span>
                <span className="text-small text-muted">{t('styleguide.a11y.labels')}</span>
              </div>
              <div className="flex flex-col gap-2 rounded-field border border-border p-3">
                <span className="text-h4 text-text">{t('styleguide.a11y.symbolsTitle')}</span>
                <span className="text-small text-muted">
                  {t('styleguide.a11y.symbolsDescription')}
                </span>
                <span className="flex flex-wrap items-center gap-3 text-muted">
                  {ICONS.map((name) => (
                    <span key={name} className="flex items-center gap-1.5">
                      <Icon name={name} />
                      <span className="meta">{name}</span>
                    </span>
                  ))}
                </span>
                <span className="flex flex-wrap items-center gap-3 text-muted">
                  {ICONS.slice(0, 3).map((name) => (
                    <Icon key={`large-${name}`} name={name} size={24} />
                  ))}
                </span>
              </div>
            </div>
          </div>
        </div>
      </Section>

      <Section title={t('styleguide.controlsSection.title')}>
        <div className="flex flex-col gap-8">
          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="02.1"
              title={t('styleguide.controls.title')}
              meta={t('styleguide.controlsSection.meta')}
            />
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.controls.descriptionBefore')}{' '}
              <code>{t('styleguide.controls.descriptionCode')}</code>{' '}
              {t('styleguide.controls.descriptionAfter')}
            </p>
            <ul className="flex flex-wrap items-center gap-2" aria-hidden="true">
              <li>
                <button type="button" className="btn btn-primary" aria-disabled="true" tabIndex={-1}>
                  {t('styleguide.controls.primary')}
                </button>
              </li>
              <li>
                <button
                  type="button"
                  className="btn btn-secondary"
                  aria-disabled="true"
                  tabIndex={-1}
                >
                  {t('styleguide.controls.secondary')}
                </button>
              </li>
              <li>
                <button type="button" className="btn btn-quiet" aria-disabled="true" tabIndex={-1}>
                  {t('styleguide.controls.quiet')}
                </button>
              </li>
              <li>
                <button type="button" className="btn btn-danger" aria-disabled="true" tabIndex={-1}>
                  {t('styleguide.controls.danger')}
                </button>
              </li>
            </ul>
            <p className="meta">{t('styleguide.controls.disabled')}</p>
            <ul className="flex flex-wrap items-center gap-2" aria-hidden="true">
              <li>
                <button type="button" className="btn btn-primary" disabled tabIndex={-1}>
                  {t('styleguide.controls.primary')}
                </button>
              </li>
              <li>
                <button type="button" className="btn btn-secondary" disabled tabIndex={-1}>
                  {t('styleguide.controls.secondary')}
                </button>
              </li>
              <li>
                <button type="button" className="btn btn-quiet" disabled tabIndex={-1}>
                  {t('styleguide.controls.quiet')}
                </button>
              </li>
              <li>
                <button type="button" className="btn btn-danger" disabled tabIndex={-1}>
                  {t('styleguide.controls.danger')}
                </button>
              </li>
            </ul>
            <p className="max-w-prose text-small text-muted">
              {t('styleguide.controls.liveStates')}
            </p>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="02.2"
              title={t('styleguide.controls.inputTitle')}
              meta={t('styleguide.controls.inputMeta')}
            />
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <Field
                label={t('styleguide.controls.fieldLabel')}
                placeholder={t('styleguide.controls.placeholder')}
                readOnly
                tabIndex={-1}
              />
              <Field
                label={t('styleguide.controls.fieldLabel')}
                hint={t('styleguide.controls.inputHint')}
                readOnly
                tabIndex={-1}
              />
              <Field
                label={t('styleguide.controls.fieldLabel')}
                error={t('styleguide.controls.inputError')}
                readOnly
                tabIndex={-1}
              />
              <Field
                label={t('styleguide.controls.fieldLabel')}
                hint={t('styleguide.controls.inputManaged')}
                readOnly
                disabled
                tabIndex={-1}
              />
            </div>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="02.3"
              title={t('styleguide.controls.selectTitle')}
              meta={t('styleguide.controls.selectMeta')}
            />
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <Select
                label={t('styleguide.controls.selectLabel')}
                tabIndex={-1}
              >
                <option>{t('styleguide.controls.selectOption1')}</option>
                <option>{t('styleguide.controls.selectOption2')}</option>
                <option>{t('styleguide.controls.selectOption3')}</option>
              </Select>
              <Select
                label={t('styleguide.controls.selectLabel')}
                hint={t('styleguide.controls.selectHint')}
                tabIndex={-1}
              >
                <option>{t('styleguide.controls.selectOption1')}</option>
                <option>{t('styleguide.controls.selectOption2')}</option>
                <option>{t('styleguide.controls.selectOption3')}</option>
              </Select>
              <Select
                label={t('styleguide.controls.selectLabel')}
                disabled
                tabIndex={-1}
              >
                <option>{t('styleguide.controls.selectOption1')}</option>
                <option>{t('styleguide.controls.selectOption2')}</option>
                <option>{t('styleguide.controls.selectOption3')}</option>
              </Select>
            </div>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="02.4"
              title={t('styleguide.controls.targets')}
              meta={t('styleguide.controls.targetsMeta')}
            />
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.controls.targetsDescription')}
            </p>
            <div className="flex flex-wrap items-end gap-6" aria-hidden="true">
              <div className="flex flex-col items-start gap-2">
                <button type="button" className="btn btn-primary" aria-disabled="true" tabIndex={-1}>
                  {t('styleguide.controls.primary')}
                </button>
                <span className="meta">44px</span>
              </div>
              <div className="flex flex-col items-start gap-2">
                <input
                  className="field w-56"
                  readOnly
                  tabIndex={-1}
                  aria-hidden="true"
                  value={t('styleguide.controls.placeholder')}
                />
                <span className="meta">44px</span>
              </div>
            </div>
          </div>
        </div>
      </Section>

      <Section title={t('styleguide.patterns.title')}>
        <div className="flex flex-col gap-8">
          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="03.1"
              title={t('styleguide.course.title')}
              meta={t('styleguide.patterns.meta')}
            />
            <p className="max-w-prose text-body text-muted">{t('styleguide.course.description')}</p>
            <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              {courses.map((course) => (
                <li key={course.name} className="card flex items-start gap-3 p-4" style={course.vars}>
                  <span className="course-marker h-12 shrink-0" aria-hidden="true" />
                  <div className="flex min-w-0 flex-col gap-1">
                    <span className="eyebrow">{course.name}</span>
                    <span className="text-h4 text-text">{course.label}</span>
                  </div>
                </li>
              ))}
            </ul>
            <p className="max-w-prose text-body text-muted">{t('styleguide.course.spec')}</p>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="03.2"
              title={t('styleguide.session.title')}
              meta={t('styleguide.session.meta')}
            />
            <div className="flex flex-col gap-2">
              <span className="eyebrow">{t('styleguide.session.next')}</span>
              <div
                className="card flex flex-wrap items-center gap-4 p-4"
                style={courseVars('indigo')}
              >
                <span className="course-marker h-12 shrink-0" aria-hidden="true" />
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="text-label text-text">{t('styleguide.session.name')}</span>
                  <span className="meta">{t('styleguide.session.course')}</span>
                </div>
                <span className="chip chip-due">{t('styleguide.chips.due')}</span>
              </div>
            </div>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="03.3"
              title={t('styleguide.chips.title')}
              meta={t('styleguide.chips.meta')}
            />
            <p className="max-w-prose text-body text-muted">{t('styleguide.chips.description')}</p>
            <ul className="flex flex-wrap items-center gap-2">
              {CHIPS.map((tone) => (
                <li key={tone} className={`chip${tone === 'neutral' ? '' : ` chip-${tone}`}`}>
                  {t(`styleguide.chips.${tone}`)}
                </li>
              ))}
            </ul>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading eyebrow="03.4" title={t('styleguide.citations.title')} />
            <p className="max-w-prose text-body text-muted">
              {t('styleguide.citations.description')}
            </p>
            <div className="flex flex-col gap-3 rounded-field border border-border p-4">
              <p className="text-body">
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
                <Citation
                  citation={{
                    type: 'web',
                    url: 'https://example.com/paper',
                    title: t('styleguide.citations.sampleWebResult'),
                  }}
                />
                {t('styleguide.citations.exampleEnd')}
              </p>
              <p className="text-small text-muted">{t('styleguide.citations.interactive')}</p>
            </div>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="03.5"
              title={t('styleguide.alerts.title')}
              meta={t('styleguide.alerts.meta')}
            />
            <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              {ALERTS.map((alert) => (
                <li
                  key={alert.title}
                  className={`note flex flex-col gap-1 ${alert.tone}`}
                >
                  <span className="text-label font-semibold">
                    {t(`styleguide.alerts.${alert.title}`)}
                  </span>
                  <span className="text-small">{t(`styleguide.alerts.${alert.reason}`)}</span>
                </li>
              ))}
            </ul>
          </div>

          <div className="flex flex-col gap-3">
            <WidgetHeading
              eyebrow="03.6"
              title={t('styleguide.modal.title')}
              meta={t('styleguide.modal.meta')}
            />
            {/* A specimen, not a live dialog: it is always open, so there is no
                trigger to return focus to and no scrim to dismiss. The focus rules
                it documents are in the note below it. */}
            <div className="float mx-auto flex w-full max-w-[37rem] flex-col gap-5 p-6 sm:p-8">
              <p className="font-heading text-h2 text-text">{t('styleguide.modal.heading')}</p>
              <p className="text-body text-muted">{t('styleguide.modal.description')}</p>
              <Field
                label={t('styleguide.modal.fieldLabel')}
                hint={t('styleguide.modal.fieldHint')}
                placeholder={t('styleguide.controls.placeholder')}
                readOnly
                tabIndex={-1}
              />
              <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                <div className="flex flex-col gap-1 rounded-field border border-primary bg-primary-tint p-4">
                  <span className="text-label font-semibold text-primary-text">
                    {t('styleguide.modal.optionSelected')}
                  </span>
                  <span className="meta">{t('styleguide.modal.optionSelectedMeta')}</span>
                </div>
                <div className="flex flex-col gap-1 rounded-field border border-border p-4">
                  <span className="text-label text-text">{t('styleguide.modal.optionDefault')}</span>
                  <span className="meta">{t('styleguide.modal.optionDefaultMeta')}</span>
                </div>
              </div>
              <div className="border-t border-border" />
              <div className="flex flex-wrap items-center justify-end gap-2" aria-hidden="true">
                <button type="button" className="btn btn-secondary" aria-disabled="true" tabIndex={-1}>
                  {t('styleguide.modal.cancel')}
                </button>
                <button type="button" className="btn btn-primary" aria-disabled="true" tabIndex={-1}>
                  {t('styleguide.modal.start')}
                </button>
              </div>
            </div>
            <p className="max-w-prose text-body text-muted">{t('styleguide.modal.note')}</p>
          </div>
        </div>
      </Section>
    </div>
  )
}
