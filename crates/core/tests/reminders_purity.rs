//! The reminder engine is a pure function, and "pure" is a property of the source, not of a
//! comment.
//!
//! This is the same technique as `planner_purity.rs`, and it is load-bearing here for a
//! sharper reason than in the planner. The dedupe key is the whole reminder-correctness
//! story: it is what stops one reminder firing on three devices. A key that depends on
//! `HashMap` iteration order, or a plan ordered by an unstable sort, is a key that differs
//! between two runs — so the second device's registration is a *second row* and the student
//! is notified twice, or the first device's row is never matched and the student is notified
//! never. Neither is visible in a behavioural test run once on one machine.
//!
//! `Utc::now` is on the list because a clock read here is a reminder whose time moves between
//! two runs of the same input, which is the same failure wearing a different hat: the
//! server's sweep and a client's registration would disagree about which local date the
//! reminder belongs to. `now` is an argument, exactly as the planner's is.
//!
//! Every name is checked as a substring of the source, comments included, so the file cannot
//! even describe the thing it must not do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// The forbidden names, and why each one is here:
///
///  * `Utc::now`/`Local::now`/`SystemTime` — a clock read makes the plan a function of when
///    it ran, and the key's local date is derived from the fire instant.
///  * `HashMap`/`HashSet` — iteration order is not a contract, and the plan's order is.
///  * `sort_unstable` — the plan's order must be a total order two runs agree on, and an
///    unstable sort is only total on the values it compares; equal `fire_at`s would come out
///    in whatever order the input happened to be in.
///  * `partial_cmp` — an `f64` comparison that can return `None` is a comparison whose order
///    is not total.
const FORBIDDEN: [&str; 7] = [
    "Utc::now",
    "Local::now",
    "SystemTime",
    "HashMap",
    "HashSet",
    "sort_unstable",
    "partial_cmp",
];

#[test]
fn the_reminder_module_has_no_clock_and_no_hash_ordered_container() {
    let source = include_str!("../src/reminders.rs");
    for forbidden in FORBIDDEN {
        assert!(
            !source.contains(forbidden),
            "crates/core/src/reminders.rs contains {forbidden}"
        );
    }
}

/// The sweep is only worth anything if it can fail, so it is run against a planted defect.
///
/// Without this, a typo in a name — `"Hashmap"`, `"sort_unstable "` — would leave the sweep
/// green forever while the module did the thing it forbids. The mutation is a real one: a
/// key that folds the fire instant in is exactly the defect A284 names, and it is the defect
/// the whole file exists to catch.
#[test]
fn the_sweep_fails_on_a_planted_defect() {
    let source = include_str!("../src/reminders.rs");
    for forbidden in FORBIDDEN {
        let planted = format!("{source}\nfn planted() {{ let _ = {forbidden}; }}");
        assert!(
            planted.contains(forbidden),
            "a source containing {forbidden} must trip the sweep"
        );
        assert!(
            !source.contains(forbidden),
            "the real module must not contain {forbidden}"
        );
    }

    // And the shape the sweep exists for: the key must not be built from `fire_at`.
    let defect = "format!(\"{}:{}:{}\", kind.as_str(), subject_key, fire_at)";
    assert!(
        !source.contains(defect),
        "the key must not fold in the fire instant"
    );
    assert!(
        source.contains("format!(\"{}:{}:{}\", kind.as_str(), subject_key, local_date)"),
        "the key is the subject and the local date"
    );
}
