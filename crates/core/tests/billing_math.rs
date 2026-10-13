//! The amount a student owes and the period they get, as pure arithmetic.
//!
//! Both halves are here because both halves are money. A quote computed in one place
//! and a charge checked in another is how a customer gets charged one number and
//! activated against a different one, so `price` is the only function that answers
//! "what does this cost" and the settlement is compared against its own stored result.
//!
//! The period cases are the two ways a renewal goes wrong: an extension that starts
//! from today instead of the period end silently deletes the days a student paid for
//! last month, and a fresh period that starts at the *old* end hands a late renewer a
//! period with days already gone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, TimeZone, Utc};
use stvdy_core::billing::{BillingPeriod, DiscountCode, apply_payment, price, upgrade_delta};
use stvdy_core::config::{PlanId, PlansFile, load_plans};
use stvdy_core::error::CoreError;

const PLANS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/plans.toml");

fn shipped() -> PlansFile {
    load_plans(Path::new(PLANS_PATH)).expect("the shipped plans.toml must load")
}

fn at(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
}

/// Paying early must not lose days: the new period starts where the old one ends.
#[test]
fn a_purchase_while_active_extends_from_the_period_end_not_from_today() {
    let end = at(2026, 11, 1);
    // Twelve days early: the mistake this pins would start the new period on the day
    // of payment and quietly delete those twelve days.
    let paid_at = at(2026, 10, 20);

    let monthly = apply_payment(Some(end), paid_at, BillingPeriod::Monthly);
    assert!(monthly.extends);
    assert_eq!(monthly.start, end);
    assert_eq!(monthly.end, at(2026, 12, 1));
    assert_eq!(
        monthly.end - monthly.start,
        Duration::days(30),
        "a renewal buys a full period, not the remainder of one"
    );

    // A yearly period is 365 days, not twelve monthlies.
    let yearly = apply_payment(Some(end), paid_at, BillingPeriod::Yearly);
    assert!(yearly.extends);
    assert_eq!(yearly.start, end);
    assert_eq!(yearly.end, end + Duration::days(365));
}

/// Paying after expiry starts at the payment, not at the old end (or the student
/// pays for the gap they did not have access in).
#[test]
fn an_expired_subscription_restarts_at_paid_at() {
    let paid_at = at(2026, 10, 20);

    // No subscription at all.
    let fresh = apply_payment(None, paid_at, BillingPeriod::Monthly);
    assert!(!fresh.extends);
    assert_eq!(fresh.start, paid_at);
    assert_eq!(fresh.end, at(2026, 11, 19));

    // A lapsed period: `current_period_end` is real and in the past. Extending from it
    // would start the new period five days before the student paid for it.
    let lapsed = apply_payment(Some(at(2026, 10, 15)), paid_at, BillingPeriod::Monthly);
    assert!(!lapsed.extends, "an end in the past is not live paid time");
    assert_eq!(lapsed.start, paid_at);
    assert!(
        lapsed.start >= paid_at,
        "a new period must never start in the past"
    );
    assert_eq!(lapsed.end - lapsed.start, Duration::days(30));

    // The boundary is not live either: a period ending exactly now leaves nothing to
    // extend, and both branches agree on the instant.
    let boundary = apply_payment(Some(paid_at), paid_at, BillingPeriod::Monthly);
    assert!(!boundary.extends);
    assert_eq!(boundary.start, paid_at);
}

