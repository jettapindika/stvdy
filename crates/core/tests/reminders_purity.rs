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

/// The module's own source, as the compiler sees it.
const SOURCE: &str = include_str!("../src/reminders.rs");

/// The sweep itself: the first forbidden name the source contains, or `None`.
///
/// A function rather than an inline loop so the planted-defect case below runs *the same
/// predicate* the real case does. A sweep that only ever sees the real file cannot be shown to
/// fail, and a sweep that is written twice can have its two copies disagree.
fn forbidden_in(source: &str) -> Option<&'static str> {
    FORBIDDEN.iter().copied().find(|name| source.contains(name))
}

#[test]
fn the_reminder_module_has_no_clock_and_no_hash_ordered_container() {
    assert_eq!(
        forbidden_in(SOURCE),
        None,
        "crates/core/src/reminders.rs contains a name that makes the plan impure"
    );
}

/// One real defect per forbidden name, written as the code that would contain it.
///
/// Deliberately **not** built from [`FORBIDDEN`]: a mutation derived from the list proves only
/// that the list is a list, and a typo in a name — `"Hashmap"`, `"sort_unstable "` — would
/// leave the sweep green forever while the module did the thing it forbids. Each string here
/// is the construct itself, so a misspelled entry fails this case instead of hiding behind it.
const DEFECTS: [(&str, &str); 7] = [
    ("Utc::now", "let _ = Utc::now();"),
    ("Local::now", "let _ = Local::now();"),
    ("SystemTime", "let _ = SystemTime::now();"),
    ("HashMap", "let mut seen = HashMap::new();"),
    ("HashSet", "let mut seen = HashSet::new();"),
    ("sort_unstable", "planned.sort_unstable();"),
    ("partial_cmp", "let _ = left.partial_cmp(&right);"),
];

/// The sweep is only worth anything if it can fail, so it is run against planted defects.
///
/// The mutation is a real one: a plan ordered by an unstable sort or keyed through a
/// `HashMap` is the defect the whole file exists to catch, and it is invisible in a
/// behavioural test that runs once on one machine.
///
/// The assertion is `FORBIDDEN.iter().any(|name| defect.contains(name))` rather than
/// `forbidden_in(planted) == Some(name)`: the latter compares against the *first* match in
/// list order, so it fails whenever an earlier entry also matches — which is not a property of
/// the sweep. This form is the one that catches a misspelled entry: with `"Hashmap"` in the
/// list, the planted `HashMap::new()` contains no forbidden name and this case fails, which is
/// exactly the silent hole the case exists to close.
#[test]
fn the_sweep_fails_on_a_planted_defect() {
    for (name, defect) in DEFECTS {
        assert!(
            FORBIDDEN.contains(&name),
            "`{name}` must be a name the sweep looks for"
        );
        assert!(
            FORBIDDEN.iter().any(|forbidden| defect.contains(forbidden)),
            "a source containing `{defect}` must trip the sweep"
        );
        assert!(
            forbidden_in(defect).is_some(),
            "the planted code itself contains `{name}`"
        );
    }
    assert_eq!(
        forbidden_in(SOURCE),
        None,
        "the real module must be clean for the planted case to mean anything"
    );
}

/// The key is the subject and the user's local date, and the fire instant is **absent** from
/// it — which is the defect A284 names, and the one a single-device behavioural test cannot
/// see.
///
/// The signature is the first half of the argument: `dedupe_key` takes no instant, so the
/// value cannot depend on one. The second half is that the key's *callers* pass the local
/// date and not `fire_at`, so the source is asserted to name the three arguments and the
/// format string, and to contain no key built from the fire instant.
#[test]
fn the_key_is_the_subject_and_the_local_date() {
    let key_body = SOURCE
        .split("pub fn dedupe_key")
        .nth(1)
        .expect("the module has one key function");
    // The body runs to the end of the function, which is the first `}` at column zero.
    let key_body = key_body
        .split("\n}\n")
        .next()
        .expect("a function body ends somewhere");

    assert!(
        key_body.contains("format!(\"{}:{}:{}\", kind.as_str(), subject_key, local_date)"),
        "the key is the kind, the subject and the local date: {key_body}"
    );
    assert!(
        !key_body.contains("fire_at"),
        "the key must not be built from the fire instant: {key_body}"
    );

    // And the deferred case, which is the shape the defect takes in the wild: a deferral moves
    // `fire_at` and must leave the key alone.
    assert!(
        !SOURCE.contains("format!(\"{}:{}:{}\", kind.as_str(), subject_key, fire_at)"),
        "a key folding in the fire instant fires once per device"
    );
}
