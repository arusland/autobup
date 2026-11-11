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
mod secrets;
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
  15 3 * * * /usr/local/bin/autobup -c /srv/myapp/autobup.toml >/dev/null 2>&1
  15 3 * * * /usr/local/bin/autobup /srv/myapp >/dev/null 2>&1";

#[derive(Parser)]
#[command(version, about = "Unattended per-application backups with restic", after_help = AFTER_HELP)]
struct Cli {
    /// Application config file (TOML), or a directory containing autobup.toml
    /// [env: AUTOBUP_CONFIG=]
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Project directory containing autobup.toml (alternative to -c)
    #[arg(value_name = "DIR", conflicts_with = "config")]
    dir: Option<PathBuf>,

    /// Verbose output: pass --verbose to restic and log the steps of `space`
    #[arg(short, long, global = true)]
    verbose: bool,

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
    /// Add a generated PASSWORD to ~/.config/autobup/<project>.env; fails if
    /// it is already set
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
    /// Create a config file for a project directory (name = directory name)
    Config {
        /// Project directory to back up
        #[arg(default_value = ".", value_name = "DIR")]
        dir: PathBuf,
        /// Where to write the config [default: DIR/autobup.toml]
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// `restic diff` with this app's settings; without ARGS compares the two
    /// latest snapshots of this app
    Diff {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "ARGS")]
        args: Vec<String>,
    },
    /// Run any restic command with this app's settings, e.g. `restic snapshots`
    Restic {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "ARGS")]
        args: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    log::set_verbose(cli.verbose);
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
    if let Some(Cmd::Config { dir, output }) = &cli.command {
        return commands::config::execute(dir, output.as_deref());
    }
    // Command line wins over AUTOBUP_CONFIG (read here, not via clap, so that
    // the env var does not conflict with DIR).
    let from_env = || {
        std::env::var_os("AUTOBUP_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let config_path = cli.dir.or(cli.config).or_else(from_env).ok_or_else(|| {
        AppError::config("no config file given: pass a project DIR, use -c FILE or set AUTOBUP_CONFIG")
    })?;
    let config_path = resolve_config_path(config_path)?;
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
        Cmd::Diff { args } => commands::diff::execute(&ctx, &args),
        Cmd::Restic { args } => commands::restic::execute(&ctx, &args),
        Cmd::Config { .. } => unreachable!("handled above"),
    }
}

/// A directory means "the autobup.toml inside it".
fn resolve_config_path(path: PathBuf) -> Result<PathBuf> {
    if !path.is_dir() {
        return Ok(path);
    }
    let file = path.join(commands::config::DEFAULT_FILE);
    if !file.is_file() {
        return Err(AppError::config(format!(
            "no {} found in directory {}",
            commands::config::DEFAULT_FILE,
            path.display()
        )));
    }
    Ok(file)
}
