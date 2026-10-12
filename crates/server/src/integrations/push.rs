//! Push delivery: APNs and FCM behind `stvdy_core::traits::push::PushProvider`.
//!
//! Same shape as [`crate::integrations::mailer`]: a provider behind the core trait, selected
//! by configuration, so a deployment with no APNs key still runs — it simply has no push
//! channel, and [`PushRouter::platforms`] says so rather than a delivery failing at 03:00
//! (A288).
//!
//! Three things here are load-bearing and each has a test:
//!
//!  * **The error classification.** [`PushError::Unregistered`] is the only variant that
//!    deletes a `push_tokens` row, and it is the only one that should: a dead token retried
//!    forever is a notification the student never gets, while a timeout that deleted a token
//!    would unregister a phone that was merely asleep. So APNs' `410 Unregistered` and FCM's
//!    `UNREGISTERED` are distinguished from a 503, deliberately and by status **and** body.
//!  * **The payloads are values, not inline literals.** An APNs `aps` dictionary and an FCM v1
//!    `message` are asserted field by field, including the `reminder_id` data key the client
//!    reads to suppress a notification it has already shown (A285).
//!  * **APNs is HTTP/2-only.** The workspace builds `reqwest` with `default-features = false`,
//!    so the `http2` feature is named in the workspace manifest (A288) — without it the
//!    request is HTTP/1.1 and APNs answers a `BadRequest` that explains nothing.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use stvdy_core::traits::push::{PushError, PushMessage, PushPlatform, PushProvider};

use crate::config::{ApnsConfig, FcmConfig, PushConfig};

/// The APNs provider-API host. The sandbox host is a different certificate chain, so a build
/// pointing at the wrong one gets a 403 that reads like a bad key.
const APNS_PRODUCTION_HOST: &str = "https://api.push.apple.com";
const APNS_SANDBOX_HOST: &str = "https://api.sandbox.push.apple.com";

/// FCM v1's send endpoint, per project.
const FCM_HOST: &str = "https://fcm.googleapis.com";

/// How long an OAuth2 access token is treated as valid. Google issues them for an hour; this
/// is deliberately shorter, so a token is never used in its last minute.
const OAUTH_TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

/// How long an APNs provider token is reused.
///
/// Apple asks that a provider token not be regenerated more than once every twenty minutes and
/// refuses more than one an hour on some accounts, so a per-request token would be rejected
/// under load. Fifty minutes is inside Apple's window and well inside the token's own
/// one-hour validity.
const PROVIDER_TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

/// The two transports, or neither.
///
/// `None` on a side means the channel is not configured, which is a supported deployment: a
/// dev box with no APNs key still delivers in-app and email reminders, and the settings screen
/// offers only the channels that will actually fire.
///
/// The fields are trait objects rather than the concrete `ApnsPush`/`FcmPush` for the same
/// reason `Providers` holds `Arc<dyn Mailer>`: a test replaces one with `ScriptedPush` and
/// exercises the delivery path without a network, which is the only way the job handler's
/// fan-out can be asserted at all.
#[derive(Default)]
pub struct PushRouter {
    apns: Option<Arc<dyn PushProvider>>,
    fcm: Option<Arc<dyn PushProvider>>,
}

impl PushRouter {
    /// A router over explicit providers, for a test or a deployment that wires its own.
    ///
    /// `None` means that platform has no transport, which is the same meaning the configured
    /// constructor gives it.
    pub fn with(apns: Option<Arc<dyn PushProvider>>, fcm: Option<Arc<dyn PushProvider>>) -> Self {
        Self { apns, fcm }
    }

