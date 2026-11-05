//! `restic <args>`: run any restic command with this app's settings.

use std::os::unix::process::CommandExt;

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{config, restic};

pub fn execute(ctx: &Ctx, args: &[String]) -> Result<()> {
    config::validate(&ctx.cfg, &ctx.config_path)?;
    // exec only returns on failure
    let err = restic::command(&ctx.cfg).args(args).exec();
    Err(AppError::fail(format!("cannot run restic: {err}")))
}
