//! Admin: the manual plan switch, the manual credit grant, and the margin scaffold.
//!
//! Three routes, deliberately API-only until Phase 13 builds the admin surface. Every one
//! takes [`AdminUser`], whose extractor answers **404** for a non-admin (A7): whether an
//! admin endpoint exists is not something a non-admin needs to learn.
//!
//! The margin report is Phase 13's, completed. What lands here is the query Phase 13
//! renders — provider cost split free from paid, with BYOK in its own column (A156) — and
//! `revenue: None` / `conversion: None` until there is a `payments` row to compute them
//! from. **A `0` there would be an invented number**, and A46's tripwires are read off this
//! report (roadmap gate 7(a)).

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::routing::{get, patch, post};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use stvdy_core::config::PlanId;
use stvdy_core::traits::payment::{WebhookEvent, WebhookKind};
use uuid::Uuid;

use crate::auth::extract::AdminUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{credits, entitlements};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/users/{id}/plan", patch(switch_plan))
        .route("/admin/users/{id}/credits", post(grant_credits))
        .route("/admin/margin", get(margin))
        // The payment tools (A308). `mark-paid` runs the webhook's own apply rather than a
        // second implementation, and `refund` records the row and leaves the subscription
        // alone — neither deletes anything.
        .route("/admin/payments", get(list_payments))
        .route("/admin/payments/{id}/mark-paid", post(mark_paid))
        .route("/admin/payments/{id}/refund", post(refund_payment))
        .route("/admin/subscriptions/{user_id}", patch(extend_subscription))
}

#[derive(Debug, Deserialize)]
pub struct PlanRequest {
    pub plan: String,
    /// The period the subscription is set to cover. Defaults to one month; the manual
    /// switch is a comp, so the admin supplies the window rather than a payment.
    #[serde(default)]
    pub period_days: Option<i64>,
}

