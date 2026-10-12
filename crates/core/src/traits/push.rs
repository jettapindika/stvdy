//! Push delivery (APNs and FCM).
//!
//! One trait with two transports, so `crates/server/src/jobs/handlers/reminders.rs` fans a
//! reminder out over the channels the student enabled without knowing which vendor is
//! behind either. The platform is on the trait because the token's vendor is a fact about
//! the token row (`push_tokens.provider` is `apns | fcm`), not a fact about the message.
//!
//! `PushError::Unregistered` is separate from the other two on purpose, and the separation
//! is load-bearing: a dead token is the only failure that should delete a
//! `push_tokens` row. A timeout that deleted a token would unregister a device whose phone
//! was merely asleep, and the student would silently stop getting reminders.

/// Which vendor a token belongs to. `push_tokens.provider` holds exactly these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PushPlatform {
    Apns,
    Fcm,
}

/// One notification.
///
/// `data` is a `BTreeMap`, not a `HashMap`, and that is not a style preference: an APNs
/// payload and an FCM `data` block are asserted byte for byte in the tests that follow, and
/// `HashMap`'s iteration order would make those assertions flaky rather than wrong — the
/// worst kind of green, because a rerun turns it red for no reason. The map carries
/// `reminder_id`, which is what lets a client suppress a notification it has already shown
/// on another device (A285).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushMessage {
    pub title: String,
    pub body: String,
    pub data: std::collections::BTreeMap<String, String>,
    /// Replaces an undelivered notification with the same key instead of stacking a
    /// second one. A reminder that fires once should read once.
    pub collapse_key: Option<String>,
    pub badge: Option<u32>,
}

/// Failure modes for a push send.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PushError {
    /// The token is dead (APNs `410 Unregistered`, FCM `UNREGISTERED`). The caller
    /// deletes the row and tries the next device; a transport error must not do that.
    #[error("the push token is no longer registered")]
    Unregistered,
    /// The request was rejected for a reason a retry cannot fix (a bad topic, a malformed
    /// payload). The token is fine.
    #[error("{0}")]
    Invalid(String),
    /// The vendor could not be reached or answered with something retryable. Nothing is
    /// known about the token, so nothing is done to it.
    #[error("{0}")]
    Transport(String),
}

#[async_trait::async_trait]
pub trait PushProvider: Send + Sync {
    fn platform(&self) -> PushPlatform;

    async fn send(&self, token: &str, msg: PushMessage) -> Result<(), PushError>;
}
