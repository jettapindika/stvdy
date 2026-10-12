use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use secrecy::{ExposeSecret, SecretString};
use stvdy_core::config::{BillingPolicy, PlansFile, load_billing_policy, load_plans};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    Dev,
    Prod,
}

impl Env {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "dev" | "development" | "test" => Ok(Env::Dev),
            "prod" | "production" => Ok(Env::Prod),
            other => anyhow::bail!("APP_ENV must be dev or prod, got {other}"),
        }
    }

    pub fn is_dev(self) -> bool {
        matches!(self, Env::Dev)
    }
}

/// Server configuration. The config files are loaded here and validated before
/// the process binds a port, so a broken plans.toml is a startup failure rather
/// than a 500 on the first request.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub env: Env,
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub jwt_secret: SecretString,
    pub encryption_key_hex: String,
    pub plans: PlansFile,
    pub billing_policy: BillingPolicy,
    pub admin_email: Option<String>,
    /// The deployed web build's origin, used for the production CORS allowlist.
    /// The native shell origins are compiled in; this is the only one that
    /// varies per deployment.
    pub web_origin: Option<String>,
    /// Directory that relative config paths resolve against: the workspace root
    /// in dev, `/app` in a container.
    pub root: PathBuf,

    // --- Phase 1 ---
    pub auth: AuthConfig,
    pub storage: StorageConfig,
    pub mailer: MailerConfig,
    pub embedder: EmbedderConfig,
    /// Where the pdfium shared library lives. `None` means PDF ingestion is
    /// unavailable and the *job* fails with a readable error; a path that is set
    /// but unloadable is a boot failure, because a configured extractor that
    /// silently does nothing is worse than an absent one.
    pub pdfium_lib_path: Option<PathBuf>,
    pub google_login: Option<GoogleLoginConfig>,
    /// The Calendar consent flow's OAuth client (A235/A237). `None` means
    /// `GET /calendar/google/connect` answers 404 rather than redirecting to a
    /// half-configured client, the same rule `google_login` follows. It is a
    /// **different** client from `google_login` on purpose: this one asks for
    /// calendar scopes, and sharing a client would make the login screen ask for
    /// calendar write access.
    pub google_calendar: Option<GoogleCalendarConfig>,
    /// The push transports (A288). Empty when neither vendor is configured.
    pub push: PushConfig,
    /// The chat and generation model. `None` means no provider is configured.
    pub llm: Option<LlmConfig>,
    pub search: SearchConfig,
    /// How long a subscription keeps its paid limits past `current_period_end`
    /// before the rollover expires it (the spec's "3-day grace period
    /// (configurable)"). Read by `jobs::handlers::counters`.
    pub grace_period_days: i64,
    /// The rupiah-per-dollar rate the margin report converts provider cost with (A41).
    ///
    /// A configured input rather than a live lookup, because the report is read for a
    /// *decision* and a rate that moved between two readings of the same month would make two
    /// reports disagree. It is printed in the report so a reader can check the arithmetic
    /// against the rate it used, and a rate that is wrong is an env change rather than a
    /// deploy. Defaults to A41's 17889.
    pub idr_per_usd: i64,
    /// How long an `upsert` change is kept before `compact_change_log` removes it (A184).
    pub change_log_retention_days: i64,
    /// How often a live calendar connection is polled (A238).
    ///
    /// Fifteen minutes: an academic calendar changes a few times a term, and the cost of a
    /// poll is one conditional GET that usually answers `304` — so the interval bounds how
    /// stale a new class can be, not how much the poll costs.
    pub calendar_poll_interval: std::time::Duration,
    /// How long a `delete` change is kept. At least the retention window, because a device
    /// that has been offline for three months must still be told that a row was removed
    /// rather than silently keep it (A184).
    pub change_log_tombstone_retention_days: i64,
    /// LibreOffice's CLI, for the PPTX/DOCX PDF preview (A158).
    ///
    /// `None` follows the same policy as `pdfium_lib_path`: the source still
    /// ingests (its text and slide numbers come from the OOXML itself), the
    /// preview is simply absent, and the ingest job logs one line. An absent
    /// converter must be honest rather than silent, because a source with no
    /// preview and no explanation looks like a scanned document.
    pub soffice_bin: Option<PathBuf>,
    /// The QRIS gateway (Phase 13).
    ///
    /// `None` is a supported deployment, exactly like `renderer`: with no gateway
    /// configured there is nothing to sell, so checkout answers 502 `provider_error`
    /// rather than minting an invoice against a mock. Every field here is
    /// env-driven because **B1 is unresolved** — the gateway has no identity and no
    /// docs, so the entire surface of the unknown is this struct plus
    /// `integrations::payment::mapping` (A291).
    pub qris: Option<QrisConfig>,
    /// The email suffixes that prove a student (A299), e.g. `.ac.id`.
    ///
    /// A list in config rather than a literal in the route: a second campus suffix is an
    /// env change, not a deploy, and the suffix list is the only thing between this
    /// endpoint and being a mail amplifier. The comparison is on the **end** of the
    /// address, so `user@ui.ac.id` matches `.ac.id` and `user@ac.id.evil.example` does not.
    pub student_email_suffixes: Vec<String>,
    /// The origin the print template's assets (KaTeX, the typefaces) are fetched from,
    /// rewritten into the document before it is printed (A339).
    ///
    /// Defaults to `http://127.0.0.1:<BIND_ADDR's port>`, which is what makes a
    /// single-container deployment work with no configuration: the renderer is in the same
    /// container as the route that serves `/assets`. A deployment where Chromium resolves the
    /// advertised address differently — or where the renderer runs beside the server rather
    /// than inside it — sets `ASSETS_BASE_URL` explicitly.
    pub assets_base_url: String,
}

