//! `pwd`: generate the repository password file. Never overwrites an existing file.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{log, restic};

pub fn execute(ctx: &Ctx) -> Result<()> {
    let file = restic::effective_env(&ctx.cfg, "RESTIC_PASSWORD_FILE")
        .ok_or_else(|| AppError::config("repository.password_file is not set"))?;
    let path = Path::new(&file);
    if path.symlink_metadata().is_ok() {
        return Err(AppError::fail(format!("password file already exists: {file}")));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| AppError::fail(format!("cannot create directory for {file}: {e}")))?;
    }

    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let pass = STANDARD.encode(bytes);

    // create_new makes the create atomic: an existing file is never touched.
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| AppError::fail(format!("cannot create password file {file}: {e}")))?;
    writeln!(f, "{pass}")
        .and_then(|_| f.sync_all())
        .map_err(|e| AppError::fail(format!("cannot write password file {file}: {e}")))?;

    log::info(format!("password written to {file}"));
    red_box(&[
        "IMPORTANT: COPY THIS PASSWORD FILE SOMEWHERE ELSE NOW",
        "",
        &file,
        "",
        "Keep the copy off this machine. If the file is lost,",
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
