//! The planner is a pure function, and "pure" is a property of the source, not of a comment.
//!
//! A clock read or a hash-ordered container is a one-line change that no behavioural test has
//! to notice, and the failure is a study week that reshuffles itself between two identical
//! requests: the same goal planned twice gives a different week, so a student who re-opens
//! the planner sees their study blocks move on their own and a replan cannot be diffed
//! against what it replaced. The module uses `BTreeMap`/`BTreeSet` and total orders only, and
//! this test says so mechanically rather than trusting a reviewer to notice.
//!
//! `sort_unstable` is on the list because a plan's block order must be a total order two runs
//! agree on; `partial_cmp` because an `f64` comparison that can return `None` is a comparison
//! whose order is not total. Both names are checked as substrings of the source, comments
//! included, so a file cannot even describe the thing it must not do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn the_planner_module_has_no_clock_and_no_hash_ordered_container() {
    for (name, source) in [
        ("mod.rs", include_str!("../src/planner/mod.rs")),
        ("model.rs", include_str!("../src/planner/model.rs")),
        ("slots.rs", include_str!("../src/planner/slots.rs")),
        ("allocate.rs", include_str!("../src/planner/allocate.rs")),
    ] {
        for forbidden in [
            "Utc::now",
            "Local::now",
            "SystemTime",
            "HashMap",
            "HashSet",
            "sort_unstable",
            "partial_cmp",
        ] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }
}