/// `PATCH /admin/users/:id/plan`
///
/// Writes `users.plan_id`, upserts the account's live `subscriptions` row and recomputes
/// the read-only flags — because a downgrade and a switch are the same event from the
/// data's point of view: either can leave rows over the new plan's limits, and the flags
/// are what make them read-only rather than deleted.
pub async fn switch_plan(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<PlanRequest>,
) -> AppResult<Json<Value>> {
    let plan = PlanId::ALL
        .into_iter()
        .find(|candidate| candidate.as_str() == body.plan.trim())
        .ok_or_else(|| {
            AppError::Validation(format!(
                "plan must be one of {}",
                PlanId::ALL
                    .iter()
                    .map(|plan| plan.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    let days = body.period_days.unwrap_or(30);
    if days <= 0 {
        return Err(AppError::Validation(
            "period_days must be positive".to_string(),
        ));
    }

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a plan switch: {error}"))
        })?;

    let affected =
        sqlx::query("update users set plan_id = $2 where id = $1 and deleted_at is null")
            .bind(user_id)
            .bind(plan.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the plan: {error}")))?;
    if affected.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    // The live row is upserted against the partial unique index
    // (`on subscriptions (user_id) where status in ('active','grace')`), so an account
    // that already has one keeps it and its id rather than accumulating subscriptions.
    sqlx::query(
        "insert into subscriptions
           (user_id, plan_id, status, billing_period, current_period_start, current_period_end)
         values ($1, $2, 'active', 'monthly', now(), now() + make_interval(days => $3::int))
         on conflict (user_id) where status in ('active', 'grace')
         do update set plan_id = excluded.plan_id,
                       status = 'active',
                       current_period_start = excluded.current_period_start,
                       current_period_end = excluded.current_period_end",
    )
    .bind(user_id)
    .bind(plan.as_str())
    .bind(i32::try_from(days).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("upserting the subscription: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a plan switch: {error}"))
    })?;

    let read_only =
        entitlements::recompute_read_only(&state.pool, &state.config.plans, user_id).await?;

    tracing::info!(%user_id, plan = plan.as_str(), days, "admin plan switch");

    Ok(Json(json!({
        "user_id": user_id,
        "plan": plan.as_str(),
        "read_only": {
            "workspaces": read_only.workspaces,
            "sources": read_only.sources,
            "devices": read_only.devices,
        },
    })))
}

#[derive(Debug, Deserialize)]
pub struct GrantRequest {
    pub credits: i64,
    /// Which bucket the credits land in. `monthly` by default; `purchased` is the case a
    /// manual grant exists for, because a support fix that put a purchase's credits in
    /// the expiring bucket would take them away again at the period boundary.
    #[serde(default)]
    pub bucket: Option<String>,
    /// The admin-supplied key that makes a double-click grant once (A152).
    pub idempotency_key: String,
}

/// `POST /admin/users/:id/credits`
///
/// One `credit_ledger` row, and a second call with the same `idempotency_key` writes
/// nothing: the key becomes the row's `period_key` (`admin:<key>`), which the partial
/// unique index on `(user_id, period_key)` refuses to duplicate (A152). Idempotency is
/// structural, not a check-then-write that a double-click can race.
///
/// The row is written through `credits::grant` — the one grant primitive — with the
/// requested bucket and `reason = 'admin'`, so the margin report can tell an admin's grant
/// from the recurring job's allowance and from revenue.
pub async fn grant_credits(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<GrantRequest>,
) -> AppResult<Json<Value>> {
    if body.credits <= 0 {
        return Err(AppError::Validation("credits must be positive".to_string()));
    }
    let key = body.idempotency_key.trim();
    if key.is_empty() {
        return Err(AppError::Validation(
            "idempotency_key is required".to_string(),
        ));
    }
    // A bucket the ledger cannot hold is a validation error, never a silent default:
    // granting monthly when a purchase was asked for would expire the credits at the
    // period boundary, which is the exact support bug this field exists to fix.
    let bucket =
        credits::Bucket::parse(body.bucket.as_deref().unwrap_or("monthly")).ok_or_else(|| {
            AppError::Validation(format!(
                "bucket must be one of {}",
                credits::Bucket::ALL
                    .iter()
                    .map(|bucket| bucket.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;

    let plan_id: Option<String> =
        sqlx::query_scalar("select plan_id from users where id = $1 and deleted_at is null")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the account: {error}")))?;
    let Some(plan_id) = plan_id else {
        return Err(AppError::NotFound);
    };

    let period_key = format!("admin:{key}");
    let granted = credits::grant(
        &state.pool,
        user_id,
        PlanId::parse(&plan_id),
        credits::Grant {
            bucket,
            reason: "admin",
            period_key: &period_key,
            credits: body.credits,
        },
    )
    .await?;

    tracing::info!(
        %user_id,
        credits = body.credits,
        bucket = bucket.as_str(),
        period_key,
        granted,
        "admin credit grant"
    );

    Ok(Json(json!({
        "user_id": user_id,
        "granted": granted,
        "credits": body.credits,
        "bucket": bucket.as_str(),
        "period_key": period_key,
    })))
}

#[derive(Debug, Deserialize)]
pub struct MarginQuery {
    /// RFC 3339. Defaults to the last 30 days when omitted.
    pub from: Option<String>,
    pub to: Option<String>,
}

/// `GET /admin/margin?from=&to=`
///
/// The report is **per month**, because that is the resolution A46's tripwires are read at: a
/// single blended figure for a quarter cannot say "the free tier's cost crossed a quarter of
/// revenue in June", and the month a student converts is the month the split has to be right
/// in (A303).
pub async fn margin(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(query): Query<MarginQuery>,
) -> AppResult<Json<MarginReport>> {
    let to = parse_instant(query.to.as_deref())?.unwrap_or_else(|| state.clock.now());
    let from = parse_instant(query.from.as_deref())?.unwrap_or_else(|| to - Duration::days(30));
    if from >= to {
        return Err(AppError::Validation(
            "from must be earlier than to".to_string(),
        ));
    }

    let mut report = margin_report(&state.pool, from, to, state.config.idr_per_usd).await?;

    // The bound beside the spend. `margin_report` takes only the pool and the rate (Phase 13
    // consumes that signature by name), so it holds no config and leaves the ceiling `None`
    // rather than inventing a figure — the route, the only caller that has the state, attaches
    // it through `credits::ceiling_usd`, the one accessor for `provider_cost_ceiling_usd`
    // (A148), so the report never reads the config field itself.
    for month in &mut report.months {
        for row in &mut month.per_plan {
            row.ceiling_usd = Some(credits::ceiling_usd(&state, PlanId::parse(&row.plan_id)));
        }
    }

    Ok(Json(report))
}

fn parse_instant(raw: Option<&str>) -> AppResult<Option<DateTime<Utc>>> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|instant| Some(instant.with_timezone(&Utc)))
        .map_err(|_| AppError::Validation(format!("{raw:?} is not an RFC 3339 timestamp")))
}

/// The margin report: what the free tier costs against what paying customers cost, per month.
///
/// Every number is a sum of rows or a ratio of two of them. **Nothing is invented**: a month
/// with no grants has no utilization, a month with no payments has 0 IDR of revenue (a sum of
/// no rows *is* zero) but a month with no rows at all has no per-plan entries, and the FX rate
/// the arithmetic used is printed beside it (A304, gate 7(a)).
#[derive(Debug, Serialize)]
pub struct MarginReport {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// The rupiah-per-dollar rate every `gross_margin_idr` below was computed with, printed
    /// as the input it is (A41/A304): a reader has to be able to check the arithmetic.
    pub idr_per_usd: i64,
    /// One entry per calendar month in the range, oldest first, including months with no data
    /// — a missing month would make "two months running" quietly mean "two entries running".
    pub months: Vec<MonthMargin>,
    pub tripwires: Tripwires,
}

/// One calendar month of the report.
#[derive(Debug, Serialize)]
pub struct MonthMargin {
    /// `YYYY-MM`. UTC, because the report is an admin view across accounts in every zone: a
    /// month boundary drawn in one student's zone would exclude another's rows from it.
    pub month: String,
    pub per_plan: Vec<PlanMargin>,
    pub revenue_idr: i64,
    pub gateway_fee_idr: i64,
    /// Free credits spent ÷ free credits granted, in this month (A46's "28% ≈ 17 of 60").
    /// `None` when nothing was granted, because `0/0` is not zero — it is unanswerable.
    pub free_utilization: Option<f64>,
    /// Accounts paying at the report's `to` date.
    pub paying: i64,
    /// Accounts on the free plan at the report's `to` date.
    pub free_accounts: i64,
    /// `paying ÷ free_accounts`, which is A46's "4%".
    pub conversion_of_free: Option<f64>,
    /// `paying ÷ (paying + free_accounts)`. Reported beside the other one because a report
    /// that showed one ratio while the tripwire named the other is a false alarm waiting to
    /// happen (A304).
    pub conversion_of_total: Option<f64>,
    /// When this row was computed. The counts are a snapshot at `to`, while the sums are over
    /// the month — the two are different kinds of number and the report says so.
    pub as_of: DateTime<Utc>,
}

/// One plan's month.
#[derive(Debug, Serialize)]
pub struct PlanMargin {
    pub plan_id: String,
    /// Our provider cost for this plan's spend, excluding BYOK (A156).
    pub provider_cost_usd: f64,
    /// Calls made on a student's own key: a column of its own, inside neither tier's figure.
    pub byok_cost_usd: f64,
    pub revenue_idr: i64,
    /// `revenue_idr − provider_cost_usd × idr_per_usd`. `None` when either side of the
    /// subtraction is unknown — never a figure that reads as a measured profit.
    pub gross_margin_idr: Option<i64>,
    /// The plan's monthly provider-cost ceiling, so an admin reads "60% of the bound" rather
    /// than a bare dollar figure. Read through `credits::ceiling_usd` (A148) by the route, the
    /// only caller that holds the config, which is why the query leaves it `None`.
    pub ceiling_usd: Option<f64>,
}

/// The four thresholds A46 watches, as booleans computed from the numbers above.
///
/// Each is `Option`: `None` means "the range cannot answer this", which is a different thing
/// from `false` ("it is not happening"). The distinction is the whole reason they are not
/// plain `bool` — a tripwire that reads `false` because it had no data would report safety it
/// never measured.
#[derive(Debug, Serialize)]
pub struct Tripwires {
    /// The most recent month's free-tier utilization is over 30%.
    pub free_utilization_over_30: Option<bool>,
    /// The most recent month's `conversion_of_free` is under 3% (A46's 4% has a floor).
    pub conversion_under_3: Option<bool>,
    /// The most recent month's free-tier cost, in IDR, is over a quarter of its revenue.
    pub free_spend_over_quarter_of_revenue: Option<bool>,
    /// Two consecutive months both ran at a loss. `None` when the range has fewer than two
    /// months of data, because "two months running" cannot be computed from one.
    pub plan_loss_two_months_running: Option<bool>,
}

/// One `(month, plan)` usage row.
#[derive(sqlx::FromRow)]
struct UsageRow {
    month: String,
    plan_id: String,
    provider_cost_usd: f64,
    byok_cost_usd: f64,
}

/// One `(month, plan)` payment row.
#[derive(sqlx::FromRow)]
struct RevenueRow {
    month: String,
    plan_id: String,
    revenue_idr: i64,
    gateway_fee_idr: i64,
}

/// The free tier's credits for one month.
#[derive(sqlx::FromRow)]
struct FreeCreditsRow {
    month: String,
    granted: i64,
    spent: i64,
}

/// The provider cost in `[from, to)`, split by month and plan, with BYOK apart.
///
/// **The plan of record is `coalesce(usage_events.plan_id, users.plan_id)`** (A303). The
/// roadmap's sketch joined `users.plan_id` outright, and that is wrong in exactly the month the
/// report exists to examine: the month a student converts, every earlier row that month is
/// re-attributed to the paid tier, so the free-tier cost is understated by precisely the cost
/// of converting. `users.plan_id` holds only the *current* plan, so the per-event plan has to
/// have been written when the spend happened. The `coalesce` is the documented fallback for
/// rows written before that column existed: the report is then wrong by approximation on old
/// history rather than wrong by omission.
///
/// BYOK is excluded from the plan's figure and reported separately (A156): a call on the
/// student's own key is not our cost, so counting it into a tier would overstate our spend and
/// make the margin look worse than it is.
pub async fn margin_report(
    pool: &PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    idr_per_usd: i64,
) -> AppResult<MarginReport> {
    let usage: Vec<UsageRow> = sqlx::query_as(
        "select to_char(date_trunc('month', ev.created_at), 'YYYY-MM') as month,
                coalesce(ev.plan_id, u.plan_id) as plan_id,
                coalesce(sum(ev.provider_cost_usd) filter (where ev.byok = false), 0)::float8
                    as provider_cost_usd,
                coalesce(sum(ev.provider_cost_usd) filter (where ev.byok = true), 0)::float8
                    as byok_cost_usd
           from usage_events ev
           join users u on u.id = ev.user_id
          where ev.created_at >= $1 and ev.created_at < $2
          group by 1, 2
          order by 1, 2",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the margin: {error}")))?;

    // Revenue is `payments` where `paid_at` is in the month, grouped by the plan the payment
    // bought. A credit pack has no plan, so it is grouped under its own name rather than
    // dropped: pack money is revenue, and a report that omitted it would understate income.
    let revenue: Vec<RevenueRow> = sqlx::query_as(
        "select to_char(date_trunc('month', paid_at), 'YYYY-MM') as month,
                coalesce(plan_id, 'credit_pack') as plan_id,
                coalesce(sum(amount_idr), 0)::bigint as revenue_idr,
                coalesce(sum(gateway_fee_idr), 0)::bigint as gateway_fee_idr
           from payments
          where status = 'paid' and paid_at >= $1 and paid_at < $2
          group by 1, 2
          order by 1, 2",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the revenue: {error}")))?;

    // The free tier's own allowance and consumption. `monthly_grant` is the grant reason and
    // the monthly bucket is the one that expires, so the two sides of the ratio are the same
    // thing measured twice — which is what makes "17 of 60 credits" a utilization rather than
    // a comparison of two unrelated sums.
    let free_credits: Vec<FreeCreditsRow> = sqlx::query_as(
        "select to_char(date_trunc('month', cl.created_at), 'YYYY-MM') as month,
                coalesce(sum(cl.delta) filter (where cl.reason = 'monthly_grant'), 0)::bigint
                    as granted,
                coalesce(-sum(cl.delta) filter (where cl.delta < 0 and cl.bucket = 'monthly'),
                         0)::bigint as spent
           from credit_ledger cl
           join users u on u.id = cl.user_id
          where cl.created_at >= $1 and cl.created_at < $2
            and coalesce(cl.plan_id, u.plan_id) = 'free'
          group by 1
          order by 1",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading free usage: {error}")))?;

    // The conversion snapshot at `to`: how many accounts pay, and how many are on free.
    //
    // An **admin's own account is not a customer**, so it is excluded from both sides: the
    // operator's account is on free by construction, and counting it would understate
    // conversion by a factor that grows with the team — a report that got worse as the company
    // hired is not a measurement of the business.
    let (paying, free_accounts): (i64, i64) = sqlx::query_as(
        "select count(*) filter (
                    where u.plan_id <> 'free'
                      and exists (select 1 from subscriptions s
                                   where s.user_id = u.id
                                     and s.status in ('active', 'grace')))::bigint,
                count(*) filter (
                    where not exists (select 1 from subscriptions s
                                       where s.user_id = u.id
                                         and s.status in ('active', 'grace')
                                         and s.plan_id <> 'free'))::bigint
           from users u
          where u.deleted_at is null and not u.is_admin",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("counting the accounts: {error}")))?;

    let months = assemble_months(from, to, idr_per_usd, usage, revenue, free_credits);
    let tripwires = tripwires(&months, idr_per_usd);
    let as_of = to;

    let mut months = months;
    for month in &mut months {
        month.as_of = as_of;
        month.paying = paying;
        month.free_accounts = free_accounts;
        month.conversion_of_free = ratio(paying, free_accounts);
        month.conversion_of_total = ratio(paying, paying + free_accounts);
    }

    Ok(MarginReport {
        from,
        to,
        idr_per_usd,
        months,
        tripwires,
    })
}

