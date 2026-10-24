//! Shared helpers for the suite's examples.
//!
//! Diagnostic logging for every example and the parts the embedding hosts
//! share.

pub mod embedding;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

/// Initializes diagnostic logging with WARN as the default level.
pub fn logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .without_time()
        .with_target(false)
        .log_internal_errors(false)
        .init();
}