/// The student promo and a code are `max(percent)`, never a sum, and the code is
/// named in the breakdown so the invoice can print it.
#[test]
fn a_student_discount_and_a_code_do_not_stack() {
    let plans = shipped();
    let pro = plans.get(PlanId::Pro); // 10 000 IDR, student_percent_off = 50

    let student_only = price(pro, BillingPeriod::Monthly, true, None);
    assert_eq!(student_only.list_idr, 10_000);
    assert_eq!(student_only.percent_off, 50);
    assert_eq!(student_only.discount_idr, 5_000);
    assert_eq!(student_only.due_idr, 5_000);
    assert!(student_only.student_applied);
    assert_eq!(student_only.code_applied, None);

    // A weaker code adds nothing: 50 + 30 is the sum A298 forbids.
    let weaker = DiscountCode {
        code: "LAUNCH30".to_string(),
        percent_off: 30,
    };
    let breakdown = price(pro, BillingPeriod::Monthly, true, Some(&weaker));
    assert_eq!(breakdown.percent_off, 50, "the larger percentage wins");
    assert_eq!(breakdown.discount_idr, 5_000);
    assert_eq!(breakdown.due_idr, 5_000);
    assert!(breakdown.student_applied);
    assert_eq!(
        breakdown.code_applied, None,
        "a code that buys nothing must not be recorded as applied, because a redemption is once per user"
    );

    // A stronger code wins, and is named so the invoice can print it.
    let stronger = DiscountCode {
        code: "LAUNCH70".to_string(),
        percent_off: 70,
    };
    let breakdown = price(pro, BillingPeriod::Monthly, true, Some(&stronger));
    assert_eq!(breakdown.percent_off, 70);
    assert_eq!(breakdown.discount_idr, 7_000);
    assert_eq!(breakdown.due_idr, 3_000);
    assert!(!breakdown.student_applied);
    assert_eq!(breakdown.code_applied.as_deref(), Some("LAUNCH70"));

    // A tie is decided, not left to whichever branch runs first: the student promo
    // needs no redemption, so it wins and the code is not spent for zero benefit.
    let tie = DiscountCode {
        code: "EVEN50".to_string(),
        percent_off: 50,
    };
    let breakdown = price(pro, BillingPeriod::Monthly, true, Some(&tie));
    assert_eq!(breakdown.percent_off, 50);
    assert!(breakdown.student_applied);
    assert_eq!(breakdown.code_applied, None);
    assert_eq!(breakdown.due_idr, 5_000);

    // Without a verified `.ac.id` address the code is the only discount, and the
    // breakdown says so: same percentage, different reason.
    let breakdown = price(pro, BillingPeriod::Monthly, false, Some(&tie));
    assert!(!breakdown.student_applied);
    assert_eq!(breakdown.code_applied.as_deref(), Some("EVEN50"));

    // The free plan has nothing to discount, and a 0% "discount" is not an applied
    // discount.
    let free = price(plans.get(PlanId::Free), BillingPeriod::Monthly, true, None);
    assert_eq!(free.list_idr, 0);
    assert_eq!(free.due_idr, 0);
    assert_eq!(free.percent_off, 0);
    assert!(!free.student_applied);
}

/// A discount can never produce a negative amount: a code row that says 150% is a
/// config mistake, and the safe direction is "free", not "we owe the student".
#[test]
fn a_discount_above_a_hundred_percent_cannot_make_the_amount_negative() {
    let plans = shipped();
    let pro = plans.get(PlanId::Pro);
    let absurd = DiscountCode {
        code: "BROKEN".to_string(),
        percent_off: 150,
    };

    let breakdown = price(pro, BillingPeriod::Monthly, false, Some(&absurd));
    assert_eq!(breakdown.percent_off, 100);
    assert_eq!(breakdown.discount_idr, breakdown.list_idr);
    assert_eq!(breakdown.due_idr, 0);
    assert!(breakdown.due_idr >= 0);
}

/// The price is read from the `PlanConfig` it is handed, and the discount is whole
/// rupiah: `list * percent / 100`, truncated, the same integer arithmetic
/// `upgrade_delta` uses.
#[test]
fn the_price_comes_from_the_plan_it_is_handed_and_truncates_to_the_rupiah() {
    let plans = shipped();
    let mut odd = plans.get(PlanId::Pro).clone();
    odd.price_idr_monthly = 999;

    let code = DiscountCode {
        code: "ODD".to_string(),
        percent_off: 33,
    };
    let breakdown = price(&odd, BillingPeriod::Monthly, false, Some(&code));
    assert_eq!(breakdown.list_idr, 999);
    assert_eq!(breakdown.discount_idr, 329, "999 * 33 / 100 = 329.67");
    assert_eq!(breakdown.due_idr, 670);
    assert_eq!(
        breakdown.discount_idr + breakdown.due_idr,
        breakdown.list_idr
    );
}

