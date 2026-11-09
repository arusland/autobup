//! `diff [ARGS]`: `restic diff` with this app's settings. Without arguments it
//! compares the two latest snapshots of this app, i.e. shows the last changes.

use std::os::unix::process::CommandExt;

use serde::Deserialize;

use super::Ctx;
use crate::error::{AppError, Result};
use crate::{config, log, restic};

#[derive(Deserialize)]
struct Snapshot {
    id: String,
    time: String,
}

pub fn execute(ctx: &Ctx, args: &[String]) -> Result<()> {
    config::validate(&ctx.cfg, &ctx.config_path)?;
    let args = if args.is_empty() {
        let (prev, last) = last_two(ctx)?;
        vec![prev, last]
    } else {
        args.to_vec()
    };
    let mut cmd = restic::command(&ctx.cfg);
    cmd.arg("diff").args(&args);
    restic::show(&cmd);
    // exec only returns on failure
    let err = cmd.exec();
    Err(AppError::fail(format!("cannot run restic: {err}")))
}

/// IDs of the second latest and the latest snapshot of this app.
fn last_two(ctx: &Ctx) -> Result<(String, String)> {
    let mut cmd = restic::command(&ctx.cfg);
    cmd.args(["snapshots", "--json", "--tag", &ctx.cfg.project_tag()]);
    restic::show(&cmd);
    let out = cmd
        .output()
        .map_err(|e| AppError::fail(format!("cannot run restic: {e}")))?;
    if !out.status.success() {
        log::raw(&out.stderr, true);
        return Err(AppError::fail("cannot list snapshots"));
    }
    let snapshots: Vec<Snapshot> = serde_json::from_slice(&out.stdout)
        .map_err(|e| AppError::fail(format!("cannot parse restic snapshots output: {e}")))?;
    pick_last_two(snapshots)
        .ok_or_else(|| AppError::fail("need at least two snapshots of this app to show the last changes"))
}

fn pick_last_two(mut snapshots: Vec<Snapshot>) -> Option<(String, String)> {
    // restic writes RFC 3339 times; unparsable ones sort first.
    snapshots.sort_by_cached_key(|s| chrono::DateTime::parse_from_rfc3339(&s.time).ok());
    let last = snapshots.pop()?;
    let prev = snapshots.pop()?;
    Some((prev.id, last.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<Snapshot> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn picks_two_latest_by_time() {
        let s = parse(
            r#"[
            {"id": "b", "time": "2026-01-02T03:00:00.123+01:00", "paths": ["/x"]},
            {"id": "c", "time": "2026-01-03T03:00:00Z"},
            {"id": "a", "time": "2026-01-01T03:00:00Z"}
        ]"#,
        );
        assert_eq!(pick_last_two(s), Some(("b".into(), "c".into())));
    }

    #[test]
    fn needs_two_snapshots() {
        assert_eq!(pick_last_two(parse("[]")), None);
        assert_eq!(
            pick_last_two(parse(r#"[{"id": "a", "time": "2026-01-01T03:00:00Z"}]"#)),
            None
        );
    }
}
