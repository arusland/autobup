#!/usr/bin/env bash
#
# autobup.sh - universal restic backup script.
# Copy this file per application and edit ONLY the "SETTINGS" block below.
#
# Usage:
#   autobup.sh [command] [args...]
#
# Commands:
#   run              (default) pre-hook -> backup -> post-hook -> forget/prune -> check
#   dry-run          show what would be backed up / forgotten, change nothing
#   init             initialise the repository
#   restic <args>    run any restic command with this app's settings, e.g.
#                      autobup.sh restic snapshots
#                      autobup.sh restic restore latest --target /tmp/restore
#
# Cron example (daily at 03:15):
#   15 3 * * * /opt/backup/myapp-backup.sh >/dev/null 2>&1
#
# Exit codes: 0 ok, 1 failure, 2 config error, 3 backup done but some files
#             were unreadable, 4 a previous run of this app is still running.

set -a   # everything in SETTINGS is exported, so restic sees RESTIC_*, AWS_*, B2_* ...
# ===========================================================================
# SETTINGS - change these per application
# ===========================================================================

# Used for log file name, lock file name and "project:<name>" snapshot tag.
PROJECT_NAME="myapp"

# ---- repository -----------------------------------------------------------
RESTIC_REPOSITORY="/mnt/backup/restic"
# RESTIC_REPOSITORY="sftp:backup@backup.example.com:/srv/restic/myapp"
# RESTIC_REPOSITORY="s3:https://s3.eu-central-1.amazonaws.com/my-bucket/myapp"
# AWS_ACCESS_KEY_ID="..."
# AWS_SECRET_ACCESS_KEY="..."
RESTIC_PASSWORD_FILE="/etc/autobup/myapp.pass"   # chmod 600
AUTO_INIT="yes"                                  # create the repo on first run

# ---- restic binary ------------------------------------------------------
AUTO_INSTALL_RESTIC="yes"        # download latest restic release if not found
RESTIC_INSTALL_DIR=""            # empty = /usr/local/bin (root) or ~/.local/bin

# ---- what to back up ------------------------------------------------------
BACKUP_PATHS=(
    "/srv/myapp/uploads"
    "/srv/myapp/config"
    "/var/backups/myapp-db"      # filled by pre_backup below
)
EXCLUDES=(
    "*.tmp"
    "/srv/myapp/uploads/cache"
)
EXCLUDE_FILE=""                  # optional file with exclude patterns
EXCLUDE_CACHES="yes"             # skip dirs containing CACHEDIR.TAG
ONE_FILE_SYSTEM="no"
BACKUP_TAGS=()                   # extra tags, e.g. ("daily")
BACKUP_HOST=""                   # override hostname stored in snapshots
BACKUP_EXTRA_ARGS=()             # e.g. (--compression max)

# ---- retention (empty = not used) ----------------------------------------
KEEP_LAST=""
KEEP_HOURLY=""
KEEP_DAILY="7"
KEEP_WEEKLY="4"
KEEP_MONTHLY="6"
KEEP_YEARLY=""
KEEP_WITHIN=""                   # e.g. "2d"
PRUNE="yes"

# ---- verification ---------------------------------------------------------
CHECK="no"
CHECK_READ_DATA_SUBSET=""        # e.g. "5%" to also read part of the data

# ---- runtime --------------------------------------------------------------
LOG_DIR="/var/log/autobup"
LOCK_DIR="/tmp"
NICE_LEVEL="10"
IONICE_CLASS="2"                 # 2 = best-effort, 3 = idle
HEALTHCHECK_URL=""               # e.g. https://hc-ping.com/<uuid> (start/success/fail)

# ---- hooks ----------------------------------------------------------------
# Run in a subshell with `set -eo pipefail`: any failing command fails the hook.
# pre_backup  - failure aborts the run (backup is not made).
# post_backup - always runs once pre_backup started; $1 = exit status so far.
# on_failure  - runs when the run fails; $1 = exit code.

pre_backup() {
    mkdir -p /var/backups/myapp-db
    # pg_dump -U myapp myapp | gzip > /var/backups/myapp-db/myapp.sql.gz
    # mysqldump --single-transaction myapp | gzip > /var/backups/myapp-db/myapp.sql.gz
    # docker compose -f /srv/myapp/compose.yml stop worker
}

post_backup() {
    # docker compose -f /srv/myapp/compose.yml start worker
    # rm -f /var/backups/myapp-db/*.sql.gz
    :
}