    /// Builds whichever transports the configuration names.
    ///
    /// A configured transport whose client cannot be built is **not** a boot failure, unlike
    /// the payment gateway: a push key that fails to parse costs one channel on one platform,
    /// and refusing to start would take down chat and sync for a notification. The error is
    /// logged at `warn` so a deploy with a typo'd key is visible rather than silent.
    pub fn from_config(cfg: &PushConfig) -> Self {
        let apns = cfg.apns.as_ref().and_then(|apns| match ApnsPush::new(apns) {
            Ok(push) => Some(Arc::new(push) as Arc<dyn PushProvider>),
            Err(error) => {
                tracing::warn!(%error, "APNs is configured but could not be built; push to iOS is off");
                None
            }
        });
        let fcm = cfg.fcm.as_ref().and_then(|fcm| match FcmPush::new(fcm) {
            Ok(push) => Some(Arc::new(push) as Arc<dyn PushProvider>),
            Err(error) => {
                tracing::warn!(%error, "FCM is configured but could not be built; push to Android is off");
                None
            }
        });
        Self { apns, fcm }
    }

    /// The transport for a `push_tokens.provider` value, or `None` when that vendor is not
    /// configured.
    pub fn for_provider(&self, provider: &str) -> Option<Arc<dyn PushProvider>> {
        match provider {
            "apns" => self.apns.clone(),
            "fcm" => self.fcm.clone(),
            _ => None,
        }
    }

    /// The transport for a token row's platform.
    pub fn for_platform(&self, platform: PushPlatform) -> Option<Arc<dyn PushProvider>> {
        match platform {
            PushPlatform::Apns => self.apns.clone(),
            PushPlatform::Fcm => self.fcm.clone(),
        }
    }

    /// The platforms with a configured transport.
    ///
    /// The input to `channels_available` (A288): a platform absent here must not be offered on
    /// the settings screen, because a channel that is granted, selectable and silently never
    /// fires is worse than one that is visibly not available.
    pub fn platforms(&self) -> Vec<PushPlatform> {
        let mut platforms = Vec::new();
        if self.apns.is_some() {
            platforms.push(PushPlatform::Apns);
        }
        if self.fcm.is_some() {
            platforms.push(PushPlatform::Fcm);
        }
        platforms
    }

    /// Whether any push transport exists at all.
    pub fn is_configured(&self) -> bool {
        self.apns.is_some() || self.fcm.is_some()
    }
}

/// The `push_tokens.provider` value for a platform, which is also the string
/// [`PushRouter::for_provider`] matches on.
pub fn provider_str(platform: PushPlatform) -> &'static str {
    match platform {
        PushPlatform::Apns => "apns",
        PushPlatform::Fcm => "fcm",
    }
}

