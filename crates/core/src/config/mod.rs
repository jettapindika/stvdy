mod billing_policy;
mod plans;

use std::path::Path;

use crate::error::{CoreError, CoreResult};

pub use billing_policy::{BillingPolicy, Platform, PlatformPolicy};
pub use plans::{
    CreditAction, Credits, ExportFormat, Meta, ModelTier, PlanConfig, PlanId, PlannerMode,
    PlansFile, ReminderChannel,
};

fn read_to_string(path: &Path) -> CoreResult<String> {
    std::fs::read_to_string(path).map_err(|source| CoreError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load_plans(path: &Path) -> CoreResult<PlansFile> {
    let raw = read_to_string(path)?;
    let parsed: PlansFile = toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })?;
    parsed.validate()?;
    Ok(parsed)
}

pub fn load_billing_policy(path: &Path) -> CoreResult<BillingPolicy> {
    let raw = read_to_string(path)?;
    let parsed: BillingPolicy = toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })?;
    parsed.validate()?;
    Ok(parsed)
}
