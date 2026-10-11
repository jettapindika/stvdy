use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use secrecy::SecretString;
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
        let get_or = |key: &str, default: &str| get(key).unwrap_or_else(|| default.to_string());

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
            jwt_secret,
            encryption_key_hex,
            plans,
            billing_policy,
            admin_email: get("ADMIN_EMAIL"),
            web_origin,
            root: root.to_path_buf(),
        })
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
