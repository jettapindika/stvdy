//! The billing route table.
//!
//! Two groups, because they carry different guarantees:
//!
//!  * [`routes`] is the authenticated group: `/billing/checkout`, the payment poll, the
//!    student verification pair and `GET /plans`. Each takes `AuthUser` or `VerifiedUser`,
//!    so "is this scoped to a real account" is decided by the extractor rather than by a
//!    line in a handler.
//!  * [`public_routes`] is the gateway's callback. It carries **no bearer token** — the
//!    gateway has no session — and the signature is its authorisation. It is deliberately
//!    not merged into the authenticated group: a route that authenticates by HMAC and one
//!    that authenticates by session are different things, and putting them in one
//!    `.merge()` is how the wrong one gets a layer it must not have.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use stvdy_core::config::PlanId;
use uuid::Uuid;

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::checkout::{self, CheckoutBody, PaymentView};
use super::{student, webhook};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/plans", get(plans))
        .route("/billing/checkout", post(checkout_route))
        .route("/billing/payments/{id}", get(payment))
        // An upgrade and a downgrade both move money or move a period end, so both take
        // `VerifiedUser` (A43). `periods::upgrade` calls `checkout_allowed` itself as its
        // first statement, so the platform rule is not repeated here.
        .route("/billing/upgrade", post(upgrade_route))
        .route("/billing/downgrade", post(downgrade_route))
}

/// Routes that send mail to a caller-supplied address, mounted in the rate-limited public
/// group (A299).
///
/// They still require a session — the limiter is a volume bound, not the authorisation —
/// but they share the credential bucket rather than the unlimited group, because the
/// suffix list is a content rule and this is the volume rule. One without the other is a
/// mail amplifier pointed at a campus domain.
pub fn limited_routes() -> Router<AppState> {
    Router::new()
        .route("/billing/student/verify", post(student_verify))
        .route("/billing/student/confirm", post(student_confirm))
}

/// The callback. Mounted in `routes::router`'s public group.
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/webhooks/qris", post(webhook::handle))
}

/// `GET /plans` — the matrix, platform-aware (A305).
///
/// **Prices are omitted where `show_prices` is false.** The platform is the device row
/// (A144), so a phone gets the plan names, the limits and the features and no prices: the
/// SPA's `policy.ts` is a second reading of the same config file rather than a second
/// decision, and a client that ignored its own policy would still be told nothing it must
/// not show.
///
/// Every number comes from the loaded matrix — the same `PlansFile` the entitlement engine
/// decides from — so a limit shown here and a limit enforced there cannot drift.
pub async fn plans(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<serde_json::Value>> {
    let policy = stvdy_core::config::Platform::ALL
        .into_iter()
        .find(|platform| platform.as_str() == user.platform)
        .map(|platform| state.config.billing_policy.for_platform(platform));

    let show_prices = policy.is_some_and(|policy| policy.show_prices);
    let purchase_allowed = user.upgrade_offerable(&state.config.billing_policy);

    let plans: Vec<serde_json::Value> = PlanId::ALL
        .into_iter()
        .map(|plan| {
            let config = state.plans().get(plan);
            let mut value = serde_json::json!({
                "id": plan.as_str(),
                "name": config.name,
                "limits": crate::entitlements::limits_view(state.plans(), plan),
                "student_percent_off": config.student_percent_off,
                "byok": config.byok,
                "credits_per_month": config.credits_per_month,
            });
            if show_prices {
                value["price_idr_monthly"] = serde_json::json!(config.price_idr_monthly);
                value["price_idr_yearly"] = serde_json::json!(config.price_idr_yearly);
                // The student price is *computed* by the same function the checkout
                // charges from, so a screen that shows it and a checkout that charges it
                // cannot disagree.
                value["student_price_idr_monthly"] = serde_json::json!(
                    stvdy_core::billing::price(
                        config,
                        stvdy_core::billing::BillingPeriod::Monthly,
                        true,
                        None,
                    )
                    .due_idr
                );
                value["student_price_idr_yearly"] = serde_json::json!(
                    stvdy_core::billing::price(
                        config,
                        stvdy_core::billing::BillingPeriod::Yearly,
                        true,
                        None,
                    )
                    .due_idr
                );
            }
            value
        })
        .collect();

    let credit_packs: Vec<serde_json::Value> = state
        .plans()
        .credit_packs
        .packs
        .iter()
        .map(|pack| {
            let mut value = serde_json::json!({ "credits": pack.credits });
            if show_prices {
                value["price_idr"] = serde_json::json!(pack.price_idr);
            }
            value
        })
        .collect();

    Ok(Json(serde_json::json!({
        "plans": plans,
        "credit_packs": credit_packs,
        "show_prices": show_prices,
        "purchase_allowed": purchase_allowed,
        "currency": state.plans().meta.currency,
        "current_plan": user.effective_plan().as_str(),
        "student_verified": user.student_verified,
    })))
}

/// `POST /billing/checkout`
async fn checkout_route(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<CheckoutBody>,
) -> AppResult<Json<PaymentView>> {
    Ok(Json(checkout::create(&state, &user, body).await?))
}

/// `GET /billing/payments/{id}` — the poll the checkout screen runs (A306).
async fn payment(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<PaymentView>> {
    Ok(Json(checkout::show(&state, &user, id).await?))
}

/// `POST /billing/student/verify`
async fn student_verify(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<student::VerifyBody>,
) -> AppResult<Json<student::VerifyOutcome>> {
    student::request(&state, &user, body).await
}

/// The body an upgrade or a downgrade carries.
#[derive(Debug, serde::Deserialize)]
struct PlanChangeBody {
    plan_id: String,
    billing_period: String,
}

impl PlanChangeBody {
    /// The target plan and period, or the validation error.
    ///
    /// Both are parsed here rather than defaulted: a body that omits the period would
    /// otherwise sell whichever period the code happens to prefer, and an unknown plan
    /// string must not become `Free` by `PlanId::parse`'s safe-direction rule — for a
    /// *request*, "unknown" is a client bug and must be a 400, not a free downgrade.
    fn parse(&self) -> AppResult<(PlanId, stvdy_core::billing::BillingPeriod)> {
        let plan = PlanId::ALL
            .into_iter()
            .find(|plan| plan.as_str() == self.plan_id)
            .ok_or_else(|| AppError::Validation(format!("unknown plan {:?}", self.plan_id)))?;
        let period =
            stvdy_core::billing::BillingPeriod::parse(&self.billing_period).ok_or_else(|| {
                AppError::Validation("billing_period must be monthly or yearly".to_string())
            })?;
        Ok((plan, period))
    }
}

/// `POST /billing/upgrade` — switch now, charge the prorated difference (A296).
async fn upgrade_route(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<PlanChangeBody>,
) -> AppResult<Json<PaymentView>> {
    let (plan, period) = body.parse()?;
    Ok(Json(
        super::periods::upgrade(&state, &user, plan, period).await?,
    ))
}

/// `POST /billing/downgrade` — record it; the plan changes at the period end (A296).
async fn downgrade_route(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(body): Json<PlanChangeBody>,
) -> AppResult<Json<super::periods::PlanChangeView>> {
    let (plan, period) = body.parse()?;
    Ok(Json(
        super::periods::downgrade(&state, &user, plan, period).await?,
    ))
}

/// `POST /billing/student/confirm`
async fn student_confirm(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<student::ConfirmBody>,
) -> AppResult<Json<serde_json::Value>> {
    student::confirm(&state, &user, body).await
}
