//! Structured logging setup.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// JSON in prod (so a collector can parse fields), human readable in dev.
pub fn init(env: &str, default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let registry = tracing_subscriber::registry().with(filter);
    if env == "prod" {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer().with_target(true)).init();
    }
}
