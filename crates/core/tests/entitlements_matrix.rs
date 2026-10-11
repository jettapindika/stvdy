//! The plan×feature matrix, generated from the shipped `config/plans.toml`.
//!
//! Nothing here enumerates pairs by hand. Every case walks `PlanId::ALL` ×
//! `Feature::ALL` and asserts a property of the whole matrix, so a new `Feature`
//! variant, a new plan, or a `plans.toml` edit is covered the moment it lands.
//! A test that listed the pairs it happened to think of would pass while the
//! arm nobody wrote went unnoticed — which is the failure mode the roadmap's
//! "generated test asserts the whole plan×feature matrix" exists to prevent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use stvdy_core::config::{ExportFormat, Limit, PlanId, PlansFile, ReminderChannel, load_plans};
use stvdy_core::entitlements::{
    Credits, EntitlementError, Feature, PlanFacts, SubStatus, UsageSnapshot, check_with,
};

const PLANS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/plans.toml");

/// A desktop or web client: one an upgrade may be offered to (A143). The matrix is about
/// what each plan decides, not about the store rules of a mobile build — the
/// `feature_locked` half of that decision belongs to the server, which reads the
/// platform from the device row.
const FACTS: PlanFacts = PlanFacts {
    upgrade_offerable: true,
};

fn plans() -> PlansFile {
    load_plans(Path::new(PLANS_PATH)).expect("shipped plans.toml")
}

/// The contract's `check(user, Feature)`, resolved against the config `plans()` names.
///
/// The matrix these expectations are computed from and the matrix the engine enforces
/// are therefore the same file: pointing `plans()` at a scratch config moves both, so
/// the suite can be run against a variant without the engine answering from a second
/// source.
fn check(u: &UsageSnapshot, f: Feature) -> Result<(), EntitlementError> {
    check_with(&plans(), &FACTS, u, f)
}

/// A snapshot with every counter fresh and the plan's full monthly grant unspent,
/// so a refusal can only come from the plan, never from usage.
fn fresh(plan: PlanId) -> UsageSnapshot {
    UsageSnapshot {
        plan,
        subscription_status: SubStatus::Active,
        credits: Credits {
            monthly: plans().get(plan).credits_per_month,
            purchased: 0,
        },
        storage_bytes: 0,
        workspaces: 0,
        sources: 0,
        devices: 0,
        chat_msgs_today: 0,
        web_searches_today: 0,
        flashcards_this_month: 0,
        quizzes_this_month: 0,
        reports_this_month: 0,
        syllabus_imports: 0,
    }
}

#[test]
fn the_matrix_is_total() {
    // Every feature × every plan returns a verdict. `check` must not panic and must
    // not be `unimplemented` for any variant: the enum is matched exhaustively, and
    // this is the test that says so out loud for a reader who never opens check.rs.
    for plan in PlanId::ALL {
        for feature in Feature::ALL {
            let _ = check(&fresh(plan), *feature);
        }
    }
}

#[test]
fn every_refusal_on_a_lower_plan_names_the_plan_that_lifts_it() {
    // The UI's upgrade prompt is built from `min_plan`. A refusal that does not name
    // a plan is a dead end for the user, so the matrix may not contain one: for every
    // feature free refuses, `min_plan` is `Some(pro)`; for every feature pro refuses,
    // `Some(power)`; on power a refusal is a genuine maximum and carries `None`.
    let plans = plans();
    for plan in PlanId::ALL {
        for feature in Feature::ALL {
            // The plan a refusal on `plan` must name: the first plan above it that
            // admits this action from a fresh snapshot. Derived by asking the engine
            // itself rather than from a second hand-written table, so it cannot drift
            // from the matrix — and a `min_plan` naming a plan that does *not* lift
            // the refusal fails here, which is what a bare "next plan up" would do for
            // a feature only the top plan has.
            let next_plan = |p: PlanId| -> Option<PlanId> {
                let above = PlanId::ALL
                    .iter()
                    .position(|candidate| *candidate == p)
                    .map(|index| index + 1)?;
                PlanId::ALL[above..].iter().copied().find(|candidate| {
                    check_with(&plans, &FACTS, &fresh(*candidate), *feature).is_ok()
                })
            };
            match check(&fresh(plan), *feature) {
                Ok(()) => {}
                Err(EntitlementError::PlanRequired { min_plan, .. })
                | Err(EntitlementError::LimitReached { min_plan, .. }) => {
                    assert_eq!(min_plan, next_plan(plan), "{plan:?} refuses {feature:?}");
                }
                Err(EntitlementError::FeatureLocked { .. }) => {
                    // Only reachable with no plan granting the feature; see below.
                }
                Err(EntitlementError::InsufficientCredits { .. }) => {
                    panic!("a fresh snapshot has its full grant, so credits cannot be short")
                }
            }
        }
    }
}

