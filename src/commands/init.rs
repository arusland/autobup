//! `init`: initialise the repository.

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{config, proc, restic};

pub fn execute(ctx: &Ctx) -> Result<()> {
    config::validate(&ctx.cfg, &ctx.config_path)?;
    let mut cmd = restic::command(&ctx.cfg);
    cmd.arg("init");
    restic::show(&cmd);
    let status = cmd
        .status()
        .map_err(|e| AppError::fail(format!("cannot run restic: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::exit(proc::exit_code(status)))
    }
}
