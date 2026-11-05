//! Shared HTTP client settings.

use std::time::Duration;

/// How often a request is tried before giving up.
pub const ATTEMPTS: usize = 3;

/// HTTP agent with an overall timeout per request.
pub fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .build()
        .into()
}
