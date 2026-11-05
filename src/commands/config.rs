//! `config`: create a config file for a project directory. The project name is
//! the directory's name. Never overwrites an existing file.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::log;

/// Config file name inside the project directory when `-o` is not given.
pub const DEFAULT_FILE: &str = "autobup.toml";

const TEMPLATE: &str = include_str!("config_template.toml");

pub fn execute(dir: &Path, output: Option<&Path>) -> Result<()> {
    let dir = std::fs::canonicalize(dir)
        .map_err(|e| AppError::config(format!("cannot open project directory {}: {e}", dir.display())))?;
    if !dir.is_dir() {
        return Err(AppError::config(format!("not a directory: {}", dir.display())));
    }
    let name = project_name(&dir)?;
    let dir_str = dir
        .to_str()
        .ok_or_else(|| AppError::config(format!("project path is not valid UTF-8: {}", dir.display())))?;

    let output = output.map_or_else(|| dir.join(DEFAULT_FILE), Path::to_path_buf);

    let text = render(&name, dir_str, &output.display().to_string());
    // Should never fail; guards the template against drifting from Config.
    Config::parse(&text).map_err(|e| AppError::fail(format!("generated config is invalid: {e}")))?;

    if output.symlink_metadata().is_ok() {
        return Err(already_exists(&output));
    }
    if let Some(parent) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::fail(format!("cannot create directory {}: {e}", parent.display())))?;
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&output)
        .map_err(|e| match e.kind() {
            ErrorKind::AlreadyExists => already_exists(&output),
            _ => AppError::fail(format!("cannot create config file {}: {e}", output.display())),
        })?;
    f.write_all(text.as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|e| AppError::fail(format!("cannot write config file {}: {e}", output.display())))?;

    log::info(format!("config for project {name:?} written to {}", output.display()));
    log::info(format!(
        "next: review it, then run 'autobup -c {0} pwd' and 'autobup -c {0} init'",
        output.display()
    ));
    Ok(())
}

fn already_exists(output: &Path) -> AppError {
    AppError::config(format!(
        "config file already exists: {0}\n\
         It was not changed. Edit it and use it with 'autobup -c {0}',\n\
         or write a new config elsewhere with 'autobup config DIR -o FILE'.",
        output.display()
    ))
}

/// The directory's own name, usable as `project_name`.
fn project_name(dir: &Path) -> Result<String> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::config(format!("cannot get a project name from {}", dir.display())))?;
    if name.starts_with('.') || name.chars().any(char::is_control) {
        return Err(AppError::config(format!(
            "directory name {name:?} cannot be used as a project name"
        )));
    }
    Ok(name.to_string())
}

fn render(name: &str, dir: &str, config: &str) -> String {
    TEMPLATE
        .replace("__NAME_VALUE__", &quote(name))
        .replace("__PATH_VALUE__", &quote(dir))
        .replace("__CONFIG__", config)
        .replace("__NAME__", &escape(name))
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

/// Escape `s` for use inside a TOML basic string.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_config_parses() {
        let text = render("my \"app\"", "/srv/my \"app\"", "/etc/autobup/x.toml");
        let cfg = Config::parse(&text).unwrap();
        assert_eq!(cfg.project_name, "my \"app\"");
        assert_eq!(cfg.backup.paths, ["/srv/my \"app\""]);
        assert!(cfg.repository.password_file.unwrap().ends_with("/my \"app\".pass"));
        assert_eq!(cfg.retention.keep_daily, Some(7));
        assert!(!text.contains("__"), "unreplaced placeholder");
    }

    #[test]
    fn name_from_dir() {
        assert_eq!(project_name(Path::new("/srv/shop")).unwrap(), "shop");
        assert!(project_name(Path::new("/srv/.hidden")).is_err());
        assert!(project_name(Path::new("/")).is_err());
    }
}
