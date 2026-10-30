#!/usr/bin/env bash
# sweep-codex-orphans.sh — stop this repo's orphaned codex-companion brokers; report suspected
# orphan tasks.
#
# Usage: bash .claude/scripts/sweep-codex-orphans.sh [--dry-run]
#
# Stops a broker (fixed argv `serve --endpoint E --cwd C --pid-file P`, codex-procs.sh) only when
# all hold: its canonical C is below this repo's .claude/worktrees/, C no longer exists, and no
# queued or running job in the companion state ($CLAUDE_PLUGIN_DATA/state, default
# ~/.claude/plugins/data/codex-openai-codex/state) has a workspaceRoot or cwd at or below C.
# Without that state no broker is stopped. Signals: SIGTERM, wait up to 10 s for all, SIGKILL the
# survivors; a process whose start time or command changed since listing is skipped.
#
# Tasks are report-only, never signalled: this repo's task-workers whose job is not running or
# whose cwd is gone, `task` processes (any repo) that no running job claims by pid, and this
# repo's running job records whose pid is not a live companion task (stale records; they do not
# keep a broker alive). --dry-run lists the brokers it would stop and changes nothing.
set -euo pipefail

DRY=0
case "${1:-}" in --dry-run | -n) DRY=1 ;; "") ;; *) echo "usage: sweep-codex-orphans.sh [--dry-run]" >&2; exit 2 ;; esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
# shellcheck source-path=SCRIPTDIR source=codex-procs.sh
. "$SCRIPT_DIR/codex-procs.sh"
GCD="$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
  || { echo "refuse: $SCRIPT_DIR is not in a git checkout" >&2; exit 1; }
MAIN="$(cd "$GCD/.." && pwd -P)"
ROOT="$(canon "$MAIN/.claude/worktrees")"
STATE="${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/codex-openai-codex}/state"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/sweep-codex.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
SNAPS="$TMP/snaps"
: >"$SNAPS"
codex_jobs "$STATE" >"$TMP/jobs"
codex_procs >"$TMP/procs"

# referenced C: a live queued or running job (no process yet, or its pid is its own companion
# process; a dead or reused pid is a stale record) has its workspaceRoot or cwd at or below C.
referenced() {
  local st ws cwd pid started p c
  while IFS="$SEP" read -r _ st ws cwd pid started; do
    is_active "$st" || continue
    if [ -n "$pid" ] && ! job_proc "$pid" "$started"; then continue; fi
    for p in "$ws" "$cwd"; do
      [ -n "$p" ] || continue
      c="$(canon "$p" || true)"
      if [ -n "$c" ] && under "$c" "$1"; then return 0; fi
    done
  done <"$TMP/jobs"
  return 1
}

# Brokers.
TARGETS=""
if [ ! -d "$STATE" ]; then
  echo "companion state $STATE is missing: no broker is stopped"
else
  while IFS="$SEP" read -r kind pid cwd _; do
    [ "$kind" = broker ] || continue
    c="$(canon "$cwd" || true)"
    case "$c" in "$ROOT"/?*) ;; *) continue ;; esac
    [ ! -e "$c" ] || continue
    if referenced "$c"; then
      echo "keep broker $pid: a running job still references $c"
      continue
    fi
    remember "$pid"
    TARGETS="$TARGETS $pid"
    echo "orphan broker $pid (worktree gone: $c)"
  done <"$TMP/procs"
fi

# Tasks: report only.
while IFS="$SEP" read -r kind pid cwd job; do
  case "$kind" in
    task-worker)
      st="$(awk -F "$SEP" -v j="$job" '$1 == j { print $2; exit }' "$TMP/jobs")"
      c="$(canon "$cwd" || true)"
      gone=""
      if [ -n "$c" ] && [ ! -e "$c" ]; then gone=" (cwd gone)"; fi
      if [ -z "$c" ] || ! under "$c" "$MAIN"; then continue; fi
      if ! is_active "${st:-unknown}" || [ -n "$gone" ]; then
        echo "suspected orphan task-worker $pid: job $job status ${st:-not in state}, cwd $cwd$gone (not stopped)"
      fi
      ;;
    task)
      if ! awk -F "$SEP" -v p="$pid" '($2 == "running" || $2 == "queued") && $5 == p { f = 1 } END { exit !f }' "$TMP/jobs"; then
        echo "suspected orphan task $pid (repository unknown): no running job claims this pid; started $(ps -o lstart= -p "$pid" 2>/dev/null | tr -s ' ') (not stopped)"
      fi
      ;;
  esac
done <"$TMP/procs"
while IFS="$SEP" read -r id st ws _ pid _; do
  is_active "$st" || continue
  c="$(canon "$ws" || true)"
  if [ -z "$c" ] || ! under "$c" "$MAIN"; then continue; fi
  if [ -z "$pid" ] || ! awk -F "$SEP" -v p="$pid" '($1 == "task" || $1 == "task-worker") && $2 == p { f = 1 } END { exit !f }' "$TMP/procs"; then
    echo "stale job record $id ($st, workspace $ws): pid ${pid:-none} is not a live companion task"
  fi
done <"$TMP/jobs"

if [ "$DRY" = 1 ] || [ -z "${TARGETS// /}" ]; then exit 0; fi
# shellcheck disable=SC2086 # TARGETS is a space-separated pid list
stop_set $TARGETS
exit 0