#[test]
fn a_granted_feature_stays_granted_as_the_plan_rises() {
    // The matrix's plan column must be monotone: if free may do it, pro may do it.
    // This is what catches a `check` arm wired to the wrong plan's field — the class
    // of bug a hand-written pair table finds only if the pair was in the table.
    for feature in Feature::ALL {
        let verdicts: Vec<bool> = PlanId::ALL
            .iter()
            .map(|p| check(&fresh(*p), *feature).is_ok())
            .collect();
        assert!(
            !verdicts.windows(2).any(|w| w[0] && !w[1]),
            "{feature:?} is allowed on a lower plan and refused on a higher one: {verdicts:?}"
        );
    }
}

#[test]
fn every_feature_is_granted_by_at_least_one_plan() {
    // `FeatureLocked` means "no plan sells this". In the shipped matrix that must be
    // unreachable, or the feature is dead code the UI can never offer.
    for feature in Feature::ALL {
        assert!(
            PlanId::ALL
                .iter()
                .any(|p| check(&fresh(*p), *feature).is_ok()),
            "{feature:?} is in no plan's matrix"
        );
    }
}

#[test]
fn a_numeric_limit_refuses_at_exactly_its_configured_maximum() {
    // Generated over every numeric feature and every plan: one below the limit passes,
    // the limit itself fails, and the refusal's `limit`/`used` are the real numbers the
    // UI renders as "3 of 3 devices". A hardcoded limit anywhere fails here.
    for plan in PlanId::ALL {
        for (feature, limit) in numeric_features(&plans(), plan) {
            let max = limit.expect("numeric_features yields only bounded limits");
            let mut u = fresh(plan);
            set_used(&mut u, feature, max - 1);
            assert!(
                check(&u, feature).is_ok(),
                "{plan:?} {feature:?} at {max}-1"
            );
            set_used(&mut u, feature, max);
            match check(&u, feature) {
                Err(EntitlementError::LimitReached { limit, used, .. }) => {
                    assert_eq!(limit, max);
                    assert_eq!(used, max);
                }
                other => panic!("{plan:?} {feature:?} at {max}: {other:?}"),
            }
        }
    }
}

