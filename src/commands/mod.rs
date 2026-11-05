//! One module per CLI command.

use std::path::PathBuf;

use crate::config::Config;

pub mod config;
pub mod dry_run;
pub mod init;
pub mod notify_test;
pub mod pwd;
pub mod restic;
pub mod run;
pub mod space;
pub mod view;

/// Everything a command needs.
pub struct Ctx {
    pub cfg: Config,
    pub config_path: PathBuf,
}

impl Ctx {
    /// `autobup -c <config> <cmd>`, for hints in error messages.
    pub fn hint(&self, cmd: &str) -> String {
        format!("autobup -c {} {cmd}", self.config_path.display())
    }
}