/// The callback signature scheme, as a closed set (A292).
///
/// A closed set rather than a string, and an unknown value is a **startup** error:
/// a default would be a scheme that verifies everything, which is the one failure
/// mode this whole verification exists to prevent. Three shapes cover the plausible
/// gateways — an HMAC in hex, the same in base64, and a plain shared token for one
/// that signs nothing — so the code is finished before the docs arrive without
/// pretending to know which one is right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureScheme {
    /// `HMAC-SHA256(QRIS_CALLBACK_TOKEN, raw_body)`, hex, in `QRIS_SIGNATURE_HEADER`.
    HmacSha256Hex,
    /// The same tag, base64.
    HmacSha256Base64,
    /// The header *is* `QRIS_CALLBACK_TOKEN`, compared in constant time.
    SharedToken,
}

impl SignatureScheme {
    pub const ALL: [SignatureScheme; 3] = [
        SignatureScheme::HmacSha256Hex,
        SignatureScheme::HmacSha256Base64,
        SignatureScheme::SharedToken,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SignatureScheme::HmacSha256Hex => "hmac_sha256_hex",
            SignatureScheme::HmacSha256Base64 => "hmac_sha256_base64",
            SignatureScheme::SharedToken => "shared_token",
        }
    }

    /// `None` for anything not in the closed set: the caller turns that into a boot
    /// failure rather than a default.
    pub fn parse(raw: &str) -> Option<SignatureScheme> {
        Self::ALL.into_iter().find(|scheme| scheme.as_str() == raw)
    }

    /// Whether the scheme needs `QRIS_CALLBACK_TOKEN` as an HMAC key rather than as
    /// the literal header value. Both need the secret; only the message differs.
    pub fn signs_with_hmac(self) -> bool {
        !matches!(self, SignatureScheme::SharedToken)
    }
}

/// Everything about the gateway that is a value rather than a shape.
///
/// The field names are **not** here — they are in
/// `integrations::payment::mapping`, which is the one file B1 changes. This struct
/// is the env half: the URL, the credentials, the scheme, the fee rate, the invoice
/// lifetime and the two switches that decide how a late or lost payment is treated.
#[derive(Debug, Clone)]
pub struct QrisConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub callback_token: SecretString,
    pub signature_scheme: SignatureScheme,
    /// Defaults to `X-Callback-Signature`. The header name is a gateway fact, so it
    /// is config too.
    pub signature_header: String,
    /// An allowlist checked **before** the signature when non-empty (A292).
    pub webhook_ips: Vec<std::net::IpAddr>,
    /// The gateway's percentage fee. Defaults to `0.0` — "unknown, record nothing" —
    /// rather than a plausible-looking rate, because B1 has not told us the rate and
    /// a made-up fee would silently corrupt the margin report (A304).
    pub fee_percent: f64,
    pub fee_fixed_idr: i64,
    pub invoice_ttl_minutes: i64,
    /// A payment confirmed after its invoice expired is honoured when the gateway
    /// says it was paid *before* the expiry (A295).
    pub honor_late_payment: bool,
    pub reconcile_hours: i64,
}

impl QrisConfig {
    /// The gateway fee for `amount_idr`, or `None` when no rate is configured.
    ///
    /// `None` is the shipped default and it means **"unknown, record nothing"**, which
    /// is a different fact from a real zero fee: the margin report must be able to say
    /// it does not know, and a `0` it invented would read as a measurement (A304). The
    /// arithmetic lives here rather than on the client because it is config, not wire —
    /// and there is one copy of it, so a gateway-reported fee and a configured one
    /// cannot be combined two ways.
    pub fn fee_idr_for(&self, amount_idr: i64) -> Option<i64> {
        if self.fee_percent == 0.0 && self.fee_fixed_idr == 0 {
            return None;
        }
        // Whole rupiah, truncated, the same integer convention `billing::price` uses.
        let percent = (amount_idr as f64) * self.fee_percent / 100.0;
        Some(self.fee_fixed_idr + percent as i64)
    }
}