#[test]
fn the_specs_documented_plan_values_are_what_the_matrix_enforces() {
    // The one hand-written table in this file, and it is the spec's own numbers
    // (`docs/SPEC.md` § Plans), so a typo in plans.toml or a transposed arm fails.
    assert!(check(&fresh(PlanId::Free), Feature::Byok).is_err());
    assert!(check(&fresh(PlanId::Power), Feature::Byok).is_ok());
    assert!(
        check(
            &fresh(PlanId::Free),
            Feature::ExportFormat(ExportFormat::Pdf)
        )
        .is_err()
    );
    assert!(
        check(
            &fresh(PlanId::Pro),
            Feature::ExportFormat(ExportFormat::Pdf)
        )
        .is_ok()
    );
    assert!(
        check(
            &fresh(PlanId::Pro),
            Feature::ExportFormat(ExportFormat::Csv)
        )
        .is_err()
    );
    assert!(
        check(
            &fresh(PlanId::Power),
            Feature::ExportFormat(ExportFormat::Csv)
        )
        .is_ok()
    );
    assert!(
        check(
            &fresh(PlanId::Free),
            Feature::ReminderChannel(ReminderChannel::Email)
        )
        .is_err()
    );
    assert!(
        check(
            &fresh(PlanId::Pro),
            Feature::ReminderChannel(ReminderChannel::Email)
        )
        .is_ok()
    );
    assert!(
        check(
            &fresh(PlanId::Pro),
            Feature::ReminderChannel(ReminderChannel::Telegram)
        )
        .is_err()
    );
    assert!(check(&fresh(PlanId::Free), Feature::PlanGeneration).is_err());
    assert!(check(&fresh(PlanId::Pro), Feature::PlanGeneration).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::AdvancedAnalytics).is_err());
    assert!(check(&fresh(PlanId::Power), Feature::AdvancedAnalytics).is_ok());
    assert!(check(&fresh(PlanId::Free), Feature::SyllabusImport).is_ok()); // 1 allowed
    let mut used_one = fresh(PlanId::Free);
    used_one.syllabus_imports = 1;
    assert!(check(&used_one, Feature::SyllabusImport).is_err());
    assert!(
        check(
            &fresh(PlanId::Free),
            Feature::UploadSource {
                size_bytes: 20 * 1024 * 1024
            }
        )
        .is_ok()
    );
    assert!(
        check(
            &fresh(PlanId::Free),
            Feature::UploadSource {
                size_bytes: 20 * 1024 * 1024 + 1
            }
        )
        .is_err()
    );
}

/// Every countable limit this plan has, paired with the `Feature` that spends it.
///
/// Generated from the config, so retuning `plans.toml` changes what the boundary walk
/// covers without anyone editing this file.
///
/// The byte-sized limits are not here. `max_file_mb` and `storage_mb` are ceilings
/// rather than counts — a 20 MB file fits a 20 MB plan — so "at exactly the maximum" is
/// the case that *passes* there, and the upload boundary is asserted by name in
/// `the_specs_documented_plan_values_are_what_the_matrix_enforces` instead. Nor is
/// `UploadSource`'s size a counter: it comes from the request, not from the account.
fn numeric_features(plans: &PlansFile, plan: PlanId) -> Vec<(Feature, Option<i64>)> {
    let cfg = plans.get(plan);
    [
        (Feature::CreateWorkspace, cfg.workspaces),
        (Feature::IngestSource, cfg.sources_total),
        (Feature::ChatMessage, cfg.chat_msgs_per_day),
        // `web_searches_per_day` and `devices_max` are plain `u32`s — no plan offers
        // unlimited — so the bounded limit is built here, the way a caller builds it.
        (Feature::WebSearch, Limit::Max(cfg.web_searches_per_day)),
        (Feature::DeviceLogin, Limit::Max(cfg.devices_max)),
        // A unit request: one card is the smallest batch, and the arm's own arithmetic
        // is what the boundary walk is asking about.
        (Feature::FlashcardBatch { n: 1 }, cfg.flashcards_per_month),
        (Feature::CreateQuiz { n: 1 }, cfg.quizzes_per_month),
        (Feature::Report, cfg.reports_per_month),
        (Feature::SyllabusImport, cfg.syllabus_imports_total),
    ]
    .into_iter()
    .filter(|(_, limit)| !limit.is_unlimited())
    .map(|(feature, limit)| (feature, limit.max().map(i64::from)))
    .collect()
}

/// Sets the counter the feature's decision reads, so the boundary can be posed without
/// the test knowing which `UsageSnapshot` field backs which `Feature`. A feature that
/// has no counter is a mistake in `numeric_features`, not a case to paper over.
fn set_used(u: &mut UsageSnapshot, feature: Feature, value: i64) {
    match feature {
        Feature::CreateWorkspace => u.workspaces = value,
        Feature::IngestSource => u.sources = value,
        Feature::ChatMessage => u.chat_msgs_today = value,
        Feature::WebSearch => u.web_searches_today = value,
        Feature::DeviceLogin => u.devices = value,
        Feature::FlashcardBatch { .. } => u.flashcards_this_month = value,
        Feature::CreateQuiz { .. } => u.quizzes_this_month = value,
        Feature::Report => u.reports_this_month = value,
        Feature::SyllabusImport => u.syllabus_imports = value,
        other => panic!("{other:?} is not a countable feature"),
    }
}
