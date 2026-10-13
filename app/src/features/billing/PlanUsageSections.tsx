import { useTranslation } from 'react-i18next'

import { asApiError } from '@/api/errors'
import { EXPORT_FORMATS } from '@/api/queries/reports'
import { useUsage, type SubStatus, type UsageLimit } from '@/api/queries/usage'
import { Section } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { formatCount } from '@/features/billing/CreditMeter'
import { planForFeature, planForFormat } from '@/features/billing/plans'
import { lockedFeaturePolicy } from '@/features/billing/policy'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { platform } from '@/platform'

/**
 * What the plan includes, what has been used, and what each action costs.
 *
 * **Every number here comes from `GET /usage`.** Not one is computed in this
 * module, and that is the honesty rule (roadmap gate 7a/7b) rather than a style
 * preference: a screen that rendered "58 credits" from a literal or a stale cache
 * while the server held 12 would be believed, because it is the screen a student
 * opens to find out why something was refused. The screen test asserts the other
 * half of that — that no number appears which the payload does not contain.
 *
 * The one derivation made here is `used >= limit`, which decides whether a row
 * says "At the limit". It compares two numbers the payload sent; it does not
 * produce a third.
 *
 * **The monthly-credits card is two counts, and it says so in words.** The design
 * draws a bar and a `90% remaining` label; a percentage is a number the server
 * does not compute, so it is deliberately not rendered. What the payload does
 * carry is the plan's monthly allowance (`limits.credits_per_month.limit`) and the
 * unspent part of it (`credits.monthly`), so the card shows those two — `60 of 60`
 * — beside a bar, and the spent count (`limit - monthly`, the one subtraction in
 * this module) as the summary. The bar is `aria-hidden`: the sentence above it is
 * the fact, and a bar that carried a fact no text carried would be unreadable to a
 * screen reader.
 *
 * **Two things the design draws are the shell's, and are deliberately not built
 * here.** The phone app bar — a back control and the page title — is `Shell`'s;
 * the account identity row (the 44px avatar, the name and address, the wordmark)
 * is `PhoneHeader`/`TopBar`'s, which already render the wordmark, the avatar and
 * the account group on every screen. Rendering either here would put two identity
 * rows and two titles on one screen. What is this screen's own is the `PageHeader`
 * below, which carries the title in the content column on every form factor.
 *
 * **This module is the body, and the two route elements are the difference
 * between them.** `/billing` renders `PlanUsageScreen` on web and desktop and
 * `MobilePlanScreen` on iOS and Android (A322), and the two must not be the same
 * module: the phone route element is chosen at the router so that whatever
 * purchase surface Phase 13 puts on the web/desktop screen is never reachable
 * from the phone. If the phone screen rendered the desktop one, that property
 * would be a policy check inside a component instead of a structural fact. What
 * the two screens genuinely share is this: the plan, the credits, the limits, the
 * costs and the locked formats, all read from one payload, with the neutral
 * `lock.notOnPlan` copy that `LockedFeature` resolves from the policy itself
 * (A143). Keeping one copy of it is what stops the two screens drifting into two
 * different sets of numbers.
 *
 * The exports come back as a fragment, not a wrapper: the caller's column is a
 * `flex flex-col gap-6`, so each section has to stay a direct child of it.
 */
