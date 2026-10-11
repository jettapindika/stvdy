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
    /// Which bucket to grant into. Only `monthly` exists as a grant path in this phase:
    /// a purchased pack is a payment (Phase 13), not a manual grant.
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
    if let Some(bucket) = body.bucket.as_deref()
        && bucket != "monthly"
    {
        return Err(AppError::Validation(
            "only the monthly bucket can be granted in this phase".to_string(),
        ));
    }

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
        &period_key,
        body.credits,
    )
    .await?;

    tracing::info!(%user_id, credits = body.credits, period_key, granted, "admin credit grant");

    Ok(Json(json!({
        "user_id": user_id,
        "granted": granted,
        "credits": body.credits,
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

    let report = margin_report(&state.pool, from, to).await?;
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

/// The margin report, as far as this phase can compute it.
#[derive(Debug, Serialize)]
pub struct MarginReport {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// Provider cost per plan, BYOK in its own column.
    pub per_plan: Vec<PlanMargin>,
    /// The same rows, split free from paid.
    pub provider_cost: CostSplit,
    /// `None` until Phase 13 writes `payments` rows. A `0` would be an invented number,
    /// and A46's tripwires would read it as a measurement (gate 7(a)).
    pub revenue: Option<Revenue>,
    /// `None` for the same reason: conversion is `paying / free`, and there is no
    /// `payments` row to count a paying account from yet.
    pub conversion: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PlanMargin {
    pub plan_id: String,
    pub provider_cost_usd: f64,
    pub byok_cost_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct CostSplit {
    pub free_usd: f64,
    pub paid_usd: f64,
    pub byok_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct Revenue {
    pub paid_idr: i64,
    pub gateway_fee_idr: i64,
}

/// The provider cost in `[from, to)`, split free from paid and with BYOK apart.
///
/// `join usage_events → users.plan_id` is the Phase 7 attribution: Phase 13 adds
/// `usage_events.plan_id` so a user who converts in July keeps June's cost in the free
/// column, but there is no per-event plan column to read yet, so the account's current
/// plan is what the report has. BYOK is excluded from both tiers (A156): a call on the
/// user's own key is not our cost, so counting it into a blended figure would overstate
/// the margin and counting it against a tier would pause an account for spending someone
/// else's money.
pub async fn margin_report(
    pool: &PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<MarginReport> {
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "select u.plan_id,
                coalesce(sum(ev.provider_cost_usd) filter (where ev.byok = false), 0)::float8,
                coalesce(sum(ev.provider_cost_usd) filter (where ev.byok = true), 0)::float8
           from usage_events ev
           join users u on u.id = ev.user_id
          where ev.created_at >= $1 and ev.created_at < $2
          group by u.plan_id
          order by u.plan_id",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the margin: {error}")))?;

    let per_plan: Vec<PlanMargin> = rows
        .into_iter()
        .map(|(plan_id, provider_cost_usd, byok_cost_usd)| PlanMargin {
            plan_id,
            provider_cost_usd,
            byok_cost_usd,
        })
        .collect();

    let mut provider_cost = CostSplit {
        free_usd: 0.0,
        paid_usd: 0.0,
        byok_usd: 0.0,
    };
    for row in &per_plan {
        if row.plan_id == PlanId::Free.as_str() {
            provider_cost.free_usd += row.provider_cost_usd;
        } else {
            provider_cost.paid_usd += row.provider_cost_usd;
        }
        provider_cost.byok_usd += row.byok_cost_usd;
    }

    Ok(MarginReport {
        from,
        to,
        per_plan,
        provider_cost,
        revenue: None,
        conversion: None,
    })
}
