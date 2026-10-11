//! Tests for the plans config loader. The shipped `config/plans.toml` is the
//! fixture, so a typo in it fails the build rather than production.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use stvdy_core::config::{CreditAction, Limit, PlanId, load_plans};
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
fn limits_distinguish_unlimited_from_a_real_zero() {
    let plans = shipped();

    assert_eq!(plans.plans[&PlanId::Free].workspaces, Limit::Max(2));
    assert_eq!(plans.plans[&PlanId::Pro].workspaces, Limit::Unlimited);
    assert_eq!(
        plans.plans[&PlanId::Pro].chat_msgs_per_day,
        Limit::Unlimited
    );
    assert_eq!(plans.plans[&PlanId::Free].chat_msgs_per_day, Limit::Max(20));

    // The case a `0 = unlimited` sentinel got wrong: free allows ZERO Google
    // accounts (ICS import only), power allows unlimited. A sentinel made free
    // read as unlimited and left the separate boolean to keep it honest.
    assert_eq!(
        plans.plans[&PlanId::Free].google_accounts_max,
        Limit::Max(0)
    );
    assert_eq!(plans.plans[&PlanId::Pro].google_accounts_max, Limit::Max(1));
    assert_eq!(
        plans.plans[&PlanId::Power].google_accounts_max,
        Limit::Unlimited
    );

    // `allows` is the check the entitlements engine will call.
    assert!(!plans.plans[&PlanId::Free].google_accounts_max.allows(0));
    assert!(plans.plans[&PlanId::Pro].google_accounts_max.allows(0));
    assert!(!plans.plans[&PlanId::Pro].google_accounts_max.allows(1));
    assert!(plans.plans[&PlanId::Power].google_accounts_max.allows(99));
}

#[test]
fn plan_limits_are_monotonic_free_le_pro_le_power() {
    let p = shipped();
    let (free, pro, power) = (
        &p.plans[&PlanId::Free],
        &p.plans[&PlanId::Pro],
        &p.plans[&PlanId::Power],
    );

    // Unlimited is greater than any concrete value.
    let le = |lo: Limit, hi: Limit| match (lo, hi) {
        (_, Limit::Unlimited) => true,
        (Limit::Unlimited, Limit::Max(_)) => false,
        (Limit::Max(a), Limit::Max(b)) => b >= a,
    };

    for (field, f, m, pw) in [
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
    ] {
        assert!(le(f, m), "{field}: free must not exceed pro");
        assert!(le(m, pw), "{field}: pro must not exceed power");
    }

    for (field, f, m, pw) in [
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
        assert!(f <= m, "{field}: free must not exceed pro");
        assert!(m <= pw, "{field}: pro must not exceed power");
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
    // Build from the shipped config so the ONLY problem is the typo. A
    // hand-written partial fixture fails on missing fields first, which would
    // make this test pass even with `deny_unknown_fields` removed. Verified by
    // deleting the attribute and watching this test still pass.
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    let mutated = shipped.replace("devices_max = 5", "device_max = 5");
    assert_ne!(mutated, shipped, "the substitution must have applied");

    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plans.toml");
    std::fs::write(&path, mutated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigParse { source, .. } => {
            let message = source.to_string();
            // Naming the key is what proves the failure came from the typo and
            // not from something else in the file.
            assert!(
                message.contains("device_max"),
                "the error must name the unknown key, got: {message}"
            );
        }
        other => panic!("expected ConfigParse, got {other:?}"),
    }
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

#[test]
fn a_regressing_limit_is_rejected_at_boot() {
    // The check lives in validate() so a mis-edited config fails at startup with
    // the offending plan pair named, rather than only in CI.
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    // pro's sources_total drops below free's 20.
    let mutated = shipped.replace("sources_total = 150", "sources_total = 5");
    assert_ne!(mutated, shipped);

    let dir = scratch_dir();
    let path = dir.join("plans.toml");
    std::fs::write(&path, mutated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => {
            assert!(msg.contains("sources_total"), "got: {msg}");
            assert!(msg.contains("free"), "must name the plan pair: {msg}");
        }
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn a_feature_enabled_on_a_cheaper_plan_only_is_rejected() {
    // byok is power-only. Flip pro's existing value rather than inserting a
    // second key, which would be a TOML duplicate-key parse error instead.
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    let pro_start = shipped.find("[plans.pro]").expect("pro section");
    let power_start = shipped.find("[plans.power]").expect("power section");
    let (head, rest) = shipped.split_at(pro_start);
    let (pro_block, tail) = rest.split_at(power_start - pro_start);

    let mutated_pro = pro_block.replace("byok = false", "byok = true");
    assert_ne!(mutated_pro, pro_block, "the substitution must have applied");
    let mutated = format!("{head}{mutated_pro}{tail}");

    let dir = scratch_dir();
    let path = dir.join("plans.toml");
    std::fs::write(&path, mutated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("byok"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("stvdy-cfg-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn model_tier_never_gets_cheaper_as_the_plan_rises() {
    // A41: free is served by the cheapest model, pro by cheap, power by best.
    // `ModelTier` derives Ord precisely so this is a comparison rather than an
    // enumerated pair check that could silently miss a case.
    use stvdy_core::config::ModelTier;

    let plans = shipped();
    let free = plans.plans[&PlanId::Free].model_tier;
    let pro = plans.plans[&PlanId::Pro].model_tier;
    let power = plans.plans[&PlanId::Power].model_tier;

    assert_eq!(free, ModelTier::Cheapest);
    assert_eq!(pro, ModelTier::Cheap);
    assert_eq!(power, ModelTier::Best);
    assert!(free <= pro && pro <= power);
    assert!(ModelTier::Cheapest < ModelTier::Cheap);
    assert!(ModelTier::Cheap < ModelTier::Best);
}

#[test]
fn a_cheaper_model_on_a_higher_plan_is_rejected() {
    let shipped = std::fs::read_to_string(PLANS_PATH).unwrap();
    // power downgraded below pro.
    let mutated = shipped.replace("model_tier = \"best\"", "model_tier = \"cheapest\"");
    assert_ne!(mutated, shipped);

    let dir = scratch_dir();
    let path = dir.join("plans.toml");
    std::fs::write(&path, mutated).unwrap();

    match load_plans(&path).unwrap_err() {
        CoreError::ConfigInvalid(msg) => assert!(msg.contains("model_tier"), "got: {msg}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

#[test]
fn the_whole_config_round_trips_through_json() {
    // `plans.entitlements` mirrors the config for auditing (the spec requires the
    // blob to match plans.toml). A `Limit` whose Serialize and Deserialize
    // disagree stored `{"Max":2}`. A blob its own reader rejects. And nothing
    // round-tripped, so no test caught it. This asserts the symmetry for the
    // entire file, which is what the audit snapshot actually serialises.
    let plans = shipped();

    let json = serde_json::to_value(&plans).expect("serialize");
    // Spot-check the shape: an integer and the literal string, not enum wrappers.
    let free = &json["plans"]["free"];
    assert_eq!(free["workspaces"], 2);
    assert_eq!(free["google_accounts_max"], 0);
    assert_eq!(json["plans"]["power"]["workspaces"], "unlimited");
    assert_eq!(json["plans"]["free"]["model_tier"], "cheapest");

    let back: stvdy_core::config::PlansFile =
        serde_json::from_value(json.clone()).expect("deserialize what we just serialized");
    // Compare the re-serialized form rather than the struct: it proves the same
    // fixpoint without requiring PartialEq on every nested type.
    assert_eq!(
        serde_json::to_value(&back).expect("re-serialize"),
        json,
        "the config must round-trip losslessly"
    );
}
