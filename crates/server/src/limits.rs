//! The plan matrix's countable-limit check, in the shape the credit spend needs.
//!
//! This module used to be the Phase 1 shim: five limits, read straight from
//! `config/plans.toml`, before `crates/core/src/entitlements/` existed. Phase 7 moved
//! every handler onto the engine — `crate::entitlements::require` builds one
//! `UsageSnapshot`, hands it to `check_with`, and maps the verdict into the same
//! `AppError::Entitlement` envelope this file has always produced, so no response body
//! changed shape.
//!
//! What is left here is the one caller the engine cannot serve: `credits::spend_in`
//! enforces a daily cap **inside the spend's own transaction**, where the count and the
//! charge have to be one atomic decision. It needs a check, not a snapshot.
//!
//! Nothing here hardcodes a limit: the matrix lives in `plans.toml`.

use stvdy_core::config::{Limit, PlanId, PlansFile};

use crate::error::AppError;

/// The plan a user would have to buy to get past this limit: the next one up the ladder,
/// derived from `PlanId::ALL` rather than written down, so a plan added later joins the
/// ladder without a second place to edit.
///
/// `None` on the top plan means "not for sale", which is the truth: a limit on the top
/// plan is a genuine maximum. The same rule and the same derivation live in
/// `crates/core/src/entitlements/check.rs`; this copy exists only because the daily cap
/// is checked on the spend's connection rather than through the snapshot.
fn upgrade_from(plan: PlanId) -> Option<PlanId> {
    PlanId::ALL
        .iter()
        .skip_while(|candidate| **candidate != plan)
        .nth(1)
        .copied()
}

fn limit_reached(plan: PlanId, limit: Limit, used: i64, feature: &'static str) -> AppError {
    let allowed = match limit.max() {
        Some(max) => max.to_string(),
        None => "unlimited".to_string(),
    };
    AppError::Entitlement {
        code: "limit_reached",
        message: format!("{feature}: the {} plan allows {allowed}", plan.as_str()),
        min_plan: upgrade_from(plan).map(|p| p.as_str()),
        limit: limit.max().map(i64::from),
        used: Some(used),
    }
}

/// A countable limit: `used` items already exist and one more is requested.
///
/// Used by `credits::spend_in` for the daily chat and web-search caps, under the spend's
/// advisory lock: a check and a spend in separate transactions can interleave, and two
/// concurrent requests both see the last slot free.
pub fn enforce_count(
    _plans: &PlansFile,
    plan: PlanId,
    limit: Limit,
    used: i64,
    feature: &'static str,
) -> Result<(), AppError> {
    let used = u32::try_from(used).unwrap_or(u32::MAX);
    if limit.allows(used) {
        return Ok(());
    }
    Err(limit_reached(plan, limit, i64::from(used), feature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stvdy_core::config::load_plans;

    fn plans() -> PlansFile {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        load_plans(&root.join("config/plans.toml")).expect("plans.toml loads")
    }

    /// The values come from the file, never from a literal here: retuning
    /// plans.toml must not leave a stale expectation behind.
    #[test]
    fn a_counted_limit_is_read_from_the_config_file() {
        let plans = plans();
        let max = i64::from(plans.get(PlanId::Free).devices_max);
        let limit = Limit::Max(plans.get(PlanId::Free).devices_max);

        assert!(enforce_count(&plans, PlanId::Free, limit, max - 1, "devices").is_ok());
        let error = enforce_count(&plans, PlanId::Free, limit, max, "devices")
            .expect_err("the limit is exclusive: `used < max`");
        assert_eq!(error.code(), "limit_reached");
        assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn an_unlimited_plan_never_trips() {
        let plans = plans();
        assert!(
            enforce_count(
                &plans,
                PlanId::Power,
                Limit::Unlimited,
                10_000,
                "workspaces"
            )
            .is_ok()
        );
    }

    #[test]
    fn a_limit_error_names_the_plan_that_would_admit_it() {
        let plans = plans();
        let limit = plans.get(PlanId::Free).workspaces;
        let max = i64::from(limit.max().expect("free workspaces are limited"));

        match enforce_count(&plans, PlanId::Free, limit, max, "workspaces") {
            Err(AppError::Entitlement {
                min_plan,
                limit,
                used,
                ..
            }) => {
                assert_eq!(min_plan, Some("pro"));
                assert_eq!(limit, Some(max));
                assert_eq!(used, Some(max));
            }
            other => panic!("expected an entitlement error, got {other:?}"),
        }

        // The top plan has nothing to sell, so it must not hint at one.
        match enforce_count(&plans, PlanId::Power, Limit::Max(1), 5, "workspaces") {
            Err(AppError::Entitlement { min_plan, .. }) => assert_eq!(min_plan, None),
            other => panic!("expected an entitlement error, got {other:?}"),
        }
    }
}
