# autobup

A single-file Bash wrapper around [restic](https://restic.net/) for unattended,
per-application backups.

You copy the script once per application and edit only the `SETTINGS` block at
the top. Each copy has its own repository, retention policy, hooks, log file
and lock.

## Features

- **One file, no dependencies** apart from Bash, `curl` and `flock`. If restic
  is missing, the script downloads the latest release from GitHub and checks
  its SHA-256 sum before installing it.
- **Full run cycle:** pre-hook → backup → post-hook → forget/prune → optional
  `restic check`.
- **Hooks** (`pre_backup`, `post_backup`, `on_failure`) for database dumps,
  stopping and starting containers, custom alerts and similar tasks.
- **Retention** via the `KEEP_*` settings. Retention only touches snapshots
  tagged `project:<PROJECT_NAME>`, so several applications can share one
  repository safely.
- **Locking:** a run is skipped if the previous run of the same app is still
  in progress.
- **Logging** to `LOG_DIR/<PROJECT_NAME>.log`, with timestamps.
- **Monitoring:**
  - [healthchecks.io](https://healthchecks.io)-compatible pings (start, success, fail).
  - Telegram notifications. For local and `sftp:` repositories the message also
    shows free space on the backup target.
- **Low priority:** restic runs under `nice` and `ionice`.
- Works with every restic backend: local, SFTP, S3, B2, rest-server and others.

## Quick start

```bash
# 1. Make a copy for your application
sudo mkdir -p /opt/backup
sudo cp autobup.sh /opt/backup/myapp-backup.sh
sudo chmod 700 /opt/backup/myapp-backup.sh

# 2. Edit the SETTINGS block (PROJECT_NAME, RESTIC_REPOSITORY, BACKUP_PATHS, ...)
sudo $EDITOR /opt/backup/myapp-backup.sh

# 3. Generate the repository password file (RESTIC_PASSWORD_FILE)
sudo /opt/backup/myapp-backup.sh pwd

# 4. Create the repository
sudo /opt/backup/myapp-backup.sh init

# 5. Preview, then run the first backup
sudo /opt/backup/myapp-backup.sh dry-run
sudo /opt/backup/myapp-backup.sh
```

> **Keep a copy of the password file somewhere else.** If you lose it, you
> cannot restore the backups.

## Usage

```
myapp-backup.sh [command] [args...]
```

| Command          | Description                                                      |
|------------------|------------------------------------------------------------------|
| `run` (default)  | pre-hook → backup → post-hook → forget/prune → check             |
| `dry-run`        | Show what would be backed up or forgotten; changes nothing       |
| `init`           | Initialise the repository                                        |
| `pwd`            | Generate `RESTIC_PASSWORD_FILE` (mode 600); fails if it exists   |
| `notify-test`    | Send a test message to the configured Telegram chat              |
| `view [N]`       | Show the last N snapshots of this app (default 10)               |
| `restic <args>`  | Run any restic command with this app's settings                  |
| `help`           | Show usage                                                       |

Examples:

```bash
myapp-backup.sh restic snapshots
myapp-backup.sh restic restore latest --target /tmp/restore
myapp-backup.sh restic mount /mnt/restore
```

### Scheduling

Cron (daily at 03:15):

```cron
15 3 * * * /opt/backup/myapp-backup.sh >/dev/null 2>&1
```

The script sets its own `PATH` and writes to its log file, so you can discard
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

All settings live in the `SETTINGS` block at the top of the script. Every
variable in that block is exported, so you can also set any restic environment
variable there, such as `RESTIC_PASSWORD_COMMAND`, `AWS_*`, `B2_*` or
`RESTIC_CACHE_DIR`.

### Repository

| Variable               | Default                    | Description                                   |
|------------------------|----------------------------|-----------------------------------------------|
| `PROJECT_NAME`         | `myapp`                    | Used in the log and lock file names and in the snapshot tag |
| `RESTIC_REPOSITORY`    | `/mnt/backup/restic`       | Any restic repository URL                     |
| `RESTIC_PASSWORD_FILE` | `/etc/autobup/myapp.pass`  | Repository password (`chmod 600`)             |
| `AUTO_INSTALL_RESTIC`  | `yes`                      | Download restic if it is not in `PATH`        |
| `RESTIC_INSTALL_DIR`   | *(empty)*                  | Install location; empty = `/usr/local/bin` (root) or `~/.local/bin` |

### What to back up

| Variable            | Description                                                    |
|---------------------|----------------------------------------------------------------|
| `BACKUP_PATHS`      | Array of paths. Missing paths are skipped with a warning.      |
| `EXCLUDES`          | Array of exclude patterns                                      |
| `EXCLUDE_FILE`      | Optional file with exclude patterns                            |
| `EXCLUDE_CACHES`    | Skip directories that contain `CACHEDIR.TAG` (default `yes`)   |
| `ONE_FILE_SYSTEM`   | Do not cross file system boundaries (default `no`)             |
| `BACKUP_TAGS`       | Extra snapshot tags                                            |
| `BACKUP_HOST`       | Override the hostname stored in snapshots                      |
| `BACKUP_EXTRA_ARGS` | Extra arguments for `restic backup`, e.g. `(--compression max)` |

### Retention

`KEEP_LAST`, `KEEP_HOURLY`, `KEEP_DAILY` (7), `KEEP_WEEKLY` (4),
`KEEP_MONTHLY` (6), `KEEP_YEARLY`, `KEEP_WITHIN`. An empty value means the
setting is not used. If every `KEEP_*` setting is empty, the forget step is
skipped. With `PRUNE="yes"`, unreferenced data is removed in the same step.

### Verification

| Variable                 | Default | Description                                   |
|--------------------------|---------|-----------------------------------------------|
| `CHECK`                  | `no`    | Run `restic check` after each backup          |
| `CHECK_READ_DATA_SUBSET` | *(empty)* | e.g. `5%` to also read part of the pack data |

### Runtime

| Variable          | Default            | Description                                      |
|-------------------|--------------------|--------------------------------------------------|
| `LOG_DIR`         | `/var/log/autobup` | Log directory; if it is not writable, the script logs to stdout only |
| `LOCK_DIR`        | `/tmp`             | Lock file directory                              |
| `NICE_LEVEL`      | `10`               | CPU priority                                     |
| `IONICE_CLASS`    | `2`                | I/O class: 2 = best-effort, 3 = idle             |
| `HEALTHCHECK_URL` | *(empty)*          | e.g. `https://hc-ping.com/<uuid>`                |

### Telegram notifications

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy its token.
2. Send the bot a message, then open
   `https://api.telegram.org/bot<TOKEN>/getUpdates` to find your chat id.
   Group chat ids are negative, for example `-100...`.
3. Set `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`, then run
   `myapp-backup.sh notify-test`.

Set `TELEGRAM_NOTIFY_SUCCESS="no"` to get messages only for failures and
partial backups.

The message includes the host, the run duration and, on failure, the error and
the log path. For local and `sftp:` repositories it also shows free space on the
target. Local repositories use `df`. SFTP repositories use the sftp `df` command,
which needs key-based authentication, as restic does, and works on SFTP-only
accounts.

## Hooks

Define these shell functions in the `SETTINGS` block. Each hook runs in a
subshell with `set -eo pipefail`, so any command that fails makes the hook
fail.

| Hook          | When it runs                                         | Arguments            |
|---------------|------------------------------------------------------|----------------------|
| `pre_backup`  | Before the backup. If it fails, the run is aborted.  | –                    |
| `post_backup` | Always, once `pre_backup` has started                | `$1` = status so far |
| `on_failure`  | When the run fails                                   | `$1` = exit code     |

Example: back up a PostgreSQL database.

```bash
BACKUP_PATHS=("/srv/myapp/uploads" "/var/backups/myapp-db")

pre_backup() {
    mkdir -p /var/backups/myapp-db
    pg_dump -U myapp myapp | gzip > /var/backups/myapp-db/myapp.sql.gz
}

post_backup() {
    rm -f /var/backups/myapp-db/*.sql.gz
}
```

## Security notes

- The script runs with `umask 077`.
- If you keep secrets (`RESTIC_PASSWORD`, `AWS_SECRET_ACCESS_KEY`,
  `B2_ACCOUNT_KEY`, `TELEGRAM_BOT_TOKEN`) directly in the script, make it
  `chmod 700`. The script warns you if it is readable by other users.
- The Telegram token is passed to `curl` through a config on stdin, so it does
  not show up in `ps`.
- Downloaded restic binaries are checked against the release's `SHA256SUMS`.

## Requirements

- Bash 4+, coreutils, `flock` (util-linux), `curl`
- `bunzip2` and `sha256sum` if restic needs to be downloaded
- `sftp` (OpenSSH) for the free-space report on SFTP repositories
- Linux. The automatic restic download supports amd64, arm64, arm, 386,
  ppc64le, riscv64 and s390x.
