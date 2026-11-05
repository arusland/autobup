//! User hooks: shell snippets from `[hooks]`, run with `bash -eo pipefail`.

use std::process::Command;

use crate::config::{Config, non_empty};
use crate::error::{AppError, Result};
use crate::{log, proc, restic};

/// Run hook `name` if it is configured. `status` is passed as `$1`.
pub fn run(cfg: &Config, name: &str, script: &Option<String>, status: Option<i32>) -> Result<()> {
    let Some(script) = non_empty(script) else {
        return Ok(());
    };
    log::info(format!("running hook: {name}"));
    let mut cmd = Command::new("bash");
    cmd.args(["-eo", "pipefail", "-c", script, "autobup"])
        .envs(restic::env_pairs(cfg))
        .env("AUTOBUP_PROJECT", &cfg.project_name)
        .env("AUTOBUP_HOOK", name);
    if let Some(s) = status {
        cmd.arg(s.to_string()).env("AUTOBUP_STATUS", s.to_string());
    }
    let status =
        proc::run_logged(&mut cmd).map_err(|e| AppError::fail(format!("cannot run hook {name}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::fail(format!(
            "hook {name} failed with exit code {}",
            proc::exit_code(status)
        )))
    }
}
