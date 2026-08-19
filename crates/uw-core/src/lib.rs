//! Core of usage-watcher: the normalized model, authentication, and the
//! provider adapters. Both the CLI and the Tauri widget build on this.

pub mod auth;
pub mod config;
pub mod model;
pub mod providers;

pub use config::Config;
/// Re-exported so downstream crates share one `reqwest` version.
pub use reqwest;
pub use model::{Meter, MeterKind, Provider, Severity, Snapshot, Status};

/// Shared HTTP client. Built once — connection reuse matters when polling on a
/// 60-second loop for hours.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("usage-watcher/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build HTTP client")
}
