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
use tokio::sync::mpsc;

use super::clock::Clock;
use super::embedder::{EmbedError, Embedder};
use super::llm::{
    ChatRequest, ChatResponse, ChatStream, LlmError, LlmProvider, StreamEvent, Usage,
};
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

/// A scripted provider: yields the next queued response per call and records
/// every request it received, so a test can assert on the prompt the model
/// actually saw rather than only on the response path.
///
/// That recorded request is the point. A double that only returns canned text
/// lets a composer bug ship green, which is exactly how a defect reaches the
/// tree: the response path is asserted, the input path is not. Here the input
/// is inspectable.
#[derive(Debug)]
pub struct ScriptedLlm {
    responses: Mutex<Vec<String>>,
    /// When set, every call fails with `LlmError::Provider` and this message:
    /// a provider that is down stays down for the duration of a test.
    failure: Option<String>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses),
            failure: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// A provider whose every call fails with `LlmError::Provider`.
    pub fn failing(message: &str) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            failure: Some(message.to_string()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Every request this double was handed, in order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn record(&self, request: ChatRequest) -> Result<(), LlmError> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| LlmError::Provider("scripted llm lock poisoned".into()))?;
        requests.push(request);
        Ok(())
    }

    fn next_response(&self) -> Result<String, LlmError> {
        if let Some(message) = &self.failure {
            return Err(LlmError::Provider(message.clone()));
        }
        let mut responses = self
            .responses
            .lock()
            .map_err(|_| LlmError::Provider("scripted llm lock poisoned".into()))?;
        if responses.is_empty() {
            return Err(LlmError::Provider(
                "scripted llm has no queued response left".into(),
            ));
        }
        Ok(responses.remove(0))
    }
}

#[async_trait::async_trait]
impl LlmProvider for ScriptedLlm {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn supports_tools(&self) -> bool {
        false
    }

    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.record(req)?;
        let text = self.next_response()?;
        Ok(ChatResponse {
            text,
            tool_calls: Vec::new(),
            usage: Usage::default(),
        })
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.record(req)?;
        let text = self.next_response()?;

        // One token per whitespace-delimited segment, separators included, so
        // concatenating the tokens reproduces the scripted text exactly.
        let tokens: Vec<&str> = text.split_inclusive(char::is_whitespace).collect();
        // Capacity covers every token plus `Done`, so no send is dropped and no
        // spawned task or runtime is needed: a double must work in a plain test.
        let (sender, receiver) = mpsc::channel(tokens.len() + 1);
        for token in tokens {
            if sender
                .try_send(StreamEvent::Token(token.to_string()))
                .is_err()
            {
                break;
            }
        }
        let _ = sender.try_send(StreamEvent::Done);
        Ok(receiver)
    }
}
