//! Email behind `stvdy_core::traits::mailer::Mailer`.
//!
//! Two implementations: SMTP through `lettre` (A4) and a dev sink that writes
//! one file per message. The sink exists because B3 keeps SMTP credentials an
//! operational blocker, and email verification gates every AI action (A43): a
//! developer who cannot receive mail cannot exercise the product at all.

use stvdy_core::traits::mailer::{Email, MailError, Mailer};

pub fn from_config(
    cfg: &crate::config::MailerConfig,
) -> anyhow::Result<std::sync::Arc<dyn Mailer>> {
    match &cfg.smtp_url {
        Some(url) => Ok(std::sync::Arc::new(SmtpMailer::new(url, cfg.from.clone())?)),
        None => Ok(std::sync::Arc::new(FileSinkMailer::new(
            cfg.dir.clone(),
            cfg.from.clone(),
        ))),
    }
}

/// Writes `<unix-millis>-<local part>.eml` under the mail directory. The file is
/// a complete RFC 5322 message, so it can be opened by a mail client and is a
/// faithful stand-in for what SMTP would have sent.
pub struct FileSinkMailer {
    dir: std::path::PathBuf,
    from: String,
}

impl FileSinkMailer {
    pub fn new(dir: std::path::PathBuf, from: String) -> Self {
        Self { dir, from }
    }
}

#[async_trait::async_trait]
impl Mailer for FileSinkMailer {
    async fn send(&self, msg: Email) -> Result<(), MailError> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|error| MailError::Transport(error.to_string()))?;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        // The local part is sanitized: it is user input and becomes a filename.
        let safe_to: String = msg
            .to
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let path = self.dir.join(format!("{stamp}-{safe_to}.eml"));

        let body = format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}\r\n",
            self.from, msg.to, msg.subject, msg.text
        );
        tokio::fs::write(&path, body)
            .await
            .map_err(|error| MailError::Transport(error.to_string()))?;

        // Not the body: a verification link is a credential.
        tracing::info!(to = %msg.to, subject = %msg.subject, path = %path.display(), "mail written to the dev sink");
        Ok(())
    }
}

pub struct SmtpMailer {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from: String,
}

impl SmtpMailer {
    pub fn new(smtp_url: &str, from: String) -> anyhow::Result<Self> {
        use lettre::transport::smtp::client::Tls;
        use lettre::{AsyncSmtpTransport, Tokio1Executor};

        // The URL carries credentials, so it is never logged.
        let parsed = url::Url::parse(smtp_url)
            .map_err(|error| anyhow::anyhow!("SMTP_URL is not a URL: {error}"))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("SMTP_URL has no host"))?
            .to_string();
        let user = if parsed.username().is_empty() {
            None
        } else {
            Some(parsed.username().to_string())
        };
        let password = parsed.password().map(str::to_string);

        let mut builder = match parsed.scheme() {
            "smtps" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
                .map_err(|error| anyhow::anyhow!("SMTP relay: {error}"))?,
            "smtp" => {
                let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host.clone());
                b = b.port(parsed.port().unwrap_or(587));
                if user.is_some() {
                    // STARTTLS is mandatory on a submission port: sending
                    // credentials in the clear is worse than failing.
                    b = b.tls(Tls::Required(
                        lettre::transport::smtp::client::TlsParameters::new(host.clone())
                            .map_err(|error| anyhow::anyhow!("SMTP TLS parameters: {error}"))?,
                    ));
                }
                b
            }
            other => anyhow::bail!("SMTP_URL scheme must be smtp or smtps, got {other}"),
        };

        if let (Some(user), Some(password)) = (user, password) {
            builder = builder.credentials(
                lettre::transport::smtp::authentication::Credentials::new(user, password),
            );
        }

        Ok(Self {
            transport: builder.build(),
            from,
        })
    }
}

#[async_trait::async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, msg: Email) -> Result<(), MailError> {
        use lettre::{AsyncTransport, Message};

        let builder = Message::builder()
            .from(
                self.from
                    .parse()
                    .map_err(|error| MailError::Invalid(format!("MAIL_FROM: {error}")))?,
            )
            .to(msg
                .to
                .parse()
                .map_err(|error| MailError::Invalid(format!("recipient: {error}")))?)
            .subject(msg.subject.clone());

        let message = match &msg.html {
            Some(html) => {
                use lettre::message::MultiPart;
                builder
                    .multipart(MultiPart::alternative_plain_html(
                        msg.text.clone(),
                        html.clone(),
                    ))
                    .map_err(|error| MailError::Invalid(error.to_string()))?
            }
            None => builder
                .body(msg.text.clone())
                .map_err(|error| MailError::Invalid(error.to_string()))?,
        };

        self.transport
            .send(message)
            .await
            .map_err(|error| MailError::Transport(error.to_string()))?;
        Ok(())
    }
}
