//! Roadmap gate 6: Indonesian end to end.
//!
//! The gate exists because both of its failure modes are **silent**. A `tsvector`
//! config that stems (`english`) stops matching Indonesian words while every
//! English test stays green. An embedding prefix that is missing or wrong puts
//! the query in a different region of the space, so the wrong chunk wins and the
//! answer is plausible but ungrounded. Neither shows up as an error, so both are
//! asserted directly here.
//!
//! The suite is written so the *keyword* half runs everywhere (no model needed)
//! and the *vector* half runs wherever the model is cached, printing why it
//! skipped when it is not. A gate that silently skips is not a gate, so the
//! keyword half and the config assertion are unconditional.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{json, signed_up, sse_events, workspace_with_text};

/// A real Indonesian passage about a real subject, not lorem ipsum: the tsvector
/// arm only finds it if Indonesian tokenisation works, and a nonsense fixture
/// would hide a tokenisation bug behind matching nonsense.
const DOKUMEN: &str = "Jaringan komputer adalah kumpulan komputer yang saling terhubung \
untuk berbagi data dan sumber daya. Topologi star menghubungkan setiap komputer ke satu \
perangkat pusat bernama switch. Jika kabel pada satu komputer terputus, komputer lain \
tetap dapat berkomunikasi karena tidak bergantung pada kabel yang sama. Kelebihan \
topologi star adalah kemudahan dalam menambah atau mengurangi perangkat tanpa \
mengganggu jaringan secara keseluruhan.";

/// The answer to this question is in exactly one chunk, and the phrase is
/// distinctive enough that a stemmed query would still match it. That is
/// deliberate: this half of the gate is about the *config*, not about stemming
/// quality, and it has to be decidable without a model.
const PERTANYAAN: &str = "perangkat pusat yang menghubungkan setiap komputer pada topologi star";

/// The **real** embedder, or `None` when the model is not cached.
///
/// The test harness deliberately uses `HashEmbedder`: deterministic, no
/// download. It also ignores the `query: ` prefix E5 requires, so a vector-arm
/// assertion against the harness embedder is vacuous and a missing prefix would
/// pass. Gate 6 exists for exactly that failure, so this half builds the real
/// model and skips loudly when it is absent.
fn real_embedder(
    harness: &TestState,
) -> Option<stvdy_server::integrations::embedder::FastembedEmbedder> {
    let cache = harness.state.config.embedder.cache_dir.clone();
    if !cache
        .join("models--intfloat--multilingual-e5-base")
        .exists()
    {
        eprintln!(
            "SKIPPED (vector arm): no cached model under {}. The keyword arm and the tsvector \
             config assertion still ran, and those are the halves that need no model; run an \
             ingest once to populate the cache and this half runs too.",
            cache.display()
        );
        return None;
    }

    let mut config = harness.state.config.embedder.clone();
    config.cache_dir = cache;
    match stvdy_server::integrations::embedder::FastembedEmbedder::new(&config) {
        Ok(embedder) => Some(embedder),
        Err(error) => {
            eprintln!("SKIPPED (vector arm): the model failed to load: {error}");
            None
        }
    }
}

/// The keyword arm finds the Indonesian chunk with no model at all. This is the
/// half that catches a `tsvector` config mistake.
#[sqlx::test(migrations = "../../migrations")]
async fn the_keyword_arm_retrieves_the_indonesian_chunk(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "indonesia@example.test").await;
    let (workspace_id, _source_id) = workspace_with_text(
        &app,
        &harness,
        &token,
        "Jaringan",
        "Catatan Jaringan",
        DOKUMEN,
    )
    .await;

    let workspace_uuid = uuid::Uuid::parse_str(&workspace_id).unwrap();
    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    // Query the keyword arm directly, so this assertion does not depend on the
    // vector arm having run at all.
    let found: Vec<String> = sqlx::query_scalar(
        "select c.content
           from chunks c
           join sources s on s.id = c.source_id
           cross join lateral (select websearch_to_tsquery('simple', $3) as q) query
          where s.user_id = $1 and s.workspace_id = $2
            and s.deleted_at is null
            and c.tsv @@ query.q
          order by ts_rank(c.tsv, query.q) desc
          limit 5",
    )
    .bind(user_id)
    .bind(workspace_uuid)
    .bind(PERTANYAAN)
    .fetch_all(&harness.state.pool)
    .await
    .unwrap();

    assert!(
        !found.is_empty(),
        "the keyword arm found nothing for an Indonesian query. The usual cause is a \
         tsvector config that stems (english) instead of simple, which fails silently for \
         Indonesian while every English test stays green"
    );
    assert!(
        found
            .iter()
            .any(|content| content.contains("switch") || content.contains("pusat")),
        "the retrieved chunks do not contain the answer: {found:?}"
    );
}