/// Token lifetimes and the verification-link window.
#[derive(Debug, Clone, Copy)]
pub struct AuthConfig {
    pub access_ttl_min: i64,
    pub refresh_ttl_days: i64,
    pub verify_ttl_hours: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBackend {
    /// A directory under the repo. What makes ingestion testable without a
    /// container: A15 removed the pullable MinIO images.
    Local,
    S3,
}

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    /// Local backend only. Relative paths resolve against `AppConfig::root`.
    pub local_dir: PathBuf,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<SecretString>,
    pub force_path_style: bool,
    pub presign_ttl: std::time::Duration,
    /// Origin the local backend's signed URLs are built against, since the
    /// client fetches through this server rather than from a bucket.
    pub public_base_url: String,
    /// HMAC key for the local backend's signed URLs. Derived from the JWT
    /// secret so a dev run needs no extra configuration, and never the same
    /// value twice in prod (the secret is required to be non-default there).
    pub sign_key: SecretString,
}

#[derive(Debug, Clone)]
pub struct MailerConfig {
    /// `None` selects the dev sink, which writes one file per message under
    /// `dir` instead of sending. B3 keeps SMTP credentials an operational
    /// blocker; the sink is what lets Phase 1 be exercised without them.
    pub smtp_url: Option<String>,
    pub from: String,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedderProvider {
    Fastembed,
    Http,
}

#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    pub provider: EmbedderProvider,
    pub model: String,
    /// Asserted equal to `Embedder::dim()` and to the `chunks.embedding` column
    /// dimension at boot (A2).
    pub dim: usize,
    pub cache_dir: PathBuf,
    /// `EMBEDDER=http` only.
    pub base_url: Option<String>,
    pub api_key: Option<SecretString>,
    /// The query convention the endpoint's model needs, e.g. `query: ` for E5.
    /// Empty means none. See `HttpEmbedder`.
    pub query_prefix: String,
}

/// Which search tier or tiers answer (A3).
///
/// `Composite` walks the tiers in order and is the default; naming one tier pins
/// it, which is what makes a tier testable in isolation and lets a deployment run
/// without the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchTier {
    Composite,
    Academic,
    Searxng,
    Exa,
    Tavily,
}

#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub tier: SearchTier,
    /// The self-hosted SearXNG service. `None` means the tier is not built.
    pub searxng_url: Option<String>,
    pub exa_api_key: Option<SecretString>,
    /// Tavily's key is `SEARCH_API_KEY` for historical reasons: the env template
    /// named it before the composite existed.
    pub tavily_api_key: Option<SecretString>,
    /// How long a cached result set serves without a provider call (A40).
    pub cache_ttl: std::time::Duration,
    /// Results requested per tier, and the merged cap.
    pub max_results: u8,
}

/// The first `LlmProvider` (A1): an [OI]-compatible endpoint. Optional as a
/// whole, because a deployment without a model can still serve everything that
/// does not generate, and chat answers `provider_error` rather than panicking.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct GoogleLoginConfig {
    pub client_id: String,
    pub client_secret: SecretString,
    pub redirect_uri: String,
}

/// The Calendar integration's OAuth client (A235).
///
/// Separate from [`GoogleLoginConfig`] rather than reused: the two flows ask for
/// different scopes and return to different redirect URIs, and one client for both
/// would mean the sign-in screen requesting calendar write access. The scopes are
/// constants in `calendar::oauth` rather than config, because they are a security
/// argument (`calendar.app.created` + `calendar.events.readonly` is what makes "Google
/// wins for an event we did not create" structural) and not a deployment choice.
#[derive(Debug, Clone)]
pub struct GoogleCalendarConfig {
    pub client_id: String,
    pub client_secret: SecretString,
    pub redirect_uri: String,
}

/// APNs' provider-API credentials (A288).
///
/// The key is a PKCS#8 EC private key, which is what Apple's `.p8` file holds. It is a
/// `SecretString` rather than a `PathBuf` because a container has no place to put a file that
/// is not a mounted secret, and the deployment's secret store hands over the bytes.
#[derive(Debug, Clone)]
pub struct ApnsConfig {
    pub key_p8: SecretString,
    /// The key's identifier, which is the JWT's `kid`. APNs rejects a token without it.
    pub key_id: String,
    /// The Apple Developer team id, which is the JWT's `iss`.
    pub team_id: String,
    /// The app's bundle id. APNs refuses a token whose `apns-topic` is not the app the token
    /// belongs to, so a wrong topic is a `403` that looks like a bad key.
    pub topic: String,
    /// The sandbox host, for a build signed with a development profile. The two hosts are
    /// different certificate chains, so pointing at the wrong one is a `403`.
    pub sandbox: bool,
}

