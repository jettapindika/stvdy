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
    /// The chat and generation model. `None` means no provider is configured.
    pub llm: Option<LlmConfig>,
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
            llm: llm_config(&get_or, &get),
        })
    }
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
