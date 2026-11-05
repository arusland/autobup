//! autobup - per-application restic backups.

mod commands;
mod config;
mod error;
mod healthcheck;
mod hooks;
mod http;
mod lock;
mod log;
mod proc;
mod restic;
mod space;
mod telegram;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use commands::Ctx;
use config::Config;
use error::{AppError, Result};

const AFTER_HELP: &str = "\
Exit codes: 0 ok, 1 failure, 2 config error, 3 backup done but some files
were unreadable, 4 a previous run of this app is still running.

Cron example (daily at 03:15):
  15 3 * * * /usr/local/bin/autobup -c /etc/autobup/myapp.toml >/dev/null 2>&1";

#[derive(Parser)]
#[command(version, about = "Unattended per-application backups with restic", after_help = AFTER_HELP)]
struct Cli {
    /// Application config file (TOML)
    #[arg(short, long, env = "AUTOBUP_CONFIG", value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// pre-hook -> backup -> post-hook -> forget/prune -> check (default)
    Run,
    /// Show what would be backed up / forgotten, change nothing
    DryRun,
    /// Initialise the repository
    Init,
    /// Generate the repository password file; fails if it already exists
    Pwd,
    /// Send a test message to the configured Telegram chat
    NotifyTest,
    /// Show the last N snapshots of this app
    View {
        #[arg(default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..))]
        count: u32,
    },
    /// Show free space and repo size (local and sftp: repositories)
    Space,
    /// Run any restic command with this app's settings, e.g. `restic snapshots`
    Restic {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "ARGS")]
        args: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    setup_process();
    let code = match real_main(cli) {
        Ok(()) => 0,
        Err(e) => {
            if !e.reported {
                log::error(&e.msg);
            }
            e.code
        }
    };
    std::process::exit(code);
}

/// umask 077 and a usable PATH (cron runs with a minimal one).
fn setup_process() {
    use nix::sys::stat::{Mode, umask};
    umask(Mode::from_bits_truncate(0o077));
    let path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: called at the start of main, before any other thread exists.
    unsafe {
        std::env::set_var(
            "PATH",
            format!("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:{path}"),
        );
    }
}

fn real_main(cli: Cli) -> Result<()> {
    let config_path = cli
        .config
        .ok_or_else(|| AppError::config("no config file given: use -c FILE or set AUTOBUP_CONFIG"))?;
    let cfg = Config::load(&config_path)?;
    log::set_project(&cfg.project_name);
    let ctx = Ctx { cfg, config_path };

    match cli.command.unwrap_or(Cmd::Run) {
        Cmd::Run => commands::run::execute(&ctx, false),
        Cmd::DryRun => commands::dry_run::execute(&ctx),
        Cmd::Init => commands::init::execute(&ctx),
        Cmd::Pwd => commands::pwd::execute(&ctx),
        Cmd::NotifyTest => commands::notify_test::execute(&ctx),
        Cmd::View { count } => commands::view::execute(&ctx, count),
        Cmd::Space => commands::space::execute(&ctx),
        Cmd::Restic { args } => commands::restic::execute(&ctx, &args),
    }
}
