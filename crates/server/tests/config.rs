use std::sync::Arc;

use stvdy_server::config::{AppConfig, Env};

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn loads_config_files_and_defaults_the_bind_address() {
    let cfg = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("BIND_ADDR", "127.0.0.1:8080"),
        ],
    )
    .expect("config must load");

    assert_eq!(cfg.bind_addr.to_string(), "127.0.0.1:8080");
    assert_eq!(cfg.env, Env::Dev);
    assert_eq!(cfg.plans.plans.len(), 3);
    assert!(
        !cfg.billing_policy
            .purchase_allowed(stvdy_core::config::Platform::Ios)
    );
    assert!(
        cfg.billing_policy
            .purchase_allowed(stvdy_core::config::Platform::Web)
    );
}

#[test]
fn a_missing_plans_file_is_reported_with_its_path() {
    let err = AppConfig::from_env_at(
        std::path::Path::new("/tmp"),
        &[("PLANS_CONFIG", "does-not-exist.toml")],
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("does-not-exist.toml"),
        "the error must name the file: {message}"
    );
}

#[test]
fn a_missing_billing_policy_is_reported_with_its_path() {
    let err = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/nope.toml"),
        ],
    )
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("nope.toml"), "got: {message}");
}

fn prod_with_jwt(jwt: &str) -> Vec<(&str, &str)> {
    vec![
        ("PLANS_CONFIG", "config/plans.toml"),
        ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ("APP_ENV", "prod"),
        // prod requires a real encryption key too; supply one, since this test
        // is about JWT_SECRET.
        (
            "ENCRYPTION_KEY_HEX",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
        ("JWT_SECRET", jwt),
        ("WEB_ORIGIN", "https://app.stvdy.test"),
    ]
}

#[test]
fn prod_requires_a_real_jwt_secret() {
    // A short secret in prod is a full-auth bypass waiting to happen.
    let err = AppConfig::from_env_at(&root(), &prod_with_jwt("too-short")).unwrap_err();
    assert!(format!("{err:#}").contains("JWT_SECRET"), "got: {err:#}");

    // A long one is accepted.
    let long = "x".repeat(32);
    let ok = AppConfig::from_env_at(&root(), &prod_with_jwt(&long));
    assert!(ok.is_ok(), "a 32-char secret must be accepted: {ok:?}");
}

#[test]
fn an_invalid_app_env_is_rejected() {
    let err = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("APP_ENV", "staging"),
        ],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("APP_ENV"));
}

fn prod_overrides(encryption_key_hex: &str) -> Vec<(&str, &str)> {
    vec![
        ("PLANS_CONFIG", "config/plans.toml"),
        ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ("APP_ENV", "prod"),
        ("JWT_SECRET", "0123456789abcdef0123456789abcdef"),
        ("ENCRYPTION_KEY_HEX", encryption_key_hex),
        ("WEB_ORIGIN", "https://app.stvdy.test"),
    ]
}

#[test]
fn prod_rejects_the_all_zero_encryption_key() {
    // This key protects Google OAuth refresh tokens and BYOK API keys at rest.
    // The all-zero dev default would leave those columns effectively plaintext
    // while looking encrypted.
    let zero = "0".repeat(64);
    let err = AppConfig::from_env_at(&root(), &prod_overrides(&zero)).unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("ENCRYPTION_KEY_HEX"),
        "must name the variable: {message}"
    );
    assert!(
        message.contains("all-zero"),
        "must explain why it is rejected: {message}"
    );
}

#[test]
fn prod_rejects_a_missing_or_short_encryption_key() {
    // A key that is set but the wrong length is rejected for its length.
    let err = AppConfig::from_env_at(&root(), &prod_overrides("abcd")).unwrap_err();
    assert!(
        format!("{err:#}").contains("64 hex characters"),
        "got: {err:#}"
    );

    // An empty value counts as absent, so it takes the dev default and is then
    // rejected for being that default. The outcome is what matters: prod must not
    // boot either way, and both messages name the variable. Asserting the exact
    // wording here would pin which of the two checks happened to fire.
    let err = AppConfig::from_env_at(&root(), &prod_overrides("")).unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("ENCRYPTION_KEY_HEX"),
        "must name the variable: {message}"
    );
}

#[test]
fn a_non_hex_encryption_key_is_rejected_in_every_environment() {
    // A wrong-length or non-hex key fails much later, at first use, so it is
    // checked at boot even in dev.
    let key = "z".repeat(64);
    let err = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("ENCRYPTION_KEY_HEX", &key),
        ],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("hexadecimal"));
}

#[test]
fn dev_accepts_the_default_encryption_key() {
    let cfg = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ],
    )
    .expect("dev must load with defaults");
    assert_eq!(cfg.encryption_key_hex.len(), 64);
}

/// Guards that the struct stays usable in the shape the server expects.
#[test]
fn config_is_cloneable_and_shares_into_state() {
    let cfg = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
        ],
    )
    .unwrap();
    let shared = Arc::new(cfg);
    assert_eq!(shared.plans.plans.len(), 3);
}

/// The env template ships several keys blank (`LLM_MODEL=`, `SMTP_URL=`), and a
/// blank value must mean "absent" so the default fires. `unwrap_or_else` alone
/// would keep the empty string, and the documented way to enable chat is to set
/// `LLM_BASE_URL` alone, so the first request would post `"model": ""` and get a
/// 400 from the provider after a credit had already been spent.
#[test]
fn an_empty_env_value_falls_back_to_its_default() {
    let config = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("LLM_BASE_URL", "http://localhost:9/v1"),
            ("LLM_MODEL", ""),
            ("LLM_API_KEY", ""),
        ],
    )
    .expect("config loads");

    let llm = config
        .llm
        .expect("a base URL is enough to configure a provider");
    assert_eq!(
        llm.model, "deepseek-v4.1-flash",
        "an empty LLM_MODEL must fall back to the default, not be sent as an empty string"
    );
    assert_eq!(llm.api_key, "", "an empty key is legitimately empty");
}

/// The same shape for the storage and embedder keys the template ships blank.
#[test]
fn blank_storage_and_embedder_keys_take_their_defaults() {
    let config = AppConfig::from_env_at(
        &root(),
        &[
            ("APP_ENV", "dev"),
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("S3_ENDPOINT", ""),
            ("S3_BUCKET", ""),
            ("EMBEDDER_QUERY_PREFIX", ""),
            ("EMBEDDER_MODEL", ""),
        ],
    )
    .expect("config loads");

    assert!(
        config.storage.endpoint.is_none(),
        "a blank endpoint is no endpoint"
    );
    assert_eq!(config.storage.bucket, "stvdy-sources");
    assert_eq!(config.embedder.query_prefix, "");
    assert_eq!(config.embedder.model, "intfloat/multilingual-e5-base");
}
