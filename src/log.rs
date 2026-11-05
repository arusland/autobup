//! Timestamped log lines on stdout/stderr, optionally copied to a log file.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static PROJECT: OnceLock<String> = OnceLock::new();
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

pub fn set_project(name: &str) {
    let _ = PROJECT.set(name.to_string());
}

/// Copy all further output (log lines and output of child processes run via
/// `proc::run_logged`) to `path`, appending.
pub fn enable_file(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    *LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    Ok(())
}

pub fn file_active() -> bool {
    LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

fn line(msg: &str) -> String {
    let project = PROJECT.get().map(String::as_str).unwrap_or("autobup");
    format!(
        "{} [{}] {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        project,
        msg
    )
}

pub fn info(msg: impl AsRef<str>) {
    raw(line(msg.as_ref()).as_bytes(), false);
}

pub fn warn(msg: impl AsRef<str>) {
    raw(line(&format!("WARNING: {}", msg.as_ref())).as_bytes(), true);
}

pub fn error(msg: impl AsRef<str>) {
    raw(line(&format!("ERROR: {}", msg.as_ref())).as_bytes(), true);
}

/// Write bytes to stdout (or stderr) and to the log file, if one is active.
pub fn raw(bytes: &[u8], to_stderr: bool) {
    if to_stderr {
        let mut err = io::stderr().lock();
        let _ = err.write_all(bytes);
        let _ = err.flush();
    } else {
        let mut out = io::stdout().lock();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }
    if let Some(file) = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = file.write_all(bytes);
    }
}