/// APNs' provider API: HTTP/2, token authentication, one request per device token.
pub struct ApnsPush {
    key_id: String,
    team_id: String,
    topic: String,
    host: String,
    signer: jsonwebtoken::EncodingKey,
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl ApnsPush {
    pub fn new(cfg: &ApnsConfig) -> anyhow::Result<Self> {
        use secrecy::ExposeSecret;
        let signer = jsonwebtoken::EncodingKey::from_ec_pem(cfg.key_p8.expose_secret().as_bytes())
            .map_err(|error| {
                anyhow::anyhow!("APNS_KEY_P8 is not a PKCS#8 EC private key: {error}")
            })?;

        Ok(Self {
            key_id: cfg.key_id.clone(),
            team_id: cfg.team_id.clone(),
            topic: cfg.topic.clone(),
            host: if cfg.sandbox {
                APNS_SANDBOX_HOST.to_string()
            } else {
                APNS_PRODUCTION_HOST.to_string()
            },
            signer,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                // The provider API is HTTP/2-only and the workspace builds `reqwest` with
                // `default-features = false` (A288), so the feature is named in the manifest.
                // `http2_prior_knowledge` is what makes the connection HTTP/2 from the first
                // byte rather than negotiating up from HTTP/1.1, which APNs does not offer.
                .http2_prior_knowledge()
                .build()?,
            token: Mutex::new(None),
        })
    }

    /// The provider token: ES256 over `{iss, iat}` with `kid`. Pure, so it is tested.
    ///
    /// `iat` is deliberately `now` and not a rounded value: APNs rejects a token more than an
    /// hour old *or* more than an hour in the future, so a token minted from a skewed clock is
    /// a `403 InvalidProviderToken` that reads like a bad key.
    ///
    /// Public for the same reason `search::academic::parse_openalex` is: the wire shape is the
    /// interesting half of this module, and `tests/push.rs` is where it is asserted. A private
    /// function would leave the shape testable only from inside the module, which is not where
    /// the suite lives.
    pub fn provider_token(&self, now: DateTime<Utc>) -> Result<String, PushError> {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        header.typ = Some("JWT".to_string());
        let claims = json!({ "iss": self.team_id, "iat": now.timestamp() });
        jsonwebtoken::encode(&header, &claims, &self.signer).map_err(|error| {
            PushError::Invalid(format!("signing the APNs provider token: {error}"))
        })
    }

    /// The `aps` payload, so the wire shape is a tested value and not an inline literal.
    ///
    /// The `aps` dictionary carries the visible alert and the collapse key; every other entry
    /// is a data key, and `reminder_id` is the one the client reads to suppress a notification
    /// it has already shown on another device (A285). It is at the top level rather than under
    /// a `data` key because that is where APNs puts custom keys.
    ///
    /// Public so `tests/push.rs` can assert the dictionary field by field rather than trusting
    /// a comment about it.
    pub fn payload(msg: &PushMessage) -> Value {
        let mut aps = json!({ "alert": { "title": msg.title, "body": msg.body } });
        if let Some(badge) = msg.badge {
            aps["badge"] = json!(badge);
        }
        if let Some(collapse_key) = &msg.collapse_key {
            // `thread-id` groups a reminder's notifications, which is what makes a second
            // delivery replace the first rather than stack.
            aps["thread-id"] = json!(collapse_key);
        }

        // Built as a map rather than as an object literal with the data keys spliced in: a
        // `json!` literal followed by an `as_object_mut` is a step that can only fail if the
        // literal changes shape. The data keys go in **first** so the `aps` dictionary cannot
        // be clobbered by a data key of the same name — APNs reads `aps` as the notification
        // itself, and a caller that named a data key `aps` would silently replace the alert
        // with a string.
        let mut payload = serde_json::Map::new();
        for (key, value) in &msg.data {
            payload.insert(key.clone(), json!(value));
        }
        payload.insert("aps".to_string(), aps);
        Value::Object(payload)
    }

    /// APNs' status and reason, classified.
    ///
    /// `410 Unregistered` is the only status that means "this token is dead". `400
    /// BadDeviceToken` and `400 DeviceTokenNotForTopic` also mean the token cannot work, and
    /// Apple's own guidance is to stop using it — so they are `Unregistered` too, because the
    /// alternative is retrying a token that will never deliver. A `403` is *our* problem (a bad
    /// key or topic) and must not delete anything.
    ///
    /// Public because the classification is the one decision this module exists to get right:
    /// `tests/push.rs` names every status that deletes a token and every status that must not.
    pub fn classify(status: u16, reason: Option<&str>) -> PushError {
        match status {
            410 => PushError::Unregistered,
            400 if matches!(reason, Some("BadDeviceToken" | "DeviceTokenNotForTopic")) => {
                PushError::Unregistered
            }
            400 => PushError::Invalid(format!(
                "APNs rejected the payload: {}",
                reason.unwrap_or("no reason given")
            )),
            403 => PushError::Invalid(format!(
                "APNs rejected the provider token or topic: {}",
                reason.unwrap_or("no reason given")
            )),
            _ => PushError::Transport(format!(
                "APNs answered {status}: {}",
                reason.unwrap_or("no reason given")
            )),
        }
    }

    /// The cached provider token, minted when there is none or the cached one is stale.
    fn cached_provider_token(&self, now: DateTime<Utc>) -> Result<String, PushError> {
        let mut guard = self
            .token
            .lock()
            .map_err(|_| PushError::Transport("the provider token lock is poisoned".to_string()))?;
        if let Some((token, minted)) = guard.as_ref() {
            if minted.elapsed() < PROVIDER_TOKEN_TTL {
                return Ok(token.clone());
            }
        }
        let token = self.provider_token(now)?;
        *guard = Some((token.clone(), Instant::now()));
        Ok(token)
    }
}

#[async_trait::async_trait]
impl PushProvider for ApnsPush {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Apns
    }