/// Every calendar month in `[from, to)`, with the rows that belong to it.
///
/// A month with no rows is present with empty/zero sums rather than absent: "two months
/// running" reads the *sequence*, and a gap would silently pair June with August.
fn assemble_months(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    idr_per_usd: i64,
    usage: Vec<UsageRow>,
    revenue: Vec<RevenueRow>,
    free_credits: Vec<FreeCreditsRow>,
) -> Vec<MonthMargin> {
    let mut months: Vec<MonthMargin> = Vec::new();
    let mut cursor = month_key(from);
    let last = month_key(to - Duration::seconds(1));
    loop {
        months.push(MonthMargin {
            month: cursor.clone(),
            per_plan: Vec::new(),
            revenue_idr: 0,
            gateway_fee_idr: 0,
            free_utilization: None,
            paying: 0,
            free_accounts: 0,
            conversion_of_free: None,
            conversion_of_total: None,
            as_of: to,
        });
        if cursor == last || months.len() > 600 {
            // 600 months is fifty years: the guard is here so a range built from a nonsense
            // `from` cannot loop forever, not because a real report ever reaches it.
            break;
        }
        cursor = next_month(&cursor);
    }

    let index =
        |months: &[MonthMargin], month: &str| months.iter().position(|entry| entry.month == month);

    for row in usage {
        if let Some(position) = index(&months, &row.month) {
            months[position].per_plan.push(PlanMargin {
                plan_id: row.plan_id,
                provider_cost_usd: row.provider_cost_usd,
                byok_cost_usd: row.byok_cost_usd,
                revenue_idr: 0,
                gross_margin_idr: None,
                ceiling_usd: None,
            });
        }
    }

    for row in revenue {
        if let Some(position) = index(&months, &row.month) {
            months[position].revenue_idr += row.revenue_idr;
            months[position].gateway_fee_idr += row.gateway_fee_idr;
            match months[position]
                .per_plan
                .iter_mut()
                .find(|plan| plan.plan_id == row.plan_id)
            {
                Some(plan) => plan.revenue_idr += row.revenue_idr,
                // A plan with revenue but no usage: a paying account that did not call a model.
                // Its row is real and belongs in the report — a plan that costs nothing and
                // earns something is the best row in it.
                None => months[position].per_plan.push(PlanMargin {
                    plan_id: row.plan_id,
                    provider_cost_usd: 0.0,
                    byok_cost_usd: 0.0,
                    revenue_idr: row.revenue_idr,
                    gross_margin_idr: None,
                    ceiling_usd: None,
                }),
            }
        }
    }

    for row in free_credits {
        if let Some(position) = index(&months, &row.month)
            && row.granted > 0
        {
            months[position].free_utilization = Some(row.spent as f64 / row.granted as f64);
        }
    }

    // Gross margin last, because it needs both sides: our cost converted at the printed rate,
    // subtracted from the revenue the same month earned.
    for month in &mut months {
        for plan in &mut month.per_plan {
            let cost_idr = (plan.provider_cost_usd * idr_per_usd as f64).round() as i64;
            plan.gross_margin_idr = Some(plan.revenue_idr - cost_idr);
        }
        month
            .per_plan
            .sort_by(|left, right| left.plan_id.cmp(&right.plan_id));
    }

    months
}

