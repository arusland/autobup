//! Building and running restic commands. restic is always run as `restic` from PATH.

use std::process::Command;

use crate::config::{Config, non_empty};
use crate::{log, proc};

pub const RESTIC: &str = "restic";

/// Environment passed to restic and the hooks: repository settings first,
/// then the `[env]` table (which may override them).
pub fn env_pairs(cfg: &Config) -> Vec<(String, String)> {
    let r = &cfg.repository;
    let mut env: Vec<(String, String)> = [
        ("RESTIC_REPOSITORY", &r.url),
        ("RESTIC_REPOSITORY_FILE", &r.url_file),
        ("RESTIC_PASSWORD_FILE", &r.password_file),
        ("RESTIC_PASSWORD", &r.password),
        ("RESTIC_PASSWORD_COMMAND", &r.password_command),
    ]
    .into_iter()
    .filter_map(|(k, v)| non_empty(v).map(|v| (k.to_string(), v.to_string())))
    .collect();
    env.extend(cfg.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// Value restic will see for `key`: from the config, else from our own environment.
pub fn effective_env(cfg: &Config, key: &str) -> Option<String> {
    env_pairs(cfg)
        .into_iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .or_else(|| std::env::var(key).ok())
        .filter(|v| !v.trim().is_empty())
}

/// `restic` with this app's environment.
pub fn command(cfg: &Config) -> Command {
    let mut cmd = Command::new(RESTIC);
    cmd.envs(env_pairs(cfg));
    cmd
}

/// Log the command line of `cmd` (program and arguments, not the environment,
/// which may hold secrets) before it is run.
pub fn show(cmd: &Command) {
    let argv = std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| shell_quote(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    log::command(&argv);
}

/// Quote `s` for a POSIX shell, leaving plain words as they are.
fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%^".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `restic` under `nice` and `ionice` (where available) with this app's environment.
pub fn niced(cfg: &Config) -> Command {
    let mut argv: Vec<String> = Vec::new();
    if proc::in_path("nice") {
        argv.extend(["nice".into(), "-n".into(), cfg.runtime.nice_level.to_string()]);
    }
    if proc::in_path("ionice") {
        argv.extend(["ionice".into(), "-c".into(), cfg.runtime.ionice_class.to_string()]);
    }
    argv.push(RESTIC.into());
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).envs(env_pairs(cfg));
    cmd
}

/// Arguments for `restic backup` (without the paths).
pub fn backup_args(cfg: &Config, dry: bool) -> Vec<String> {
    let b = &cfg.backup;
    let mut args = vec!["--tag".to_string(), cfg.project_tag()];
    for t in &b.tags {
        args.extend(["--tag".into(), t.clone()]);
    }
    for e in &b.excludes {
        args.extend(["--exclude".into(), e.clone()]);
    }
    if let Some(f) = non_empty(&b.exclude_file) {
        args.extend(["--exclude-file".into(), f.into()]);
    }
    if b.exclude_caches {
        args.push("--exclude-caches".into());
    }
    if b.one_file_system {
        args.push("--one-file-system".into());
    }
    if let Some(h) = non_empty(&b.host) {
        args.extend(["--host".into(), h.into()]);
    }
    if dry {
        args.extend(["--dry-run".into(), "-v".into()]);
    }
    args.extend(b.extra_args.iter().cloned());
    args
}

/// Arguments for `restic forget`, or None if no keep policy is set.
pub fn forget_args(cfg: &Config, dry: bool) -> Option<Vec<String>> {
    let r = &cfg.retention;
    let mut keep: Vec<String> = [
        ("--keep-last", r.keep_last),
        ("--keep-hourly", r.keep_hourly),
        ("--keep-daily", r.keep_daily),
        ("--keep-weekly", r.keep_weekly),
        ("--keep-monthly", r.keep_monthly),
        ("--keep-yearly", r.keep_yearly),
    ]
    .into_iter()
    .filter_map(|(flag, v)| v.map(|n| [flag.to_string(), n.to_string()]))
    .flatten()
    .collect();
    if let Some(w) = non_empty(&r.keep_within) {
        keep.extend(["--keep-within".into(), w.into()]);
    }
    if keep.is_empty() {
        return None;
    }
    let mut args = vec![
        "--tag".into(),
        cfg.project_tag(),
        "--group-by".into(),
        "host,tags".into(),
    ];
    args.extend(keep);
    if dry {
        args.push("--dry-run".into());
    } else if r.prune {
        args.push("--prune".into());
    }
    Some(args)
}

/// Arguments for `restic check`.
pub fn check_args(cfg: &Config) -> Vec<String> {
    match non_empty(&cfg.check.read_data_subset) {
        Some(s) => vec!["--read-data-subset".into(), s.into()],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(extra: &str) -> Config {
        Config::parse(&format!("project_name = \"app\"\n{extra}")).unwrap()
    }

    #[test]
    fn shell_quote_args() {
        assert_eq!(shell_quote("--tag"), "--tag");
        assert_eq!(shell_quote("project:app"), "project:app");
        assert_eq!(shell_quote("/a b"), "'/a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn backup_args_defaults() {
        assert_eq!(
            backup_args(&cfg(""), false),
            ["--tag", "project:app", "--exclude-caches"]
        );
    }

    #[test]
    fn backup_args_full() {
        let c = cfg(r#"
[backup]
tags = ["daily"]
excludes = ["*.tmp"]
exclude_file = "/x"
exclude_caches = false
one_file_system = true
host = "h"
extra_args = ["--compression", "max"]
"#);
        assert_eq!(
            backup_args(&c, true),
            [
                "--tag",
                "project:app",
                "--tag",
                "daily",
                "--exclude",
                "*.tmp",
                "--exclude-file",
                "/x",
                "--one-file-system",
                "--host",
                "h",
                "--dry-run",
                "-v",
                "--compression",
                "max"
            ]
        );
    }

    #[test]
    fn forget_args_none_without_policy() {
        assert!(forget_args(&cfg(""), false).is_none());
    }

    #[test]
    fn forget_args_prune_and_dry() {
        let c = cfg("[retention]\nkeep_daily = 7\nkeep_within = \"2d\"");
        let base = [
            "--tag",
            "project:app",
            "--group-by",
            "host,tags",
            "--keep-daily",
            "7",
            "--keep-within",
            "2d",
        ];
        let mut wet = base.to_vec();
        wet.push("--prune");
        assert_eq!(forget_args(&c, false).unwrap(), wet);
        let mut dry = base.to_vec();
        dry.push("--dry-run");
        assert_eq!(forget_args(&c, true).unwrap(), dry);
    }

    #[test]
    fn env_table_overrides_repository() {
        let c = cfg("[repository]\nurl = \"/a\"\n[env]\nRESTIC_REPOSITORY = \"/b\"");
        assert_eq!(effective_env(&c, "RESTIC_REPOSITORY").as_deref(), Some("/b"));
    }
}
