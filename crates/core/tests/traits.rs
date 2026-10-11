//! Invariants of the provider traits and their deterministic doubles. These
//! are the properties every later phase's integration tests lean on, so a
//! regression here silently weakens all of them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use stvdy_core::traits::{
    Clock, Email, Embedder, Mailer, Storage, StorageError,
    testing::{FixedClock, HashEmbedder, InMemoryStorage, RecordingMailer},
};

#[tokio::test]
async fn hash_embedder_is_deterministic_and_unit_norm() {
    let embedder = HashEmbedder::new();

    let first = embedder
        .embed(&["photosynthesis".to_string()])
        .await
        .unwrap();
    let second = embedder
        .embed(&["photosynthesis".to_string()])
        .await
        .unwrap();
    assert_eq!(first, second, "same text must give the same vector");

    assert_eq!(embedder.dim(), 768);
    assert_eq!(first[0].len(), 768, "dim() must match the vector length");

    let norm = first[0].iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "vector must be L2-normalised, got {norm}"
    );

    let other = embedder.embed(&["chlorophyll".to_string()]).await.unwrap();
    assert_ne!(first, other, "different text must give a different vector");
}

/// The default `embed_query` must delegate to `embed`, or an embedder with no
/// query convention would silently embed queries differently from passages and
/// retrieval would degrade with no error.
#[tokio::test]
async fn embed_query_defaults_to_embed() {
    let embedder = HashEmbedder::new();
    let texts = vec!["how does TCP connect".to_string()];

    let query = embedder.embed_query(&texts).await.unwrap();
    let passage = embedder.embed(&texts).await.unwrap();

    assert_eq!(query, passage);
}

#[tokio::test]
async fn in_memory_storage_round_trips_and_reports_not_found() {
    let storage = InMemoryStorage::new();
    let key = "sources/42/paper.pdf";
    let ttl = Duration::from_secs(300);

    assert!(matches!(
        storage.get(key).await,
        Err(StorageError::NotFound(_))
    ));

    storage
        .put(key, Bytes::from_static(b"hello pdf"), "application/pdf")
        .await
        .unwrap();
    assert_eq!(&storage.get(key).await.unwrap()[..], b"hello pdf");

    let get_url = storage.presign_get(key, ttl).await.unwrap();
    let put_url = storage.presign_put(key, ttl).await.unwrap();
    assert!(!get_url.is_empty());
    assert!(!put_url.is_empty());
    assert_ne!(
        get_url, put_url,
        "a GET and a PUT URL must be distinguishable"
    );

    storage.delete(key).await.unwrap();
    assert!(matches!(
        storage.get(key).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn recording_mailer_captures_messages() {
    let mailer = RecordingMailer::new();
    assert!(mailer.sent().is_empty());

    mailer
        .send(Email {
            to: "ada@example.com".to_string(),
            subject: "Verify your email".to_string(),
            text: "follow the link".to_string(),
            html: Some("<a href=\"#\">link</a>".to_string()),
        })
        .await
        .unwrap();
    mailer
        .send(Email {
            to: "grace@example.com".to_string(),
            subject: "Welcome".to_string(),
            text: "hello".to_string(),
            html: None,
        })
        .await
        .unwrap();

    let sent = mailer.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].to, "ada@example.com");
    assert_eq!(sent[0].subject, "Verify your email");
    assert_eq!(sent[0].html.as_deref(), Some("<a href=\"#\">link</a>"));
    assert_eq!(sent[1].to, "grace@example.com");
    assert!(sent[1].html.is_none());
}

#[test]
fn fixed_clock_does_not_advance() {
    let ts: DateTime<Utc> = DateTime::parse_from_rfc3339("2026-10-11T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc);
    let clock = FixedClock::at(ts);

    assert_eq!(clock.now(), ts);
    assert_eq!(
        clock.now(),
        ts,
        "a fixed clock must return the same instant"
    );
}
