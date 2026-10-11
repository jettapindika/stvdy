//! Deterministic doubles for the provider traits.
//!
//! These are public, not `#[cfg(test)]`, because every later phase's
//! integration tests reuse them: a hash embedder keeps ingestion tests
//! offline and reproducible, and an in-memory storage keeps them free of a
//! bucket. They must behave like the real thing for the properties a test
//! asserts (unit norm, real 768 dims, NotFound on a missing key).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};

use super::clock::Clock;
use super::embedder::{EmbedError, Embedder};
use super::mailer::{Email, MailError, Mailer};
use super::storage::{Storage, StorageError};

/// The dimension the hash embedder reports, matching `chunks.embedding`.
pub const HASH_EMBEDDER_DIM: usize = 768;

/// A clock that never advances.
#[derive(Debug, Clone)]
pub struct FixedClock {
    at: DateTime<Utc>,
}

impl FixedClock {
    pub fn at(ts: DateTime<Utc>) -> Self {
        Self { at: ts }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.at
    }
}

/// Deterministic, dependency-free embedder.
///
/// Same text maps to the same L2-normalised vector, different text to a
/// different one, so retrieval tests can assert ordering without a model
/// download. It is a hash, not semantics: cosine similarity here means nothing
/// about meaning, and no test may read it that way.
#[derive(Debug, Default)]
pub struct HashEmbedder;

impl HashEmbedder {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        HASH_EMBEDDER_DIM
    }

    fn model_id(&self) -> &str {
        "hash-embedder-768"
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| embed_text(t)).collect())
    }
}

fn embed_text(text: &str) -> Vec<f32> {
    let seed = fnv1a(text.as_bytes());
    let mut vector: Vec<f32> = Vec::with_capacity(HASH_EMBEDDER_DIM);
    for index in 0..HASH_EMBEDDER_DIM {
        let mixed = splitmix64(seed.wrapping_add((index as u64).wrapping_mul(0x9e3779b97f4a7c15)));
        // Take the high 53 bits so the value is exact in f64, then map to
        // [-1, 1) to avoid a systematic positive bias.
        let unit = (mixed >> 11) as f64 / (1u64 << 53) as f64;
        vector.push((unit * 2.0 - 1.0) as f32);
    }
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut vector {
            *x /= norm;
        }
    } else if let Some(first) = vector.first_mut() {
        // A zero vector cannot be normalised and would break cosine distance;
        // this is unreachable for a 768-dim hash but keeps the contract total.
        *first = 1.0;
    }
    vector
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Blob storage held in a map, for tests.
///
/// The MIME type is accepted and discarded: nothing downstream reads it back,
/// and keeping it would only be a dead field. Presigned URLs are deterministic
/// and opaque enough to be non-empty; nothing verifies them because no HTTP
/// route reads them back.
#[derive(Default)]
pub struct InMemoryStorage {
    objects: Mutex<HashMap<String, Bytes>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    fn objects(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Bytes>>, StorageError> {
        self.objects
            .lock()
            .map_err(|_| StorageError::Provider("in-memory storage lock poisoned".into()))
    }
}

#[async_trait::async_trait]
impl Storage for InMemoryStorage {
    async fn put(&self, key: &str, body: Bytes, _mime: &str) -> Result<(), StorageError> {
        let mut objects = self.objects()?;
        let _ = objects.insert(key.to_string(), body);
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Bytes, StorageError> {
        let objects = self.objects()?;
        match objects.get(key) {
            Some(body) => Ok(body.clone()),
            None => Err(StorageError::NotFound(format!("object {key} not found"))),
        }
    }

    async fn presign_get(&self, key: &str, ttl: Duration) -> Result<String, StorageError> {
        Ok(presign("get", key, ttl))
    }

    async fn presign_put(&self, key: &str, ttl: Duration) -> Result<String, StorageError> {
        Ok(presign("put", key, ttl))
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let mut objects = self.objects()?;
        let _ = objects.remove(key);
        Ok(())
    }
}

fn presign(method: &str, key: &str, ttl: Duration) -> String {
    format!("memory://{key}?method={method}&ttl={}", ttl.as_secs())
}

/// A mailer that keeps every message instead of sending it.
#[derive(Default)]
pub struct RecordingMailer {
    sent: Mutex<Vec<Email>>,
}

impl RecordingMailer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of the captured messages, oldest first.
    pub fn sent(&self) -> Vec<Email> {
        self.sent
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl Mailer for RecordingMailer {
    async fn send(&self, msg: Email) -> Result<(), MailError> {
        let mut sent = self
            .sent
            .lock()
            .map_err(|_| MailError::Transport("recording mailer lock poisoned".into()))?;
        sent.push(msg);
        Ok(())
    }
}
