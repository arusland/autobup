//! Telegram notifications.

use crate::config::{Config, non_empty};
use crate::{http, log, space};

/// Telegram rejects messages longer than 4096 characters.
const MAX_CHARS: usize = 4000;

/// Send a plain-text message to the configured chat. Does nothing if
/// Telegram is not configured.
pub fn send(cfg: &Config, text: &str) -> Result<(), String> {
    let Some(chat_id) = non_empty(&cfg.telegram.chat_id) else {
        return Ok(());
    };
    let Some(token) = cfg.bot_token()? else {
        return Ok(());
    };
    let token = token.as_str();
    let text = truncate(text);
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let form = [
        ("chat_id", chat_id),
        ("text", text.as_str()),
        ("disable_web_page_preview", "true"),
    ];
    let agent = http::agent(15);
    let mut last_err = String::new();
    for _ in 0..http::ATTEMPTS {
        match agent.post(&url).send_form(form) {
            Ok(_) => return Ok(()),
            // The URL contains the bot token: never let it reach the log.
            Err(e) => last_err = e.to_string().replace(token, "***"),
        }
    }
    Err(format!("Telegram notification failed: {last_err}"))
}

/// Send the end-of-run message for exit code `code`.
pub fn notify_result(cfg: &Config, code: i32, error: Option<&str>, secs: u64) {
    if !cfg.telegram_configured() {
        return;
    }
    let Some(mut msg) = result_message(cfg, code, error, secs) else {
        return;
    };
    if let Ok(space) = space::target_free_space(cfg) {
        log::info(format!("target free space: {space}"));
        msg.push_str(&format!("\nTarget free space: {space}"));
    }
    if let Err(e) = send(cfg, &msg) {
        log::warn(e);
    }
}

/// End-of-run message without the free-space line, or None when nothing
/// should be sent (success with `notify_success = false`).
fn result_message(cfg: &Config, code: i32, error: Option<&str>, secs: u64) -> Option<String> {
    let (icon, status) = match code {
        0 if !cfg.telegram.notify_success => return None,
        0 => ("✅", "backup succeeded".to_string()),
        3 => (
            "⚠️",
            "backup finished, but some files could not be read".to_string(),
        ),
        c => ("❌", format!("backup FAILED (exit code {c})")),
    };
    let mut msg = format!(
        "{icon} [{}] {status}\nHost: {}\nDuration: {secs}s",
        cfg.project_name,
        hostname()
    );
    if code != 0
        && code != 3
        && let Some(e) = error.filter(|e| !e.is_empty())
    {
        msg.push_str(&format!("\nError: {e}"));
    }
    if code != 0 {
        msg.push_str(&format!("\nLog: {}", cfg.log_file().display()));
    }
    Some(msg)
}

fn truncate(text: &str) -> String {
    if text.chars().count() > MAX_CHARS {
        text.chars().take(MAX_CHARS).chain("...".chars()).collect()
    } else {
        text.to_string()
    }
}

pub fn hostname() -> String {
    gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(extra: &str) -> Config {
        Config::parse(&format!("project_name = \"app\"\n{extra}")).unwrap()
    }

    #[test]
    fn success_message() {
        let msg = result_message(&cfg(""), 0, None, 5).unwrap();
        assert!(msg.starts_with("✅ [app] backup succeeded\nHost: "));
        assert!(msg.ends_with("\nDuration: 5s"));
    }

    #[test]
    fn success_suppressed() {
        assert!(result_message(&cfg("[telegram]\nnotify_success = false"), 0, None, 1).is_none());
        assert!(result_message(&cfg("[telegram]\nnotify_success = false"), 3, None, 1).is_some());
    }

    #[test]
    fn failure_has_error_and_log() {
        let msg = result_message(&cfg(""), 1, Some("boom"), 1).unwrap();
        assert!(msg.contains("backup FAILED (exit code 1)"));
        assert!(msg.contains("\nError: boom"));
        assert!(msg.contains("\nLog: /var/log/autobup/app.log"));
    }

    #[test]
    fn partial_has_log_but_no_error() {
        let msg = result_message(&cfg(""), 3, Some("x"), 1).unwrap();
        assert!(!msg.contains("Error:"));
        assert!(msg.contains("\nLog: "));
    }

    #[test]
    fn long_text_is_truncated() {
        let t = truncate(&"é".repeat(5000));
        assert_eq!(t.chars().count(), MAX_CHARS + 3);
        assert!(t.ends_with("..."));
    }
}
