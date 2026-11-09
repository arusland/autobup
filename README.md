# autobup

A small Rust CLI around [restic](https://restic.net/) for unattended,
per-application backups.

Each application gets its own TOML config file with its own repository,
retention policy, hooks, log file and lock. restic itself is an external
dependency: autobup always runs `restic` from `PATH`.

## Features

- **Full run cycle:** pre-hook → backup → post-hook → forget/prune → optional
  `restic check`.
- **Hooks** (`pre_backup`, `post_backup`, `on_failure`) for database dumps,
  stopping and starting containers, custom alerts and similar tasks.
- **Retention** via the `keep_*` settings. Retention only touches snapshots
  tagged `project:<project_name>`, so several applications can share one
  repository safely.
- **Locking:** a run is skipped if the previous run of the same app is still
  in progress.
- **Logging** to `<log_dir>/<project_name>.log`, with timestamps.
- **Monitoring:**
  - [healthchecks.io](https://healthchecks.io)-compatible pings (start, success, fail).
  - Telegram notifications. For local and `sftp:` repositories the message also
    shows free space on the backup target and the repository size.
- Works with every restic backend: local, SFTP, S3, B2, rest-server and others.

## Install

restic must be installed separately, e.g. with your distribution's package
manager (`apt install restic`, `dnf install restic`, ...).

```bash
cargo install --path .        # installs autobup to ~/.cargo/bin
```

## Quick start

```bash
# 1. Create a config for your application (project name = directory name)
autobup config /srv/myapp              # writes /srv/myapp/autobup.toml
$EDITOR /srv/myapp/autobup.toml        # repository, backup.paths, hooks, ...

# 2. Generate the repository password: adds PASSWORD=... to
#    ~/.config/autobup/myapp.env (used as password = "${PASSWORD}")
autobup -c /srv/myapp/autobup.toml pwd

# 3. Create the repository
autobup -c /srv/myapp/autobup.toml init

# 4. Preview, then run the first backup
autobup -c /srv/myapp/autobup.toml dry-run
autobup -c /srv/myapp/autobup.toml
```

> **Keep a copy of the secrets file somewhere else.** If you lose it, you
> cannot restore the backups.

autobup can only back up files the user running it can read. Unreadable files
are skipped and the run ends with exit code 3.

## Usage

```
autobup -c <FILE> [command] [args...]
autobup <DIR> [command] [args...]
```

`DIR` is a project directory containing `autobup.toml`, e.g.
`autobup ~/Projects/my-project/ dry-run`. `-c` also accepts a directory.
The config file can also be given with the `AUTOBUP_CONFIG` environment
variable; a `DIR` or `-c` on the command line takes precedence over it.

| Command          | Description                                                      |
|------------------|------------------------------------------------------------------|
| `run` (default)  | pre-hook → backup → post-hook → forget/prune → check             |
| `dry-run`        | Show what would be backed up or forgotten; changes nothing       |
| `init`           | Initialise the repository                                        |
| `config [DIR]`   | Create a config for project DIR (default `.`); see below         |
| `pwd`            | Add a generated `PASSWORD` to the secrets file (mode 600); fails if it is already set |
| `notify-test`    | Send a test message to the configured Telegram chat              |
| `view [N]`       | Show the last N snapshots of this app (default 10)               |
| `space`          | Show free space and repo size (local and `sftp:` repositories)   |
| `diff [args]`    | `restic diff`; without args compares the two latest snapshots    |
| `restic <args>`  | Run any restic command with this app's settings                  |
| `help`           | Show usage                                                       |

`config` needs no `-c`. It names the project after the directory, backs up
that directory, and writes `DIR/autobup.toml` (mode 600), or the file given
with `-o FILE`. If that file exists it stops with an error (exit 2) and leaves it as it is. The generated
file documents every setting in comments.

Examples:

```bash
autobup config ~/projects/shop               # ~/projects/shop/autobup.toml
autobup config ~/projects/shop -o ~/.config/autobup/shop.toml
autobup -c /srv/myapp/autobup.toml diff      # changes in the last backup
autobup -c /srv/myapp/autobup.toml diff 1a2b3c4d 5e6f7a8b
autobup -c /srv/myapp/autobup.toml restic snapshots
autobup -c /srv/myapp/autobup.toml restic restore latest --target /tmp/restore
autobup -c /srv/myapp/autobup.toml restic mount /mnt/restore
```

### Scheduling

Cron (daily at 03:15), added with `crontab -e`:

```cron
15 3 * * * $HOME/.cargo/bin/autobup -c /srv/myapp/autobup.toml >/dev/null 2>&1
```

autobup sets its own `PATH` and writes to its log file, so you can discard
cron's output.

### Exit codes

| Code | Meaning                                                   |
|------|-----------------------------------------------------------|
| 0    | Success                                                   |
| 1    | Failure                                                   |
| 2    | Configuration error                                       |
| 3    | Backup was made, but some files could not be read         |
| 4    | A previous run of this app is still running               |

## Configuration

Run `autobup config` to get a complete, commented config
(template: [`src/commands/config_template.toml`](src/commands/config_template.toml)). Unknown keys are rejected, so typos are caught early.

### Paths

`~` (as `~` or `~/...`) and `$HOME` / `${HOME}` are expanded in these settings:
`repository.url` (local repositories only, remote URLs such as `sftp:` are
left as they are), `repository.url_file`, `repository.password_file`,
`backup.paths`, `backup.exclude_file`, `telegram.bot_token_file`,
`runtime.log_dir` and `runtime.lock_dir`. `~user` is not supported. Other settings and `[env]`
values are passed on unchanged.

```toml
[repository]
url = "~/backup/restic"
password_file = "~/.config/autobup/myapp.pass"
```

### Secrets file

Secrets can live outside the config in `~/.config/autobup/<project_name>.env`,
one `NAME=value` per line (`#` comments, an optional `export ` prefix and
surrounding quotes are allowed). Use them in any config value as `${NAME}`;
names not in the file are taken from autobup's environment, and an undefined
name is an error. `[hooks]` are not substituted, but restic and the hooks get
all variables of the file in their environment. `${HOME}` keeps its path meaning
(see above). autobup warns if the file is readable by other users.

```bash
# ~/.config/autobup/myapp.env
PASSWORD=...                     # written by `autobup -c ... pwd`
TELEGRAM_BOT_TOKEN=123456789:AAE...
AWS_SECRET_ACCESS_KEY=...
```

```toml
[repository]
password = "${PASSWORD}"
[telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
[env]
AWS_SECRET_ACCESS_KEY = "${AWS_SECRET_ACCESS_KEY}"
```

`pwd` creates the file (with a commented `TELEGRAM_BOT_TOKEN` line) if needed
and appends `PASSWORD=...`; it never changes an existing `PASSWORD`.

### `project_name`

Used in the log and lock file names and in the `project:<name>` snapshot tag.

### `[repository]`

| Key                | Description                                         |
|--------------------|-----------------------------------------------------|
| `url`              | Any restic repository URL (`RESTIC_REPOSITORY`)     |
| `url_file`         | File containing the URL (`RESTIC_REPOSITORY_FILE`)  |
| `password_file`    | Repository password file, mode 600 (`RESTIC_PASSWORD_FILE`) |
| `password`         | Password (`RESTIC_PASSWORD`), usually `"${PASSWORD}"` from the secrets file |
| `password_command` | Command printing the password (`RESTIC_PASSWORD_COMMAND`) |

### `[backup]`

| Key               | Default | Description                                         |
|-------------------|---------|-----------------------------------------------------|
| `paths`           | config dir | Paths to back up. Missing paths are skipped with a warning. If unset or empty, the directory containing the config file is backed up. |
| `excludes`        | `[]`    | Exclude patterns                                    |
| `exclude_file`    |         | File with exclude patterns                          |
| `exclude_caches`  | `true`  | Skip directories that contain `CACHEDIR.TAG`        |
| `one_file_system` | `false` | Do not cross file system boundaries                 |
| `tags`            | `[]`    | Extra snapshot tags                                 |
| `host`            |         | Override the hostname stored in snapshots           |
| `extra_args`      | `[]`    | Extra arguments for `restic backup`, e.g. `["--compression", "max"]` |

### `[retention]`

`keep_last`, `keep_hourly`, `keep_daily`, `keep_weekly`, `keep_monthly`,
`keep_yearly` (numbers) and `keep_within` (e.g. `"2d"`). Leave a key out to
not use it. If none is set, the forget step is skipped. With `prune = true`
(default), unreferenced data is removed in the same step.

### `[check]`

| Key                | Default | Description                                   |
|--------------------|---------|-----------------------------------------------|
| `enabled`          | `false` | Run `restic check` after each backup          |
| `read_data_subset` |         | e.g. `"5%"` to also read part of the pack data |

### `[runtime]`

| Key               | Default            | Description                                      |
|-------------------|--------------------|--------------------------------------------------|
| `log_dir`         | `/var/log/autobup` | Log directory; if it is not writable, autobup logs to stdout only. Use e.g. `~/.local/state/autobup` when not running as root |
| `lock_dir`        | `/tmp`             | Lock file directory                              |
| `healthcheck_url` |                    | e.g. `https://hc-ping.com/<uuid>`                |

### `[env]`

Any extra environment variables for restic and the hooks, such as `AWS_*`,
`B2_*` or `RESTIC_CACHE_DIR`.

### `[telegram]`

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy its token.
2. Send the bot a message, then open
   `https://api.telegram.org/bot<TOKEN>/getUpdates` to find your chat id.
   Group chat ids are negative, for example `-100...`.
3. Set `bot_token` and `chat_id`, then run `autobup -c ... notify-test`.

To keep the token out of the config, put it in a file (mode 600) and set
`bot_token_file` instead of `bot_token`. Surrounding whitespace is ignored.
Setting both is an error.

```toml
[telegram]
bot_token_file = "~/.config/autobup/telegram.token"
chat_id = "123456789"
```

Set `notify_success = false` to get messages only for failures and partial
backups.

The message includes the host, the run duration and, on failure, the error and
the log path. For local and `sftp:` repositories it also shows free space on the
target and the repository size. Local repositories are measured directly
(`statvfs` and a walk over the repository folder). SFTP repositories use the
sftp `df` and `ls` commands, which need key-based authentication (as restic
does) and work on SFTP-only accounts. If the size cannot be read, autobup logs
a warning and shows only the free space. The `space` command prints the same
value on demand. It needs neither restic nor the repository password.

The value also says where the space is:

```
repository size 4.2G, 16G free of 116G (87% used) at /mnt/backup/restic (/dev/sdb1 on /mnt/backup)
repository size 800G, 1.2T free of 5.0T (76% used) at backup@backup.example.com:/srv/restic/myapp
```

## Hooks

Hooks are shell snippets in the `[hooks]` table. Each one runs with
`bash -eo pipefail -c`, so any command that fails makes the hook fail. Hooks
get the restic environment plus `AUTOBUP_PROJECT`, `AUTOBUP_HOOK` and, where
there is one, `AUTOBUP_STATUS` (same as `$1`).

| Hook          | When it runs                                         | Arguments            |
|---------------|------------------------------------------------------|----------------------|
| `pre_backup`  | Before the backup. If it fails, the run is aborted.  | –                    |
| `post_backup` | Always, once `pre_backup` has started                | `$1` = status so far |
| `on_failure`  | When the run fails                                   | `$1` = exit code     |

Example: back up a PostgreSQL database.

```toml
[backup]
paths = ["/srv/myapp/uploads", "/var/backups/myapp-db"]

[hooks]
pre_backup = """
mkdir -p /var/backups/myapp-db
pg_dump -U myapp myapp | gzip > /var/backups/myapp-db/myapp.sql.gz
"""
post_backup = "rm -f /var/backups/myapp-db/*.sql.gz"
```

## Security notes

- autobup runs with `umask 077`.
- Prefer the secrets file (`~/.config/autobup/<project>.env`, mode 600) over
  secrets in the config. If you write secrets (`repository.password`,
  `telegram.bot_token`, keys in `[env]`) into the config itself, make it `chmod 600`. autobup warns you if it is
  readable by other users.
- The Telegram token is used in-process and never passed on a command line.
  With `bot_token_file`, autobup warns if the file is readable by other users.
- `pwd` only appends to the secrets file and never replaces an existing `PASSWORD`.

## Requirements

- Linux
- `restic` in `PATH`
- `bash` for hooks
- `sftp` (OpenSSH) for the free-space report on SFTP repositories
- `HOME` set in the environment when the config uses `~` or `$HOME` (cron sets it)
- Rust 1.89+ to build

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