/// FCM's HTTP v1 credentials (A288).
///
/// `private_key` and `client_email` are unpacked from the service-account JSON at boot rather
/// than carried as a blob, because the assertion's `iss` is the email and the signer is the
/// key: a config struct that held the JSON would make every reader parse it again.
#[derive(Debug, Clone)]
pub struct FcmConfig {
    pub project_id: String,
    pub client_email: String,
    pub private_key: String,
}

/// The push transports, each optional.
///
/// `None` on a side is a supported deployment and not a degraded one: a dev box with no APNs
/// key still delivers in-app and email reminders, and the settings screen offers only the
/// channels that will fire (A288). `Default` is therefore the honest empty configuration.
#[derive(Debug, Clone, Default)]
pub struct PushConfig {
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<FcmConfig>,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let root = std::env::current_dir().context("resolving the working directory")?;
        Self::from_env_at(&root, &[])
    }

    /// Testable variant: `overrides` replaces process env for the given keys, so
    /// tests do not have to mutate the process environment (which is global and
    /// racy across threads).
    pub fn from_env_at(root: &Path, overrides: &[(&str, &str)]) -> anyhow::Result<Self> {
        let get = |key: &str| -> Option<String> {
            overrides
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
                .or_else(|| std::env::var(key).ok())
        };
        // An empty value counts as absent. The env template ships keys blank
        // (`LLM_MODEL=`, `SMTP_URL=`), and `unwrap_or_else` would keep the empty
        // string, so a default would never fire: the documented way to enable chat
        // is to set `LLM_BASE_URL` alone, and it would have posted `"model": ""`.
        let get_or = |key: &str, default: &str| {
            get(key)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.to_string())
        };

        let env = Env::parse(&get_or("APP_ENV", "dev"))?;

        let bind_addr: SocketAddr = get_or("BIND_ADDR", "127.0.0.1:8080")
            .parse()
            .context("BIND_ADDR must be host:port")?;

        let plans_path = root.join(get_or("PLANS_CONFIG", "config/plans.toml"));
        let plans = load_plans(&plans_path)
            .with_context(|| format!("loading plans config at {}", plans_path.display()))?;

        let policy_path = root.join(get_or(
            "BILLING_POLICY_CONFIG",
            "config/billing_policy.toml",
        ));
        let billing_policy = load_billing_policy(&policy_path)
            .with_context(|| format!("loading billing policy at {}", policy_path.display()))?;

        let database_url = get_or(
            "DATABASE_URL",
            "postgres://postgres:postgres@localhost:5432/stvdy",
        );

        // A prod deploy with the dev secret would be a silent full-auth bypass.
        let jwt_secret_raw = get("JWT_SECRET");
        if !env.is_dev() {
            anyhow::ensure!(
                jwt_secret_raw.as_deref().is_some_and(|s| s.len() >= 32),
                "JWT_SECRET must be set to at least 32 characters when APP_ENV=prod"
            );
        }
        let jwt_secret =
            SecretString::from(jwt_secret_raw.unwrap_or_else(|| "dev-only-change-me".to_string()));

        // The same reasoning as JWT_SECRET, with higher stakes: this key protects
        // Google OAuth refresh tokens and BYOK API keys at rest. The all-zero
        // default would leave those columns effectively plaintext while looking
        // encrypted, so anyone with database or backup access could read them.
        let encryption_key_hex = get_or("ENCRYPTION_KEY_HEX", &"0".repeat(64));
        validate_encryption_key(&encryption_key_hex, env)?;

        // Prod must name its web origin: the CORS allowlist would otherwise
        // contain only the native shell origins, and the deployed web build
        // (admin, checkout) would be blocked.
        let web_origin = get("WEB_ORIGIN");
        if !env.is_dev() {
            anyhow::ensure!(
                web_origin.as_deref().is_some_and(|o| !o.is_empty()),
                "WEB_ORIGIN must be set when APP_ENV=prod: the web build's origin is \
                 part of the CORS allowlist"
            );
        }

        Ok(AppConfig {
            env,
            bind_addr,
            database_url,
            jwt_secret: jwt_secret.clone(),
            encryption_key_hex,
            plans,
            billing_policy,
            admin_email: get("ADMIN_EMAIL"),
            web_origin,
            root: root.to_path_buf(),
            auth: AuthConfig {
                access_ttl_min: get_i64(&get, "ACCESS_TOKEN_TTL_MIN", 15)?,
                refresh_ttl_days: get_i64(&get, "REFRESH_TOKEN_TTL_DAYS", 30)?,
                verify_ttl_hours: get_i64(&get, "VERIFY_TOKEN_TTL_HOURS", 24)?,
            },
            storage: storage_config(&get, &get_or, root, &jwt_secret)?,
            mailer: MailerConfig {
                smtp_url: get("SMTP_URL").filter(|s| !s.is_empty()),
                from: get_or("MAIL_FROM", "no-reply@stvdy.app"),
                dir: root.join(get_or("MAIL_DIR", "var/mail")),
            },
            embedder: EmbedderConfig {
                provider: match get_or("EMBEDDER", "fastembed").as_str() {
                    "fastembed" => EmbedderProvider::Fastembed,
                    "http" => EmbedderProvider::Http,
                    other => anyhow::bail!("EMBEDDER must be fastembed or http, got {other}"),
                },
                model: get_or("EMBEDDER_MODEL", "intfloat/multilingual-e5-base"),
                dim: get_usize(&get, "EMBEDDING_DIM", 768)?,
                cache_dir: root.join(get_or("FASTEMBED_CACHE_DIR", ".fastembed_cache")),
                base_url: get("EMBEDDER_BASE_URL").filter(|s| !s.is_empty()),
                api_key: get("EMBEDDER_API_KEY")
                    .filter(|s| !s.is_empty())
                    .map(SecretString::from),
                query_prefix: get_or("EMBEDDER_QUERY_PREFIX", ""),
            },
            pdfium_lib_path: get("PDFIUM_LIB_PATH")
                .filter(|s| !s.is_empty())
                .map(|s| root.join(s)),
            google_login: google_login_config(&get)?,
            google_calendar: google_calendar_config(&get)?,
            push: push_config(&get, &get_or)?,
            llm: llm_config(&get_or, &get),
            search: search_config(&get, &get_or)?,
            grace_period_days: get_i64(&get, "GRACE_PERIOD_DAYS", 3)?,
            idr_per_usd: get_i64(&get, "IDR_PER_USD", 17889)?,
            change_log_retention_days: get_i64(&get, "CHANGE_LOG_RETENTION_DAYS", 90)?,
            calendar_poll_interval: {
                // A zero interval is not "poll constantly": it makes the dedupe window one
                // second wide, so a poll pass enqueues a job per connection per second. Refused
                // at boot rather than at the first scheduler tick.
                let secs = get_i64(&get, "CALENDAR_POLL_INTERVAL_SECS", 900)?;
                anyhow::ensure!(
                    secs > 0,
                    "CALENDAR_POLL_INTERVAL_SECS must be positive, got {secs}"
                );
                std::time::Duration::from_secs(secs as u64)
            },
            change_log_tombstone_retention_days: get_i64(
                &get,
                "CHANGE_LOG_TOMBSTONE_RETENTION_DAYS",
                180,
            )?,
            soffice_bin: get("SOFFICE_BIN")
                .filter(|s| !s.is_empty())
                .map(|s| root.join(s)),
            qris: qris_config(&get, &get_or)?,
            student_email_suffixes: get_or("STUDENT_EMAIL_SUFFIXES", ".ac.id")
                .split(',')
                .map(|suffix| suffix.trim().to_ascii_lowercase())
                .filter(|suffix| !suffix.is_empty())
                .collect(),
            // The loopback default is what makes a single-container deployment need no
            // configuration: the renderer and the `/assets` route are the same process. It is
            // built from `BIND_ADDR`'s *port* rather than its address, because `0.0.0.0` is a
            // bind address and not something a browser can fetch from.
            assets_base_url: get_or(
                "ASSETS_BASE_URL",
                &format!("http://127.0.0.1:{}", bind_addr.port()),
            ),
        })
    }
}

