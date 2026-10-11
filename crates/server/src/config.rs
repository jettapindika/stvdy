//! Application configuration. Filled in by Task 8.

use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    Dev,
    Prod,
}

impl Env {
    pub fn is_dev(self) -> bool {
        matches!(self, Env::Dev)
    }
}

/// Placeholder until Task 8 wires the plans + billing-policy loaders in.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub env: Env,
    pub bind_addr: SocketAddr,
    pub database_url: String,
}
