import { useTranslation } from 'react-i18next'

import { asApiError } from '@/api/errors'
import { EXPORT_FORMATS } from '@/api/queries/reports'
import { useUsage, type SubStatus, type UsageLimit } from '@/api/queries/usage'
import { PageHeader, Section } from '@/components/layout/Section'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { formatCount } from '@/features/billing/CreditMeter'
import { planForFormat } from '@/features/billing/plans'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'

/**
 * Plan and usage: what the plan includes, what has been used, and what each
 * action costs.
 *
 * **Every number on this screen comes from `GET /usage`.** Not one is computed
 * here, and that is the honesty rule (roadmap gate 7a/7b) rather than a style
 * preference: a screen that rendered "58 credits" from a literal or a stale cache
 * while the server held 12 would be believed, because it is the screen a student
 * opens to find out why something was refused. The screen test asserts the other
 * half of that — that no number appears which the payload does not contain.
 *
 * The one derivation the screen does make is `used >= limit`, which decides
 * whether a row says "At the limit". It compares two numbers the payload sent; it
 * does not produce a third.
 *
 * **There is no price and no checkout here.** What the plan costs, and the
 * purchase flow, are Phase 13, and `config/billing_policy.toml` governs whether a
 * purchase UI may exist at all on the running platform. The one gated thing on
 * this screen — the export formats the plan does not include — is `LockedFeature`,
 * which reads that policy itself, so on iOS and Android it states the fact with no
 * price, no button and no link (A143).
 */
export function PlanUsageScreen() {
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

  return (
    <div className="flex flex-col gap-6 px-5 py-6">
      <PageHeader title={t('billing.planAndUsage')} subtitle={t('plan.subtitle')} />

      {usage.isPending && <SkeletonRows rows={3} label={t('plan.loading')} />}

      {error !== null && <ApiErrorNotice error={error} />}

      {data !== undefined && (
        <>
          <Section title={t('plan.subscription')}>
            <div className="flex flex-wrap items-center gap-3">
              <p className="text-lg font-medium">{data.name}</p>
              <span className={statusChip[data.subscription.status]}>
                {statusLabel[data.subscription.status]}
              </span>
            </div>
            {data.subscription.period_end !== null && (
              <p className="text-sm text-muted">
                {t('plan.periodEnd', { date: date(data.subscription.period_end) })}
              </p>
            )}
          </Section>

          <Section title={t('plan.credits')}>
            <dl className="flex flex-col gap-1 text-sm">
              {[
                { label: t('plan.creditsMonthly'), value: data.credits.monthly },
                { label: t('plan.creditsPurchased'), value: data.credits.purchased },
                { label: t('plan.creditsTotal'), value: data.credits.total },
              ].map((bucket) => (
                <div key={bucket.label} className="flex items-baseline justify-between gap-4">
                  <dt className="text-muted">{bucket.label}</dt>
                  <dd className="meta">{count(bucket.value)}</dd>
                </div>
              ))}
            </dl>
          </Section>

          <Section title={t('plan.limits')}>
            <dl className="flex flex-col divide-y divide-border border-y border-border text-sm">
              {limits.map((limit) => (
                <div
                  key={limit.label}
                  className="flex items-baseline justify-between gap-4 py-1.5"
                >
                  <dt className="text-muted">{limit.label}</dt>
                  <dd className="flex items-baseline gap-2 text-right">
                    <span className="meta">{limit.value}</span>
                    {limit.atLimit && <span className="chip">{t('plan.limit.atLimit')}</span>}
                  </dd>
                </div>
              ))}
            </dl>
          </Section>

          <Section title={t('plan.costs')}>
            <dl className="flex flex-col divide-y divide-border border-y border-border text-sm">
              {costRows.map((cost) => (
                <div key={cost.label} className="flex items-baseline justify-between gap-4 py-1.5">
                  <dt className="text-muted">{cost.label}</dt>
                  <dd className="meta">{count(cost.credits)}</dd>
                </div>
              ))}
            </dl>
            {costs !== undefined && (
              <p className="text-xs text-muted">
                {t('plan.cost.longDoc', {
                  count: costs.long_doc_credits_per_1k_tokens,
                  tokens: count(costs.long_doc_threshold_tokens),
                })}
              </p>
            )}
          </Section>

          <Section title={t('plan.formats.title')}>
            {includedFormats.length > 0 && (
              <p className="text-sm">
                {t('plan.formats.included', {
                  formats: includedFormats.map((format) => t(`plan.formats.${format}`)).join(', '),
                })}
              </p>
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
          </Section>

          {data.provider_cost.paused && (
            <p className="note note-warning text-sm">
              {t('plan.paused', { date: date(data.provider_cost.resets_at) })}
            </p>
          )}
        </>
      )}
    </div>
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
