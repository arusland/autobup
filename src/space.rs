//! Free space and size of the repository target (local and sftp: repositories).
//!
//! Output looks like
//!   repository size 4.2G, 16G free of 116G (87% used) at /mnt/backup/restic (/dev/sdb1 on /mnt/backup)
//!   repository size 800G, 1.2T free of 5.0T (76% used) at backup@host:/srv/restic/myapp

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::config::Config;
use crate::{log, proc, restic};

const DF_TIMEOUT: Duration = Duration::from_secs(60);
const SIZE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, PartialEq)]
pub enum SpaceError {
    /// Backend without a free-space concept (s3, b2, rest, ...).
    Unsupported,
    /// The value could not be read; a warning was logged.
    Failed,
}

/// Repository location restic will use, if any.
pub fn repository(cfg: &Config) -> Option<String> {
    if let Some(r) = restic::effective_env(cfg, "RESTIC_REPOSITORY") {
        return Some(r);
    }
    let file = restic::effective_env(cfg, "RESTIC_REPOSITORY_FILE")?;
    log::step(format!("reading repository location from {file}"));
    match std::fs::read_to_string(&file) {
        Ok(s) => s
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty()),
        Err(e) => {
            log::warn(format!("cannot read {file}: {e}"));
            None
        }
    }
}

pub fn target_free_space(cfg: &Config) -> Result<String, SpaceError> {
    let repo = repository(cfg).ok_or(SpaceError::Failed)?;
    if repo.starts_with("sftp:") {
        log::step("repository backend: sftp");
        return sftp_free_space(&repo);
    }
    const REMOTE: [&str; 7] = ["s3:", "b2:", "rest:", "rclone:", "swift:", "azure:", "gs:"];
    if let Some(p) = REMOTE.iter().find(|p| repo.starts_with(*p)) {
        // The URL is not logged: rest: and others may hold credentials.
        log::step(format!(
            "repository backend: {}, no free space available",
            p.trim_end_matches(':')
        ));
        return Err(SpaceError::Unsupported);
    }
    let path = repo.strip_prefix("local:").unwrap_or(&repo);
    log::step(format!("repository backend: local, path {path}"));
    local_free_space(path)
}

fn fail<T>(msg: String) -> Result<T, SpaceError> {
    log::warn(msg);
    Err(SpaceError::Failed)
}

/// Like `numfmt --to=iec` / `df -h`: rounds up, one decimal below 10.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 8] = ["K", "M", "G", "T", "P", "E", "Z", "Y"];
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    value /= 1024.0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        let v = (value * 10.0).ceil() / 10.0;
        if v < 10.0 {
            return format!("{v:.1}{}", UNITS[unit]);
        }
        value = v;
    }
    let v = value.ceil();
    if v >= 1024.0 && unit < UNITS.len() - 1 {
        return format!("1.0{}", UNITS[unit + 1]);
    }
    format!("{v:.0}{}", UNITS[unit])
}

fn percent_used(used: u64, avail: u64) -> String {
    let total = used + avail;
    if total == 0 {
        return "-".into();
    }
    format!("{}%", (used * 100).div_ceil(total))
}

// ---------------------------------------------------------------------------
// local
// ---------------------------------------------------------------------------

/// A missing folder is an error and not a reason to look at its parent: an
/// unmounted backup disk would otherwise be reported with the free space of
/// the disk below the mount point.
fn local_free_space(path: &str) -> Result<String, SpaceError> {
    log::step(format!("reading file system stats of {path}"));
    let p = path.to_string();
    let stat = match proc::with_timeout(DF_TIMEOUT, move || nix::sys::statvfs::statvfs(p.as_str())) {
        None => return fail(format!("cannot read free space of {path}: timed out")),
        Some(Err(e)) => return fail(format!("cannot read free space of {path}: {e}")),
        Some(Ok(s)) => s,
    };
    let frsize = stat.fragment_size() as u64;
    let size = stat.blocks() as u64 * frsize;
    let avail = stat.blocks_available() as u64 * frsize;
    let used = (stat.blocks() as u64).saturating_sub(stat.blocks_free() as u64) * frsize;
    log::step(format!("file system: {} free of {}", human(avail), human(size)));
    log::step("looking up device and mount point in /proc/self/mountinfo");
    let (dev, mount) = find_mount(Path::new(path)).unwrap_or_else(|| ("?".into(), "?".into()));
    log::step(format!("mounted: {dev} on {mount}"));
    let repo_size = local_repo_size(path)
        .map(|s| format!("repository size {s}, "))
        .unwrap_or_default();
    Ok(format!(
        "{repo_size}{} free of {} ({} used) at {path} ({dev} on {mount})",
        human(avail),
        human(size),
        percent_used(used, avail)
    ))
}

