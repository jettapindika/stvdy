use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanId {
    Free,
    Pro,
    Power,
}

impl PlanId {
    pub const ALL: [PlanId; 3] = [PlanId::Free, PlanId::Pro, PlanId::Power];

    pub fn as_str(self) -> &'static str {
        match self {
            PlanId::Free => "free",
            PlanId::Pro => "pro",
            PlanId::Power => "power",
        }
    }

    /// The cheapest paid plan, used by `EntitlementError::PlanRequired` hints.
    pub fn min_paid(self) -> PlanId {
        match self {
            PlanId::Free => PlanId::Pro,
            other => other,
        }
    }

    /// The plan a stored `users.plan_id` names. An unrecognised value is `Free`,
    /// which is the safe direction: a corrupt or future plan string must not grant
    /// more than the free tier.
    pub fn parse(raw: &str) -> PlanId {
        match raw {
            "pro" => PlanId::Pro,
            "power" => PlanId::Power,
            _ => PlanId::Free,
        }
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub version: u32,
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Credits {
    pub chat_message: i64,
    pub web_search: i64,
    pub flashcard_batch_10: i64,
    pub quiz_10: i64,
    pub report_outline: i64,
    pub report_section: i64,
    pub task_breakdown: i64,
    pub syllabus_import: i64,
    pub plan_generation: i64,
    pub agent_turn: i64,
    pub long_doc_threshold_tokens: i64,
    pub long_doc_credits_per_1k_tokens: i64,
}

impl Credits {
    pub fn cost(&self, action: CreditAction) -> i64 {
        match action {
            CreditAction::ChatMessage => self.chat_message,
            CreditAction::WebSearch => self.web_search,
            CreditAction::FlashcardBatch10 => self.flashcard_batch_10,
            CreditAction::Quiz10 => self.quiz_10,
            CreditAction::ReportOutline => self.report_outline,
            CreditAction::ReportSection => self.report_section,
            CreditAction::TaskBreakdown => self.task_breakdown,
            CreditAction::SyllabusImport => self.syllabus_import,
            CreditAction::PlanGeneration => self.plan_generation,
            CreditAction::AgentTurn => self.agent_turn,
        }
    }

    /// Base cost plus a linear surcharge for long inputs (the spec says
    /// "long-document operations scale with input tokens (simple multiplier)"
    /// without a formula; this is the pinned definition, A25).
    pub fn long_doc_cost(&self, action: CreditAction, input_tokens: i64) -> i64 {
        let base = self.cost(action);
        let over = input_tokens.saturating_sub(self.long_doc_threshold_tokens);
        if over <= 0 {
            return base;
        }
        // `i64::div_ceil` is unstable; `over` is positive here, so this is an
        // exact ceiling division.
        let thousands = (over + 999) / 1000;
        base + thousands * self.long_doc_credits_per_1k_tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditAction {
    ChatMessage,
    WebSearch,
    FlashcardBatch10,
    Quiz10,
    ReportOutline,
    ReportSection,
    TaskBreakdown,
    SyllabusImport,
    PlanGeneration,
    AgentTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlannerMode {
    Manual,
    Auto,
    /// `auto+balancer+weakness_blocks`. One variant so the config string is
    /// copied verbatim from the spec.
    #[serde(rename = "auto+balancer+weakness_blocks")]
    AutoBalancerWeaknessBlocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderChannel {
    Web,
    Push,
    Email,
    Telegram,
    Discord,
}

/// Model tiers, ordered cheapest to best. The `Ord` derive is load-bearing: the
/// plan-matrix check asserts that a higher plan never gets a *cheaper* model by
/// comparing these values, rather than by enumerating pairs (A41).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Cheapest,
    Cheap,
    Best,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Md,
    Pdf,
    Docx,
    Csv,
    Json,
}

/// A plan limit. Explicit rather than a `0 = unlimited` sentinel, because a
/// sentinel cannot express a genuine zero and the two are both meaningful:
/// `free` allows **zero** Google accounts, while `pro`/`power` allow unlimited
/// workspaces. With a sentinel, `google_accounts_max = 0` deserialized to
/// "unlimited", so free read as unlimited Google accounts and only the separate
/// `google_calendar` boolean kept it honest. Two fields encoding one policy,
/// with the numeric one stating the opposite of the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Unlimited,
    Max(u32),
}

impl Limit {
    pub fn is_unlimited(self) -> bool {
        matches!(self, Limit::Unlimited)
    }

    /// `None` when unlimited.
    pub fn max(self) -> Option<u32> {
        match self {
            Limit::Unlimited => None,
            Limit::Max(n) => Some(n),
        }
    }

    /// Whether `used` is within the limit. Unlimited always passes.
    pub fn allows(self, used: u32) -> bool {
        match self {
            Limit::Unlimited => true,
            Limit::Max(max) => used < max,
        }
    }
}

/// Serializes to the SAME shape `Deserialize` accepts: an integer for `Max`, the
/// string `"unlimited"` for `Unlimited`.
///
/// This must be hand-written. The derived form produced `{"Max":2}` and
/// `"Unlimited"`, which the hand-written visitor cannot read back. So
/// `plans.entitlements` (which mirrors the config for auditing, per the spec)
/// stored a blob that could never be parsed again. Nothing round-tripped, so no
/// test noticed.
impl Serialize for Limit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Limit::Unlimited => serializer.serialize_str("unlimited"),
            Limit::Max(n) => serializer.serialize_u32(*n),
        }
    }
}