on_failure() {
    # echo "Backup of myapp failed with code $1" | mail -s "backup failed" admin@example.com
    :
}

# ===========================================================================
# END OF SETTINGS - nothing below needs to change
# ===========================================================================
set +a

set -Eeuo pipefail
umask 077

# cron runs with a minimal PATH
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:${PATH:-}"

SCRIPT_PATH="$(realpath "$0")"
PROJECT_TAG="project:${PROJECT_NAME}"
BACKUP_PARTIAL=0
PRE_HOOK_STARTED=0
POST_HOOK_DONE=0

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------
log()  { printf '%s [%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$PROJECT_NAME" "$*"; }
warn() { log "WARNING: $*" >&2; }
die()  { log "ERROR: $1" >&2; exit "${2:-1}"; }

usage() { sed -n '3,21p' "$SCRIPT_PATH" | sed 's/^# \{0,1\}//'; exit 2; }

is_true()     { [[ "${1,,}" =~ ^(1|y|yes|true|on)$ ]]; }
is_function() { [[ "$(type -t "$1" 2>/dev/null)" == "function" ]]; }

# Ping a healthchecks.io-compatible URL: ping_hc "" | "/start" | "/fail"
ping_hc() {
    [[ -n "$HEALTHCHECK_URL" ]] || return 0
    curl -fsS -m 10 --retry 3 -o /dev/null "${HEALTHCHECK_URL%/}$1" \
        || warn "healthcheck ping '${1:-success}' failed"
}

# Run a hook function if defined. Returns the hook's exit code.
# NOTE: must not be called as `run_hook ... || x` / `if run_hook` - bash would
# then silently disable errexit inside the hook.
run_hook() {
    local name="$1"; shift
    is_function "$name" || return 0
    log "running hook: $name"
    local rc=0
    set +e
    ( set -eo pipefail; "$name" "$@" )
    rc=$?
    set -e
    (( rc == 0 )) || log "ERROR: hook $name failed with exit code $rc" >&2
    return "$rc"
}

restic_cmd() {
    local -a cmd=()
    command -v nice   >/dev/null && cmd+=(nice -n "$NICE_LEVEL")
    command -v ionice >/dev/null && cmd+=(ionice -c "$IONICE_CLASS")
    "${cmd[@]}" restic "$@"
}

# Make sure restic is available; download the latest release if it is not.
RESTIC_TMP=""
ensure_restic() {
    local dir="${RESTIC_INSTALL_DIR:-}"
    if [[ -z "$dir" ]]; then
        if (( EUID == 0 )); then dir="/usr/local/bin"; else dir="$HOME/.local/bin"; fi
    fi
    export PATH="$dir:$PATH"
    command -v restic >/dev/null && return 0

    is_true "$AUTO_INSTALL_RESTIC" || die "restic not found in PATH (AUTO_INSTALL_RESTIC=no)" 2
    local tool
    for tool in curl bunzip2 sha256sum; do
        command -v "$tool" >/dev/null || die "restic not found and '$tool' is needed to download it" 2
    done

    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    case "$(uname -m)" in
        x86_64|amd64)   arch="amd64" ;;
        aarch64|arm64)  arch="arm64" ;;
        armv6*|armv7*)  arch="arm" ;;
        i?86)           arch="386" ;;
        ppc64le)        arch="ppc64le" ;;
        riscv64)        arch="riscv64" ;;
        s390x)          arch="s390x" ;;
        *) die "restic not found; no prebuilt binary for architecture $(uname -m)" 2 ;;
    esac

    # releases/latest redirects to .../releases/tag/vX.Y.Z (no API rate limit)
    local url tag ver
    url="$(curl -fsSL -m 30 -o /dev/null -w '%{url_effective}' \
        https://github.com/restic/restic/releases/latest)" \
        || die "cannot query latest restic release"
    tag="${url##*/}"; ver="${tag#v}"
    [[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "cannot parse latest restic version from '$url'"

    local base="https://github.com/restic/restic/releases/download/${tag}"
    local file="restic_${ver}_${os}_${arch}.bz2"
    log "restic not found, downloading restic ${ver} (${os}/${arch}) to $dir"

    RESTIC_TMP="$(mktemp -d)" || die "cannot create temp dir"
    trap 'rm -rf -- "$RESTIC_TMP"' EXIT
    curl -fsSL -m 300 --retry 3 -o "$RESTIC_TMP/$file" "$base/$file" \
        || die "download failed: $base/$file"
    curl -fsSL -m 30 --retry 3 -o "$RESTIC_TMP/SHA256SUMS" "$base/SHA256SUMS" \
        || die "download failed: $base/SHA256SUMS"
    grep -E "^[0-9a-f]{64}  ${file//./\\.}\$" "$RESTIC_TMP/SHA256SUMS" > "$RESTIC_TMP/sum" \
        || die "no checksum for $file in SHA256SUMS"
    ( cd "$RESTIC_TMP" && sha256sum -c --quiet sum ) || die "checksum mismatch for $file"
    bunzip2 -c "$RESTIC_TMP/$file" > "$RESTIC_TMP/restic" || die "cannot decompress $file"
    chmod 755 "$RESTIC_TMP/restic"
    "$RESTIC_TMP/restic" version >/dev/null || die "downloaded restic binary does not run"

    mkdir -p "$dir" || die "cannot create $dir" 2
    install -m 755 "$RESTIC_TMP/restic" "$dir/.restic.new.$$" || die "cannot write to $dir" 2
    mv -f "$dir/.restic.new.$$" "$dir/restic" || die "cannot install restic to $dir" 2
    rm -rf -- "$RESTIC_TMP"; RESTIC_TMP=""
    trap - EXIT
    hash -r
    log "installed $("$dir/restic" version)"
}

validate() {
    ensure_restic
    [[ -n "${RESTIC_REPOSITORY:-}${RESTIC_REPOSITORY_FILE:-}" ]] \
        || die "RESTIC_REPOSITORY (or RESTIC_REPOSITORY_FILE) is not set" 2
    [[ -n "${RESTIC_PASSWORD:-}${RESTIC_PASSWORD_FILE:-}${RESTIC_PASSWORD_COMMAND:-}" ]] \
        || die "RESTIC_PASSWORD_FILE / RESTIC_PASSWORD / RESTIC_PASSWORD_COMMAND is not set" 2
    [[ -z "${RESTIC_PASSWORD_FILE:-}" || -r "$RESTIC_PASSWORD_FILE" ]] \
        || die "password file not readable: $RESTIC_PASSWORD_FILE" 2
    if [[ -n "${RESTIC_PASSWORD:-}${AWS_SECRET_ACCESS_KEY:-}${B2_ACCOUNT_KEY:-}" ]] \
       && (( (8#$(stat -c '%a' "$SCRIPT_PATH") & 8#044) != 0 )); then
        warn "script contains secrets but is readable by others: chmod 700 $SCRIPT_PATH"
    fi
}

# ---------------------------------------------------------------------------
# steps
# ---------------------------------------------------------------------------
ensure_repo() {
    restic cat config >/dev/null 2>&1 && return 0
    if is_true "$AUTO_INIT"; then
        log "repository not accessible, trying to initialise it"
        restic init || die "cannot open or initialise repository"
    else
        restic cat config >/dev/null || die "cannot open repository (AUTO_INIT=no)"
    fi
}

do_backup() {
    local dry="${1:-}"
    (( ${#BACKUP_PATHS[@]} > 0 )) || die "BACKUP_PATHS is empty" 2

    local -a paths=() args=(--tag "$PROJECT_TAG")
    local p t e
    for p in "${BACKUP_PATHS[@]}"; do
        if [[ -e "$p" ]]; then paths+=("$p"); else warn "path does not exist, skipping: $p"; fi
    done
    (( ${#paths[@]} > 0 )) || die "none of BACKUP_PATHS exist"

    for t in "${BACKUP_TAGS[@]}"; do args+=(--tag "$t"); done
    for e in "${EXCLUDES[@]}";    do args+=(--exclude "$e"); done
    [[ -n "$EXCLUDE_FILE" ]]   && args+=(--exclude-file "$EXCLUDE_FILE")
    is_true "$EXCLUDE_CACHES"  && args+=(--exclude-caches)
    is_true "$ONE_FILE_SYSTEM" && args+=(--one-file-system)
    [[ -n "$BACKUP_HOST" ]]    && args+=(--host "$BACKUP_HOST")
    [[ -n "$dry" ]]            && args+=(--dry-run -v)
    args+=("${BACKUP_EXTRA_ARGS[@]}")

    log "backing up: ${paths[*]}"
    local rc=0
    restic_cmd backup "${args[@]}" -- "${paths[@]}" || rc=$?
    case $rc in
        0) log "backup finished" ;;
        3) warn "backup finished, but some files could not be read"; BACKUP_PARTIAL=1 ;;
        *) die "backup failed (restic exit code $rc)" ;;
    esac
}

do_forget() {
    local dry="${1:-}"
    local -a args=(--tag "$PROJECT_TAG" --group-by host,tags)
    local base_len=${#args[@]}
    [[ -n "$KEEP_LAST"    ]] && args+=(--keep-last    "$KEEP_LAST")
    [[ -n "$KEEP_HOURLY"  ]] && args+=(--keep-hourly  "$KEEP_HOURLY")
    [[ -n "$KEEP_DAILY"   ]] && args+=(--keep-daily   "$KEEP_DAILY")
    [[ -n "$KEEP_WEEKLY"  ]] && args+=(--keep-weekly  "$KEEP_WEEKLY")
    [[ -n "$KEEP_MONTHLY" ]] && args+=(--keep-monthly "$KEEP_MONTHLY")
    [[ -n "$KEEP_YEARLY"  ]] && args+=(--keep-yearly  "$KEEP_YEARLY")
    [[ -n "$KEEP_WITHIN"  ]] && args+=(--keep-within  "$KEEP_WITHIN")

    if (( ${#args[@]} == base_len )); then
        log "no KEEP_* policy set, skipping forget"
        return 0
    fi
    if [[ -n "$dry" ]]; then
        args+=(--dry-run)
    elif is_true "$PRUNE"; then
        args+=(--prune)
    fi

    log "applying retention policy"
    restic_cmd forget "${args[@]}" || die "forget/prune failed"
}

do_check() {
    is_true "$CHECK" || return 0
    local -a args=()
    [[ -n "$CHECK_READ_DATA_SUBSET" ]] && args+=(--read-data-subset "$CHECK_READ_DATA_SUBSET")
    log "checking repository"
    restic_cmd check "${args[@]}" || die "repository check failed"
}

# ---------------------------------------------------------------------------
# run lifecycle
# ---------------------------------------------------------------------------
acquire_lock() {
    mkdir -p "$LOCK_DIR"
    exec 9>"$LOCK_DIR/autobup-${PROJECT_NAME}.lock"
    flock -n 9 || die "another backup of '$PROJECT_NAME' is already running" 4
}

setup_logging() {
    if mkdir -p "$LOG_DIR" 2>/dev/null && [[ -w "$LOG_DIR" ]]; then
        exec > >(tee -a "$LOG_DIR/${PROJECT_NAME}.log") 2>&1
    else
        warn "log dir '$LOG_DIR' not writable, logging to stdout only"
    fi
}

on_exit() {
    local rc=$?
    trap - EXIT
    set +e
    if (( PRE_HOOK_STARTED == 1 && POST_HOOK_DONE == 0 )); then
        POST_HOOK_DONE=1
        run_hook post_backup "$rc"
    fi
    if (( rc == 0 )); then
        (( BACKUP_PARTIAL == 1 )) && rc=3
        log "done in ${SECONDS}s"
        ping_hc ""
    else
        log "FAILED with exit code $rc after ${SECONDS}s"
        run_hook on_failure "$rc"
        ping_hc "/fail"
    fi
    exit "$rc"
}

cmd_run() {
    local dry="${1:-}"
    acquire_lock
    setup_logging
    log "===== starting ${dry:+dry-run }backup"
    [[ -z "$dry" ]] && ping_hc "/start"
    trap on_exit EXIT

    ensure_repo

    PRE_HOOK_STARTED=1
    run_hook pre_backup          # failure -> errexit -> on_exit

    do_backup "$dry"

    POST_HOOK_DONE=1
    run_hook post_backup 0

    do_forget "$dry"
    [[ -z "$dry" ]] && do_check
    return 0
}

main() {
    local cmd="${1:-run}"; [[ $# -gt 0 ]] && shift
    case "$cmd" in
        -h|--help|help) usage ;;
    esac
    validate
    case "$cmd" in
        run)      cmd_run ;;
        dry-run)  cmd_run dry ;;
        init)     restic init ;;
        restic)   exec restic "$@" ;;
        *)        die "unknown command: $cmd (see --help)" 2 ;;
    esac
}

main "$@"
