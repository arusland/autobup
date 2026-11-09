//! `pwd`: add a generated repository password to the project's secrets file
//! as `PASSWORD=...`. Never changes an existing PASSWORD.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{log, secrets};

const VAR: &str = "PASSWORD";

/// Header of a new secrets file.
const TEMPLATE: &str = "\
# autobup secrets, mode 600. Use them in the config as \"${NAME}\".
# They are also passed to restic and the hooks as environment variables.

# Telegram bot token, for telegram.bot_token = \"${TELEGRAM_BOT_TOKEN}\"
# TELEGRAM_BOT_TOKEN=123456789:AAE...

# Repository password, for repository.password = \"${PASSWORD}\"
";

pub fn execute(ctx: &Ctx) -> Result<()> {
    let path = ctx
        .cfg
        .secrets_file
        .as_deref()
        .ok_or_else(|| AppError::config("HOME is not set, cannot locate the secrets file"))?;
    let file = path.display().to_string();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AppError::fail(format!("cannot create directory for {file}: {e}")))?;
    }

    // Append-only: existing lines are never rewritten. New files get mode 600.
    let mut f = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| AppError::fail(format!("cannot open secrets file {file}: {e}")))?;
    let mut text = String::new();
    f.read_to_string(&mut text)
        .map_err(|e| AppError::fail(format!("cannot read secrets file {file}: {e}")))?;
    let vars = secrets::parse(&text).map_err(|e| AppError::config(format!("secrets file {file}: {e}")))?;
    if vars.contains_key(VAR) {
        return Err(AppError::fail(format!("{VAR} is already set in {file}")));
    }

    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let pass = STANDARD.encode(bytes);
    let prefix = if text.is_empty() {
        TEMPLATE
    } else if text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    writeln!(f, "{prefix}{VAR}={pass}")
        .and_then(|_| f.sync_all())
        .map_err(|e| AppError::fail(format!("cannot write secrets file {file}: {e}")))?;

    log::info(format!("{VAR} written to {file}"));
    if !ctx.cfg.vars_used.contains(VAR) {
        log::warn(format!(
            "the config does not use it yet: set password = \"${{{VAR}}}\" in [repository] of {}",
            ctx.config_path.display()
        ));
    }
    red_box(&[
        "IMPORTANT: COPY THIS SECRETS FILE SOMEWHERE ELSE NOW",
        "",
        &file,
        "",
        "Keep the copy off this machine. If the password is lost,",
        "the backups cannot be restored.",
    ]);
    Ok(())
}

/// Print the lines in a box; red only on a terminal.
fn red_box(lines: &[&str]) {
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let (on, off) = if std::io::stdout().is_terminal() {
        ("\x1b[1;31m", "\x1b[0m")
    } else {
        ("", "")
    };
    let bar = "═".repeat(width + 2);
    println!("{on}╔{bar}╗{off}");
    for l in lines {
        let pad = " ".repeat(width - l.chars().count());
        println!("{on}║ {l}{pad} ║{off}");
    }
    println!("{on}╚{bar}╝{off}");
}
