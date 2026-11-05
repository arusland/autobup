//! One run per project at a time.

use std::fs::{File, OpenOptions, TryLockError};

use crate::config::Config;
use crate::error::{AppError, EXIT_LOCKED, Result};

/// Held for the whole run; the lock is released when it is dropped.
pub struct Lock(#[allow(dead_code)] File);

pub fn acquire(cfg: &Config) -> Result<Lock> {
    let dir = &cfg.runtime.lock_dir;
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::fail(format!("cannot create lock dir {}: {e}", dir.display())))?;
    let path = dir.join(format!("autobup-{}.lock", cfg.project_name));
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| AppError::fail(format!("cannot open lock file {}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(Lock(file)),
        Err(TryLockError::WouldBlock) => Err(AppError::new(
            EXIT_LOCKED,
            format!("another backup of '{}' is already running", cfg.project_name),
        )),
        Err(TryLockError::Error(e)) => Err(AppError::fail(format!("cannot lock {}: {e}", path.display()))),
    }
}
