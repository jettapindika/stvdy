use crate::config::{Limit, PlanId, PlannerMode, PlansFile};

use super::feature::Feature;
use super::usage::{PlanFacts, UsageSnapshot};

/// One mebibyte. The config states file and storage limits in MB; the counters are in
/// bytes, and they are the same bytes `sources.file_size` holds.
const MIB: i64 = 1024 * 1024;

/// The batch the credit costs are quoted in: `flashcard_batch_10` and `quiz_10` are the
/// price of one *started* batch of ten, so eleven cards cost two batches.
const BATCH: i64 = 10;

/// The verdict for one action. Pure: no IO, no clock, no database.
///
/// The matrix is passed in rather than read here: `config/plans.toml` is the authority,
/// the server loads it once at startup, and handing it over is what lets the whole
/// matrix be tested against a scratch config — and what keeps a handler from ending up
/// with a second answer to "what does this plan allow".
///
/// `facts.upgrade_offerable` is the one piece of client state this consults, and it comes
/// from the *device row*, not the request body (A144/A143): on iOS and Android
/// `config/billing_policy.toml` forbids a purchase call to action, so a locked feature is
/// `FeatureLocked` rather than `PlanRequired`.
pub fn check_with(
    plans: &PlansFile,
    facts: &PlanFacts,
    u: &UsageSnapshot,
    f: Feature,
) -> Result<(), EntitlementError> {
    let plan = u.plan;
    let cfg = plans.get(plan);

    match f {
        Feature::CreateWorkspace => counted(f, cfg.workspaces, u.workspaces, plan),
        Feature::IngestSource => counted(f, cfg.sources_total, u.sources, plan),
        Feature::UploadSource { size_bytes } => {
            counted(f, cfg.sources_total, u.sources, plan)?;
            // A file and the storage it occupies are ceilings, not counts: a 20 MB file
            // fits a 20 MB plan, so these two are inclusive where `counted` is not.
            let max_file = i64::from(cfg.max_file_mb) * MIB;
            if size_bytes > max_file {
                return Err(limit_reached(f, max_file, size_bytes, plan));
            }
            let after = u.storage_bytes.saturating_add(size_bytes);
            let quota = i64::from(cfg.storage_mb) * MIB;
            if after > quota {
                return Err(limit_reached(f, quota, after, plan));
            }
            Ok(())
        }
        Feature::ChatMessage => {
            counted(f, cfg.chat_msgs_per_day, u.chat_msgs_today, plan)?;
            affordable(u, plans.credits.chat_message)
        }
        Feature::WebSearch => {
            counted(
                f,
                Limit::Max(cfg.web_searches_per_day),
                u.web_searches_today,
                plan,
            )?;
            affordable(u, plans.credits.web_search)
        }
        // A deck is not a limited resource; its cards are, which is `FlashcardBatch`.
        Feature::CreateDeck => Ok(()),
        Feature::FlashcardBatch { n } => {
            // The whole request must fit: checking one card at a time would let thirty
            // requests of thirty cards each through.
            batch_within(
                f,
                cfg.flashcards_per_month,
                u.flashcards_this_month,
                n,
                plan,
            )?;
            affordable(u, batch_count(n) * plans.credits.flashcard_batch_10)
        }
        Feature::CreateQuiz { n } => {
            // One quiz per request whatever it holds (A94); `n` only prices it.
            counted(f, cfg.quizzes_per_month, u.quizzes_this_month, plan)?;
            affordable(u, batch_count(n) * plans.credits.quiz_10)
        }
        Feature::Report => {
            counted(f, cfg.reports_per_month, u.reports_this_month, plan)?;
            affordable(u, plans.credits.report_outline)
        }
        Feature::SyllabusImport => {
            counted(f, cfg.syllabus_imports_total, u.syllabus_imports, plan)?;
            affordable(u, plans.credits.syllabus_import)
        }
        Feature::DeviceLogin => counted(f, Limit::Max(cfg.devices_max), u.devices, plan),
        // The Google account cap (A245), counted against the live connections. `free`'s
        // `google_accounts_max = 0` makes this refuse before `google_calendar` is even
        // consulted, which is why the connect route checks the boolean first: the student
        // should be told the feature is not in their plan, not that they have 0 of 0.
        Feature::GoogleAccount => counted(f, cfg.google_accounts_max, u.google_accounts, plan),
        // Everything a plan either has or does not. `grants` is the one place that
        // mapping lives, so no arm here can be wired to the wrong plan field.
        Feature::ExportFormat(_)
        | Feature::TaskBreakdown
        | Feature::QuickCapture
        | Feature::GoogleCalendar
        | Feature::PlanGeneration
        | Feature::AgentAction
        | Feature::Byok
        | Feature::ReminderChannel(_)
        | Feature::AdvancedAnalytics => locked(plans, facts, plan, f),
    }
}

/// The verdict's failure: one variant per refusal the roadmap § 3 contract names.
///
/// `min_plan` is an `Option` on both plan-related variants because the envelope writes
/// `min_plan: null` when there is nothing to sell — a refusal on the top plan is a
/// genuine maximum, and "not for sale" is the truth (A140). The client reads the one
/// field either way, so both variants carry it in the same shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementError {
    /// A higher plan has this feature, and this client may be told which one.
    PlanRequired {
        feature: Feature,
        min_plan: Option<PlanId>,
    },
    /// A counted limit is exhausted. `limit` and `used` are the numbers the UI renders
    /// as "3 of 3 devices".
    LimitReached {
        feature: Feature,
        limit: i64,
        used: i64,
        min_plan: Option<PlanId>,
    },
    /// The balance cannot pay for the action.
    InsufficientCredits { needed: i64, available: i64 },
    /// Locked, and no upgrade can be offered to this client (A143).
    FeatureLocked { feature: Feature },
}