export function PlanUsageSections() {
  const { t, i18n } = useTranslation()
  const usage = useUsage()
  const session = useSession()
  const data = usage.data
  const error = asApiError(usage.error)

  // The server computes `resets_at` as the start of the *user's* local month
  // (`util::next_local_month_start`), so the instant has to be rendered in that
  // zone. Formatted in UTC it can name the previous day, which would send a
  // student in Jakarta back a day early.
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  const date = (iso: string) => formatInZone(new Date(iso), timezone, { dateStyle: 'medium' })
  const count = (value: number) => formatCount(value, i18n.language)

  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const statusLabel: Record<SubStatus, string> = {
    none: t('plan.status.none'),
    active: t('plan.status.active'),
    grace: t('plan.status.grace'),
    expired: t('plan.status.expired'),
    canceled: t('plan.status.canceled'),
  }
  // The word carries the state; the tone only repeats it, so the row survives
  // monochrome (roadmap gate 5d).
  const statusChip: Record<SubStatus, string> = {
    none: 'chip',
    active: 'chip chip-ready',
    grace: 'chip chip-due',
    expired: 'chip chip-failed',
    canceled: 'chip',
  }

  /**
   * One limit as the user reads it: `used of limit`, "Unlimited" for an axis the
   * plan does not cap, or the ceiling alone for an axis with no counter to
   * report.
   */
  const row = (label: string, axis: UsageLimit): LimitRow => {
    if (axis.limit === null) return { label, value: t('plan.limit.unlimited'), atLimit: false }
    if (axis.used === null) return { label, value: count(axis.limit), atLimit: false }
    return {
      label,
      value: t('plan.limit.usedOfLimit', { used: count(axis.used), limit: count(axis.limit) }),
      atLimit: axis.used >= axis.limit,
    }
  }

  const limits: LimitRow[] = data === undefined ? [] : [
    row(t('plan.limit.workspaces'), data.limits.workspaces),
    row(t('plan.limit.sourcesTotal'), data.limits.sources_total),
    row(t('plan.limit.maxFileMb'), data.limits.max_file_mb),
    // Storage is its own shape: the ceiling is in MB and what the account holds
    // is `storage_bytes`, reported in bytes rather than as a rounded megabyte
    // count the server would have had to invent. The null branch is the uniform
    // wire shape's (`limit_json` types every axis as `number | null`) rather than
    // something `config/plans.toml` can produce — a plan's `storage_mb` is always
    // a maximum — so it reads as a byte count with no ceiling to compare against.
    data.limits.storage_mb.limit === null
      ? {
          label: t('plan.limit.storage'),
          value: t('plan.limit.storageUsedNoLimit', { used: count(data.storage_bytes) }),
          atLimit: false,
        }
      : {
          label: t('plan.limit.storage'),
          value: t('plan.limit.storageUsed', {
            used: count(data.storage_bytes),
            limit: count(data.limits.storage_mb.limit),
          }),
          atLimit: false,
        },
    row(t('plan.limit.devicesMax'), data.limits.devices_max),
    row(t('plan.limit.creditsPerMonth'), data.limits.credits_per_month),
    row(t('plan.limit.chatMsgsPerDay'), data.limits.chat_msgs_per_day),
    row(t('plan.limit.webSearchesPerDay'), data.limits.web_searches_per_day),
    row(t('plan.limit.flashcardsPerMonth'), data.limits.flashcards_per_month),
    row(t('plan.limit.quizzesPerMonth'), data.limits.quizzes_per_month),
    row(t('plan.limit.reportsPerMonth'), data.limits.reports_per_month),
    row(t('plan.limit.syllabusImportsTotal'), data.limits.syllabus_imports_total),
    row(t('plan.limit.googleAccountsMax'), data.limits.google_accounts_max),
  ]

  const costs = data?.costs
  const costRows: CostRow[] =
    costs === undefined
      ? []
      : [
          { label: t('plan.cost.chatMessage'), credits: costs.chat_message },
          { label: t('plan.cost.webSearch'), credits: costs.web_search },
          { label: t('plan.cost.flashcards'), credits: costs.flashcard_batch_10 },
          { label: t('plan.cost.quiz'), credits: costs.quiz_10 },
          { label: t('plan.cost.reportOutline'), credits: costs.report_outline },
          { label: t('plan.cost.reportSection'), credits: costs.report_section },
          { label: t('plan.cost.taskBreakdown'), credits: costs.task_breakdown },
          { label: t('plan.cost.syllabusImport'), credits: costs.syllabus_import },
          { label: t('plan.cost.planGeneration'), credits: costs.plan_generation },
          { label: t('plan.cost.agentTurn'), credits: costs.agent_turn },
        ]

  // The formats the app can actually export, not every format the matrix names:
  // `config/plans.toml` grants `docx`/`csv`/`json` on higher plans, and the export
  // route answers only `md` and `pdf`. Offering a format the server would refuse
  // would be this screen promising something no route implements.
  const includedFormats =
    data === undefined
      ? []
      : EXPORT_FORMATS.filter((format) => data.features.export_formats.includes(format))
  const lockedFormats = EXPORT_FORMATS.filter((format) => !includedFormats.includes(format))

  // The monthly-credits card, in the payload's own terms. `limit` is the plan's
  // allowance for a month; `monthly` is the part of it not yet spent. Those two are
  // the card, and the bar's width is their ratio.
  //
  // **There is deliberately no "used this month" count here.** `limits.
  // credits_per_month.used` is `null` — the server does not report usage on that
  // axis — and the tempting substitute, `allowance - balance`, is not that fact:
  // `credits.monthly` is a ledger sum, so a plan changed mid-period (a balance
  // larger than the new allowance), or a period whose grant has not landed yet (a
  // zero balance against a full allowance), both make the difference something
  // other than what was spent. An earlier revision printed it behind a
  // `Math.max(…, 0)`, and the clamp is what gives the defect away: it exists only
  // to hide the state where the balance exceeds the allowance, which is to say it
  // manufactures a usage figure rather than omitting one. The frame's `50 used` is
  // illustrative; the two balances the payload does carry are on screen instead.
  const creditsLimit = data === undefined ? null : data.limits.credits_per_month.limit
  const creditsRemaining = data?.credits.monthly
  const creditsShare =
    creditsLimit !== null && creditsLimit > 0 && creditsRemaining !== undefined
      ? Math.min(creditsRemaining / creditsLimit, 1) * 100
      : null

  // Which features are included is the payload's answer, never a list written
  // here: a plan edited in `config/plans.toml` moves these rows without a code
  // change, and a feature the server does not grant cannot read as included.
  const features: FeatureRow[] =
    data === undefined
      ? []
      : [
          { id: 'advanced_analytics', included: data.features.advanced_analytics },
          { id: 'task_ai_breakdown', included: data.features.task_ai_breakdown },
          { id: 'quick_capture', included: data.features.quick_capture },
          { id: 'google_calendar', included: data.features.google_calendar },
          { id: 'agent_actions', included: data.features.agent_actions },
          { id: 'byok', included: data.features.byok },
        ]
  const includedFeatures = features.filter((feature) => feature.included)
  const lockedFeatures = features.filter((feature) => !feature.included)
  // The same policy `LockedFeature` resolves for the export formats: web and
  // desktop may name the plan and link to it, iOS and Android may not (A24). On a
  // phone a locked feature is therefore a row stating the fact, and on
  // web/desktop it is `LockedFeature`, which is the component that owns that copy.
  const featurePolicy = lockedFeaturePolicy(platform.kind())
  const featureRows = featurePolicy.showUpgradeCta ? includedFeatures : features

  return (
    <>
      {usage.isPending && <SkeletonRows rows={3} label={t('plan.loading')} />}

      {error !== null && <ApiErrorNotice error={error} />}

      {data !== undefined && (
        <>
          <Section title={t('plan.subscription')}>
            <div className="card flex flex-col gap-3 p-5">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <p className="eyebrow">{t('plan.currentPlan')}</p>
                <span className="chip chip-ready">{data.name}</span>
              </div>
              <div className="flex flex-wrap items-baseline justify-between gap-3 border-t border-border pt-3">
                <span className="text-label text-muted">{t('plan.statusLabel')}</span>
                <span className={statusChip[data.subscription.status]}>
                  {statusLabel[data.subscription.status]}
                </span>
              </div>
              {data.subscription.period_end !== null && (
                <div className="flex flex-wrap items-baseline justify-between gap-3">
                  <span className="text-label text-muted">{t('plan.renewalDate')}</span>
                  <span className="meta">{date(data.subscription.period_end)}</span>
                </div>
              )}
            </div>
          </Section>

          <div className="raised flex flex-col gap-3 p-5">
            <h3 className="text-h4">{t('plan.creditsCard.title')}</h3>
            {creditsRemaining !== undefined && (
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="text-h2 font-semibold text-primary-text">
                  {count(creditsRemaining)}
                </span>
                <span className="text-label text-muted">{t('plan.creditsCard.left')}</span>
              </div>
            )}
            {creditsShare !== null && creditsLimit !== null && creditsRemaining !== undefined && (
              <div className="flex flex-col gap-1">
                {/* The bar is decorative: the sentence under it carries the same two
                    counts, so the fact is not locked in a width. `bg-border` rather
                    than the raised tint the card already is, which would make the
                    track invisible against its own surface. */}
                <div
                  aria-hidden="true"
                  className="h-2 w-full overflow-hidden rounded-field bg-border"
                >
                  <div className="h-full rounded-field bg-primary" style={{ width: `${creditsShare}%` }} />
                </div>
                <p className="meta">
                  {t('plan.creditsCard.remainingOfTotal', {
                    remaining: count(creditsRemaining),
                    total: count(creditsLimit),
                  })}
                </p>
              </div>
            )}
            <p className="text-caption text-muted">
              {t('plan.creditsCard.reset', { date: date(data.provider_cost.resets_at) })}
            </p>
          </div>

          <div className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <h3 className="text-h4">{t('plan.features.title')}</h3>
              <p className="text-small text-muted">{t('plan.features.policy')}</p>
            </div>
            {featureRows.length > 0 && (
              <div className="card flex flex-col divide-y divide-border">
                {featureRows.map((feature) => (
                  <div key={feature.id} className="flex min-h-11 items-center gap-3 p-3">
                    <Icon
                      name={feature.included ? 'check' : 'lock'}
                      size={18}
                      className={`shrink-0 ${feature.included ? 'text-success' : 'text-subtle'}`}
                    />
                    <span className="min-w-0 flex-1 text-small">{t(`plan.feature.${feature.id}`)}</span>
                    <span className="meta">
                      {feature.included ? t('plan.features.included') : t('plan.features.locked')}
                    </span>
                  </div>
                ))}
              </div>
            )}
            {featurePolicy.showUpgradeCta &&
              lockedFeatures.map((feature) => (
                <LockedFeature
                  key={feature.id}
                  whatKey={`plan.feature.${feature.id}`}
                  // The plan that includes it comes from `config/plans.toml` through
                  // the same reader the feature's own grant is declared in, so a plan
                  // edited there cannot leave this message naming the wrong one.
                  minPlan={planForFeature(feature.id) ?? 'Pro'}
                />
              ))}
          </div>

          <Section title={t('plan.credits')}>
            <div className="card flex flex-col divide-y divide-border p-4">
              {[
                { label: t('plan.creditsMonthly'), value: data.credits.monthly },
                { label: t('plan.creditsPurchased'), value: data.credits.purchased },
                { label: t('plan.creditsTotal'), value: data.credits.total },
              ].map((bucket) => (
                <div
                  key={bucket.label}
                  className="flex items-baseline justify-between gap-4 py-1.5"
                >
                  <span className="text-small text-muted">{bucket.label}</span>
                  <span className="meta">{count(bucket.value)}</span>
                </div>
              ))}
            </div>
          </Section>

          <Section title={t('plan.limits')}>
            <div className="card flex flex-col divide-y divide-border p-4">
              {limits.map((limit) => (
                /* `flex-wrap` is load-bearing. `justify-between` on a row that
                   cannot wrap puts the label and the value on one line whatever it
                   costs, and the value here is a count *plus* an "At the limit"
                   chip: measured in Chromium at 390px with 200% text, the
                   Workspaces row's `scrollWidth` was 327 against a `clientWidth` of
                   310, so the chip sat outside the row's own box. The credits and
                   costs rows share the row shape and were measured with it — both
                   are label plus a short number and neither overflows, so neither is
                   changed here. */
                <div
                  key={limit.label}
                  className="flex flex-wrap items-baseline justify-between gap-4 py-1.5"
                >
                  <span className="text-small text-muted">{limit.label}</span>
                  <span className="flex items-baseline gap-2 text-right">
                    <span className="meta">{limit.value}</span>
                    {limit.atLimit && <span className="chip">{t('plan.limit.atLimit')}</span>}
                  </span>
                </div>
              ))}
            </div>
          </Section>

          <Section title={t('plan.costs')}>
            <div className="card flex flex-col gap-3 p-4">
              <div className="flex flex-col divide-y divide-border">
                {costRows.map((cost) => (
                  <div key={cost.label} className="flex items-baseline justify-between gap-4 py-1.5">
                    <span className="text-small text-muted">{cost.label}</span>
                    <span className="meta">{count(cost.credits)}</span>
                  </div>
                ))}
              </div>
              {costs !== undefined && (
                <p className="text-caption text-muted">
                  {t('plan.cost.longDoc', {
                    count: costs.long_doc_credits_per_1k_tokens,
                    tokens: count(costs.long_doc_threshold_tokens),
                  })}
                </p>
              )}
            </div>
          </Section>

          <Section title={t('plan.formats.title')}>
            <div className="flex flex-col gap-3">
              {includedFormats.length > 0 && (
                <div className="card p-4">
                  <p className="text-small">
                    {t('plan.formats.included', {
                      formats: includedFormats.map((format) => t(`plan.formats.${format}`)).join(', '),
                    })}
                  </p>
                </div>
              )}
              {lockedFormats.map((format) => (
                <LockedFeature
                  key={format}
                  whatKey={`plan.formats.${format}`}
                  // The plan that includes it comes from `config/plans.toml` through
                  // the same reader the export menu uses, so a format added to a plan
                  // there cannot leave this message naming the wrong one.
                  minPlan={planForFormat(format) ?? 'Pro'}
                />
              ))}
            </div>
          </Section>

          {data.provider_cost.paused && (
            <p className="note note-warning text-small">
              {t('plan.paused', { date: date(data.provider_cost.resets_at) })}
            </p>
          )}
        </>
      )}
    </>
  )
}

/** One limit row, resolved to the strings the user reads. */
interface LimitRow {
  label: string
  value: string
  /** True when the account has reached the ceiling, so the row says so in words. */
  atLimit: boolean
}

/** One action and what it costs, both from the payload's `costs` block. */
interface CostRow {
  label: string
  credits: number
}

/** One switch from the payload's `features` block, and whether this plan grants it. */
interface FeatureRow {
  id: string
  included: boolean
}
