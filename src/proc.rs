//! Helpers for running external programs.

use std::env;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::log;

/// True if an executable called `name` is found in PATH.
pub fn in_path(name: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|dir| {
        std::fs::metadata(dir.join(name))
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// Exit code of a finished process; 128+N for a process killed by signal N.
pub fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Run `cmd` with its stdout/stderr going to the terminal and, when a log file
/// is active, also to the log file.
pub fn run_logged(cmd: &mut Command) -> io::Result<ExitStatus> {
    if !log::file_active() {
        return cmd.status();
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let out = child.stdout.take().map(|r| thread::spawn(move || pump(r, false)));
    let err = child.stderr.take().map(|r| thread::spawn(move || pump(r, true)));
    let status = child.wait()?;
    for t in [out, err].into_iter().flatten() {
        let _ = t.join();
    }
    Ok(status)
}

fn pump(mut r: impl Read, to_stderr: bool) {
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => log::raw(&buf[..n], to_stderr),
        }
    }
}

pub struct Captured {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Captured {
    /// Last non-empty line of stderr, or of stdout if stderr is empty.
    pub fn last_line(&self) -> &str {
        last_line(&self.stderr)
            .or_else(|| last_line(&self.stdout))
            .unwrap_or("")
    }
}

pub fn last_line(s: &str) -> Option<&str> {
    s.lines().map(str::trim).rfind(|l| !l.is_empty())
}

/// Run `cmd` with `input` on stdin and capture its output. The process is
/// killed after `timeout`.
pub fn run_capture(cmd: &mut Command, input: Option<String>, timeout: Duration) -> io::Result<Captured> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let (Some(mut stdin), Some(data)) = (child.stdin.take(), input) {
        thread::spawn(move || {
            let _ = stdin.write_all(data.as_bytes());
        });
    }
    let read_all = |r: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let mut buf = Vec::new();
                let _ = r.read_to_end(&mut buf);
                s = String::from_utf8_lossy(&buf).into_owned();
            }
            let _ = tx.send(s);
        });
        rx
    };
    let out = read_all(child.stdout.take().map(|r| Box::new(r) as Box<dyn Read + Send>));
    let err = read_all(child.stderr.take().map(|r| Box::new(r) as Box<dyn Read + Send>));

    let Some(status) = child.wait_timeout(timeout)? else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out after {}s", timeout.as_secs()),
        ));
    };
    // Grandchildren (e.g. ssh started by sftp) may keep the pipes open for a
    // moment; do not wait for them forever.
    let grace = Duration::from_secs(5);
    Ok(Captured {
        success: status.success(),
        stdout: out.recv_timeout(grace).unwrap_or_default(),
        stderr: err.recv_timeout(grace).unwrap_or_default(),
    })
}

/// Run `f` on a helper thread and give up waiting after `timeout` (e.g. a
/// statvfs call on a hung NFS mount).
pub fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(timeout).ok()
}
