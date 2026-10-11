//! RRF merge behaviour. These are the properties the hybrid retrieval contract
//! rests on: a chunk both arms agree on wins, a chunk only one arm found still
//! surfaces, and two runs of the same query return the same order.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use uuid::Uuid;

use stvdy_core::retrieval::{RankedChunk, rrf_merge};

/// A stable id from a small integer, so failures name a chunk a human can map.
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn ranked(n: u128, score: f32) -> RankedChunk {
    RankedChunk {
        chunk_id: id(n),
        source_id: id(1000 + n),
        page: Some(n as i32),
        score,
    }
}

const K: u32 = 60;

#[test]
fn a_chunk_in_both_arms_outranks_one_in_a_single_arm() {
    // `both` is second in the vector arm and first in the keyword arm; `vector`
    // is first in the vector arm only. Agreement must beat a single first place.
    let vector = vec![ranked(1, 0.9), ranked(2, 0.8)];
    let keyword = vec![ranked(2, 12.0)];

    let merged = rrf_merge(&[vector, keyword], K, 10);

    assert_eq!(
        merged[0].chunk_id,
        id(2),
        "the agreed chunk must rank first"
    );
    assert_eq!(merged[0].arms, vec![0, 1]);
    let expected = 1.0 / (K as f64 + 2.0) + 1.0 / (K as f64 + 1.0);
    assert!((merged[0].rrf_score - expected).abs() < 1e-12);
}

#[test]
fn a_chunk_only_in_the_weaker_arm_still_appears() {
    // This is the whole point of hybrid: a chunk the vector arm ranked poorly
    // but the keyword arm matched exactly must survive the merge.
    let vector = vec![ranked(1, 0.9), ranked(2, 0.1)];
    let keyword = vec![ranked(3, 5.0), ranked(2, 4.0)];

    let merged = rrf_merge(&[vector, keyword], K, 10);

    assert!(
        merged.iter().any(|entry| entry.chunk_id == id(3)),
        "a keyword-only hit must not be dropped by the merge"
    );
    assert_eq!(merged.len(), 3);
}

#[test]
fn the_same_input_twice_gives_the_same_output() {
    let arms = || {
        vec![
            vec![ranked(1, 0.9), ranked(2, 0.8), ranked(3, 0.7)],
            vec![ranked(3, 5.0), ranked(1, 4.0), ranked(4, 3.0)],
        ]
    };

    assert_eq!(rrf_merge(&arms(), K, 10), rrf_merge(&arms(), K, 10));
}

#[test]
fn ties_break_by_chunk_id_ascending() {
    // Two chunks each appearing once at rank 1 in their own arm score equally;
    // the order must still be total, so the smaller id comes first.
    let first = vec![ranked(9, 1.0)];
    let second = vec![ranked(4, 1.0)];

    let merged = rrf_merge(&[first, second], K, 10);

    assert_eq!(merged[0].chunk_id, id(4));
    assert_eq!(merged[1].chunk_id, id(9));
}

#[test]
fn limit_truncates_after_scoring_never_before() {
    // `agreed` is second in the keyword arm. If the arms were truncated to
    // `limit` before merging, that second place would be lost and `solo` would
    // win the tie by id. Scoring first is what promotes `agreed`.
    let vector = vec![ranked(1, 0.9)];
    let keyword = vec![ranked(2, 9.0), ranked(1, 8.0)];

    let merged = rrf_merge(&[vector, keyword], K, 1);

    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0].chunk_id,
        id(1),
        "the both-arms chunk must win even when the limit is 1"
    );
}

#[test]
fn an_empty_arm_is_not_an_error() {
    let empty: Vec<RankedChunk> = Vec::new();
    let keyword = vec![ranked(1, 3.0)];

    let merged = rrf_merge(&[empty, keyword], K, 10);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].arms, vec![1]);
}

#[test]
fn no_arms_at_all_is_empty() {
    assert!(rrf_merge(&[], K, 10).is_empty());
}

#[test]
fn a_duplicate_id_inside_one_arm_is_counted_once() {
    // A duplicate row is a query artifact, not evidence of double relevance:
    // the chunk keeps its best rank and one arm contribution.
    let arm = vec![ranked(1, 0.9), ranked(1, 0.1)];

    let merged = rrf_merge(&[arm], K, 10);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].arms, vec![0]);
    assert!((merged[0].rrf_score - 1.0 / (K as f64 + 1.0)).abs() < 1e-12);
}

#[test]
fn limit_zero_returns_nothing() {
    let arm = vec![ranked(1, 1.0)];

    assert!(rrf_merge(&[arm], K, 0).is_empty());
}