/// Whether `plan`'s config grants `feature`.
///
/// The one place the feature→`PlanConfig` field mapping lives: the matrix arms call it
/// through `locked`, and `Feature::min_plan` scans `PlanId::ALL` with it, so a refusal
/// cannot name a plan whose config does not grant the feature it is refusing.
pub(crate) fn grants(plans: &PlansFile, plan: PlanId, f: Feature) -> bool {
    let cfg = plans.get(plan);
    match f {
        // A countable limit: every plan grants the feature, some plans grant more of it.
        Feature::CreateWorkspace
        | Feature::UploadSource { .. }
        | Feature::IngestSource
        | Feature::ChatMessage
        | Feature::WebSearch
        | Feature::CreateDeck
        | Feature::FlashcardBatch { .. }
        | Feature::CreateQuiz { .. }
        | Feature::Report
        | Feature::SyllabusImport
        | Feature::DeviceLogin
        | Feature::GoogleAccount => true,
        // A feature a plan either has or does not.
        Feature::ExportFormat(format) => cfg.export_formats.contains(&format),
        Feature::TaskBreakdown => cfg.task_ai_breakdown,
        Feature::QuickCapture => cfg.quick_capture,
        Feature::GoogleCalendar => cfg.google_calendar,
        Feature::PlanGeneration => cfg.planner != PlannerMode::Manual,
        Feature::AgentAction => cfg.agent_actions,
        Feature::Byok => cfg.byok,
        Feature::ReminderChannel(channel) => cfg.reminder_channels.contains(&channel),
        Feature::AdvancedAnalytics => cfg.advanced_analytics,
    }
}

/// A feature the plan either has or does not.
///
/// `FeatureLocked` means "locked, and no upgrade can be offered to this client": either
/// no plan in the matrix grants the feature at all, or the client is one the billing
/// policy forbids selling to (A143). `PlanRequired` means a higher plan has it and this
/// client may be told so, naming the cheapest such plan.
fn locked(
    plans: &PlansFile,
    facts: &PlanFacts,
    plan: PlanId,
    f: Feature,
) -> Result<(), EntitlementError> {
    if grants(plans, plan, f) {
        return Ok(());
    }
    match f.min_plan(plans) {
        Some(min_plan) if facts.upgrade_offerable => Err(EntitlementError::PlanRequired {
            feature: f,
            min_plan: Some(min_plan),
        }),
        _ => Err(EntitlementError::FeatureLocked { feature: f }),
    }
}

/// A countable limit: there must be room for one more.
fn counted(
    feature: Feature,
    limit: Limit,
    used: i64,
    plan: PlanId,
) -> Result<(), EntitlementError> {
    if limit.allows(count(used)) {
        return Ok(());
    }
    Err(limit_reached(feature, limit_max(limit), used, plan))
}

/// A countable limit where the request adds more than one item.
fn batch_within(
    feature: Feature,
    limit: Limit,
    used: i64,
    requested: u32,
    plan: PlanId,
) -> Result<(), EntitlementError> {
    if fits(limit, used, i64::from(requested)) {
        return Ok(());
    }
    Err(limit_reached(feature, limit_max(limit), used, plan))
}

/// Whether `used + requested` items fit under `limit` — `Limit::allows` generalised to a
/// request of more than one, and the same comparison the Phase 1 shim's `enforce_batch`
/// makes, so replacing a call site does not move the boundary.
fn fits(limit: Limit, used: i64, requested: i64) -> bool {
    match limit.max() {
        None => true,
        Some(max) => used.saturating_add(requested) <= i64::from(max),
    }
}

/// `Limit::allows` takes a `u32` count. A counter too large for one is over every limit
/// the config can express, so saturating to `u32::MAX` refuses it.
fn count(used: i64) -> u32 {
    u32::try_from(used).unwrap_or(u32::MAX)
}

/// The number a refusal reports. Every caller is a refusal path and `Unlimited` never
/// refuses (`Limit::allows` is true for it), so the value is a real maximum; `i64::MAX`
/// is what "no maximum" would mean if that ever stopped being true, rather than a zero
/// that reads as a real cap.
fn limit_max(limit: Limit) -> i64 {
    limit.max().map_or(i64::MAX, i64::from)
}

/// `ceil(n / 10)`: the credits are charged per started batch.
fn batch_count(n: u32) -> i64 {
    (i64::from(n) + BATCH - 1) / BATCH
}

/// The 422 the roadmap § 5 table maps to `limit_reached`.
///
/// `min_plan` is the next plan up, or `None` on the top plan where a refusal is a
/// genuine maximum — the rule the Phase 1 shim's `upgrade_from` already applies, so the
/// shipped response body does not change.
fn limit_reached(feature: Feature, limit: i64, used: i64, plan: PlanId) -> EntitlementError {
    EntitlementError::LimitReached {
        feature,
        limit,
        used,
        min_plan: upgrade_from(plan),
    }
}

/// The next plan up the ladder, `None` on the top plan.
///
/// Derived from `PlanId::ALL` rather than written out, so a plan added later joins the
/// ladder without a second place to edit.
fn upgrade_from(plan: PlanId) -> Option<PlanId> {
    PlanId::ALL
        .iter()
        .skip_while(|candidate| **candidate != plan)
        .nth(1)
        .copied()
}

/// The 402 for an action the balance cannot pay for.
fn affordable(u: &UsageSnapshot, cost: i64) -> Result<(), EntitlementError> {
    let available = u.credits.total();
    if available >= cost {
        return Ok(());
    }
    Err(EntitlementError::InsufficientCredits {
        needed: cost,
        available,
    })
}
