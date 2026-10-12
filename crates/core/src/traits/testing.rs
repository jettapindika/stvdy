//! Deterministic doubles for the provider traits.
//!
//! These are public, not `#[cfg(test)]`, because every later phase's
//! integration tests reuse them: a hash embedder keeps ingestion tests
//! offline and reproducible, and an in-memory storage keeps them free of a
//! bucket. They must behave like the real thing for the properties a test
//! asserts (unit norm, real 768 dims, NotFound on a missing key).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use http::HeaderMap;
use sha2::Sha256;
use tokio::sync::mpsc;

use super::clock::Clock;
use super::embedder::{EmbedError, Embedder};
use super::llm::{
    ChatRequest, ChatResponse, ChatStream, LlmError, LlmProvider, StreamEvent, ToolCall, Usage,
};
use super::mailer::{Email, MailError, Mailer};
use super::payment::{
    GatewayStatus, Invoice, InvoiceRequest, PaymentError, PaymentProvider, WebhookEvent,
    WebhookKind,
};
use super::push::{PushError, PushMessage, PushPlatform, PushProvider};
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
    /// When set, `stream` yields exactly these fragments for the next call
    /// instead of splitting a response on whitespace.
    ///
    /// Needed because whitespace splitting can never put a citation marker across
    /// a token boundary, and a real provider does exactly that. The streaming
    /// hold-back exists for that case, so a test has to be able to produce it.
    tokens: Option<Vec<String>>,
    /// When set, every call fails with `LlmError::Provider` and this message:
    /// a provider that is down stays down for the duration of a test.
    failure: Option<String>,
    /// When set, this provider *supports tools* and emits exactly these calls,
    /// after any scripted tokens.
    ///
    /// `supports_tools()` reads this field rather than a flag beside it, so the
    /// claim and the emission cannot disagree: a double that says it can call a
    /// tool always has the calls it would make, and the injection test's
    /// anti-vacuity rests on that (A279).
    tools: Option<Vec<ToolCall>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses),
            tokens: None,
            failure: None,
            tools: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Streams exactly these fragments for the next call.
    ///
    /// Use it when the *boundary* is what the test is about: a citation marker
    /// split across two tokens, or a marker that arrives one character at a time.
    /// Concatenating the fragments is the answer the caller sees.
    pub fn with_tokens(tokens: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            tokens: Some(tokens),
            failure: None,
            tools: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// A provider whose every call fails with `LlmError::Provider`.
    pub fn failing(message: &str) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            tokens: None,
            failure: Some(message.to_string()),
            tools: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// A provider that supports tools and emits these calls, in order, after any
    /// scripted tokens. `supports_tools()` is true for this instance.
    ///
    /// This is the double the injection test's anti-vacuity depends on: without
    /// it a "no task was created" assertion passes with the gate deleted, because
    /// nothing ever called a tool (A279). An empty list is a legitimate instance
    /// too — a provider that supports tools and whose model chose not to call
    /// one — which is why the claim is derived from this field and not from the
    /// list being non-empty.
    pub fn tool_calling(calls: Vec<ToolCall>) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            tokens: None,
            failure: None,
            tools: Some(calls),
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

    /// The calls this instance emits, if it is a tool-calling one.
    fn scripted_calls(&self) -> Result<Vec<ToolCall>, LlmError> {
        Ok(self.tools.clone().unwrap_or_default())
    }

    /// A provider that is down stays down: every call fails with the same error.
    fn check_failure(&self) -> Result<(), LlmError> {
        match &self.failure {
            Some(message) => Err(LlmError::Provider(message.clone())),
            None => Ok(()),
        }
    }

    /// The next queued response, or `None` when none is left.
    fn take_response(&self) -> Result<Option<String>, LlmError> {
        let mut responses = self
            .responses
            .lock()
            .map_err(|_| LlmError::Provider("scripted llm lock poisoned".into()))?;
        if responses.is_empty() {
            return Ok(None);
        }
        Ok(Some(responses.remove(0)))
    }
}

/// The error a double returns when its script ran out: a test that asked for one more
/// response than it queued has a bug, and this is the message that names it.
fn exhausted() -> LlmError {
    LlmError::Provider("scripted llm has no queued response left".into())
}

#[async_trait::async_trait]
impl LlmProvider for ScriptedLlm {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn supports_tools(&self) -> bool {
        self.tools.is_some()
    }

    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.record(req)?;
        self.check_failure()?;
        let tool_calls = self.scripted_calls()?;
        let text = match self.take_response()? {
            Some(text) => text,
            // A tool-only turn is a legitimate reply, and the chat route persists
            // its assistant message for exactly this shape (A281): the model asked
            // for a tool and said nothing, which is what the injection fixture's
            // first round looks like.
            None if !tool_calls.is_empty() => String::new(),
            None => return Err(exhausted()),
        };
        Ok(ChatResponse {
            text,
            tool_calls,
            usage: Usage::default(),
        })
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.record(req)?;
        self.check_failure()?;
        let calls = self.scripted_calls()?;