/// The QRIS gateway configuration, or `None` when no base URL is set.
///
/// **The signature scheme is parsed even when no gateway is configured**, and an
/// unknown value is a boot failure either way: a typo in `QRIS_SIGNATURE_SCHEME` is
/// an operator who believes a scheme is in force, and failing at startup is the only
/// place that can be said loudly (A292).
///
/// A configured gateway with no `QRIS_API_KEY` or no `QRIS_CALLBACK_TOKEN` is also a
/// boot failure rather than a first-webhook surprise. The callback token is the HMAC
/// key (or the compared value), so booting without it would mean verifying signatures
/// against an empty secret — which an attacker can reproduce.
fn qris_config(
    get: &impl Fn(&str) -> Option<String>,
    get_or: &impl Fn(&str, &str) -> String,
) -> anyhow::Result<Option<QrisConfig>> {
    let scheme_raw = get_or("QRIS_SIGNATURE_SCHEME", "hmac_sha256_hex");
    let Some(signature_scheme) = SignatureScheme::parse(&scheme_raw) else {
        anyhow::bail!(
            "QRIS_SIGNATURE_SCHEME must be one of {}, got {scheme_raw}",
            SignatureScheme::ALL
                .map(|scheme| scheme.as_str())
                .join(", ")
        );
    };

    let Some(base_url) = get("QRIS_BASE_URL").filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let api_key = get("QRIS_API_KEY")
        .filter(|value| !value.is_empty())
        .context("QRIS_BASE_URL is set but QRIS_API_KEY is not")?;
    let callback_token = get("QRIS_CALLBACK_TOKEN")
        .filter(|value| !value.is_empty())
        .context(
            "QRIS_BASE_URL is set but QRIS_CALLBACK_TOKEN is not: it is the HMAC key \
             (or the compared value) for every callback, and an empty one verifies \
             anything",
        )?;

    let webhook_ips = match get("QRIS_WEBHOOK_IPS").filter(|value| !value.is_empty()) {
        None => Vec::new(),
        Some(raw) => {
            let mut parsed = Vec::new();
            for candidate in raw.split(',').map(str::trim).filter(|c| !c.is_empty()) {
                parsed.push(candidate.parse::<std::net::IpAddr>().with_context(|| {
                    format!("QRIS_WEBHOOK_IPS entry {candidate:?} is not an IP address")
                })?);
            }
            parsed
        }
    };

    let fee_percent = get_or("QRIS_FEE_PERCENT", "0")
        .parse::<f64>()
        .context("QRIS_FEE_PERCENT must be a number")?;
    anyhow::ensure!(
        (0.0..=100.0).contains(&fee_percent),
        "QRIS_FEE_PERCENT must be between 0 and 100, got {fee_percent}"
    );

    let fee_fixed_idr = get_i64(get, "QRIS_FEE_FIXED_IDR", 0)?;
    anyhow::ensure!(
        fee_fixed_idr >= 0,
        "QRIS_FEE_FIXED_IDR must not be negative, got {fee_fixed_idr}"
    );

    let invoice_ttl_minutes = get_i64(get, "QRIS_INVOICE_TTL_MINUTES", 15)?;
    anyhow::ensure!(
        invoice_ttl_minutes > 0,
        "QRIS_INVOICE_TTL_MINUTES must be positive, got {invoice_ttl_minutes}"
    );

    let reconcile_hours = get_i64(get, "QRIS_RECONCILE_HOURS", 48)?;
    anyhow::ensure!(
        reconcile_hours > 0,
        "QRIS_RECONCILE_HOURS must be positive, got {reconcile_hours}"
    );

    Ok(Some(QrisConfig {
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key: SecretString::from(api_key),
        callback_token: SecretString::from(callback_token),
        signature_scheme,
        signature_header: get_or("QRIS_SIGNATURE_HEADER", "X-Callback-Signature"),
        webhook_ips,
        fee_percent,
        fee_fixed_idr,
        invoice_ttl_minutes,
        honor_late_payment: get_or("QRIS_HONOR_LATE_PAYMENT", "true") != "false",
        reconcile_hours,
    }))
}

