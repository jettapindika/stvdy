//! Tests for the plans config loader. The shipped `config/plans.toml` is the
//! fixture, so a typo in it fails the build rather than production.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use stvdy_core::config::{CreditAction, PlanId, load_plans};
use stvdy_core::error::CoreError;

const PLANS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/plans.toml");

fn shipped() -> stvdy_core::config::PlansFile {
    load_plans(Path::new(PLANS_PATH)).expect("shipped plans.toml must load")
}

#[test]
fn loads_the_shipped_plan_file() {
    let plans = shipped();
    assert_eq!(plans.plans.len(), 3);
    assert_eq!(plans.plans[&PlanId::Free].price_idr_monthly, 0);
    assert_eq!(plans.plans[&PlanId::Pro].price_idr_monthly, 10_000);
    assert_eq!(plans.plans[&PlanId::Pro].price_idr_yearly, 100_000);
    assert_eq!(plans.plans[&PlanId::Power].price_idr_monthly, 25_000);
    assert_eq!(plans.plans[&PlanId::Power].price_idr_yearly, 250_000);
    assert_eq!(plans.meta.currency, "IDR");
}

#[test]
fn zero_means_unlimited() {
    let plans = shipped();
    assert_eq!(plans.plans[&PlanId::Free].workspaces, Some(2));
    assert_eq!(plans.plans[&PlanId::Pro].workspaces, None);
    assert_eq!(plans.plans[&PlanId::Pro].chat_msgs_per_day, None);
    assert_eq!(plans.plans[&PlanId::Free].chat_msgs_per_day, Some(20));
    assert_eq!(plans.plans[&PlanId::Power].google_accounts_max, None);
    assert_eq!(plans.plans[&PlanId::Pro].google_accounts_max, Some(1));
}

#[test]
fn plan_limits_are_monotonic_free_le_pro_le_power() {
    let p = shipped();
    let (free, pro, power) = (
        &p.plans[&PlanId::Free],
        &p.plans[&PlanId::Pro],
        &p.plans[&PlanId::Power],
    );

    // A missing value means unlimited, which is >= any concrete value.
    let ge = |lo: Option<u32>, hi: Option<u32>| match (lo, hi) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => b >= a,
    };

    for (field, f, m, pw) in [
        (
            "sources_total",
            free.sources_total,
            pro.sources_total,
            power.sources_total,
        ),
        (
            "storage_mb",
            Some(free.storage_mb),
            Some(pro.storage_mb),
            Some(power.storage_mb),
        ),
        (
            "devices_max",
            Some(free.devices_max),
            Some(pro.devices_max),
            Some(power.devices_max),
        ),
        (
            "web_searches_per_day",
            Some(free.web_searches_per_day),
            Some(pro.web_searches_per_day),
            Some(power.web_searches_per_day),
        ),
    ] {
        assert!(ge(f, m), "{field}: free must not exceed pro");
        assert!(ge(m, pw), "{field}: pro must not exceed power");
    }

    assert!(
        free.credits_per_month <= pro.credits_per_month
            && pro.credits_per_month <= power.credits_per_month,
        "credit grants must be monotonic"
    );

    // Boolean features must be monotonic too.
    assert!(!free.agent_actions && pro.agent_actions && power.agent_actions);
    assert!(!free.byok && !pro.byok && power.byok);
    assert!(!free.quick_capture && pro.quick_capture && power.quick_capture);
    assert!(!free.google_calendar && pro.google_calendar && power.google_calendar);
    assert!(!free.task_ai_breakdown && pro.task_ai_breakdown && power.task_ai_breakdown);
    assert!(!free.advanced_analytics && !pro.advanced_analytics && power.advanced_analytics);
}

#[test]
fn credit_costs_match_the_spec() {
    let c = shipped().credits;
    assert_eq!(c.cost(CreditAction::ChatMessage), 1);
    assert_eq!(c.cost(CreditAction::WebSearch), 2);
    assert_eq!(c.cost(CreditAction::FlashcardBatch10), 4);
    assert_eq!(c.cost(CreditAction::Quiz10), 6);
    assert_eq!(c.cost(CreditAction::ReportOutline), 3);
    assert_eq!(c.cost(CreditAction::ReportSection), 6);
    assert_eq!(c.cost(CreditAction::TaskBreakdown), 2);
    assert_eq!(c.cost(CreditAction::SyllabusImport), 8);
    assert_eq!(c.cost(CreditAction::PlanGeneration), 2);
    assert_eq!(c.cost(CreditAction::AgentTurn), 2);
}

#[test]
fn long_document_operations_scale_with_input_tokens() {
    let c = shipped().credits;
    // At or below the threshold: base cost only.
    assert_eq!(c.long_doc_cost(CreditAction::ChatMessage, 20_000), 1);
    assert_eq!(c.long_doc_cost(CreditAction::ChatMessage, 5_000), 1);
    // Above it: base + 1 credit per started 1k tokens over the threshold.
    assert_eq!(c.long_doc_cost(CreditAction::ChatMessage, 22_500), 1 + 3);
    assert_eq!(c.long_doc_cost(CreditAction::ChatMessage, 20_001), 1 + 1);
    // The surcharge stacks on the action's own base cost.
    assert_eq!(c.long_doc_cost(CreditAction::ReportSection, 21_000), 6 + 1);
}

#[test]
fn a_missing_file_is_a_read_error_not_a_panic() {
    let err = load_plans(Path::new("/nonexistent/plans.toml")).unwrap_err();
    assert!(matches!(err, CoreError::ConfigRead { .. }), "got {err:?}");
}

#[test]
fn unknown_keys_are_rejected() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(
        &path,
        "[meta]\nversion = 1\ncurrency = \"IDR\"\n[credits]\nchat_message = 1\n\
         [plans.free]\nname = \"Free\"\nprice_idr_monthly = 0\nprice_idr_yearly = 0\n\
         student_percent_off = 0\ntypo_workspaces = 2\n",
    )
    .unwrap();

    let err = load_plans(&path).unwrap_err();
    assert!(matches!(err, CoreError::ConfigParse { .. }), "got {err:?}");
}

#[test]
fn validation_requires_free_pro_and_power() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");

    // Derive from the shipped file so the credits/ceiling sections stay valid,
    // then drop every plan section. The file now parses but has no plans, which
    // is what `validate()` must reject.
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    let without_plans: String = shipped
        .lines()
        .take_while(|line| !line.starts_with("[plans."))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !without_plans.contains("[plans."),
        "the plan sections must have been stripped"
    );
    std::fs::write(&path, without_plans).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("free"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn a_nonzero_free_plan_is_rejected() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");

    // Build a complete file from the shipped one, then make free cost money.
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    let mutated = shipped.replace(
        "[plans.free]\nname = \"Free\"\nprice_idr_monthly = 0",
        "[plans.free]\nname = \"Free\"\nprice_idr_monthly = 1000",
    );
    assert_ne!(mutated, shipped, "the substitution must have applied");
    std::fs::write(&path, mutated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("free"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn a_missing_cost_ceiling_is_rejected() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");

    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    let mutated = shipped.replace("power = 6.00", "");
    assert_ne!(mutated, shipped, "the substitution must have applied");
    std::fs::write(&path, mutated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("ceiling"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn a_partially_defined_plan_set_names_the_missing_plan() {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");

    // Keep free and pro, drop power.
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    let truncated: String = shipped
        .lines()
        .take_while(|line| !line.starts_with("[plans.power]"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!truncated.contains("[plans.power]"));
    std::fs::write(&path, truncated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("power"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}
