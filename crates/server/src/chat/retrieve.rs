//! Retrieval: two arms over `chunks`, merged by RRF.
//!
//! Hybrid because a study app is asked for exact terms as often as for meaning:
//! "what does the syllabus say about UTS" wants the keyword arm, "why does
//! entropy increase" wants the vector arm, and neither alone answers both.
//!
//! Every query joins through `sources`. `chunks` has no `user_id` (migration
//! 0007), so ownership is `chunks.source_id -> sources.user_id`, and a query
//! without that join is a cross-tenant read. The keyword arm uses the `simple`
//! tsvector config the column was generated with: it does not stem, which is what
//! makes Indonesian work.

use sqlx::Row;
use stvdy_core::retrieval::{RankedChunk, ScoredChunk, rrf_merge};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Per-arm candidate count. Larger than the merged limit on purpose: RRF needs
/// each arm to contribute candidates the other missed, and 20 is cheap because
/// both indexes are already there.
pub const ARM_TOP_K: i64 = 20;
/// How many chunks the prompt gets after merging.
pub const MERGED_LIMIT: usize = 12;
/// The RRF smoothing constant. 60 is the value from the original paper and the
/// one every implementation uses; it is not tuned here.
pub const RRF_K: u32 = 60;

/// A retrieved chunk with everything the prompt and the citation need.
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: Uuid,
    pub source_id: Uuid,
    pub source_title: String,
    pub page: Option<i32>,
    pub content: String,
    pub rrf_score: f64,
}

/// Hybrid retrieval for one query.
///
/// `source_ids` narrows both arms to the user's selected sources; an empty slice
/// means the whole workspace. Both arms are scoped to the workspace and the
/// caller, so a workspace id from another account returns nothing rather than
/// someone else's text.
pub async fn retrieve(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    source_ids: &[Uuid],
) -> AppResult<Vec<RetrievedChunk>> {
    let vector_arm = vector_search(state, user_id, workspace_id, query, source_ids).await?;
    let keyword_arm = keyword_search(state, user_id, workspace_id, query, source_ids).await?;

    let merged = rrf_merge(&[vector_arm, keyword_arm], RRF_K, MERGED_LIMIT);
    if merged.is_empty() {
        return Ok(Vec::new());
    }

    hydrate(state, user_id, &merged).await
}

/// The vector arm. The query is embedded through the `Embedder` with the `query:`
/// prefix (A2): E5 puts a query and a passage in different regions of the space,
/// and embedding a query without its prefix degrades retrieval silently rather
/// than erroring.
async fn vector_search(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    source_ids: &[Uuid],
) -> AppResult<Vec<RankedChunk>> {
    let vectors = state
        .embedder
        .embed_query(&[query.to_string()])
        .await
        .map_err(|error| AppError::Provider(format!("embedding the query: {error}")))?;
    let Some(vector) = vectors.into_iter().next() else {
        return Err(AppError::Provider(
            "the embedder returned no vector for the query".to_string(),
        ));
    };

    let rows = sqlx::query(
        // `<=>` is pgvector's cosine distance, which is what the HNSW index is
        // built for (`vector_cosine_ops`). `1 - distance` is the similarity.
        "select c.id, c.source_id, c.page, 1 - (c.embedding <=> $1) as score
           from chunks c
           join sources s on s.id = c.source_id
          where s.user_id = $2
            and s.workspace_id = $3
            and s.deleted_at is null
            and c.embedding is not null
            and (cardinality($5::uuid[]) = 0 or c.source_id = any($5))
          order by c.embedding <=> $1
          limit $4",
    )
    .bind(pgvector::Vector::from(vector))
    .bind(user_id)
    .bind(workspace_id)
    .bind(ARM_TOP_K)
    .bind(source_ids)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("the vector arm: {error}")))?;

    Ok(rows
        .into_iter()
        .map(|row| RankedChunk {
            chunk_id: row.get("id"),
            source_id: row.get("source_id"),
            page: row.get("page"),
            score: row.get::<f64, _>("score") as f32,
        })
        .collect())
}