/// Disk usage of a folder, like `du -s`. Unreadable entries are an error, since
/// a partial total would look like a valid size.
fn local_repo_size(path: &str) -> Option<String> {
    log::step(format!("measuring repository size of {path}"));
    let start = Instant::now();
    let p = path.to_string();
    let result = proc::with_timeout(SIZE_TIMEOUT, move || -> Result<(u64, u64), String> {
        let (mut total, mut files) = (0u64, 0u64);
        for entry in walkdir::WalkDir::new(&p) {
            let meta = entry.and_then(|e| e.metadata()).map_err(|e| e.to_string())?;
            total += meta.blocks() * 512;
            files += u64::from(meta.is_file());
        }
        Ok((total, files))
    });
    match result {
        Some(Ok((bytes, files))) => {
            log::step(format!(
                "repository size: {} in {files} files ({:.1}s)",
                human(bytes),
                start.elapsed().as_secs_f64()
            ));
            Some(human(bytes))
        }
        Some(Err(e)) => {
            log::warn(format!("cannot read repository size of {path}: {e}"));
            None
        }
        None => {
            log::warn(format!("cannot read repository size of {path}: timed out"));
            None
        }
    }
}

/// Device and mount point of the file system holding `path`, from /proc/self/mountinfo.
fn find_mount(path: &Path) -> Option<(String, String)> {
    let path = path.canonicalize().ok()?;
    let dev = std::fs::metadata(&path).ok()?.dev();
    let dev_id = format!("{}:{}", nix::sys::stat::major(dev), nix::sys::stat::minor(dev));
    let info = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    parse_mountinfo(&info, &dev_id, &path)
}

fn parse_mountinfo(info: &str, dev_id: &str, path: &Path) -> Option<(String, String)> {
    let mut best: Option<(usize, String, String)> = None;
    for line in info.lines() {
        // id parent major:minor root mountpoint options [optional...] - fstype source superopts
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let left: Vec<&str> = left.split(' ').collect();
        let right: Vec<&str> = right.split(' ').collect();
        if left.len() < 5 || right.len() < 2 || left[2] != dev_id {
            continue;
        }
        let mount = unescape(left[4]);
        let mount_path = PathBuf::from(&mount);
        if !path.starts_with(&mount_path) {
            continue;
        }
        let len = mount_path.as_os_str().len();
        if best.as_ref().is_none_or(|(l, _, _)| len >= *l) {
            best = Some((len, unescape(right[1]), mount));
        }
    }
    best.map(|(_, dev, mount)| (dev, mount))
}

/// mountinfo escapes space, tab, newline and backslash as \ooo.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && let Ok(digits) = std::str::from_utf8(&b[i + 1..i + 4])
            && let Ok(v) = u8::from_str_radix(digits, 8)
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// sftp
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub struct SftpTarget {
    /// `[user@]host`
    pub host: String,
    pub port: Option<String>,
    pub path: String,
}

static RE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^sftp://([^/]+)/(.*)$").unwrap());
static RE_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*):([0-9]+)$").unwrap());
static RE_SCP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^sftp:([^@/]*@)?(\[[^\]]*\]|[^:]+):(.*)$").unwrap());

