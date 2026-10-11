//! Reciprocal rank fusion for hybrid retrieval.
//!
//! A study app is asked for exact terms ("pasal 5") as often as for meaning
//! ("how do cells get energy"), and neither arm alone covers both: the keyword
//! arm matches the term, the vector arm matches the sense. RRF merges their
//! rankings without needing the scores to be comparable, which they are not
//! (cosine distance and `ts_rank` share no scale). Pure function, no IO, so the
//! merge is unit tested rather than trusted.

use uuid::Uuid;

/// One arm's hit for a chunk.
///
/// `score` is the arm's own score and is *not* used by the merge. It is kept
/// so the admin/debug view can show why a chunk ranked where it did; treating
/// it as comparable across arms is the mistake RRF exists to avoid.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RankedChunk {
    pub chunk_id: Uuid,
    pub source_id: Uuid,
    pub page: Option<i32>,
    pub score: f32,
}

/// A chunk after fusion.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoredChunk {
    pub chunk_id: Uuid,
    pub source_id: Uuid,
    pub page: Option<i32>,
    /// Summed `1/(k + rank)` contributions from every arm that ranked it.
    pub rrf_score: f64,
    /// 0-based indices of the arms that contributed, ascending.
    pub arms: Vec<usize>,
}

/// Reciprocal rank fusion. `k` is the smoothing constant (60 in practice).
///
/// Ranks are 1-based within each arm. A chunk in several arms sums its
/// contributions, which is what makes a chunk both arms agree on outrank one
/// only the vector arm found. A chunk that appears twice in one arm counts
/// once, at its better rank: a duplicate row is an artifact of the query, not
/// evidence the chunk is twice as relevant.
///
/// Ties break by `chunk_id` ascending so the order is total and stable: two
/// runs of the same query must return the same order, or pagination and the
/// golden tests are flaky.
///
/// `limit` truncates *after* scoring, never before: capping the arms first
/// would drop a chunk the other arm would have promoted into the result.
pub fn rrf_merge(arms: &[Vec<RankedChunk>], k: u32, limit: usize) -> Vec<ScoredChunk> {
    // chunk_id -> (score, contributing arms, first-seen chunk metadata)
    let mut merged: Vec<ScoredChunk> = Vec::new();

    for (arm_index, arm) in arms.iter().enumerate() {
        // A chunk listed twice in one arm keeps only its best (first) rank;
        // the arm is ordered, so the first occurrence is the best one.
        let mut seen_in_arm: Vec<Uuid> = Vec::new();
        for (position, ranked) in arm.iter().enumerate() {
            if seen_in_arm.contains(&ranked.chunk_id) {
                continue;
            }
            seen_in_arm.push(ranked.chunk_id);

            let rank = position as u64 + 1;
            let contribution = 1.0 / (k as f64 + rank as f64);

            match merged
                .iter_mut()
                .find(|entry| entry.chunk_id == ranked.chunk_id)
            {
                Some(entry) => {
                    entry.rrf_score += contribution;
                    entry.arms.push(arm_index);
                }
                None => merged.push(ScoredChunk {
                    chunk_id: ranked.chunk_id,
                    source_id: ranked.source_id,
                    page: ranked.page,
                    rrf_score: contribution,
                    arms: vec![arm_index],
                }),
            }
        }
    }

    // Highest score first; equal scores by chunk id, so the order is total.
    merged.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    merged.truncate(limit);
    merged
}