/// The keyword arm.
///
/// `websearch_to_tsquery` rather than `to_tsquery`: it accepts what a person
/// actually types (quoted phrases, `or`, a stray `-`) without erroring on the
/// punctuation a user will inevitably include. `ts_rank` orders it.
async fn keyword_search(
    state: &AppState,
    user_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    source_ids: &[Uuid],
) -> AppResult<Vec<RankedChunk>> {
    // An all-punctuation query yields an empty tsquery, and `@@` with an empty
    // query matches nothing, so the arm correctly returns nothing rather than
    // erroring.
    let rows = sqlx::query(
        // The cast is required: `ts_rank` returns `real` (float4), and sqlx will
        // not decode float4 into f64. Caught by the source-filter test.
        "select c.id, c.source_id, c.page, ts_rank(c.tsv, q.query)::float8 as score
           from chunks c
           join sources s on s.id = c.source_id
           cross join lateral (select websearch_to_tsquery('simple', $4) as query) q
          where s.user_id = $1
            and s.workspace_id = $2
            and s.deleted_at is null
            and c.tsv @@ q.query
            and (cardinality($5::uuid[]) = 0 or c.source_id = any($5))
          order by score desc, c.id
          limit $3",
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(ARM_TOP_K)
    .bind(query)
    .bind(source_ids)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("the keyword arm: {error}")))?;

    Ok(rows
        .into_iter()
        .map(|row| RankedChunk {
            chunk_id: row.get("id"),
            source_id: row.get("source_id"),
            page: row.get("page"),
            score: row.get::<f64, _>("score") as f32,
        })
        .collect())
}

/// Loads the text and the source title for the merged ids.
///
/// A second query rather than joining in both arms: the arms only need ids and
/// ranks to merge, and fetching the content twice would double the bytes read for
/// the candidates the merge then discards.
async fn hydrate(
    state: &AppState,
    user_id: Uuid,
    merged: &[ScoredChunk],
) -> AppResult<Vec<RetrievedChunk>> {
    let ids: Vec<Uuid> = merged.iter().map(|chunk| chunk.chunk_id).collect();

    let rows = sqlx::query(
        "select c.id, c.content, s.title
           from chunks c
           join sources s on s.id = c.source_id
          where c.id = any($1) and s.user_id = $2 and s.deleted_at is null",
    )
    .bind(&ids)
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading chunk text: {error}")))?;

    let mut by_id: std::collections::HashMap<Uuid, (String, String)> = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<Uuid, _>("id"),
                (
                    row.get::<String, _>("content"),
                    row.get::<String, _>("title"),
                ),
            )
        })
        .collect();

    // The merge order is the ranking order, so it is preserved here rather than
    // re-sorted: RRF's whole output is the order.
    Ok(merged
        .iter()
        .filter_map(|scored| {
            by_id
                .remove(&scored.chunk_id)
                .map(|(content, source_title)| RetrievedChunk {
                    chunk_id: scored.chunk_id,
                    source_id: scored.source_id,
                    source_title,
                    page: scored.page,
                    content,
                    rrf_score: scored.rrf_score,
                })
        })
        .collect())
}

#[cfg(test)]
mod tests {

    /// The production half of this file, with the test module cut off.
    ///
    /// The assertions below forbid certain literals, and a scan of the whole file
    /// would find them in its own assertion messages and fail on itself. Splitting
    /// at the test module is what makes them honest.
    fn production_source() -> &'static str {
        let whole = include_str!("retrieve.rs");
        match whole.find("#[cfg(test)]") {
            Some(index) => &whole[..index],
            None => whole,
        }
    }

    #[test]
    fn the_keyword_arm_uses_the_simple_config() {
        // The column is generated with `to_tsvector('simple', content)`
        // (migration 0007). `simple` does not stem, which is what makes
        // Indonesian match; switching the query to `english` would silently stop
        // Indonesian from being found while every English test stayed green. This
        // asserts the config in the source rather than trusting the column.
        let source = production_source();
        assert!(
            source.contains("websearch_to_tsquery('simple'"),
            "the keyword arm must query with the same config the column uses"
        );
        // The forbidden literal is built at run time so this assertion cannot
        // match its own text.
        assert!(
            !source.contains(&format!("websearch_to_tsquery('{}'", "english")),
            "the keyword arm must not stem: gate 6 depends on it"
        );
    }

    #[test]
    fn every_retrieval_query_is_scoped_through_sources() {
        // `chunks` has no `user_id`, so a query that forgets the join through
        // `sources` is a cross-tenant read. Asserted on the source text because
        // this is the mistake that is invisible in review.
        let source = production_source();
        let joins = source
            .matches("join sources s on s.id = c.source_id")
            .count();
        assert!(
            joins >= 3,
            "each arm and the hydration query must join through sources, found {joins}"
        );
        assert_eq!(
            source.matches("s.user_id =").count(),
            joins,
            "every join through sources must also be scoped by the caller"
        );
    }
}