/// The four thresholds, computed from the assembled months.
///
/// The three single-month tripwires read the **most recent month that has the figure they
/// need**, not the last month in the range: a range ending on a quiet month would otherwise
/// report "not tripped" about a month that had no data, which is the one answer a tripwire
/// must never give.
fn tripwires(months: &[MonthMargin], idr_per_usd: i64) -> Tripwires {
    let latest_utilization = months.iter().rev().find_map(|month| month.free_utilization);
    let latest_conversion = months
        .iter()
        .rev()
        .find_map(|month| month.conversion_of_free);

    // Free spend over a quarter of revenue, and the two-month loss run, both need a month that
    // actually has spend or revenue: a month with neither has nothing to compare.
    let latest_with_data = months
        .iter()
        .rev()
        .find(|month| !month.per_plan.is_empty() || month.revenue_idr > 0);

    let free_spend_over_quarter = latest_with_data.map(|month| {
        let free_cost: f64 = month
            .per_plan
            .iter()
            .filter(|plan| plan.plan_id == PlanId::Free.as_str())
            .map(|plan| plan.provider_cost_usd)
            .sum();
        // Compared in USD against revenue in IDR, converted: comparing two currencies without
        // the rate would make the tripwire depend on the exchange rate it did not apply.
        let free_cost_idr = (free_cost * idr_per_usd as f64).round() as i64;
        free_cost_idr * 4 > month.revenue_idr
    });

    let with_data: Vec<&MonthMargin> = months
        .iter()
        .filter(|month| !month.per_plan.is_empty() || month.revenue_idr > 0)
        .collect();
    let two_month_loss = match with_data.as_slice() {
        [.., previous, latest] => {
            let loss = |month: &MonthMargin| {
                let margin: i64 = month
                    .per_plan
                    .iter()
                    .filter_map(|plan| plan.gross_margin_idr)
                    .sum();
                margin < 0
            };
            Some(loss(previous) && loss(latest))
        }
        // Fewer than two months of data: "two months running" cannot be computed from one, and
        // `false` would claim it was measured.
        _ => None,
    };

    Tripwires {
        free_utilization_over_30: latest_utilization.map(|utilization| utilization > 0.30),
        conversion_under_3: latest_conversion.map(|conversion| conversion < 0.03),
        free_spend_over_quarter_of_revenue: free_spend_over_quarter,
        plan_loss_two_months_running: two_month_loss,
    }
}