/// Accepts either an integer (`0` is a real zero) or the string `"unlimited"`.
impl<'de> Deserialize<'de> for Limit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct LimitVisitor;

        impl serde::de::Visitor<'_> for LimitVisitor {
            type Value = Limit;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a non-negative integer or the string \"unlimited\"")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Limit, E> {
                u32::try_from(v)
                    .map(Limit::Max)
                    .map_err(|_| E::custom(format!("limit {v} does not fit in u32")))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Limit, E> {
                u32::try_from(v)
                    .map(Limit::Max)
                    .map_err(|_| E::custom(format!("limit {v} must be a non-negative integer")))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Limit, E> {
                match v {
                    "unlimited" => Ok(Limit::Unlimited),
                    other => Err(E::custom(format!(
                        "expected an integer or \"unlimited\", got {other:?}"
                    ))),
                }
            }
        }

        deserializer.deserialize_any(LimitVisitor)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanConfig {
    pub name: String,
    pub price_idr_monthly: i64,
    pub price_idr_yearly: i64,
    pub student_percent_off: u8,

    pub workspaces: Limit,
    pub sources_total: Limit,
    pub max_file_mb: u32,
    pub storage_mb: u32,
    pub devices_max: u32,
    pub credits_per_month: i64,
    pub chat_msgs_per_day: Limit,
    pub web_searches_per_day: u32,
    pub flashcards_per_month: Limit,
    pub quizzes_per_month: Limit,
    pub reports_per_month: Limit,

    pub export_formats: Vec<ExportFormat>,
    pub task_ai_breakdown: bool,
    pub quick_capture: bool,
    pub google_calendar: bool,
    pub google_accounts_max: Limit,
    pub syllabus_imports_total: Limit,
    pub planner: PlannerMode,
    pub agent_actions: bool,
    pub reminder_channels: Vec<ReminderChannel>,
    pub model_tier: ModelTier,
    pub byok: bool,
    pub advanced_analytics: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlansFile {
    pub meta: Meta,
    pub credits: Credits,
    pub provider_cost_ceiling_usd: BTreeMap<PlanId, f64>,
    /// Defaulted so that a config with no plan tables produces the actionable
    /// "must define a [free] plan" error from `validate()` rather than serde's
    /// "missing field `plans`".
    #[serde(default)]
    pub plans: BTreeMap<PlanId, PlanConfig>,
}

impl PlansFile {
    pub fn get(&self, plan: PlanId) -> &PlanConfig {
        // `validate()` guarantees presence; a lookup miss would be a programming
        // error, and this must not panic in a request path, so fall back to free.
        self.plans.get(&plan).unwrap_or(&self.plans[&PlanId::Free])
    }

    pub fn monthly_price(&self, plan: PlanId) -> i64 {
        self.get(plan).price_idr_monthly
    }

    pub fn yearly_price(&self, plan: PlanId) -> i64 {
        self.get(plan).price_idr_yearly
    }

    pub fn validate(&self) -> CoreResult<()> {
        for required in PlanId::ALL {
            if !self.plans.contains_key(&required) {
                return Err(CoreError::ConfigInvalid(format!(
                    "plans.toml must define a [{required}] plan"
                )));
            }
        }

        for (plan, config) in &self.plans {
            if config.price_idr_monthly < 0 || config.price_idr_yearly < 0 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: prices must not be negative"
                )));
            }
            if config.credits_per_month < 0 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: credits_per_month must not be negative"
                )));
            }
            if config.export_formats.is_empty() {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: export_formats must not be empty"
                )));
            }
            if config.reminder_channels.is_empty() {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: reminder_channels must not be empty"
                )));
            }
            if config.student_percent_off > 100 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: student_percent_off must be 0..=100"
                )));
            }
            if config.max_file_mb == 0 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: max_file_mb must be greater than 0"
                )));
            }
            if config.devices_max == 0 {
                return Err(CoreError::ConfigInvalid(format!(
                    "{plan}: devices_max must be greater than 0"
                )));
            }
        }

        if self.plans[&PlanId::Free].price_idr_monthly != 0 {
            return Err(CoreError::ConfigInvalid(
                "the free plan must cost 0 IDR".into(),
            ));
        }

        for plan in PlanId::ALL {
            if !self.provider_cost_ceiling_usd.contains_key(&plan) {
                return Err(CoreError::ConfigInvalid(format!(
                    "provider_cost_ceiling_usd is missing an entry for {plan}"
                )));
            }
        }

        // Limits must not shrink as the price rises. Checked here rather than only
        // in a test, because a mis-edited config must fail at boot with the
        // offending plan pair named. The test can only report it after the fact,
        // and only if CI runs.
        let free = self.get(PlanId::Free);
        let pro = self.get(PlanId::Pro);
        let power = self.get(PlanId::Power);

        let numeric: [(&str, Limit, Limit, Limit); 7] = [
            (
                "workspaces",
                free.workspaces,
                pro.workspaces,
                power.workspaces,
            ),
            (
                "sources_total",
                free.sources_total,
                pro.sources_total,
                power.sources_total,
            ),
            (
                "chat_msgs_per_day",
                free.chat_msgs_per_day,
                pro.chat_msgs_per_day,
                power.chat_msgs_per_day,
            ),
            (
                "flashcards_per_month",
                free.flashcards_per_month,
                pro.flashcards_per_month,
                power.flashcards_per_month,
            ),
            (
                "quizzes_per_month",
                free.quizzes_per_month,
                pro.quizzes_per_month,
                power.quizzes_per_month,
            ),
            (
                "reports_per_month",
                free.reports_per_month,
                pro.reports_per_month,
                power.reports_per_month,
            ),
            (
                "syllabus_imports_total",
                free.syllabus_imports_total,
                pro.syllabus_imports_total,
                power.syllabus_imports_total,
            ),
        ];
        for (field, f, p, pw) in numeric {
            if !limit_le(f, p) {
                return Err(CoreError::ConfigInvalid(format!(
                    "{field}: free must not exceed pro (free={f:?}, pro={p:?})"
                )));
            }
            if !limit_le(p, pw) {
                return Err(CoreError::ConfigInvalid(format!(
                    "{field}: pro must not exceed power (pro={p:?}, power={pw:?})"
                )));
            }
        }

        // Concrete maxima that are not Limit-typed.
        for (field, f, p, pw) in [
            (
                "storage_mb",
                free.storage_mb,
                pro.storage_mb,
                power.storage_mb,
            ),
            (
                "devices_max",
                free.devices_max,
                pro.devices_max,
                power.devices_max,
            ),
            (
                "web_searches_per_day",
                free.web_searches_per_day,
                pro.web_searches_per_day,
                power.web_searches_per_day,
            ),
        ] {
            if f > p || p > pw {
                return Err(CoreError::ConfigInvalid(format!(
                    "{field} must not decrease as the plan rises (free={f}, pro={p}, power={pw})"
                )));
            }
        }

        if free.credits_per_month > pro.credits_per_month
            || pro.credits_per_month > power.credits_per_month
        {
            return Err(CoreError::ConfigInvalid(
                "credits_per_month must not decrease as the plan rises".into(),
            ));
        }

        // Boolean features must not be available on a cheaper plan only.
        for (field, f, p, pw) in [
            (
                "agent_actions",
                free.agent_actions,
                pro.agent_actions,
                power.agent_actions,
            ),
            (
                "quick_capture",
                free.quick_capture,
                pro.quick_capture,
                power.quick_capture,
            ),
            (
                "task_ai_breakdown",
                free.task_ai_breakdown,
                pro.task_ai_breakdown,
                power.task_ai_breakdown,
            ),
        ] {
            if (f && !p) || (p && !pw) {
                return Err(CoreError::ConfigInvalid(format!(
                    "{field} must not be enabled on a cheaper plan only (free={f}, pro={p}, power={pw})"
                )));
            }
        }

        // Features the spec grants to power ONLY. Monotonicity alone cannot
        // express this: enabling `byok` on pro does not break free <= pro <=
        // power, because power also has it. Without an explicit rule a config
        // edit could silently hand pro a power-plan entitlement.
        for (field, f, p, pw) in [
            ("byok", free.byok, pro.byok, power.byok),
            (
                "advanced_analytics",
                free.advanced_analytics,
                pro.advanced_analytics,
                power.advanced_analytics,
            ),
        ] {
            if f || p || !pw {
                return Err(CoreError::ConfigInvalid(format!(
                    "{field} is a power-only feature (free={f}, pro={p}, power={pw})"
                )));
            }
        }

        // `google_calendar` is the one feature where power is not a superset of
        // pro in the boolean sense, so it is checked separately.
        if free.google_calendar || !pro.google_calendar || !power.google_calendar {
            return Err(CoreError::ConfigInvalid(
                "google_calendar must be off on free and on for pro and power".into(),
            ));
        }

        // A higher plan must never be served by a cheaper model. This is why
        // ModelTier derives Ord: the comparison replaces an enumerated pair check,
        // so adding a tier later cannot silently skip a case (A41).
        if !(free.model_tier <= pro.model_tier && pro.model_tier <= power.model_tier) {
            return Err(CoreError::ConfigInvalid(format!(
                "model_tier must not get cheaper as the plan rises \
                 (free={:?}, pro={:?}, power={:?})",
                free.model_tier, pro.model_tier, power.model_tier
            )));
        }

        Ok(())
    }
}

/// `Unlimited` is greater than any `Max`.
fn limit_le(lo: Limit, hi: Limit) -> bool {
    match (lo, hi) {
        (_, Limit::Unlimited) => true,
        (Limit::Unlimited, Limit::Max(_)) => false,
        (Limit::Max(a), Limit::Max(b)) => b >= a,
    }
}
