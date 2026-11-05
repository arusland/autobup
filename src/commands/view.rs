//! `view [N]`: show the latest N snapshots of this app.
//!
//! restic applies --latest per host and path set, so a change in backup.paths
//! can list more than N snapshots in total.

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{config, proc, restic};

pub fn execute(ctx: &Ctx, count: u32) -> Result<()> {
    config::validate(&ctx.cfg, &ctx.config_path)?;
    let status = restic::command(&ctx.cfg)
        .args([
            "snapshots",
            "--tag",
            &ctx.cfg.project_tag(),
            "--latest",
            &count.to_string(),
        ])
        .status()
        .map_err(|e| AppError::fail(format!("cannot run restic: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::exit(proc::exit_code(status)))
    }
}
