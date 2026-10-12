//! Billing: the checkout policy Phase 7 landed, and Phase 13's money path.
//!
//! Two layers, and the split is the point. `crates/core/src/billing/` is **pure** —
//! `price()`, `apply_payment()`, `upgrade_delta()` — and knows nothing about a database.
//! This module owns the `payments` row, the checkout route, the webhook and the period
//! rules; `crates/server/src/integrations/payment/` owns the wire. The amount a student
//! is quoted and the amount the webhook checks come out of the same pure function, so
//! they cannot disagree.
//!
//! [`checkout_allowed`] is Phase 7's platform refusal (A151). It lives here rather than
//! in `checkout` because the *control* is testable without a route, and because a second
//! copy of the rule in the handler would be a second place to forget it.

pub mod apply;
pub mod byok;
pub mod checkout;
pub mod periods;
pub mod poll;
pub mod routes;
pub mod student;
pub mod webhook;

use stvdy_core::config::{BillingPolicy, Platform};

use crate::error::{AppError, AppResult};

/// Refuses a purchase call to action on a platform the billing policy forbids.
///
/// `ios` and `android` are refused with 403 `feature_locked`, not `plan_required`: on
/// mobile there is no upgrade to offer (A143), because the App Store and Play rules keep
/// a purchase call to action out of the build. The plan then syncs to the account on
/// every device, so the remedy is "buy it on the web", which is not an in-app CTA.
///
/// **The `platform` argument is the calling device's row, never a request body field**
/// (A144). A body that claims `platform: "web"` while the device row says `ios` must
/// still be refused, and that is the caller's obligation: it reads the platform from
/// `AuthUser.platform`, which the extractor fills from `devices.platform`.
///
/// An unrecognised platform is refused, matching `AuthUser::upgrade_offerable`'s
/// deny-by-default rule: a value the policy cannot classify must fail closed rather than
/// sell.
pub fn checkout_allowed(policy: &BillingPolicy, platform: &str) -> AppResult<()> {
    let known = Platform::ALL
        .into_iter()
        .find(|candidate| candidate.as_str() == platform);

    if known.is_some_and(|platform| policy.purchase_allowed(platform)) {
        return Ok(());
    }

    Err(AppError::Entitlement {
        code: "feature_locked",
        message: "checkout is not available on this device; a plan bought on the web \
                  applies to every device"
            .to_string(),
        min_plan: None,
        limit: None,
        used: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use stvdy_core::config::load_billing_policy;

    fn policy() -> BillingPolicy {
        load_billing_policy(&crate::testing::root().join("config/billing_policy.toml"))
            .expect("billing_policy.toml must load")
    }

    #[test]
    fn a_mobile_platform_is_refused_and_a_desktop_one_is_not() {
        let policy = policy();

        for mobile in ["ios", "android"] {
            let error = checkout_allowed(&policy, mobile)
                .expect_err("a mobile platform must not be sold to");
            assert_eq!(error.status(), axum::http::StatusCode::FORBIDDEN);
            assert_eq!(error.code(), "feature_locked");
        }

        for desktop in ["web", "macos", "windows"] {
            assert!(checkout_allowed(&policy, desktop).is_ok(), "{desktop}");
        }
    }

    #[test]
    fn an_unrecognised_platform_fails_closed() {
        // A platform the policy cannot classify must not sell: deny-by-default is the
        // same rule `AuthUser::upgrade_offerable` applies.
        let error = checkout_allowed(&policy(), "blackberry").expect_err("unknown must refuse");
        assert_eq!(error.code(), "feature_locked");
    }
}
