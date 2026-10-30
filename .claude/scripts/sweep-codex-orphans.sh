#!/usr/bin/env bash
# sweep-codex-orphans.sh — stop this repo's orphaned codex-companion brokers; report suspected
# orphan tasks.
#
# Usage: bash .claude/scripts/sweep-codex-orphans.sh [--dry-run]
#
# From ONE process snapshot (codex-procs.sh), it stops a broker (fixed argv
# `serve --endpoint E --cwd C --pid-file P`) only when all hold: its canonical C is below this
# repo's .claude/worktrees/, C no longer exists, no live job (queued, or running with a live pid)
# in either companion state store has its workspaceRoot or cwd at or below C, and no task or
# task-worker process (fixed-argv --cwd, or process cwd) is at or below C. With no state store, no
# broker is stopped. Signals: SIGTERM, wait up to 10 s for all, SIGKILL the survivors; a pid whose
# pid, start time or command differs from the snapshot is skipped.
#
# Tasks are report-only, never signalled: this repo's task-workers whose job is not live or whose
# cwd is gone, `task` processes (any repo) that no running job claims by pid, and this repo's
# running job records whose pid is not a live companion task. --dry-run lists the brokers it would
# stop and changes nothing.
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

TMP="$(mktemp -d "${TMPDIR:-/tmp}/sweep-codex.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
SNAP="$TMP/snap" PROCS="$TMP/procs" JOBS="$TMP/jobs"
load_roots
snapshot_procs
codex_procs >"$PROCS"
codex_jobs ${STATE_ROOTS[@]+"${STATE_ROOTS[@]}"} >"$JOBS"

# Brokers.
TARGETS=""
if [ ${#STATE_ROOTS[@]} -eq 0 ]; then
  echo "no companion state store exists: no broker is stopped"
else
  while IFS="$SEP" read -r kind pid cwd _; do
    [ "$kind" = broker ] || continue
    c="$(canon "$cwd" || true)"
    case "$c" in "$ROOT"/?*) ;; *) continue ;; esac
    [ ! -e "$c" ] || continue
    if [ -n "$(live_jobs_at "$c")" ]; then
      echo "keep broker $pid: a live job references $c"
      continue
    fi
    if [ -n "$(tasks_at "$c")" ]; then
      echo "keep broker $pid: a task process is at or below $c"
      continue
    fi
    TARGETS="$TARGETS $pid"
    echo "orphan broker $pid (worktree gone: $c)"
  done <"$PROCS"
fi

# Tasks: report only.
while IFS="$SEP" read -r kind pid cwd job pcwd; do
  case "$kind" in
    task-worker)
      st="$(awk -F "$SEP" -v j="$job" '$2 == j { print $3; exit }' "$JOBS")"
      c="$(canon "${cwd:-$pcwd}" 2>/dev/null || true)"
      if [ -z "$c" ] || ! under "$c" "$MAIN"; then continue; fi
      gone=""
      if [ ! -e "$c" ]; then gone=" (cwd gone)"; fi
      case "${st:-}" in queued | running) [ -z "$gone" ] && continue ;; esac
      echo "suspected orphan task-worker $pid: job ${job:-?} status ${st:-not in state}, cwd $c$gone (not stopped)"
      ;;
    task)
      if ! awk -F "$SEP" -v p="$pid" '($3 == "running" || $3 == "queued") && $6 == p { f = 1 } END { exit !f }' "$JOBS"; then
        echo "suspected orphan task $pid (cwd ${pcwd:-unknown}): no running job claims this pid; started $(awk -F "$SEP" -v p="$pid" '$1 == p { print $2; exit }' "$SNAP") (not stopped)"
      fi
      ;;
  esac
done <"$PROCS"
while IFS="$SEP" read -r _ id st ws _ pid; do
  [ "$st" = running ] || continue
  c="$(canon "$ws" 2>/dev/null || true)"
  if [ -z "$c" ] || ! under "$c" "$MAIN"; then continue; fi
  if [ -z "$pid" ] || ! awk -F "$SEP" -v p="$pid" '$1 != "broker" && $2 == p { f = 1 } END { exit !f }' "$PROCS"; then
    echo "stale job record $id (running, workspace $ws): pid ${pid:-none} is not a live companion task"
  fi
done <"$JOBS"

if [ "$DRY" = 1 ] || [ -z "$TARGETS" ]; then exit 0; fi
# shellcheck disable=SC2086 # TARGETS is a space-separated pid list
stop_set $TARGETS
exit 0
