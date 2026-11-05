//! healthchecks.io-compatible pings.

use crate::config::{Config, non_empty};
use crate::{http, log};

/// Ping the configured URL: suffix "" (success), "/start" or "/fail".
pub fn ping(cfg: &Config, suffix: &str) {
    let Some(base) = non_empty(&cfg.runtime.healthcheck_url) else {
        return;
    };
    let url = format!("{}{suffix}", base.trim_end_matches('/'));
    let agent = http::agent(10);
    let ok = (0..http::ATTEMPTS).any(|_| agent.get(&url).call().is_ok());
    if !ok {
        let what = if suffix.is_empty() { "success" } else { suffix };
        log::warn(format!("healthcheck ping '{what}' failed"));
    }
}
