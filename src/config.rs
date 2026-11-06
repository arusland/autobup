//! Per-application TOML configuration.

use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::{log, proc, restic};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Used for the log and lock file names and the `project:<name>` snapshot tag.
    pub project_name: String,
    #[serde(default)]
    pub repository: Repository,
    #[serde(default)]
    pub backup: Backup,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub check: Check,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub telegram: Telegram,
    #[serde(default)]
    pub hooks: Hooks,
    /// Extra environment for restic and the hooks (AWS_*, B2_*, RESTIC_CACHE_DIR, ...).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub url: Option<String>,
    pub url_file: Option<String>,
    pub password_file: Option<String>,
    pub password: Option<String>,
    pub password_command: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Backup {
    pub paths: Vec<String>,
    pub excludes: Vec<String>,
    pub exclude_file: Option<String>,
    pub exclude_caches: bool,
    pub one_file_system: bool,
    pub tags: Vec<String>,
    pub host: Option<String>,
    pub extra_args: Vec<String>,
}

impl Default for Backup {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            excludes: Vec::new(),
            exclude_file: None,
            exclude_caches: true,
            one_file_system: false,
            tags: Vec::new(),
            host: None,
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Retention {
    pub keep_last: Option<u32>,
    pub keep_hourly: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_yearly: Option<u32>,
    pub keep_within: Option<String>,
    pub prune: bool,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            keep_last: None,
            keep_hourly: None,
            keep_daily: None,
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
            keep_within: None,
            prune: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Check {
    pub enabled: bool,
    pub read_data_subset: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Runtime {
    pub log_dir: PathBuf,
    pub lock_dir: PathBuf,
    pub nice_level: i32,
    pub ionice_class: u8,
    pub healthcheck_url: Option<String>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("/var/log/autobup"),
            lock_dir: PathBuf::from("/tmp"),
            nice_level: 10,
            ionice_class: 2,
            healthcheck_url: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Telegram {
    pub bot_token: Option<String>,
    /// File holding the bot token, as an alternative to `bot_token`.
    pub bot_token_file: Option<String>,
    pub chat_id: Option<String>,
    pub notify_success: bool,
}

impl Default for Telegram {
    fn default() -> Self {
        Self {
            bot_token: None,
            bot_token_file: None,
            chat_id: None,
            notify_success: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Hooks {
    pub pre_backup: Option<String>,
    pub post_backup: Option<String>,
    pub on_failure: Option<String>,
}

/// Treat empty strings like missing values.
pub fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| AppError::config(format!("cannot read config {}: {e}", path.display())))?;
        Self::parse(&text).map_err(|e| AppError::config(format!("invalid config {}: {e}", path.display())))
    }

    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        Self::parse_with_home(text, std::env::var("HOME").ok().as_deref())
    }

    fn parse_with_home(text: &str, home: Option<&str>) -> std::result::Result<Self, String> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.expand_paths(home)?;
        let name = cfg.project_name.trim();
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            return Err(format!(
                "project_name {:?} must be a non-empty file name",
                cfg.project_name
            ));
        }
        if !(1..=3).contains(&cfg.runtime.ionice_class) {
            return Err("runtime.ionice_class must be 1, 2 or 3".into());
        }
        if non_empty(&cfg.telegram.bot_token).is_some() && non_empty(&cfg.telegram.bot_token_file).is_some() {
            return Err("set only one of telegram.bot_token and telegram.bot_token_file".into());
        }
        Ok(cfg)
    }

    /// Expand `~` and `$HOME` in the settings that hold local paths.
    fn expand_paths(&mut self, home: Option<&str>) -> std::result::Result<(), String> {
        let opt = |v: &mut Option<String>| -> std::result::Result<(), String> {
            if let Some(s) = v {
                *s = expand_home(s, home)?;
            }
            Ok(())
        };
        let path = |p: &mut PathBuf| -> std::result::Result<(), String> {
            if let Some(s) = p.to_str() {
                *p = PathBuf::from(expand_home(s, home)?);
            }
            Ok(())
        };
        if let Some(url) = &mut self.repository.url {
            *url = expand_repo_url(url, home)?;
        }
        opt(&mut self.repository.url_file)?;
        opt(&mut self.repository.password_file)?;
        for p in &mut self.backup.paths {
            *p = expand_home(p, home)?;
        }
        opt(&mut self.backup.exclude_file)?;
        opt(&mut self.telegram.bot_token_file)?;
        path(&mut self.runtime.log_dir)?;
        path(&mut self.runtime.lock_dir)?;
        Ok(())
    }

    pub fn project_tag(&self) -> String {
        format!("project:{}", self.project_name)
    }

    pub fn log_file(&self) -> PathBuf {
        self.runtime.log_dir.join(format!("{}.log", self.project_name))
    }

    fn has_bot_token(&self) -> bool {
        non_empty(&self.telegram.bot_token).is_some() || non_empty(&self.telegram.bot_token_file).is_some()
    }

    pub fn telegram_configured(&self) -> bool {
        self.has_bot_token() && non_empty(&self.telegram.chat_id).is_some()
    }

    /// The bot token from `bot_token`, or read from `bot_token_file`.
    pub fn bot_token(&self) -> std::result::Result<Option<String>, String> {
        if let Some(token) = non_empty(&self.telegram.bot_token) {
            return Ok(Some(token.to_string()));
        }
        let Some(file) = non_empty(&self.telegram.bot_token_file) else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(file)
            .map_err(|e| format!("cannot read telegram.bot_token_file {file}: {e}"))?;
        match text.trim() {
            "" => Err(format!("telegram.bot_token_file {file} is empty")),
            token => Ok(Some(token.to_string())),
        }
    }

    fn has_secrets(&self) -> bool {
        non_empty(&self.repository.password).is_some()
            || non_empty(&self.telegram.bot_token).is_some()
            || self.env.keys().any(|k| {
                let k = k.to_ascii_uppercase();
                ["SECRET", "PASSWORD", "TOKEN", "KEY"]
                    .iter()
                    .any(|s| k.contains(s))
            })
    }
}

/// Expand a leading `~` (`~` or `~/...`) and `$HOME` / `${HOME}` anywhere in `s`.
/// `~user` is not supported and left as it is.
pub fn expand_home(s: &str, home: Option<&str>) -> std::result::Result<String, String> {
    let tilde = s == "~" || s.starts_with("~/");
    if !tilde && !s.contains("$HOME") && !s.contains("${HOME}") {
        return Ok(s.to_string());
    }
    let home = home
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("HOME is not set, cannot expand {s:?}"))?;
    let home = if home.len() > 1 {
        home.trim_end_matches('/')
    } else {
        home
    };