        let scripted: Vec<String> = match &self.tokens {
            Some(tokens) => tokens.clone(),
            None => match self.take_response()? {
                Some(text) => {
                    // One token per whitespace-delimited segment, separators
                    // included, so concatenating the tokens reproduces the scripted
                    // text exactly.
                    text.split_inclusive(char::is_whitespace)
                        .map(str::to_string)
                        .collect()
                }
                None if !calls.is_empty() => Vec::new(),
                None => return Err(exhausted()),
            },
        };
        let tokens = scripted;
        // Capacity covers every token, every call and `Done`, so no send is dropped
        // and no spawned task or runtime is needed: a double must work in a plain
        // test.
        let (sender, receiver) = mpsc::channel(tokens.len() + calls.len() + 1);
        for token in tokens {
            if sender
                .try_send(StreamEvent::Token(token.to_string()))
                .is_err()
            {
                break;
            }
        }
        // The calls follow the text, which is the order a provider streams them in:
        // a model that explains and then asks for a tool.
        for call in calls {
            let _ = sender.try_send(StreamEvent::ToolCall(call));
        }
        let _ = sender.try_send(StreamEvent::Done);
        Ok(receiver)
    }
}

/// A push provider that records every attempt, including a failed one.
///
/// The attempt is recorded even when the send fails, because `reminder_deliveries` records
/// the attempt: a double that hid a failed send would let a test assert a delivery that
/// never happened, or worse, assert that no delivery row was written when one was.
#[derive(Debug)]
pub struct ScriptedPush {
    platform: PushPlatform,
    sends: Mutex<Vec<(String, PushMessage)>>,
    /// When set, every send fails with this error.
    failure: Option<PushError>,
}

impl ScriptedPush {
    /// A provider that succeeds and records what it was asked to send.
    pub fn new(platform: PushPlatform) -> Self {
        Self {
            platform,
            sends: Mutex::new(Vec::new()),
            failure: None,
        }
    }

    /// A provider whose every send fails with `error`, so a test can tell a dead token
    /// from an unreachable vendor.
    pub fn failing(platform: PushPlatform, error: PushError) -> Self {
        Self {
            platform,
            sends: Mutex::new(Vec::new()),
            failure: Some(error),
        }
    }

