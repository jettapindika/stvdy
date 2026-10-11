use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Web,
    Macos,
    Windows,
    Ios,
    Android,
}

impl Platform {
    pub const ALL: [Platform; 5] = [
        Platform::Web,
        Platform::Macos,
        Platform::Windows,
        Platform::Ios,
        Platform::Android,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Web => "web",
            Platform::Macos => "macos",
            Platform::Windows => "windows",
            Platform::Ios => "ios",
            Platform::Android => "android",
        }
    }

    pub fn is_mobile(self) -> bool {
        matches!(self, Platform::Ios | Platform::Android)
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformPolicy {
    pub allow_purchase_ui: bool,
    pub show_prices: bool,
    pub allow_purchase_cta_in_locked_messages: bool,
}

impl PlatformPolicy {
    /// The policy used when a platform key is absent. Deny-by-default: a missing
    /// entry must never accidentally enable selling on mobile.
    pub const DENY: PlatformPolicy = PlatformPolicy {
        allow_purchase_ui: false,
        show_prices: false,
        allow_purchase_cta_in_locked_messages: false,
    };
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BillingPolicy {
    pub platforms: BTreeMap<Platform, PlatformPolicy>,
}

impl BillingPolicy {
    pub fn for_platform(&self, platform: Platform) -> &PlatformPolicy {
        self.platforms
            .get(&platform)
            .unwrap_or(&PlatformPolicy::DENY)
    }

    pub fn purchase_allowed(&self, platform: Platform) -> bool {
        self.for_platform(platform).allow_purchase_ui
    }

    /// Enforced at load time, so a config edit cannot enable store-policy
    /// violations. Mobile must never sell: QRIS checkout is web/desktop only,
    /// and the plan then syncs to the account.
    pub fn validate(&self) -> CoreResult<()> {
        for platform in Platform::ALL {
            let policy = self.platforms.get(&platform).ok_or_else(|| {
                CoreError::ConfigInvalid(format!(
                    "billing_policy.toml must declare [platforms.{platform}]"
                ))
            })?;

            if platform.is_mobile() && policy.allow_purchase_ui {
                return Err(CoreError::ConfigInvalid(format!(
                    "{platform} must not allow purchase UI: QRIS checkout is web/desktop only \
                     (App Store / Play billing is out of scope for the MVP)"
                )));
            }
            if platform.is_mobile() && policy.show_prices {
                return Err(CoreError::ConfigInvalid(format!(
                    "{platform} must not show prices"
                )));
            }
            if policy.allow_purchase_cta_in_locked_messages && !policy.allow_purchase_ui {
                return Err(CoreError::ConfigInvalid(format!(
                    "{platform}: a purchase CTA is meaningless when purchase UI is disabled"
                )));
            }
        }
        Ok(())
    }
}