/// `YYYY-MM` for an instant, in UTC.
fn month_key(at: DateTime<Utc>) -> String {
    at.format("%Y-%m").to_string()
}

/// The month after `key`.
///
/// Arithmetic on the string rather than on a date, because the key is all this needs and a
/// parse-and-reformat round trip is a second place for the format to be wrong.
fn next_month(key: &str) -> String {
    let (year, month) = key.split_once('-').unwrap_or((key, "01"));
    let year: i32 = year.parse().unwrap_or(1970);
    let month: u32 = month.parse().unwrap_or(1);
    if month >= 12 {
        format!("{}-01", year + 1)
    } else {
        format!("{year}-{:02}", month + 1)
    }
}

/// A ratio, or `None` when the denominator is zero.
///
/// `0/0` is not zero: it is "no accounts to convert", and reporting it as 0% would trip the
/// conversion tripwire on a range where nothing could have converted.
fn ratio(numerator: i64, denominator: i64) -> Option<f64> {
    if denominator == 0 {
        None
    } else {
        Some(numerator as f64 / denominator as f64)
    }
}

#[derive(Debug, Deserialize)]
pub struct PaymentsQuery {
    /// `pending`, `paid`, `review`, `refunded`, `expired`, `failed`. Omitted lists every row,
    /// which is what the admin screen's first load wants.
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /admin/payments?status=review`
///
/// The queue A294 and A301 feed: a mismatched amount and a gateway transaction we cannot
/// explain both land here as `review` rows with a reason, and this is where a human reads them.
/// The reason travels with the row, because it *is* what the human reads — a queue of ids would
/// be a queue nobody works.
pub async fn list_payments(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(query): Query<PaymentsQuery>,
) -> AppResult<Json<Value>> {
    let status = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let limit = query.limit.unwrap_or(100).clamp(1, 500);

    let rows: Vec<PaymentRow> = sqlx::query_as(
        // `amount_idr` and `gateway_fee_idr` are `int` (migration 0014) while the DTO reads
        // them as `i64`, so they are cast here: sqlx decodes by SQL type, and an `INT4`
        // column read into an `i64` field is a decode error, not a widening.
        "select p.id, p.user_id, p.kind, p.plan_id, p.billing_period, p.credits, p.provider,
                p.order_id, p.provider_payment_id, p.amount_idr::bigint as amount_idr,
                p.discount_code, p.gateway_fee_idr::bigint as gateway_fee_idr, p.status,
                p.review_reason, p.expires_at, p.paid_at, p.reconciled_at, p.created_at
           from payments p
          where $1::text is null or p.status = $1
          order by p.created_at desc
          limit $2",
    )
    .bind(status)
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing payments: {error}")))?;

    Ok(Json(json!({
        "payments": rows,
        "count": rows.len(),
    })))
}

/// One `payments` row as the admin surface reads it.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PaymentRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub plan_id: Option<String>,
    pub billing_period: Option<String>,
    pub credits: Option<i32>,
    pub provider: String,
    pub order_id: String,
    pub provider_payment_id: Option<String>,
    pub amount_idr: i64,
    pub discount_code: Option<String>,
    pub gateway_fee_idr: Option<i64>,
    pub status: String,
    pub review_reason: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub reconciled_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

/// `POST /admin/payments/:id/mark-paid`
///
/// Runs **the same `apply_paid` the webhook runs** (A308/A301), so a manually confirmed payment
/// and a gateway-confirmed one cannot activate differently — a second implementation here would
/// be a second answer to "what does a payment do". The row records `provider = 'manual'` so the
/// margin report can tell a comp from money that arrived.
///
/// Idempotent by the guarded state transition `apply_paid` performs: a second call finds the
/// payment already `paid` and changes nothing.
pub async fn mark_paid(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(payment_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row: Option<(String, i64, String)> =
        sqlx::query_as("select status, amount_idr, order_id from payments where id = $1")
            .bind(payment_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the payment: {error}")))?;

    let Some((_, amount_idr, order_id)) = row else {
        return Err(AppError::NotFound);
    };

    // A synthetic, verified event: the admin *is* the verification here, which is what the
    // audit row and the `provider = 'manual'` mark record. The amount is the row's own, so the
    // amount check inside `apply_paid` cannot fail for a reason the admin did not choose.
    let event = WebhookEvent {
        event_id: format!("manual:{payment_id}"),
        kind: WebhookKind::Paid,
        order_id: order_id.clone(),
        amount_idr,
        currency: "IDR".to_string(),
        paid_at: Some(state.clock.now()),
        provider_payment_id: None,
        fee_idr: None,
        raw: json!({ "manual": true }),
    };

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a mark-paid: {error}"))
        })?;

    let applied = crate::billing::apply::apply_paid(&mut tx, &state, payment_id, &event).await?;

    sqlx::query("update payments set provider = 'manual', reconciled_at = now() where id = $1")
        .bind(payment_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("marking the payment manual: {error}"))
        })?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a mark-paid: {error}")))?;

    tracing::info!(%payment_id, ?applied, "admin marked a payment paid");

    Ok(Json(json!({
        "payment_id": payment_id,
        "applied": format!("{applied:?}"),
    })))
}

/// `POST /admin/payments/:id/refund`
///
/// Records the refund and **leaves the subscription alone** (A308). A refund is a money fact;
/// revoking a plan the student may already be using is a decision a human makes deliberately,
/// and doing it as a side effect of a row update would be the app taking away something a
/// student paid for. The gateway fee is zeroed rather than deleted: the row stays, and a fee we
/// are no longer charged must not keep overstating our cost.
pub async fn refund_payment(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(payment_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let affected = sqlx::query(
        "update payments
            set status = 'refunded', gateway_fee_idr = 0, reconciled_at = now()
          where id = $1 and status = 'paid'",
    )
    .bind(payment_id)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("refunding the payment: {error}")))?;

    if affected.rows_affected() == 0 {
        // Either the payment does not exist or it is not `paid`. A second refund changes
        // nothing, which is the idempotence A308 asks for; a refund of a pending payment is a
        // conflict rather than a silent no-op, because an admin who meant to refund money
        // needs to know none was taken.
        let exists: Option<String> =
            sqlx::query_scalar("select status from payments where id = $1")
                .bind(payment_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("reading the payment: {error}"))
                })?;
        return match exists.as_deref() {
            None => Err(AppError::NotFound),
            Some("refunded") => Ok(Json(json!({
                "payment_id": payment_id,
                "status": "refunded",
                "changed": false,
            }))),
            Some(status) => Err(AppError::Conflict(format!(
                "only a paid payment can be refunded; this one is {status}"
            ))),
        };
    }

    tracing::info!(%payment_id, "admin refunded a payment");
    Ok(Json(json!({
        "payment_id": payment_id,
        "status": "refunded",
        "changed": true,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionRequest {
    /// How many days to add to `current_period_end`. Positive only: a manual switch that
    /// *shortened* paid time would be the app taking away time a student paid for.
    pub extend_days: i64,
}

/// `PATCH /admin/subscriptions/:user_id`
///
/// Extends the live subscription and recomputes the read-only flags — the same
/// `recompute_read_only` the rollover and the plan switch call, because a manual extension can
/// lift an account back over a limit it was flagged for, and an upgrade must clear the flag
/// rather than leave it behind (A149).
pub async fn extend_subscription(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<SubscriptionRequest>,
) -> AppResult<Json<Value>> {
    if body.extend_days <= 0 {
        return Err(AppError::Validation(
            "extend_days must be positive".to_string(),
        ));
    }
    if body.extend_days > 3650 {
        return Err(AppError::Validation(
            "extend_days must be at most 3650".to_string(),
        ));
    }

    let affected = sqlx::query(
        "update subscriptions
            set current_period_end = current_period_end + make_interval(days => $2::int),
                status = 'active'
          where user_id = $1 and status in ('active', 'grace')",
    )
    .bind(user_id)
    .bind(i32::try_from(body.extend_days).unwrap_or(i32::MAX))
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("extending the subscription: {error}")))?;

    if affected.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    let read_only =
        entitlements::recompute_read_only(&state.pool, &state.config.plans, user_id).await?;
    let end: DateTime<Utc> =
        sqlx::query_scalar("select current_period_end from subscriptions where user_id = $1")
            .bind(user_id)
            .fetch_one(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the window: {error}")))?;

    tracing::info!(%user_id, days = body.extend_days, %end, "admin extended a subscription");

    Ok(Json(json!({
        "user_id": user_id,
        "current_period_end": end,
        "read_only": {
            "workspaces": read_only.workspaces,
            "sources": read_only.sources,
            "devices": read_only.devices,
        },
    })))
}