/// Supports `sftp:[user@]host:path` and `sftp://[user@]host[:port]//abs/path`.
pub fn parse_sftp(repo: &str) -> Option<SftpTarget> {
    let (host, port, path) = if let Some(c) = RE_URL.captures(repo) {
        let host = c[1].to_string();
        match RE_PORT.captures(&host) {
            Some(p) => (p[1].to_string(), Some(p[2].to_string()), c[2].to_string()),
            None => (host, None, c[2].to_string()),
        }
    } else if let Some(c) = RE_SCP.captures(repo) {
        let user = c.get(1).map_or("", |m| m.as_str());
        (format!("{user}{}", &c[2]), None, c[3].to_string())
    } else {
        return None;
    };
    let path = if path.is_empty() { ".".to_string() } else { path };
    Some(SftpTarget { host, port, path })
}

fn sftp_command(t: &SftpTarget) -> Command {
    let mut cmd = Command::new("sftp");
    cmd.args(["-q", "-b", "-", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15"]);
    if let Some(port) = &t.port {
        cmd.args(["-P", port]);
    }
    cmd.arg(&t.host);
    cmd
}

fn quote(path: &str) -> String {
    path.replace('"', "\\\"")
}

/// Uses the sftp `df` command (statvfs extension of OpenSSH), so it also works
/// on SFTP-only accounts without a shell.
fn sftp_free_space(repo: &str) -> Result<String, SpaceError> {
    if !proc::in_path("sftp") {
        return fail("sftp not found, cannot read free space".into());
    }
    let t = parse_sftp(repo).ok_or(SpaceError::Failed)?;
    let location = format!(
        "{}{}:{}",
        t.host,
        t.port.as_ref().map(|p| format!(":{p}")).unwrap_or_default(),
        t.path
    );
    let input = format!("df -h \"{}\"\n", quote(&t.path));
    log::step(format!("reading free space of {location} with sftp df"));
    let mut cmd = sftp_command(&t);
    restic::show(&cmd);
    let out = match proc::run_capture(&mut cmd, Some(input), DF_TIMEOUT) {
        Err(e) => return fail(format!("cannot read free space of {}:{}: {e}", t.host, t.path)),
        Ok(o) if !o.success => {
            return fail(format!(
                "cannot read free space of {}:{}: {}",
                t.host,
                t.path,
                o.last_line()
            ));
        }
        Ok(o) => o,
    };
    let Some((size, avail, cap)) = parse_sftp_df(&out.stdout) else {
        return fail(format!("unexpected sftp df output: {}", out.last_line()));
    };
    log::step(format!("file system: {avail} free of {size}"));
    let repo_size = sftp_repo_size(&t)
        .map(|s| format!("repository size {s}, "))
        .unwrap_or_default();
    Ok(format!(
        "{repo_size}{avail} free of {size} ({cap} used) at {location}"
    ))
}

/// Last line of sftp `df -h`: Size Used Avail (root) %Capacity.
fn parse_sftp_df(out: &str) -> Option<(String, String, String)> {
    let f: Vec<&str> = proc::last_line(out)?.split_whitespace().collect();
    if f.len() < 5 || !f[4].ends_with('%') {
        return None;
    }
    Some((f[0].into(), f[2].into(), f[4].into()))
}

/// Total file size of the repository on the sftp server, without a shell on
/// the server or the repository password. sftp has no recursive listing, so
/// this lists each folder of the restic layout. A leading "-" keeps the batch
/// going when one of them is missing.
fn sftp_repo_size(t: &SftpTarget) -> Option<String> {
    let path = quote(t.path.trim_end_matches('/'));
    let mut input = String::new();
    for sub in ["config", "index", "keys", "snapshots", "locks"] {
        input.push_str(&format!("-ls -ln \"{path}/{sub}\"\n"));
    }
    for i in 0..256 {
        input.push_str(&format!("-ls -ln \"{path}/data/{i:02x}\"\n"));
    }
    let where_ = format!("{}:{}", t.host, t.path);
    log::step(format!(
        "measuring repository size of {where_} (listing 261 folders with sftp)"
    ));
    let start = Instant::now();
    let mut cmd = sftp_command(t);
    restic::show(&cmd);
    let out = match proc::run_capture(&mut cmd, Some(input), SIZE_TIMEOUT) {
        Err(e) => {
            log::warn(format!("cannot read repository size of {where_}: {e}"));
            return None;
        }
        Ok(o) if !o.success => {
            log::warn(format!(
                "cannot read repository size of {where_}: {}",
                o.last_line()
            ));
            return None;
        }
        Ok(o) => o,
    };
    match sum_ls(&out.stdout) {
        0 => {
            log::warn(format!("no repository files found at {where_}"));
            None
        }
        total => {
            log::step(format!(
                "repository size: {} ({:.1}s)",
                human(total),
                start.elapsed().as_secs_f64()
            ));
            Some(human(total))
        }
    }
}

/// Sum the sizes of regular files in `ls -ln` output
/// (`-rw------- 1 uid gid size month day time name`).
fn sum_ls(out: &str) -> u64 {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 5 && f[0].starts_with('-'))
                .then(|| f[4].parse::<u64>().ok())
                .flatten()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_matches_numfmt() {
        assert_eq!(human(0), "0");
        assert_eq!(human(1023), "1023");
        assert_eq!(human(1024), "1.0K");
        assert_eq!(human(1536), "1.5K");
        assert_eq!(human(1025), "1.1K");
        assert_eq!(human(10 * 1024), "10K");
        assert_eq!(human(10 * 1024 + 1), "11K");
        assert_eq!(human(1024 * 1024 - 1), "1.0M");
        assert_eq!(human(5 * 1024 * 1024 * 1024 * 1024), "5.0T");
    }

    #[test]
    fn percent() {
        assert_eq!(percent_used(76, 24), "76%");
        assert_eq!(percent_used(1, 199), "1%");
        assert_eq!(percent_used(0, 0), "-");
    }

    #[test]
    fn sftp_scp_form() {
        assert_eq!(
            parse_sftp("sftp:backup@host.example:/srv/restic/app"),
            Some(SftpTarget {
                host: "backup@host.example".into(),
                port: None,
                path: "/srv/restic/app".into()
            })
        );
        assert_eq!(
            parse_sftp("sftp:host:"),
            Some(SftpTarget {
                host: "host".into(),
                port: None,
                path: ".".into()
            })
        );
        assert_eq!(
            parse_sftp("sftp:u@[::1]:repo"),
            Some(SftpTarget {
                host: "u@[::1]".into(),
                port: None,
                path: "repo".into()
            })
        );
    }

    #[test]
    fn sftp_url_form() {
        assert_eq!(
            parse_sftp("sftp://u@host:2222//srv/restic"),
            Some(SftpTarget {
                host: "u@host".into(),
                port: Some("2222".into()),
                path: "/srv/restic".into()
            })
        );
        assert_eq!(
            parse_sftp("sftp://host/rel"),
            Some(SftpTarget {
                host: "host".into(),
                port: None,
                path: "rel".into()
            })
        );
        assert_eq!(parse_sftp("sftp:nocolon"), None);
    }

    #[test]
    fn sftp_df_output() {
        let out = "sftp> df -h \"/srv\"\n    Size     Used    Avail   (root)    %Capacity\n   5.0TB    3.8TB    1.2TB    1.2TB          76%\n";
        assert_eq!(
            parse_sftp_df(out),
            Some(("5.0TB".into(), "1.2TB".into(), "76%".into()))
        );
        assert_eq!(parse_sftp_df("garbage"), None);
    }

    #[test]
    fn ls_sum() {
        let out = "-rw-------    1 1000     1000          155 Nov  5 03:15 /r/config\n\
                   drwx------    2 1000     1000         4096 Nov  5 03:15 /r/data/00\n\
                   -rw-------    1 1000     1000         1000 Nov  5 03:15 /r/keys/abc\n\
                   Can't ls: \"/r/locks\" not found\n";
        assert_eq!(sum_ls(out), 1155);
    }

    #[test]
    fn mountinfo_longest_prefix() {
        let info = "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
                    40 22 8:17 / /mnt/back\\040up rw,relatime shared:2 - ext4 /dev/sdb1 rw\n";
        assert_eq!(
            parse_mountinfo(info, "8:17", Path::new("/mnt/back up/restic")),
            Some(("/dev/sdb1".into(), "/mnt/back up".into()))
        );
        assert_eq!(
            parse_mountinfo(info, "8:1", Path::new("/home/x")),
            Some(("/dev/sda1".into(), "/".into()))
        );
    }
}
