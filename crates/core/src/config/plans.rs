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
    /// `auto+balancer+weakness_blocks` — one variant so the config string is
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanConfig {
    pub name: String,
    pub price_idr_monthly: i64,
    pub price_idr_yearly: i64,
    pub student_percent_off: u8,

    /// `None` (written as `0` in TOML) means unlimited.
    #[serde(deserialize_with = "limit_of")]
    pub workspaces: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub sources_total: Option<u32>,
    pub max_file_mb: u32,
    pub storage_mb: u32,
    pub devices_max: u32,
    pub credits_per_month: i64,
    #[serde(deserialize_with = "limit_of")]
    pub chat_msgs_per_day: Option<u32>,
    pub web_searches_per_day: u32,
    #[serde(deserialize_with = "limit_of")]
    pub flashcards_per_month: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub quizzes_per_month: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub reports_per_month: Option<u32>,

    pub export_formats: Vec<ExportFormat>,
    pub task_ai_breakdown: bool,
    pub quick_capture: bool,
    pub google_calendar: bool,
    #[serde(deserialize_with = "limit_of")]
    pub google_accounts_max: Option<u32>,
    #[serde(deserialize_with = "limit_of")]
    pub syllabus_imports_total: Option<u32>,
    pub planner: PlannerMode,
    pub agent_actions: bool,
    pub reminder_channels: Vec<ReminderChannel>,
    pub model_tier: ModelTier,
    pub byok: bool,
    pub advanced_analytics: bool,
}

/// `0` in the TOML means "unlimited" and becomes `None`.
fn limit_of<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = u32::deserialize(deserializer)?;
    Ok(if raw == 0 { None } else { Some(raw) })
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

        Ok(())
    }
}
