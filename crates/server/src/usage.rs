//! `GET /usage` — the one source of the numbers the plan and usage screen shows.
//!
//! A159: the screen needs the plan, the limits, the credit buckets and the cost table
//! (roadmap gates 7(b) and 7(d)); the user needs to know *that* the provider-cost ceiling
//! is reached and when it resets, not our unit economics. So the payload carries
//! `provider_cost: { paused, resets_at }` and never the dollars — those appear only in
//! `GET /admin/margin`.
//!
//! **Every number here is computed.** A limit comes from the loaded matrix, a used count
//! comes from the same snapshot the decision uses, and the cost table comes from
//! `credits::costs` — the one function that builds it, so the number this view reports and
//! the number a route charges cannot drift (gate 7(d)). There is no placeholder and no
//! `0` standing in for "unknown".

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use stvdy_core::config::Limit;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{credits, entitlements, util};

pub fn routes() -> Router<AppState> {
    Router::new().route("/usage", get(usage))
}

/// `GET /usage`
pub async fn usage(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    let snapshot = entitlements::snapshot(&state, &user).await?;
    let plan = snapshot.plan;
    let cfg = state.config.plans.get(plan);

    let period_end: Option<DateTime<Utc>> = sqlx::query_scalar(
        "select current_period_end from subscriptions
          where user_id = $1 and status in ('active', 'grace')
          order by current_period_end desc
          limit 1",
    )
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the period end: {error}")))?;

    // The pause is computed by the same comparison the spend path makes (A159); the
    // dollars themselves stay in `GET /admin/margin`.
    let paused = credits::ceiling_paused(&state, &user, &snapshot).await;
    let resets_at = util::next_local_month_start(&user.timezone, state.clock.as_ref());

    Ok(Json(json!({
        "plan": plan.as_str(),
        "name": cfg.name,
        "subscription": {
            "status": status_str(snapshot.subscription_status),
            "period_end": period_end,
        },
        "credits": {
            "monthly": snapshot.credits.monthly,
            "purchased": snapshot.credits.purchased,
            "total": snapshot.credits.total(),
        },
        "storage_bytes": snapshot.storage_bytes,
        "limits": {
            "workspaces": limit_json(cfg.workspaces, Some(snapshot.workspaces)),
            "sources_total": limit_json(cfg.sources_total, Some(snapshot.sources)),
            "max_file_mb": limit_json(Limit::Max(cfg.max_file_mb), None),
            "storage_mb": limit_json(Limit::Max(cfg.storage_mb), None),
            "devices_max": limit_json(Limit::Max(cfg.devices_max), Some(snapshot.devices)),
            "credits_per_month": json!({ "limit": cfg.credits_per_month, "used": Value::Null }),
            "chat_msgs_per_day": limit_json(cfg.chat_msgs_per_day, Some(snapshot.chat_msgs_today)),
            "web_searches_per_day": limit_json(
                Limit::Max(cfg.web_searches_per_day),
                Some(snapshot.web_searches_today),
            ),
            "flashcards_per_month": limit_json(
                cfg.flashcards_per_month,
                Some(snapshot.flashcards_this_month),
            ),
            "quizzes_per_month": limit_json(cfg.quizzes_per_month, Some(snapshot.quizzes_this_month)),
            "reports_per_month": limit_json(cfg.reports_per_month, Some(snapshot.reports_this_month)),
            "syllabus_imports_total": limit_json(
                cfg.syllabus_imports_total,
                Some(snapshot.syllabus_imports),
            ),
            "google_accounts_max": limit_json(cfg.google_accounts_max, None),
        },
        "features": {
            "export_formats": cfg.export_formats,
            "task_ai_breakdown": cfg.task_ai_breakdown,
            "quick_capture": cfg.quick_capture,
            "google_calendar": cfg.google_calendar,
            "planner": cfg.planner,
            "agent_actions": cfg.agent_actions,
            "reminder_channels": cfg.reminder_channels,
            "model_tier": cfg.model_tier,
            "byok": cfg.byok,
            "advanced_analytics": cfg.advanced_analytics,
        },
        "costs": credits::costs(&state.config.plans),
        "provider_cost": {
            "paused": paused,
            "resets_at": resets_at,
        },
    })))
}

/// A limit with the count against it, or `null` for an unlimited plan's limit.
///
/// `used` is `null` for an axis that has no running counter (a file's size ceiling, the
/// monthly allowance, the Google-account ceiling): the storage axis reports its bytes
/// separately as `storage_bytes`, in bytes, rather than as a rounded megabyte count that
/// would be an invented number.
fn limit_json(limit: Limit, used: Option<i64>) -> Value {
    json!({
        "limit": limit.max().map_or(Value::Null, |max| json!(max)),
        "used": used.map_or(Value::Null, |used| json!(used)),
    })
}

/// The subscription status as the wire spells it, so the client renders the same word the
/// row holds and `SubStatus`'s own `Debug` is not a payload format.
fn status_str(status: stvdy_core::entitlements::SubStatus) -> &'static str {
    use stvdy_core::entitlements::SubStatus;
    match status {
        SubStatus::None => "none",
        SubStatus::Active => "active",
        SubStatus::Grace => "grace",
        SubStatus::Expired => "expired",
        SubStatus::Canceled => "canceled",
    }
}