/// The upgrade charges the difference for the remaining days only.
#[test]
fn an_upgrade_charges_the_prorated_difference_for_the_remaining_days() {
    let plans = shipped();
    let pro = plans.get(PlanId::Pro);
    let power = plans.get(PlanId::Power);

    // (25 000 - 10 000) * 15 / 30
    assert_eq!(upgrade_delta(pro, power, BillingPeriod::Monthly, 15), 7_500);
    // A full period's difference is the most an upgrade can cost.
    assert_eq!(
        upgrade_delta(pro, power, BillingPeriod::Monthly, 30),
        15_000
    );
    assert_eq!(upgrade_delta(pro, power, BillingPeriod::Monthly, 0), 0);

    // A yearly proration divides by the year's own length.
    assert_eq!(
        upgrade_delta(pro, power, BillingPeriod::Yearly, 100),
        150_000 * 100 / 365
    );

    // A downgrade is not a charge. An upgrade that bills for a downgrade is a refund
    // request, and the same-plan case is the same rule.
    assert_eq!(upgrade_delta(power, pro, BillingPeriod::Monthly, 15), 0);
    assert_eq!(upgrade_delta(pro, pro, BillingPeriod::Monthly, 15), 0);

    // A caller that hands over more remaining days than the period has cannot charge
    // more than one period's difference.
    assert_eq!(
        upgrade_delta(pro, power, BillingPeriod::Monthly, 400),
        15_000
    );
    // Nor can a negative remainder become a credit.
    assert_eq!(upgrade_delta(pro, power, BillingPeriod::Monthly, -5), 0);
}

/// A yearly price is not twelve monthlies; it is the config's number, and the
/// monthly equivalent is never computed (an invented number is the defect).
#[test]
fn the_yearly_price_is_the_config_number_and_nothing_is_derived_from_it() {
    let plans = shipped();

    for plan in [PlanId::Pro, PlanId::Power] {
        let config = plans.get(plan);
        let yearly = price(config, BillingPeriod::Yearly, false, None);
        assert_eq!(yearly.list_idr, config.price_idr_yearly);
        assert_ne!(
            yearly.list_idr,
            config.price_idr_monthly * 12,
            "{plan}: twelve monthlies is a number nobody set"
        );
        let monthly = price(config, BillingPeriod::Monthly, false, None);
        assert_eq!(monthly.list_idr, config.price_idr_monthly);
    }

    assert_eq!(
        price(plans.get(PlanId::Free), BillingPeriod::Yearly, false, None).list_idr,
        0
    );
}

/// The period spellings are the spec's, and an unknown one is `None` rather than a
/// default: a default would sell whichever period the code happened to prefer.
#[test]
fn the_billing_period_spellings_round_trip_and_reject_anything_else() {
    for period in BillingPeriod::ALL {
        assert_eq!(BillingPeriod::parse(period.as_str()), Some(period));
    }
    assert_eq!(
        BillingPeriod::parse("monthly"),
        Some(BillingPeriod::Monthly)
    );
    assert_eq!(BillingPeriod::parse("yearly"), Some(BillingPeriod::Yearly));
    assert_eq!(BillingPeriod::parse("annually"), None);
    assert_eq!(BillingPeriod::parse("Monthly"), None);
    assert_eq!(BillingPeriod::parse(""), None);

    assert_eq!(BillingPeriod::Monthly.days(), 30);
    assert_eq!(BillingPeriod::Yearly.days(), 365);
}

