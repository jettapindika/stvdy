//! Plan limits, read from `config/plans.toml`.
//!
//! This is the Phase 1 shim. The roadmap puts `check(&UsageSnapshot, Feature)`
//! in `crates/core/src/entitlements/` in Phase 7, and every handler calls it.
//! Phase 1 needs five limits before that engine exists, so it reads
//! `PlanConfig` directly here and returns the *same* `AppError::Entitlement`
//! shape the engine will return. Phase 7 replaces the call sites with `check`
//! and the response bodies do not change.
//!
//! Nothing here hardcodes a limit: the matrix lives in `plans.toml`.

use stvdy_core::config::{Limit, PlanId, PlansFile};

use crate::error::AppError;

/// The plan a user would have to buy to get past this limit. Every plan in the
/// matrix raises or removes every limit of the plan below it, so "the next one
/// up" is the honest answer, and `None` on the top plan means "not for sale",
/// which is the truth.
fn upgrade_from(plan: PlanId) -> Option<PlanId> {
    match plan {
        PlanId::Free => Some(PlanId::Pro),
        PlanId::Pro => Some(PlanId::Power),
        PlanId::Power => None,
    }
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

/// A file-size limit. `max_file_mb` is a `u32` rather than a `Limit` because no
/// plan offers unlimited uploads.
pub fn enforce_max_file_mb(
    plans: &PlansFile,
    plan: PlanId,
    size_bytes: i64,
) -> Result<(), AppError> {
    let max_mb = i64::from(plans.get(plan).max_file_mb);
    let max_bytes = max_mb * 1024 * 1024;
    if size_bytes <= max_bytes {
        return Ok(());
    }
    Err(AppError::Entitlement {
        code: "limit_reached",
        message: format!(
            "files on the {} plan are limited to {max_mb} MB",
            plan.as_str()
        ),
        min_plan: upgrade_from(plan).map(|p| p.as_str()),
        limit: Some(max_bytes),
        used: Some(size_bytes),
    })
}

/// Storage is a running total, so the check is "existing + new".
pub fn enforce_storage_mb(
    plans: &PlansFile,
    plan: PlanId,
    used_bytes: i64,
    add_bytes: i64,
) -> Result<(), AppError> {
    let max_bytes = i64::from(plans.get(plan).storage_mb) * 1024 * 1024;
    let after = used_bytes + add_bytes;
    if after <= max_bytes {
        return Ok(());
    }
    Err(AppError::Entitlement {
        code: "limit_reached",
        message: format!(
            "the {} plan includes {} MB of storage",
            plan.as_str(),
            plans.get(plan).storage_mb
        ),
        min_plan: upgrade_from(plan).map(|p| p.as_str()),
        limit: Some(max_bytes),
        used: Some(after),
    })
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
    ///
    /// `devices_max` is a plain `u32` in the config (no plan offers unlimited
    /// devices), so the limit is built here the way a caller would build it.
    #[test]
    fn the_device_limit_is_read_from_the_config_file() {
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

    #[test]
    fn storage_is_a_running_total() {
        let plans = plans();
        let mb = 1024 * 1024;
        let max = i64::from(plans.get(PlanId::Free).storage_mb);
        assert!(enforce_storage_mb(&plans, PlanId::Free, (max - 1) * mb, mb).is_ok());
        assert!(enforce_storage_mb(&plans, PlanId::Free, max * mb, 1).is_err());
    }

    #[test]
    fn file_size_is_checked_against_the_plan() {
        let plans = plans();
        let mb = 1024 * 1024;
        let max = i64::from(plans.get(PlanId::Free).max_file_mb);
        assert!(enforce_max_file_mb(&plans, PlanId::Free, max * mb).is_ok());
        assert!(enforce_max_file_mb(&plans, PlanId::Free, max * mb + 1).is_err());
    }
}