/// The model provider. Optional as a whole: with no base URL there is no chat,
/// and the routes say so rather than failing at the first request with a
/// transport error that looks like an outage.
fn llm_config(
    get_or: &impl Fn(&str, &str) -> String,
    get: &impl Fn(&str) -> Option<String>,
) -> Option<LlmConfig> {
    let base_url = get("LLM_BASE_URL").filter(|s| !s.is_empty())?;
    Some(LlmConfig {
        base_url,
        api_key: get_or("LLM_API_KEY", ""),
        model: get_or("LLM_MODEL", "deepseek-v4.1-flash"),
    })
}

/// The search configuration. The composite needs no keys at all: the academic
/// tier is keyless and SearXNG is self-hosted, so a deployment with no paid
/// fallback still has a working web tier.
fn search_config(
    get: &impl Fn(&str) -> Option<String>,
    get_or: &impl Fn(&str, &str) -> String,
) -> anyhow::Result<SearchConfig> {
    let tier = match get_or("SEARCH_PROVIDER", "composite").as_str() {
        "composite" => SearchTier::Composite,
        "academic" => SearchTier::Academic,
        "searxng" => SearchTier::Searxng,
        "exa" => SearchTier::Exa,
        "tavily" => SearchTier::Tavily,
        other => anyhow::bail!(
            "SEARCH_PROVIDER must be composite, academic, searxng, exa or tavily, got {other}"
        ),
    };

    let exa_api_key = get("EXA_API_KEY").filter(|value| !value.is_empty());
    let tavily_api_key = get("SEARCH_API_KEY").filter(|value| !value.is_empty());

    // A pinned tier with no key is a startup failure, not a first-query 401: the
    // operator asked for that tier specifically, so booting without it would
    // silently serve nothing.
    match tier {
        SearchTier::Exa if exa_api_key.is_none() => {
            anyhow::bail!("SEARCH_PROVIDER=exa requires EXA_API_KEY")
        }
        SearchTier::Tavily if tavily_api_key.is_none() => {
            anyhow::bail!("SEARCH_PROVIDER=tavily requires SEARCH_API_KEY")
        }
        _ => {}
    }

    let ttl_hours = get_or("SEARCH_CACHE_TTL_HOURS", "24")
        .parse::<i64>()
        .map_err(|error| anyhow::anyhow!("SEARCH_CACHE_TTL_HOURS must be an integer: {error}"))?;
    anyhow::ensure!(
        ttl_hours > 0,
        "SEARCH_CACHE_TTL_HOURS must be positive, got {ttl_hours}"
    );

    Ok(SearchConfig {
        tier,
        searxng_url: get("SEARXNG_URL").filter(|value| !value.is_empty()),
        exa_api_key: exa_api_key.map(SecretString::from),
        tavily_api_key: tavily_api_key.map(SecretString::from),
        cache_ttl: std::time::Duration::from_secs(ttl_hours as u64 * 3600),
        max_results: 8,
    })
}

