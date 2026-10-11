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

#[test]
fn prod_requires_a_real_jwt_secret() {
    // A short secret in prod is a full-auth bypass waiting to happen.
    let err = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("APP_ENV", "prod"),
            ("JWT_SECRET", "too-short"),
        ],
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("JWT_SECRET"), "got: {err:#}");

    // A long one is accepted.
    let ok = AppConfig::from_env_at(
        &root(),
        &[
            ("PLANS_CONFIG", "config/plans.toml"),
            ("BILLING_POLICY_CONFIG", "config/billing_policy.toml"),
            ("APP_ENV", "prod"),
            ("JWT_SECRET", &"x".repeat(32)),
        ],
    );
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