    let mut out = s.replace("${HOME}", home);
    // $HOME only as a whole name, so $HOMEDIR stays untouched.
    let mut i = 0;
    while let Some(pos) = out[i..].find("$HOME").map(|p| p + i) {
        let end = pos + "$HOME".len();
        let next = out[end..].chars().next();
        if next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            i = end;
        } else {
            out.replace_range(pos..end, home);
            i = pos + home.len();
        }
    }
    if tilde {
        out.replace_range(..1, home);
    }
    Ok(out)
}

/// Expand `~`/`$HOME` in a local repository path; remote URLs (`sftp:`,
/// `s3:`, `rest:`, ...) are left alone.
fn expand_repo_url(url: &str, home: Option<&str>) -> std::result::Result<String, String> {
    if let Some(rest) = url.strip_prefix("local:") {
        return Ok(format!("local:{}", expand_home(rest, home)?));
    }
    let remote = url
        .split_once(':')
        .is_some_and(|(scheme, _)| !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric()));
    if remote {
        Ok(url.to_string())
    } else {
        expand_home(url, home)
    }
}

/// Checks needed before anything talks to restic.
pub fn validate(cfg: &Config, config_path: &Path) -> Result<()> {
    if !proc::in_path(restic::RESTIC) {
        return Err(AppError::config("restic not found in PATH, please install it"));
    }
    let env = |key| restic::effective_env(cfg, key);
    if env("RESTIC_REPOSITORY").is_none() && env("RESTIC_REPOSITORY_FILE").is_none() {
        return Err(AppError::config(
            "repository.url (or repository.url_file) is not set",
        ));
    }
    if [
        "RESTIC_PASSWORD",
        "RESTIC_PASSWORD_FILE",
        "RESTIC_PASSWORD_COMMAND",
    ]
    .iter()
    .all(|k| env(k).is_none())
    {
        return Err(AppError::config(
            "repository.password_file / password / password_command is not set",
        ));
    }
    if let Some(file) = env("RESTIC_PASSWORD_FILE")
        && File::open(&file).is_err()
    {
        if Path::new(&file).exists() {
            return Err(AppError::config(format!("password file not readable: {file}")));
        }
        return Err(AppError::config(format!(
            "password file not readable: {file} (run 'autobup -c {} pwd' to create it)",
            config_path.display()
        )));
    }
    if cfg.has_bot_token() != non_empty(&cfg.telegram.chat_id).is_some() {
        log::warn("Telegram notifications need telegram.chat_id and telegram.bot_token (or bot_token_file)");
    }
    if let Some(file) = non_empty(&cfg.telegram.bot_token_file) {
        match std::fs::metadata(file) {
            Err(e) => log::warn(format!("cannot read telegram.bot_token_file {file}: {e}")),
            Ok(meta) if meta.permissions().mode() & 0o044 != 0 => {
                log::warn(format!("bot token file is readable by others: chmod 600 {file}"))
            }
            Ok(_) => {}
        }
    }
    if cfg.has_secrets()
        && let Ok(meta) = std::fs::metadata(config_path)
        && meta.permissions().mode() & 0o044 != 0
    {
        log::warn(format!(
            "config contains secrets but is readable by others: chmod 600 {}",
            config_path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_uses_defaults() {
        let cfg = Config::parse("project_name = \"app\"").unwrap();
        assert!(cfg.backup.exclude_caches);
        assert!(!cfg.backup.one_file_system);
        assert!(cfg.retention.prune);
        assert_eq!(cfg.retention.keep_daily, None);
        assert_eq!(cfg.runtime.nice_level, 10);
        assert_eq!(cfg.runtime.log_dir, PathBuf::from("/var/log/autobup"));
        assert!(cfg.telegram.notify_success);
        assert_eq!(cfg.project_tag(), "project:app");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(Config::parse("project_name = \"app\"\n[backup]\npath = []").is_err());
    }

    #[test]
    fn expand_home_forms() {
        let h = Some("/home/u");
        assert_eq!(expand_home("~", h).unwrap(), "/home/u");
        assert_eq!(expand_home("~/.config/a", h).unwrap(), "/home/u/.config/a");
        assert_eq!(expand_home("$HOME/a", h).unwrap(), "/home/u/a");
        assert_eq!(expand_home("${HOME}/a", h).unwrap(), "/home/u/a");
        assert_eq!(expand_home("/x/$HOME/$HOME", h).unwrap(), "/x//home/u//home/u");
        assert_eq!(expand_home("$HOMEDIR/a", h).unwrap(), "$HOMEDIR/a");
        assert_eq!(expand_home("~user/a", h).unwrap(), "~user/a");
        assert_eq!(expand_home("/a/~/b", h).unwrap(), "/a/~/b");
        assert_eq!(expand_home("~/a", Some("/home/u/")).unwrap(), "/home/u/a");
        assert_eq!(expand_home("/abs", None).unwrap(), "/abs");
        assert!(expand_home("~/a", None).is_err());
        assert!(expand_home("$HOME/a", Some("")).is_err());
    }

    #[test]
    fn repo_url_expands_only_local() {
        let h = Some("/home/u");
        assert_eq!(expand_repo_url("~/restic", h).unwrap(), "/home/u/restic");
        assert_eq!(
            expand_repo_url("local:~/restic", h).unwrap(),
            "local:/home/u/restic"
        );
        assert_eq!(expand_repo_url("sftp:host:~/r", h).unwrap(), "sftp:host:~/r");
        assert_eq!(
            expand_repo_url("s3:https://x/$HOME", h).unwrap(),
            "s3:https://x/$HOME"
        );
    }

    #[test]
    fn config_paths_are_expanded() {
        let cfg = Config::parse_with_home(
            r#"
project_name = "app"
[repository]
url = "~/backup/restic"
password_file = "~/.config/autobup/app.pass"
url_file = "$HOME/repo"
[backup]
paths = ["~/Documents", "/srv/data"]
exclude_file = "${HOME}/.config/autobup/excludes"
excludes = ["~/keep-as-is"]
[runtime]
log_dir = "~/.local/state/autobup"
lock_dir = "$HOME/.cache"
[hooks]
pre_backup = "echo ~"
"#,
            Some("/home/u"),
        )
        .unwrap();
        assert_eq!(cfg.repository.url.as_deref(), Some("/home/u/backup/restic"));
        assert_eq!(
            cfg.repository.password_file.as_deref(),
            Some("/home/u/.config/autobup/app.pass")
        );
        assert_eq!(cfg.repository.url_file.as_deref(), Some("/home/u/repo"));
        assert_eq!(cfg.backup.paths, ["/home/u/Documents", "/srv/data"]);
        assert_eq!(
            cfg.backup.exclude_file.as_deref(),
            Some("/home/u/.config/autobup/excludes")
        );
        assert_eq!(cfg.backup.excludes, ["~/keep-as-is"]);
        assert_eq!(cfg.runtime.log_dir, PathBuf::from("/home/u/.local/state/autobup"));
        assert_eq!(cfg.runtime.lock_dir, PathBuf::from("/home/u/.cache"));
        assert_eq!(cfg.hooks.pre_backup.as_deref(), Some("echo ~"));
    }

    #[test]
    fn bot_token_from_file() {
        let dir = std::env::temp_dir().join(format!("autobup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, "123:abc\n").unwrap();
        let cfg = Config::parse(&format!(
            "project_name = \"app\"\n[telegram]\nbot_token_file = {:?}\nchat_id = \"1\"",
            file.to_str().unwrap()
        ))
        .unwrap();
        assert!(cfg.telegram_configured());
        assert_eq!(cfg.bot_token().unwrap().as_deref(), Some("123:abc"));

        std::fs::write(&file, "  \n").unwrap();
        assert!(cfg.bot_token().unwrap_err().contains("is empty"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(cfg.bot_token().unwrap_err().contains("cannot read"));
    }

    #[test]
    fn bot_token_and_file_are_exclusive() {
        let err =
            Config::parse("project_name = \"app\"\n[telegram]\nbot_token = \"x\"\nbot_token_file = \"/t\"")
                .unwrap_err();
        assert!(err.contains("only one of"));
    }

    #[test]
    fn bot_token_file_is_expanded() {
        let cfg = Config::parse_with_home(
            "project_name = \"app\"\n[telegram]\nbot_token_file = \"~/tg.token\"",
            Some("/home/u"),
        )
        .unwrap();
        assert_eq!(cfg.telegram.bot_token_file.as_deref(), Some("/home/u/tg.token"));
    }

    #[test]
    fn bad_project_name_is_rejected() {
        assert!(Config::parse("project_name = \"a/b\"").is_err());
        assert!(Config::parse("project_name = \"\"").is_err());
    }
}