/// Google login is optional as a whole: without a client id there is no
/// "Continue with Google" button, and `GET /auth/google/start` answers 404
/// rather than redirecting to a half-configured client.
fn google_login_config(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<GoogleLoginConfig>> {
    let Some(client_id) = get("GOOGLE_LOGIN_CLIENT_ID").filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let client_secret = get("GOOGLE_LOGIN_CLIENT_SECRET")
        .filter(|s| !s.is_empty())
        .context("GOOGLE_LOGIN_CLIENT_ID is set but GOOGLE_LOGIN_CLIENT_SECRET is not")?;
    let redirect_uri = get("GOOGLE_LOGIN_REDIRECT_URI")
        .filter(|s| !s.is_empty())
        .context("GOOGLE_LOGIN_CLIENT_ID is set but GOOGLE_LOGIN_REDIRECT_URI is not")?;
    Ok(Some(GoogleLoginConfig {
        client_id,
        client_secret: SecretString::from(client_secret),
        redirect_uri,
    }))
}

/// The Calendar consent flow's client, parsed exactly like [`google_login_config`]:
/// optional as a whole, and a half-configured client is a boot failure rather than a
/// redirect that fails at Google.
///
/// Its own three variables (`GOOGLE_CALENDAR_CLIENT_ID`/`_SECRET`/`_REDIRECT_URI`) rather
/// than the login ones: the calendar client carries calendar scopes, and reusing the
/// login client would make the sign-in screen ask for them.
fn google_calendar_config(
    get: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<GoogleCalendarConfig>> {
    let Some(client_id) = get("GOOGLE_CALENDAR_CLIENT_ID").filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let client_secret = get("GOOGLE_CALENDAR_CLIENT_SECRET")
        .filter(|s| !s.is_empty())
        .context("GOOGLE_CALENDAR_CLIENT_ID is set but GOOGLE_CALENDAR_CLIENT_SECRET is not")?;
    let redirect_uri = get("GOOGLE_CALENDAR_REDIRECT_URI")
        .filter(|s| !s.is_empty())
        .context("GOOGLE_CALENDAR_CLIENT_ID is set but GOOGLE_CALENDAR_REDIRECT_URI is not")?;
    Ok(Some(GoogleCalendarConfig {
        client_id,
        client_secret: SecretString::from(client_secret),
        redirect_uri,
    }))
}

/// The push transports (A288), each optional as a whole.
///
/// APNs is configured by its four values or not at all: a key without a key id, or a topic
/// without a team id, is an operator who believes push is on. Half of a pair is a **boot
/// failure** rather than a silently absent transport — the same reasoning `qris_config`
/// applies to its credentials, and the opposite of a wrong key, which is caught at
/// `PushRouter::from_config` and logged.
///
/// FCM is configured by the service-account JSON. The two fields the assertion needs are
/// unpacked here so a malformed blob fails at boot with a message naming the missing field,
/// rather than at 03:00 with a signing error.
fn push_config(
    get: &impl Fn(&str) -> Option<String>,
    get_or: &impl Fn(&str, &str) -> String,
) -> anyhow::Result<PushConfig> {
    let key_p8 = get("APNS_KEY_P8").filter(|value| !value.is_empty());
    let apns = match key_p8 {
        None => None,
        Some(key_p8) => {
            let require = |name: &str| -> anyhow::Result<String> {
                get(name)
                    .filter(|value| !value.is_empty())
                    .with_context(|| format!("APNS_KEY_P8 is set but {name} is not"))
            };
            Some(ApnsConfig {
                key_p8: SecretString::from(key_p8),
                key_id: require("APNS_KEY_ID")?,
                team_id: require("APNS_TEAM_ID")?,
                topic: require("APNS_TOPIC")?,
                // The sandbox host is opt-in: a production build that defaulted to sandbox
                // would deliver nothing and report success, which is the failure mode hardest
                // to notice.
                sandbox: get_or("APNS_SANDBOX", "false") == "true",
            })
        }
    };

    let service_account = get("FCM_SERVICE_ACCOUNT_JSON").filter(|value| !value.is_empty());
    let fcm = match service_account {
        None => None,
        Some(raw) => {
            let parsed: serde_json::Value = serde_json::from_str(&raw)
                .context("FCM_SERVICE_ACCOUNT_JSON must be the service account's JSON")?;
            let field = |name: &str| -> anyhow::Result<String> {
                parsed
                    .get(name)
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .with_context(|| {
                        format!("FCM_SERVICE_ACCOUNT_JSON has no {name}; it is what the assertion signs with")
                    })
            };
            Some(FcmConfig {
                project_id: get("FCM_PROJECT_ID")
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| field("project_id").unwrap_or_default()),
                client_email: field("client_email")?,
                private_key: field("private_key")?,
            })
        }
    };

    Ok(PushConfig { apns, fcm })
}

fn storage_config(
    get: &impl Fn(&str) -> Option<String>,
    get_or: &impl Fn(&str, &str) -> String,
    root: &Path,
    jwt_secret: &SecretString,
) -> anyhow::Result<StorageConfig> {
    let backend = match get_or("STORAGE_BACKEND", "local").as_str() {
        "local" => StorageBackend::Local,
        "s3" => StorageBackend::S3,
        other => anyhow::bail!("STORAGE_BACKEND must be local or s3, got {other}"),
    };

    let access_key_id = get("S3_ACCESS_KEY_ID").filter(|s| !s.is_empty());
    let secret_access_key = get("S3_SECRET_ACCESS_KEY").filter(|s| !s.is_empty());
    if backend == StorageBackend::S3 {
        anyhow::ensure!(
            access_key_id.is_some() && secret_access_key.is_some(),
            "STORAGE_BACKEND=s3 requires S3_ACCESS_KEY_ID and S3_SECRET_ACCESS_KEY"
        );
    }

    Ok(StorageConfig {
        backend,
        local_dir: root.join(get_or("STORAGE_LOCAL_DIR", "var/objects")),
        endpoint: get("S3_ENDPOINT").filter(|s| !s.is_empty()),
        region: get_or("S3_REGION", "us-east-1"),
        bucket: get_or("S3_BUCKET", "stvdy-sources"),
        access_key_id,
        secret_access_key: secret_access_key.map(SecretString::from),
        force_path_style: get_or("S3_FORCE_PATH_STYLE", "true") == "true",
        presign_ttl: std::time::Duration::from_secs(get_i64(get, "PRESIGN_TTL_SECS", 900)? as u64),
        public_base_url: get_or("OBJECTS_PUBLIC_BASE_URL", "http://localhost:8080")
            .trim_end_matches('/')
            .to_string(),
        sign_key: derive_storage_sign_key(jwt_secret)?,
    })
}

/// The local backend signs its own URLs, so it needs a key that is stable across
/// restarts (a restart must not invalidate a link a client is holding) and is
/// never a constant. Deriving it from `JWT_SECRET` through HMAC means one secret
/// to configure, with domain separation so the two uses cannot be confused.
fn derive_storage_sign_key(jwt_secret: &SecretString) -> anyhow::Result<SecretString> {
    use hmac::digest::KeyInit;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // HMAC takes a key of any length, so the only failure here is one the hash
    // itself cannot accept, which `new_from_slice` reports instead of panicking
    // (the workspace forbids `expect` outside tests).
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(jwt_secret.expose_secret().as_bytes())
        .map_err(|error| anyhow::anyhow!("deriving the storage signing key: {error}"))?;
    mac.update(b"stvdy.local-storage.sign.v1");
    Ok(SecretString::from(hex::encode(mac.finalize().into_bytes())))
}

fn get_i64(get: &impl Fn(&str) -> Option<String>, key: &str, default: i64) -> anyhow::Result<i64> {
    match get(key).filter(|s| !s.is_empty()) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<i64>()
            .with_context(|| format!("{key} must be an integer, got {raw}")),
    }
}

fn get_usize(
    get: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: usize,
) -> anyhow::Result<usize> {
    match get(key).filter(|s| !s.is_empty()) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<usize>()
            .with_context(|| format!("{key} must be an integer, got {raw}")),
    }
}

/// A wrong-length key fails much later, at first use, so check it at boot. The
/// length and hex checks apply everywhere; the "not the dev default" check only
/// in prod, where shipping it would silently defeat encryption at rest.
fn validate_encryption_key(key_hex: &str, env: Env) -> anyhow::Result<()> {
    anyhow::ensure!(
        key_hex.len() == 64,
        "ENCRYPTION_KEY_HEX must be 64 hex characters (32 bytes for aes-gcm), got {}",
        key_hex.len()
    );
    anyhow::ensure!(
        key_hex.chars().all(|c| c.is_ascii_hexdigit()),
        "ENCRYPTION_KEY_HEX must be hexadecimal"
    );
    if !env.is_dev() {
        anyhow::ensure!(
            key_hex.chars().any(|c| c != '0'),
            "ENCRYPTION_KEY_HEX must not be the all-zero dev default when APP_ENV=prod: \
             it encrypts OAuth refresh tokens and BYOK keys at rest"
        );
    }
    Ok(())
}
