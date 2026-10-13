import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import {
  useAdminMargin,
  useAdminPayments,
  useAdminUsers,
  type AdminPayment,
  type AdminUserRow,
  type MarginReport,
  type MonthMargin,
  type Tripwires,
} from '@/api/queries/admin'
import { PageFooter, PageHeader, Section, WidgetHeading } from '@/components/layout/Section'
import { asApiError } from '@/api/errors'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { formatCount } from '@/features/billing/CreditMeter'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { platform } from '@/platform'

/**
 * The admin console: user activity, the payment audit log and the margin report.
 *
 * **Web only, and it says so.** The design's own admin frame carries "ADMIN / WEB"
 * and "ILLUSTRATIVE DATA / NOT A LIVE ACCOUNT"; the first is a real constraint and
 * the second is a warning this screen takes seriously. An operator console on a
 * phone is a screen that shows account data to whoever is holding the phone, and
 * the product has no reason to expose it there — so a phone is told it is not
 * available rather than shown a cramped table.
 *
 * **The access control is the server's, not this file's.** Every route behind this
 * screen takes `AdminUser`, whose extractor answers **404** for a non-admin (A7):
 * whether an admin endpoint exists is not something a non-admin needs to learn.
 * That is why the screen renders "not found" for a signed-in student rather than a
 * permission error — and why hiding this route would be cosmetic rather than a
 * security measure. A client-side check here would be exactly the "client
 * contributes intent, server decides" boundary the threat model draws, so there
 * deliberately is not one.
 *
 * **Every figure is a sum or a count of rows**, and each one is printed with the
 * period it was computed over. The design's frame does the same thing for the same
 * reason: an operator reads these numbers to decide things, and a table whose
 * period is implied by a heading is a table someone will attribute to the wrong
 * month. A figure the server could not compute renders as absent, never as a zero.
 *
 * **Nothing here is a score.** No engagement metric, no account health, no churn
 * prediction, no blended margin. The four summary cards are the margin report's own
 * fields, the two tables are the rows themselves, and the tripwires render as
 * three-valued — "not measured" is a different fact from "not happening".
 */
