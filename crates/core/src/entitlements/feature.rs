use crate::config::{ExportFormat, PlanId, PlansFile, ReminderChannel};

use super::check::grants;

/// Every action the plan matrix governs. One variant per thing a plan can refuse.
///
/// The set is the roadmap § 3 contract, copied, not extended: adding a variant is a
/// contract change. `ALL` exists so the matrix test can walk the whole space rather
/// than the pairs someone remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    CreateWorkspace,
    UploadSource {
        size_bytes: i64,
    },
    IngestSource,
    ChatMessage,
    WebSearch,
    CreateDeck,
    FlashcardBatch {
        n: u32,
    },
    CreateQuiz {
        n: u32,
    },
    Report,
    ExportFormat(ExportFormat),
    TaskBreakdown,
    QuickCapture,
    GoogleCalendar,
    /// The Google *account* cap, counted rather than locked (A245).
    ///
    /// Distinct from [`Feature::GoogleCalendar`], and both are needed: the boolean says
    /// whether a plan may connect Google at all, and this counts how many accounts one
    /// plan allows. `google_accounts_max` was in the matrix and in the entitlement view
    /// and enforced **nowhere** until this variant existed — a limit that is configured
    /// but not enforced is worse than none, because it looks like a control.
    GoogleAccount,
    SyllabusImport,
    PlanGeneration,
    AgentAction,
    DeviceLogin,
    Byok,
    /// The payload type is the config's enum, not a second copy of it (A142).
    ReminderChannel(ReminderChannel),
    AdvancedAnalytics,
}

impl Feature {
    /// Every variant, for the generated matrix test. `ExportFormat` and
    /// `ReminderChannel` contribute one entry per variant of their own enum, so the
    /// walk covers the payload space too and not just the discriminant.
    ///
    /// The two payloads that carry an amount are the interesting case, and one value
    /// each is enough to walk them: an upload every plan admits (so the walk's verdict
    /// comes from the plan's counts, not from a file size) and a batch larger than the
    /// cheapest plan's monthly allowance, which is the one refusal in the shipped
    /// matrix that is about a count rather than about a boolean.
    pub const ALL: &'static [Feature] = &[
        Feature::CreateWorkspace,
        Feature::UploadSource {
            size_bytes: 1024 * 1024,
        },
        Feature::IngestSource,
        Feature::ChatMessage,
        Feature::WebSearch,
        Feature::CreateDeck,
        Feature::FlashcardBatch { n: 40 },
        Feature::CreateQuiz { n: 10 },
        Feature::Report,
        Feature::ExportFormat(ExportFormat::Md),
        Feature::ExportFormat(ExportFormat::Pdf),
        Feature::ExportFormat(ExportFormat::Docx),
        Feature::ExportFormat(ExportFormat::Csv),
        Feature::ExportFormat(ExportFormat::Json),
        Feature::TaskBreakdown,
        Feature::QuickCapture,
        Feature::GoogleCalendar,
        Feature::GoogleAccount,
        Feature::SyllabusImport,
        Feature::PlanGeneration,
        Feature::AgentAction,
        Feature::DeviceLogin,
        Feature::Byok,
        Feature::ReminderChannel(ReminderChannel::Web),
        Feature::ReminderChannel(ReminderChannel::Push),
        Feature::ReminderChannel(ReminderChannel::Email),
        Feature::ReminderChannel(ReminderChannel::Telegram),
        Feature::ReminderChannel(ReminderChannel::Discord),
        Feature::AdvancedAnalytics,
    ];

    /// The cheapest plan that grants this feature, or `None` when no plan does.
    ///
    /// Derived from the matrix, never from a list: the returned plan is the first in
    /// `PlanId::ALL` whose `PlanConfig` grants the feature, which is what makes
    /// `min_plan` in an error message true by construction. A feature only the top plan
    /// has (BYOK) therefore names that plan, not the one directly above the caller —
    /// naming a plan that does not grant it would send the user to a purchase that does
    /// not fix their refusal.
    pub fn min_plan(self, plans: &PlansFile) -> Option<PlanId> {
        PlanId::ALL
            .into_iter()
            .find(|plan| grants(plans, *plan, self))
    }
}
