//! `run`: pre-hook -> backup -> post-hook -> forget/prune -> check.
//!
//! Every failure after the start is reported: post_backup still runs once
//! pre_backup has started, then on_failure, the healthcheck and Telegram.

use std::path::Path;
use std::time::Instant;

use super::Ctx;
use crate::error::{AppError, EXIT_PARTIAL, Result};
use crate::lock::{self, Lock};
use crate::{config, healthcheck, hooks, log, proc, restic, telegram};

#[derive(Default)]
struct State {
    pre_started: bool,
    post_done: bool,
    partial: bool,
    lock: Option<Lock>,
}

pub fn execute(ctx: &Ctx, dry: bool) -> Result<()> {
    let start = Instant::now();
    let mut state = State::default();
    let result = steps(ctx, dry, &mut state);
    finalize(ctx, dry, &mut state, result, start)
}

fn steps(ctx: &Ctx, dry: bool, st: &mut State) -> Result<()> {
    let cfg = &ctx.cfg;
    config::validate(cfg, &ctx.config_path)?;
    st.lock = Some(lock::acquire(cfg)?);
    setup_logging(ctx);
    log::info(format!(
        "===== starting {}backup",
        if dry { "dry-run " } else { "" }
    ));
    if !dry {
        healthcheck::ping(cfg, "/start");
    }

    ensure_repo(ctx)?;

    st.pre_started = true;
    hooks::run(cfg, "pre_backup", &cfg.hooks.pre_backup, None)?;

    st.partial = backup(ctx, dry)?;

    st.post_done = true;
    hooks::run(cfg, "post_backup", &cfg.hooks.post_backup, Some(0))?;

    forget(ctx, dry)?;
    if !dry {
        check(ctx)?;
    }
    Ok(())
}

fn finalize(ctx: &Ctx, dry: bool, st: &mut State, result: Result<()>, start: Instant) -> Result<()> {
    let cfg = &ctx.cfg;
    let (mut code, error) = match &result {
        Ok(()) => (0, None),
        Err(e) => {
            log::error(&e.msg);
            (e.code, Some(e.msg.as_str()))
        }
    };
    if st.pre_started && !st.post_done {
        st.post_done = true;
        if let Err(e) = hooks::run(cfg, "post_backup", &cfg.hooks.post_backup, Some(code)) {
            log::error(&e.msg);
        }
    }
    let secs = start.elapsed().as_secs();
    if code == 0 {
        if st.partial {
            code = EXIT_PARTIAL;
        }
        log::info(format!("done in {secs}s"));
        if !dry {
            healthcheck::ping(cfg, "");
        }
    } else {
        log::info(format!("FAILED with exit code {code} after {secs}s"));
        if let Err(e) = hooks::run(cfg, "on_failure", &cfg.hooks.on_failure, Some(code)) {
            log::error(&e.msg);
        }
        if !dry {
            healthcheck::ping(cfg, "/fail");
        }
    }
    if !dry {
        telegram::notify_result(cfg, code, error, secs);
    }
    if code == 0 {
        Ok(())
    } else {
        Err(AppError::exit(code))
    }
}

fn setup_logging(ctx: &Ctx) {
    let dir = &ctx.cfg.runtime.log_dir;
    let ok = std::fs::create_dir_all(dir).is_ok() && log::enable_file(&ctx.cfg.log_file()).is_ok();
    if !ok {
        log::warn(format!(
            "log dir '{}' not writable, logging to stdout only",
            dir.display()
        ));
    }
}

fn restic_failed(e: std::io::Error) -> AppError {
    AppError::fail(format!("cannot run restic: {e}"))
}

fn ensure_repo(ctx: &Ctx) -> Result<()> {
    let out = restic::command(&ctx.cfg)
        .args(["cat", "config"])
        .output()
        .map_err(restic_failed)?;
    if out.status.success() {
        return Ok(());
    }
    log::raw(&out.stderr, true);
    Err(AppError::fail(format!(
        "cannot open repository (run '{}' to create it)",
        ctx.hint("init")
    )))
}

/// Returns true if some files could not be read (restic exit code 3).
fn backup(ctx: &Ctx, dry: bool) -> Result<bool> {
    let cfg = &ctx.cfg;
    if cfg.backup.paths.is_empty() {
        return Err(AppError::config("backup.paths is empty"));
    }
    let paths: Vec<&String> = cfg
        .backup
        .paths
        .iter()
        .filter(|p| {
            let exists = Path::new(p).exists();
            if !exists {
                log::warn(format!("path does not exist, skipping: {p}"));
            }
            exists
        })
        .collect();
    if paths.is_empty() {
        return Err(AppError::fail("none of backup.paths exist"));
    }

    log::info(format!(
        "backing up: {}",
        paths.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(" ")
    ));
    let status = proc::run_logged(
        restic::niced(cfg)
            .arg("backup")
            .args(restic::backup_args(cfg, dry))
            .arg("--")
            .args(&paths),
    )
    .map_err(restic_failed)?;
    match proc::exit_code(status) {
        0 => {
            log::info("backup finished");
            Ok(false)
        }
        3 => {
            log::warn("backup finished, but some files could not be read");
            Ok(true)
        }
        c => Err(AppError::fail(format!("backup failed (restic exit code {c})"))),
    }
}

fn forget(ctx: &Ctx, dry: bool) -> Result<()> {
    let cfg = &ctx.cfg;
    let Some(args) = restic::forget_args(cfg, dry) else {
        log::info("no retention policy set, skipping forget");
        return Ok(());
    };
    log::info("applying retention policy");
    let status = proc::run_logged(restic::niced(cfg).arg("forget").args(args)).map_err(restic_failed)?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::fail("forget/prune failed"))
    }
}

fn check(ctx: &Ctx) -> Result<()> {
    let cfg = &ctx.cfg;
    if !cfg.check.enabled {
        return Ok(());
    }
    log::info("checking repository");
    let status = proc::run_logged(restic::niced(cfg).arg("check").args(restic::check_args(cfg)))
        .map_err(restic_failed)?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::fail("repository check failed"))
    }
}
