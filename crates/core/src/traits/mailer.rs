//! Outbound mail capability.

/// A message to send. `html` is optional: every mail has a text part so it is
/// readable when HTML is stripped or blocked.
#[derive(Debug, Clone)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
}

/// Failure modes for mail.
///
/// `Transport` is an SMTP or network failure; `Invalid` is a malformed address
/// or message and must not be retried.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Invalid(String),
}

/// Sends transactional mail (verification links, later notifications).
#[async_trait::async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, msg: Email) -> Result<(), MailError>;
}