/// The tsvector column is generated with `simple`, asserted against the database
/// rather than trusted to the migration, so a later migration that switched it
/// fails here rather than in production.
#[sqlx::test(migrations = "../../migrations")]
async fn the_tsvector_column_uses_the_simple_config(pool: PgPool) {
    let definition: String = sqlx::query_scalar(
        "select pg_get_expr(d.adbin, d.adrelid)
           from pg_attrdef d
           join pg_attribute a on a.attrelid = d.adrelid and a.attnum = d.adnum
          where d.adrelid = 'chunks'::regclass and a.attname = 'tsv'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(
        definition.contains("'simple'"),
        "chunks.tsv must be generated with the simple config, found: {definition}"
    );
    assert!(
        !definition.contains("'english'"),
        "an english config stems and stops matching Indonesian: {definition}"
    );

    // And the behavior the config is for: a stemmed config would collapse these
    // two words, `simple` keeps them distinct.
    let distinct: bool = sqlx::query_scalar(
        "select to_tsvector('simple', 'menghubungkan') <> to_tsvector('simple', 'hubung')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        distinct,
        "the simple config must not stem Indonesian, or the keyword arm loses recall"
    );
}

/// The whole pipeline: an Indonesian question answered from the Indonesian
/// source, with the citation resolving to that source's page.
#[sqlx::test(migrations = "../../migrations")]
async fn an_indonesian_question_is_answered_with_a_resolved_citation(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "tanya@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(
        &app,
        &harness,
        &token,
        "Jaringan",
        "Catatan Jaringan",
        DOKUMEN,
    )
    .await;

    let user_id: uuid::Uuid = sqlx::query_scalar("select id from users limit 1")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();

    if let Some(embedder) = real_embedder(&harness) {
        use stvdy_core::traits::embedder::Embedder;

        // The vector arm must also find it, which is what catches a missing or
        // wrong E5 query prefix. This runs against the real model: the harness's
        // HashEmbedder would ignore the prefix and pass either way.
        let vectors = embedder
            .embed_query(&[PERTANYAAN.to_string()])
            .await
            .expect("the query embeds");
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), 768, "the configured dimension");

        // The prefix is the property under test, so assert it directly as well:
        // E5 puts a query and a passage in different regions of the space, and
        // the same text embedded both ways must NOT be identical.
        let as_query = embedder
            .embed_query(&[DOKUMEN.to_string()])
            .await
            .expect("embeds");
        let as_passage = embedder
            .embed(&[DOKUMEN.to_string()])
            .await
            .expect("embeds");
        assert_ne!(
            as_query[0], as_passage[0],
            "embed_query must apply the model's query convention; if it does not, a query \
             lands in the passage region and the wrong chunk wins with no error"
        );

        let top: Option<(String, f64)> = sqlx::query_as(
            "select c.content, 1 - (c.embedding <=> $1) as score
               from chunks c
               join sources s on s.id = c.source_id
              where s.user_id = $2 and s.workspace_id = $3 and c.embedding is not null
              order by c.embedding <=> $1
              limit 1",
        )
        .bind(pgvector::Vector::from(vectors[0].clone()))
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&workspace_id).unwrap())
        .fetch_optional(&harness.state.pool)
        .await
        .unwrap();

        let (content, score) = top.expect("the vector arm returned a chunk");
        assert!(
            content.contains("switch") || content.contains("pusat"),
            "the vector arm returned the wrong chunk (score {score:.4}): a missing or wrong \
             query prefix puts the query in a different region of the space and the wrong \
             chunk wins. Got: {content:?}"
        );
    }

    // The answer, streamed through the real route with a scripted model.
    let llm = Arc::new(ScriptedLlm::new(vec![
        "Perangkat pusat itu disebut switch [1].".to_string(),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let (status, events) = sse_events(
        &app,
        &format!("/workspaces/{workspace_id}/chat"),
        &token,
        serde_json::json!({ "message": PERTANYAAN }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events:?}");

    let citations = events
        .iter()
        .find(|event| event.kind() == "citations")
        .map(|event| event.json()["citations"].clone())
        .expect("a citations event");
    let array = citations.as_array().expect("an array");
    assert_eq!(array.len(), 1, "the citation must resolve: {citations}");
    assert_eq!(
        array[0]["source_id"].as_str().unwrap(),
        source_id,
        "the citation must point at the Indonesian source"
    );
    // Pasted text has no page, so the citation carries null rather than an
    // invented page number. A PDF source would carry its real page.
    assert!(array[0]["page"].is_null(), "{citations}");

    // And the prompt carried the Indonesian text, so the model was actually given
    // something to answer from rather than asked to guess.
    let requests = llm.requests();
    let system = &requests[0].messages[0].content;
    assert!(
        system.contains("topologi star") || system.contains("Jaringan komputer"),
        "the Indonesian source must be in the prompt: {system}"
    );

    // Persisted with the resolved citation, so the second device sees the same.
    let (_, thread) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/messages"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[1]["citations"][0]["source_id"].as_str().unwrap(),
        source_id
    );
}