export function AdminScreen() {
  const { t, i18n } = useTranslation()
  const session = useSession()
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  const isPhone = platform.kind() === 'ios' || platform.kind() === 'android'

  // A 30-day window, which is the server's own default. It is sent explicitly
  // rather than omitted so the screen can print the period it asked for before the
  // answer arrives — an operator should not have to infer the window from a response.
  //
  // **Memoized on the empty dependency list, and that is load-bearing.** The window is
  // part of the query key (`adminKeys.users(from, to)`), so a `new Date()` per render
  // makes a new key every time — each one a fresh cache entry that starts `pending`, and
  // never resolves before the next render replaces it. The screen sat on "Loading
  // accounts" forever while the payments table (a constant key) rendered fine, and the
  // refetches were unbounded. An empty list is correct rather than lazy: the window is
  // the last 30 days *as of when the operator opened the console*, and re-deriving it
  // mid-session would silently move the period out from under the figures on screen.
  const window = useMemo(() => {
    const now = new Date()
    const from = new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000)
    return { from: from.toISOString(), to: now.toISOString() }
  }, [])

  const users = useAdminUsers(window)
  const payments = useAdminPayments()
  const margin = useAdminMargin(window)

  if (isPhone) {
    return (
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-5 py-8">
        <PageHeader title={t('admin.title')} subtitle={t('admin.webOnly')} />
        <EmptyState title={t('admin.webOnlyTitle')} reason={t('admin.webOnlyReason')} />
      </div>
    )
  }

  // The 404 a non-admin gets is the answer, not a failure. It is rendered as an
  // error notice because from a signed-in student's side that is what it is — the
  // screen does not exist for them — and the notice carries the remedy (go back).
  const deniedError = asApiError(users.error)

  return (
    // `w-full` is load-bearing, and this screen is where it was missing. `mx-auto`
    // sets both cross-axis margins to `auto`, and a flex item with an auto cross
    // margin is **not stretched**: it is sized by its content instead. So the page
    // container took its max-content width — which the tables' `min-w-3xl` made
    // 1714px at 200% text — and the whole document scrolled sideways at 390px while
    // every inner `overflow-x-auto` sat there containing nothing. A definite width
    // (`w-full`, capped by `max-w-6xl`) is what puts the scroll back inside the
    // tables' own boxes, and it is the same remedy the dashboard's `w-full` records.
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-5 py-8">
      <PageHeader
        eyebrow={t('admin.eyebrow')}
        title={t('admin.title')}
        subtitle={t('admin.subtitle')}
        action={
          <Link to="/" className="btn btn-secondary">
            {t('admin.returnToApp')}
          </Link>
        }
      />

      {deniedError !== null && <ApiErrorNotice error={deniedError} noun={t('admin.noun')} />}

      {deniedError === null && (
        <>
          <MarginSummary margin={margin.data} loading={margin.isPending} timezone={timezone} />

          <Section title={t('admin.users.title')}>
            {users.isPending && <SkeletonRows rows={3} label={t('admin.users.loading')} />}
            {users.data !== undefined && (
              <div className="card flex flex-col gap-4 p-6">
                <WidgetHeading
                  title={t('admin.users.cardTitle')}
                  eyebrow={t('admin.window', {
                    from: formatInZone(new Date(users.data.from), timezone, { dateStyle: 'medium' }),
                    to: formatInZone(new Date(users.data.to), timezone, { dateStyle: 'medium' }),
                  })}
                  meta={t('admin.users.count', { count: users.data.users.length })}
                />
                {users.data.users.length === 0 ? (
                  <EmptyState
                    title={t('admin.users.emptyTitle')}
                    reason={t('admin.users.emptyReason')}
                  />
                ) : (
                  // A table on a desktop, which is the only place this screen
                  // renders. It scrolls sideways inside its own box rather than
                  // widening the page: six columns of email addresses do not fit
                  // every laptop, and a page that scrolls sideways is the defect
                  // the layout gates exist to catch.
                  <div className="min-w-0 overflow-x-auto">
                    <table className="w-full min-w-3xl border-collapse text-left">
                      <thead>
                        <tr className="border-b border-border">
                          {[
                            t('admin.users.columnUser'),
                            t('admin.users.columnPlan'),
                            t('admin.users.columnRequests'),
                            t('admin.users.columnSources'),
                            t('admin.users.columnLastActive'),
                          ].map((label) => (
                            <th key={label} scope="col" className="eyebrow py-2 pr-4">
                              {label}
                            </th>
                          ))}
                        </tr>
                      </thead>
                      <tbody>
                        {users.data.users.map((row) => (
                          <UserRow
                            key={row.id}
                            row={row}
                            timezone={timezone}
                            language={i18n.language}
                          />
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            )}
          </Section>

          <PaymentsSection payments={payments.data?.payments} loading={payments.isPending} timezone={timezone} />

          <MarginTable margin={margin.data} loading={margin.isPending} />
        </>
      )}

      <PageFooter
        left={t('admin.footer', { zone: timezone })}
        right={t('admin.scope')}
      />
    </div>
  )
}

/**
 * The four summary figures, from the newest month of the margin report.
 *
 * They are the report's own fields rather than a separate query, so the card and
 * the table below it cannot disagree. The month is named on every card's caption
 * because "this month" is a claim about a calendar, and the report's months are
 * UTC (`MonthMargin.month` says why).
 */
function MarginSummary({
  margin,
  loading,
  timezone,
}: {
  /** Absent while the read is in flight, and absent is not zero. */
  margin: MarginReport | undefined
  loading: boolean
  timezone: string
}) {
  const { t, i18n } = useTranslation()
  if (loading) return <SkeletonRows rows={1} label={t('admin.loading')} />
  const month = margin?.months.at(-1)
  if (month === undefined) return null

  const revenue = month.revenue_idr
  const cost = month.per_plan.reduce((sum, row) => sum + row.provider_cost_usd, 0)
  const costIdr = Math.round(cost * (margin?.idr_per_usd ?? 0))
  // The margin is computed here rather than read from a plan row because the
  // report's `gross_margin_idr` is per plan: a blended figure has to be derived, and
  // deriving it is what makes the derivation printable beside it.
  const contribution = revenue - costIdr
  const percent = revenue === 0 ? null : Math.round((contribution / revenue) * 1000) / 10

  const cards = [
    // The locale is the screen's, not a literal: `formatCount` maps it to `INTL_LOCALE`,
    // and the table below groups the same kind of number the Indonesian way. Hard-coding
    // `en` here put `1,500` on this card above a table reading `1.500`.
    { label: t('admin.summary.activeUsers'), value: formatCount(month.paying + month.free_accounts, i18n.language), note: t('admin.summary.activeUsersNote', { paying: month.paying, free: month.free_accounts }) },
    { label: t('admin.summary.revenue'), value: formatIdr(revenue), note: t('admin.summary.revenueNote') },
    {
      label: t('admin.summary.cost'),
      value: formatIdr(costIdr),
      // The rate is a rupiah amount, so it goes through the rupiah formatter:
      // rendering it with `en` grouping put "Rp 16,000" on this card and
      // "Rp 16.000" in the margin section for the same number.
      note: t('admin.summary.costNote', { rate: formatIdr(margin?.idr_per_usd ?? 0) }),
    },
    {
      label: t('admin.summary.margin'),
      // `null` when there was no revenue: a percentage of nothing is unanswerable,
      // and a `0%` would read as a measured loss.
      value: percent === null ? t('admin.summary.noRevenue') : `${percent}%`,
      note: t('admin.summary.marginNote', { contribution: formatIdr(contribution) }),
    },
  ]

  return (
    <Section title={t('admin.summary.title')}>
      <p className="eyebrow">{t('admin.summary.month', { month: month.month })}</p>
      {/* `auto-fit` with a **rem** floor rather than `sm:grid-cols-2 xl:grid-cols-4`, and
          the floor is what makes this survive 200% text.

          A fixed count cannot: at a 32px root the rail is 432px of a 1365px window (it is
          216px in rems), so the section has 853px, four fixed columns leave each card
          189px, and `p-6` is 48px a side — 93px of content for a 64px-font figure that
          needs 162px. Measured, and the figure was clipped. A rem floor grows with the
          root, so the track count falls as the text grows: four across at 100% (the
          design's row) and two at 200%, which is the reflow a fixed count cannot express.
          The floor is 9rem because the widest line here is a rupiah figure at `text-h2`
          (162px measured) and the Indonesian copy is ~25% longer, against a card whose
          own padding is 96px at that scale.

          The same instrument, and the same reasoning, as the study screen's grade row. */}
      <ul className="grid gap-4 sm:grid-cols-[repeat(auto-fit,minmax(9rem,1fr))]">
        {cards.map((card) => (
          <li key={card.label} className="card flex min-w-0 flex-col gap-1 p-6">
            <span className="text-label text-muted">{card.label}</span>
            <span className="text-h2 break-words">{card.value}</span>
            <span className="meta break-words">{card.note}</span>
          </li>
        ))}
      </ul>
      <p className="meta">
        {t('admin.summary.asOf', {
          when: formatInZone(new Date(month.as_of), timezone, { dateStyle: 'medium', timeStyle: 'short' }),
        })}
      </p>
    </Section>
  )
}

/** One account's row. */
function UserRow({
  row,
  timezone,
  language,
}: {
  row: AdminUserRow
  timezone: string
  language: string
}) {
  const { t } = useTranslation()
  return (
    <tr className="border-b border-border">
      <td className="py-3 pr-4">
        <span className="flex flex-col">
          <span className="text-label">{row.email}</span>
          {row.is_admin && <span className="meta">{t('admin.users.admin')}</span>}
        </span>
      </td>
      <td className="py-3 pr-4 text-label">{row.plan_id}</td>
      <td className="py-3 pr-4 meta">{formatCount(row.requests, language)}</td>
      <td className="py-3 pr-4 meta">{formatCount(row.sources_added, language)}</td>
      <td className="py-3 meta">
        {/* `null` is "has never taken an action", which is a different fact from a
            date, and it is stated rather than left blank. */}
        {row.last_active_at === null
          ? t('admin.users.neverActive')
          : formatInZone(new Date(row.last_active_at), timezone, {
              dateStyle: 'medium',
              timeStyle: 'short',
            })}
      </td>
    </tr>
  )
}

/**
 * The payment audit log.
 *
 * The rows are the payments table, unsummarised: an audit log that grouped or
 * rounded would not be an audit log. The statuses are the gateway's own strings,
 * shown as they arrived rather than mapped to a friendlier word, because an
 * operator reconciling against the gateway's dashboard needs the two to match.
 */
function PaymentsSection({
  payments,
  loading,
  timezone,
}: {
  payments: AdminPayment[] | undefined
  loading: boolean
  timezone: string
}) {
  const { t } = useTranslation()

  return (
    <Section title={t('admin.payments.title')}>
      {loading && <SkeletonRows rows={3} label={t('admin.payments.loading')} />}
      {payments !== undefined && (
        <div className="card flex flex-col gap-4 p-6">
          <WidgetHeading
            title={t('admin.payments.cardTitle')}
            meta={t('admin.payments.count', { count: payments.length })}
          />
          {payments.length === 0 ? (
            <EmptyState title={t('admin.payments.emptyTitle')} reason={t('admin.payments.emptyReason')} />
          ) : (
            <div className="min-w-0 overflow-x-auto">
              <table className="w-full min-w-3xl border-collapse text-left">
                <thead>
                  <tr className="border-b border-border">
                    {[
                      t('admin.payments.columnOrder'),
                      t('admin.payments.columnPlan'),
                      t('admin.payments.columnAmount'),
                      t('admin.payments.columnStatus'),
                      t('admin.payments.columnUpdated'),
                    ].map((label) => (
                      <th key={label} scope="col" className="eyebrow py-2 pr-4">
                        {label}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {payments.map((payment) => (
                    <tr key={payment.id} className="border-b border-border">
                      {/* The order id in mono: it is the reference an operator
                          pastes into the gateway's dashboard. */}
                      <td className="py-3 pr-4 meta">{payment.order_id}</td>
                      <td className="py-3 pr-4 text-label">
                        {payment.plan_id ?? t('admin.payments.credits', { count: payment.credits ?? 0 })}
                      </td>
                      <td className="py-3 pr-4 meta">{formatIdr(payment.amount_idr)}</td>
                      <td className="py-3 pr-4">
                        <span className={statusChip(payment.status)}>{payment.status}</span>
                        {payment.review_reason !== null && (
                          <span className="meta block">{payment.review_reason}</span>
                        )}
                      </td>
                      <td className="py-3 meta">
                        {formatInZone(
                          new Date(payment.paid_at ?? payment.created_at ?? payment.expires_at),
                          timezone,
                          { dateStyle: 'medium', timeStyle: 'short' },
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </Section>
  )
}

/**
 * A payment's status, in a tone.
 *
 * The word is always present — the tone only repeats it — and `review` gets the
 * warning tone rather than the danger one: an order under review is not a failure,
 * it is one a human has to look at, which is the distinction the server's own
 * `review` status exists to make.
 */
function statusChip(status: string): string {
  switch (status) {
    case 'paid':
      return 'chip chip-ready'
    case 'review':
      return 'chip chip-due'
    case 'failed':
    case 'refunded':
      return 'chip chip-failed'
    case 'pending':
      return 'chip chip-working'
    default:
      return 'chip'
  }
}

/** The margin report, by month and plan, with its tripwires. */
function MarginTable({
  margin,
  loading,
}: {
  margin: MarginReport | undefined
  loading: boolean
}) {
  const { t } = useTranslation()
  if (loading) return <SkeletonRows rows={3} label={t('admin.margin.loading')} />
  if (margin === undefined) return null

  return (
    <Section title={t('admin.margin.title')}>
      <div className="card flex flex-col gap-4 p-6">
        <WidgetHeading
          title={t('admin.margin.cardTitle')}
          meta={t('admin.margin.months', { count: margin.months.length })}
        />
        {margin.months.length === 0 ? (
          <EmptyState title={t('admin.margin.emptyTitle')} reason={t('admin.margin.emptyReason')} />
        ) : (
          <ul className="flex flex-col gap-4">
            {margin.months.map((month) => (
              <MonthBlock key={month.month} month={month} />
            ))}
          </ul>
        )}

        {/* The rate the arithmetic used, printed as the input it is: an operator
            has to be able to check the conversion, and a rate that arrived from
            config is not something this screen can verify. */}
        <p className="meta">{t('admin.margin.rate', { rate: formatIdr(margin.idr_per_usd) })}</p>
      </div>

      <TripwireList tripwires={margin.tripwires} />
    </Section>
  )
}

function MonthBlock({ month }: { month: MonthMargin }) {
  const { t } = useTranslation()
  return (
    <li className="flex flex-col gap-2 border-t border-border pt-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h4>{month.month}</h4>
        <span className="meta">
          {t('admin.margin.monthLine', {
            paying: month.paying,
            free: month.free_accounts,
            revenue: formatIdr(month.revenue_idr),
            fee: formatIdr(month.gateway_fee_idr),
          })}
        </span>
      </div>
      <div className="min-w-0 overflow-x-auto">
        {month.per_plan.length === 0 ? (
          // A month in range with no rows at all. The report includes it on purpose
          // (`MarginReport.months` says why: a missing month would make "two months
          // running" quietly mean "two entries running"), so it is rendered as a
          // stated absence rather than an empty table that reads as a rendering bug.
          <p className="meta">{t('admin.margin.monthEmpty')}</p>
        ) : (
          <table className="w-full min-w-2xl border-collapse text-left">
            <thead>
              <tr className="border-b border-border">
                {[
                  t('admin.margin.columnPlan'),
                  t('admin.margin.columnCost'),
                  t('admin.margin.columnByok'),
                  t('admin.margin.columnRevenue'),
                  t('admin.margin.columnMargin'),
                  t('admin.margin.columnCeiling'),
                ].map((label) => (
                  <th key={label} scope="col" className="eyebrow py-2 pr-4">
                    {label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {month.per_plan.map((row) => (
                <tr key={row.plan_id} className="border-b border-border">
                  <td className="py-3 pr-4 text-label">{row.plan_id}</td>
                  <td className="py-3 pr-4 meta">
                    {t('admin.margin.usd', { amount: row.provider_cost_usd.toFixed(4) })}
                  </td>
                  <td className="py-3 pr-4 meta">
                    {t('admin.margin.usd', { amount: row.byok_cost_usd.toFixed(4) })}
                  </td>
                  <td className="py-3 pr-4 meta">{formatIdr(row.revenue_idr)}</td>
                  <td className="py-3 pr-4 meta">
                    {/* `null` is "either side of the subtraction is unknown", so it is
                        stated rather than shown as a break-even zero. */}
                    {row.gross_margin_idr === null
                      ? t('admin.margin.unknown')
                      : formatIdr(row.gross_margin_idr)}
                  </td>
                  <td className="py-3 pr-4 meta">
                    {row.ceiling_usd === null
                      ? t('admin.margin.noCeiling')
                      : t('admin.margin.usd', { amount: row.ceiling_usd.toFixed(2) })}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      <p className="meta">
        {t('admin.margin.conversion', {
          ofFree: percent(month.conversion_of_free),
          ofTotal: percent(month.conversion_of_total),
          utilization: percent(month.free_utilization),
        })}
      </p>
    </li>
  )
}

/**
 * The four thresholds, three-valued.
 *
 * `null` renders as "not measured", never as "not triggered": a tripwire with no
 * data that read as `false` would report safety it never measured, which is the
 * defect the server's `Option<bool>` exists to prevent and this renderer has to
 * preserve. The tone follows the value, and the word is always present.
 */
function TripwireList({ tripwires }: { tripwires: Tripwires }) {
  const { t } = useTranslation()
  const entries: [string, boolean | null][] = [
    [t('admin.tripwire.freeUtilization'), tripwires.free_utilization_over_30],
    [t('admin.tripwire.conversion'), tripwires.conversion_under_3],
    [t('admin.tripwire.freeSpend'), tripwires.free_spend_over_quarter_of_revenue],
    [t('admin.tripwire.twoMonths'), tripwires.plan_loss_two_months_running],
  ]

  return (
    <div className="card flex flex-col gap-2 p-6">
      <WidgetHeading title={t('admin.tripwire.title')} eyebrow={t('admin.tripwire.eyebrow')} />
      <ul className="flex flex-col gap-2">
        {entries.map(([label, value]) => (
          <li key={label} className="flex flex-wrap items-center gap-2">
            <span className="text-label">{label}</span>
            <span className={value === null ? 'chip' : value ? 'chip chip-due' : 'chip chip-ready'}>
              {value === null
                ? t('admin.tripwire.notMeasured')
                : value
                  ? t('admin.tripwire.triggered')
                  : t('admin.tripwire.clear')}
            </span>
          </li>
        ))}
      </ul>
    </div>
  )
}

/**
 * A percentage the report could not compute, or the figure.
 *
 * `null` is rendered as a word rather than as `0%`, because a ratio with no
 * denominator is unanswerable and a zero would read as a measured value. The one
 * place this app is allowed to round is here — an operator reads a rate, not a
 * ledger — and it rounds to one decimal so a rate near a threshold does not read
 * as the threshold.
 */
function percent(value: number | null): string {
  return value === null ? '—' : `${Math.round(value * 1000) / 10}%`
}

/** Rupiah, grouped the Indonesian way: `Rp 45.000`, which is forty-five thousand. */
function formatIdr(amount: number): string {
  return `Rp ${new Intl.NumberFormat('id-ID').format(amount)}`
}