    async fn send(&self, token: &str, msg: PushMessage) -> Result<(), PushError> {
        let provider_token = self.cached_provider_token(Utc::now())?;
        let mut request = self
            .http
            .post(format!("{}/3/device/{token}", self.host))
            .bearer_auth(provider_token)
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .json(&Self::payload(&msg));
        if let Some(collapse_key) = &msg.collapse_key {
            request = request.header("apns-collapse-id", collapse_key);
        }

        let response = request
            .send()
            .await
            .map_err(|error| PushError::Transport(format!("reaching APNs: {error}")))?;
        let status = response.status().as_u16();
        if status == 200 {
            return Ok(());
        }

        // APNs' error body is `{"reason": "..."}`. A body that does not parse is not a reason
        // to guess: the status alone decides, and the raw text goes in the message.
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let reason = body.get("reason").and_then(Value::as_str);
        Err(Self::classify(status, reason))
    }
}

/// FCM's HTTP v1 API: OAuth2 with a service-account assertion, one request per device token.
pub struct FcmPush {
    project_id: String,
    client_email: String,
    signer: jsonwebtoken::EncodingKey,
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl FcmPush {
    pub fn new(cfg: &FcmConfig) -> anyhow::Result<Self> {
        let signer = jsonwebtoken::EncodingKey::from_rsa_pem(cfg.private_key.as_bytes()).map_err(
            |error| {
                anyhow::anyhow!(
                    "FCM_SERVICE_ACCOUNT_JSON's private_key is not a PKCS#8 RSA key: {error}"
                )
            },
        )?;
        Ok(Self {
            project_id: cfg.project_id.clone(),
            client_email: cfg.client_email.clone(),
            signer,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
            token: Mutex::new(None),
        })
    }

    /// The OAuth2 assertion for `https://oauth2.googleapis.com/token` (RS256).
    ///
    /// Google's own contract: `iss` is the service account's email, `aud` is the token
    /// endpoint, `scope` is the messaging scope, and the token lives an hour. A wrong `aud` is
    /// the classic failure — Google answers `invalid_grant` with no hint that the audience was
    /// the problem — so the value is a named constant rather than a literal at the call site.
    ///
    /// Public so `tests/push.rs` can decode the token and assert the claims, which is the only
    /// way a wrong `aud` is caught before Google answers `invalid_grant` with no hint.
    pub fn assertion(&self, now: DateTime<Utc>) -> Result<String, PushError> {
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let claims = json!({
            "iss": self.client_email,
            "scope": FCM_SCOPE,
            "aud": FCM_TOKEN_ENDPOINT,
            "iat": now.timestamp(),
            "exp": (now + chrono::Duration::seconds(3600)).timestamp(),
        });
        jsonwebtoken::encode(&header, &claims, &self.signer)
            .map_err(|error| PushError::Invalid(format!("signing the FCM assertion: {error}")))
    }

    /// The v1 message body.
    ///
    /// `data` is a map of strings, which is what FCM requires: a non-string value is a
    /// `INVALID_ARGUMENT` that names the field but not the type. `collapse_key` is Android's
    /// own field rather than a top-level one, because on iOS the equivalent is APNs'
    /// `thread-id` and the two are not the same parameter.
    ///
    /// Public so `tests/push.rs` can assert the v1 message shape field by field.
    pub fn payload(token: &str, msg: &PushMessage) -> Value {
        let mut message = serde_json::Map::new();
        message.insert("token".to_string(), json!(token));
        message.insert(
            "notification".to_string(),
            json!({ "title": msg.title, "body": msg.body }),
        );
        message.insert("data".to_string(), json!(msg.data));
        if let Some(collapse_key) = &msg.collapse_key {
            // Android's own field rather than a top-level one: the iOS equivalent is APNs'
            // `thread-id` and the two are not the same parameter, so they cannot share a key.
            message.insert(
                "android".to_string(),
                json!({ "collapse_key": collapse_key }),
            );
        }
        json!({ "message": Value::Object(message) })
    }

    /// FCM's status and body, classified.
    ///
    /// FCM answers `404` with `{"error":{"status":"NOT_FOUND","details":[{"errorCode":
    /// "UNREGISTERED"}]}}` for a token that is gone, and `400 UNREGISTERED` in some client
    /// versions. Both are the same fact. Everything the body does not identify is classified by
    /// status: `5xx` and `429` are transport, and a `401`/`403` is our credentials.
    ///
    /// Public for the same reason as APNs' `classify`: it is the decision that deletes a
    /// `push_tokens` row, and `tests/push.rs` asserts both sides of it.
    pub fn classify(status: u16, body: &Value) -> Option<PushError> {
        if (200..300).contains(&status) {
            return None;
        }

        let error = body.get("error").unwrap_or(body);
        let fcm_status = error.get("status").and_then(Value::as_str);
        let unregistered = fcm_status == Some("UNREGISTERED")
            || error
                .get("details")
                .and_then(Value::as_array)
                .is_some_and(|details| {
                    details.iter().any(|detail| {
                        detail.get("errorCode").and_then(Value::as_str) == Some("UNREGISTERED")
                    })
                });
        if unregistered {
            return Some(PushError::Unregistered);
        }

        let message = fcm_status.unwrap_or("no status in the body").to_string();
        Some(match status {
            400 | 401 | 403 => PushError::Invalid(format!("FCM rejected the message: {message}")),
            _ => PushError::Transport(format!("FCM answered {status}: {message}")),
        })
    }

    /// The OAuth2 access token, exchanged once and reused until it is nearly stale.
    async fn access_token(&self) -> Result<String, PushError> {
        {
            let guard = self.token.lock().map_err(|_| {
                PushError::Transport("the OAuth token lock is poisoned".to_string())
            })?;
            if let Some((token, fetched)) = guard.as_ref() {
                if fetched.elapsed() < OAUTH_TOKEN_TTL {
                    return Ok(token.clone());
                }
            }
        }

        let assertion = self.assertion(Utc::now())?;
        let response = self
            .http
            .post(FCM_TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
            .send()
            .await
            .map_err(|error| {
                PushError::Transport(format!("reaching the FCM token endpoint: {error}"))
            })?;

        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            return Err(PushError::Invalid(format!(
                "the FCM token endpoint answered {status}: {}",
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("no error in the body")
            )));
        }
        let Some(access_token) = body.get("access_token").and_then(Value::as_str) else {
            return Err(PushError::Transport(
                "the FCM token endpoint answered without an access_token".to_string(),
            ));
        };

        let mut guard = self
            .token
            .lock()
            .map_err(|_| PushError::Transport("the OAuth token lock is poisoned".to_string()))?;
        *guard = Some((access_token.to_string(), Instant::now()));
        Ok(access_token.to_string())
    }
}

/// The messaging scope, and the token endpoint that must appear as the assertion's `aud`.
pub const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
pub const FCM_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

#[async_trait::async_trait]
impl PushProvider for FcmPush {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Fcm
    }

    async fn send(&self, token: &str, msg: PushMessage) -> Result<(), PushError> {
        let access_token = self.access_token().await?;
        let response = self
            .http
            .post(format!(
                "{FCM_HOST}/v1/projects/{}/messages:send",
                self.project_id
            ))
            .bearer_auth(access_token)
            .json(&Self::payload(token, &msg))
            .send()
            .await
            .map_err(|error| PushError::Transport(format!("reaching FCM: {error}")))?;

        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        match Self::classify(status, &body) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