/// A pack price is config, not a literal in the checkout path (A21), and the pack is
/// identified by the credits it grants — so the amounts must be unique and a pack
/// must never be a better per-credit deal than a subscription, or nobody subscribes.
#[test]
fn credit_pack_prices_come_from_the_config_and_never_undercut_a_subscription() {
    let plans = shipped();
    let packs = &plans.credit_packs.packs;
    assert_eq!(packs.len(), 3, "the shipped file sells three packs");

    // The price is the file's number, asserted here as the record of it.
    let small = plans.credit_packs.pack(100).expect("the 100-credit pack");
    assert_eq!(small.price_idr, 3_000);
    assert!(
        plans.credit_packs.pack(101).is_none(),
        "an amount that is not in the file is not a pack"
    );

    // Bigger packs are cheaper per credit, which is the only reason to buy one.
    let rates: Vec<f64> = packs
        .iter()
        .map(|pack| pack.price_idr as f64 / pack.credits as f64)
        .collect();
    assert!(
        packs
            .windows(2)
            .all(|pair| pair[0].credits < pair[1].credits),
        "the packs are ordered by size: {packs:?}"
    );
    assert!(
        rates.windows(2).all(|pair| pair[0] > pair[1]),
        "the per-credit rate improves with size: {rates:?}"
    );

    // A subscription's own rate is the floor: a pack that beats it makes the
    // subscription pointless.
    for plan in [PlanId::Pro, PlanId::Power] {
        let config = plans.get(plan);
        let subscription_rate = config.price_idr_monthly as f64 / config.credits_per_month as f64;
        for (pack, rate) in packs.iter().zip(&rates) {
            assert!(
                *rate > subscription_rate,
                "{plan} is {subscription_rate} IDR/credit and the {}-credit pack is {rate}",
                pack.credits
            );
        }
    }
}

#[test]
fn a_pack_that_grants_nothing_or_charges_nothing_is_rejected() {
    let cases = [
        (
            "credits = 500, price_idr = 12500",
            "credits = 500, price_idr = 0",
            "price",
        ),
        (
            "credits = 100, price_idr = 3000",
            "credits = 0, price_idr = 3000",
            "credits",
        ),
    ];
    for (from, to, needle) in cases {
        let raw = std::fs::read_to_string(PLANS_PATH).expect("plans.toml");
        let mutated = raw.replace(from, to);
        assert_ne!(mutated, raw, "the substitution must have applied: {from}");

        match load_plans(&scratch(&mutated)).unwrap_err() {
            CoreError::ConfigInvalid(message) => {
                assert!(message.contains(needle), "got: {message}");
            }
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }
}

#[test]
fn two_packs_with_the_same_amount_or_no_packs_at_all_are_rejected() {
    let raw = std::fs::read_to_string(PLANS_PATH).expect("plans.toml");

    // Two products a buyer cannot tell apart, and a lookup that answers ambiguously.
    let duplicate = raw.replace(
        "credits = 1000, price_idr = 22000",
        "credits = 100, price_idr = 22000",
    );
    assert_ne!(duplicate, raw, "the substitution must have applied");
    match load_plans(&scratch(&duplicate)).unwrap_err() {
        CoreError::ConfigInvalid(message) => assert!(message.contains("unique"), "got: {message}"),
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }

    // An empty table is a config mistake, not "we sell nothing": the pricing page
    // would render an empty list with no error anywhere.
    let start = raw.find("packs = [").expect("the packs array");
    let end = raw[start..].find(']').expect("the array's close") + start + 1;
    let emptied = format!("{}packs = []{}", &raw[..start], &raw[end..]);
    assert_ne!(emptied, raw);
    match load_plans(&scratch(&emptied)).unwrap_err() {
        CoreError::ConfigInvalid(message) => {
            assert!(message.contains("credit_packs"), "got: {message}");
        }
        other => panic!("expected ConfigInvalid, got {other:?}"),
    }
}

/// A scratch copy, the way the plan-matrix suite does it: `load_plans` takes a path,
/// and mutating the shipped file to test a variant would make the suite
/// order-dependent.
fn scratch(raw: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stvdy-billing-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("plans.toml");
    std::fs::write(&path, raw).expect("writing the scratch config");
    path
}