    /// Every `(token, message)` this double was handed, in order.
    pub fn sends(&self) -> Vec<(String, PushMessage)> {
        self.sends
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl PushProvider for ScriptedPush {
    fn platform(&self) -> PushPlatform {
        self.platform
    }

    async fn send(&self, token: &str, msg: PushMessage) -> Result<(), PushError> {
        let mut sends = self
            .sends
            .lock()
            .map_err(|_| PushError::Transport("scripted push lock poisoned".into()))?;
        sends.push((token.to_string(), msg));
        match &self.failure {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

/// The header `MockPayment` signs with.
pub const MOCK_SIGNATURE_HEADER: &str = "x-mock-signature";

/// The secret [`MockPayment::new`] signs with.
pub const MOCK_SECRET: &[u8] = b"mock-secret";

/// The body of a callback, as a gateway would send it.
///
/// A separate type from [`WebhookEvent`] on purpose: `raw` is not a field a gateway
/// sends, it is the parsed body itself. Deriving `Deserialize` on the event would
/// have made `raw` a required key of the wire format.
#[derive(Debug, serde::Deserialize)]
struct CallbackBody {
    event_id: String,
    kind: WebhookKind,
    order_id: String,
    amount_idr: i64,
    currency: String,
    #[serde(default)]
    paid_at: Option<DateTime<Utc>>,
    #[serde(default)]
    provider_payment_id: Option<String>,
    #[serde(default)]
    fee_idr: Option<i64>,
}

/// A gateway that signs bodies the way a real one would.
///
/// The HMAC is real, and that is the point. The webhook suite asserts that a
/// tampered body is rejected; a double that returned a canned event could not fail,
/// so the assertion would hold no matter what the route did — a vacuous test in the
/// same class as a determinism fixture whose inputs were all distinguishable.
///
/// It is scripted rather than smart: `get_status` pops the next queued status, so a
/// test says what the gateway will report and in what order.
#[derive(Debug)]
pub struct MockPayment {
    secret: Vec<u8>,
    statuses: Mutex<VecDeque<GatewayStatus>>,
    invoices: Mutex<Vec<InvoiceRequest>>,
    refunds: Mutex<Vec<(String, i64)>>,
}

impl MockPayment {
    /// A gateway signing with [`MOCK_SECRET`].
    pub fn new() -> Self {
        Self::with_secret(MOCK_SECRET)
    }

    /// A gateway with its own secret, for the "another gateway cannot sign for this
    /// one" case.
    pub fn with_secret(secret: &[u8]) -> Self {
        Self {
            secret: secret.to_vec(),
            statuses: Mutex::new(VecDeque::new()),
            invoices: Mutex::new(Vec::new()),
            refunds: Mutex::new(Vec::new()),
        }
    }

    /// The headers a real gateway would send for this body, so a test signs the
    /// same bytes it posts and the route's verification is exercised for real.
    pub fn sign(&self, body: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        // `from_str` cannot fail for hex, which is ASCII by construction; if it ever
        // did, the header would be absent and verification would fail closed rather
        // than pass with an empty signature.
        if let Ok(value) = http::HeaderValue::from_str(&hex::encode(self.tag(body))) {
            headers.insert(MOCK_SIGNATURE_HEADER, value);
        }
        headers
    }

    /// The next status `get_status` will report.
    pub fn push_status(&self, status: GatewayStatus) {
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.push_back(status);
        }
    }

    /// Every invoice request this double was handed, in order.
    pub fn invoices(&self) -> Vec<InvoiceRequest> {
        self.invoices
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Every refund note it was handed, in order.
    pub fn refund_notes(&self) -> Vec<(String, i64)> {
        self.refunds
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn tag(&self, body: &[u8]) -> Vec<u8> {
        // HMAC accepts a key of any length, so this cannot fail; the empty tag keeps
        // the function total without a panic, and an empty tag never verifies.
        let Ok(mut mac) = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.secret) else {
            return Vec::new();
        };
        mac.update(body);
        mac.finalize().into_bytes().to_vec()
    }
}

impl Default for MockPayment {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PaymentProvider for MockPayment {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn create_invoice(&self, req: InvoiceRequest) -> Result<Invoice, PaymentError> {
        let invoice = Invoice {
            order_id: req.order_id.clone(),
            // Deterministic and derived from the request, so a test can assert the
            // client was handed *this* invoice rather than merely a non-empty string.
            qr_string: format!("MOCKQRIS:{}:{}", req.order_id, req.amount_idr),
            expires_at: req.expires_at,
            provider_payment_id: Some(format!("mock-{}", req.order_id)),
        };
        let mut invoices = self
            .invoices
            .lock()
            .map_err(|_| PaymentError::Provider("mock payment lock poisoned".into()))?;
        invoices.push(req);
        Ok(invoice)
    }

    async fn get_status(&self, _order_id: &str) -> Result<GatewayStatus, PaymentError> {
        let mut statuses = self
            .statuses
            .lock()
            .map_err(|_| PaymentError::Provider("mock payment lock poisoned".into()))?;
        // An empty queue is an error, not a default `Pending`: a poller test that
        // forgot to script a status must fail, not silently assert that nothing
        // happened.
        statuses
            .pop_front()
            .ok_or_else(|| PaymentError::Provider("mock payment has no queued status left".into()))
    }

    fn parse_and_verify_webhook(
        &self,
        headers: &HeaderMap,
        raw_body: &[u8],
    ) -> Result<WebhookEvent, PaymentError> {
        // Signature first, parse second. The other order would let a tampered body
        // decide which failure the route reports.
        let presented = headers
            .get(MOCK_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|hex_tag| hex::decode(hex_tag).ok())
            .ok_or(PaymentError::Signature)?;
        let Ok(mut mac) = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.secret) else {
            return Err(PaymentError::NotConfigured);
        };
        mac.update(raw_body);
        // `verify_slice` is the constant-time comparison; `==` on the tag would leak
        // the prefix that matched.
        mac.verify_slice(&presented)
            .map_err(|_| PaymentError::Signature)?;

        let raw: serde_json::Value = serde_json::from_slice(raw_body).map_err(|error| {
            PaymentError::Invalid(format!("callback body is not JSON: {error}"))
        })?;
        let body: CallbackBody = serde_json::from_value(raw.clone()).map_err(|error| {
            PaymentError::Invalid(format!("callback body is not an event: {error}"))
        })?;

        Ok(WebhookEvent {
            event_id: body.event_id,
            kind: body.kind,
            order_id: body.order_id,
            amount_idr: body.amount_idr,
            currency: body.currency,
            paid_at: body.paid_at,
            provider_payment_id: body.provider_payment_id,
            fee_idr: body.fee_idr,
            raw,
        })
    }

    async fn refund_note(&self, order_id: &str, amount_idr: i64) -> Result<(), PaymentError> {
        let mut refunds = self
            .refunds
            .lock()
            .map_err(|_| PaymentError::Provider("mock payment lock poisoned".into()))?;
        refunds.push((order_id.to_string(), amount_idr));
        Ok(())
    }
}
