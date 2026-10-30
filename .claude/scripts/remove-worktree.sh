#!/usr/bin/env bash
# remove-worktree.sh — remove a .claude/worktrees/ worktree after stopping its Codex processes.
#
# Usage: bash "$MAIN/.claude/scripts/remove-worktree.sh" [--dry-run] [--force-no-state] <worktree> [<branch>]
#
# In this order:
#   1. Check the arguments.
#   2. Refuse if the caller's cwd (pwd -P) is inside the worktree.
#   3. Refuse unless the worktree is registered in this repo's `git worktree list --porcelain`
#      and sits directly under .claude/worktrees/, compared on canonical paths. With <branch>,
#      refuse unless the porcelain shows `branch refs/heads/<branch>` for this worktree and the
#      branch is merged into origin/main or has an upstream.
#   4. Collect, from the companion's own state ($CLAUDE_PLUGIN_DATA/state, default
#      ~/.claude/plugins/data/codex-openai-codex/state; refuse if it is missing unless
#      --force-no-state), every queued or running job whose workspaceRoot or cwd is the worktree
#      or below it, and the process each one runs; plus every broker and task-worker whose
#      fixed-position --cwd is the worktree or below it (codex-procs.sh). A plain `task`
#      process is found only through its job, never through its command line.
#   5. Cancel each job with the companion's own `cancel`, then SIGTERM every collected process,
#      wait up to 10 s for all of them, SIGKILL the survivors. A process whose start time or
#      command changed since listing is skipped.
#   6. Confirm no matching job or process is left; otherwise exit 1 and remove nothing.
#   7. git worktree remove --force (discards uncommitted changes; no repo-wide prune).
#   8. With <branch>, git branch -D.
# --dry-run prints what would be stopped and removed, and changes nothing.
set -euo pipefail

die() { echo "remove-worktree: $*" >&2; exit 1; }
USAGE="usage: remove-worktree.sh [--dry-run] [--force-no-state] <worktree-path> [<branch>]"

# 1. Arguments.
DRY=0 NOSTATE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1 ;;
    --force-no-state) NOSTATE=1 ;;
    --) shift; break ;;
    -*) die "$USAGE" ;;
    *) break ;;
  esac
  shift
done
[ $# -ge 1 ] && [ $# -le 2 ] || die "$USAGE"
ARG="$1"
BRANCH="${2:-}"
case "$ARG" in /*) ;; *) die "refuse: path must be absolute: $ARG" ;; esac
case "$BRANCH" in main | master | HEAD | -*) die "refuse: will not delete branch $BRANCH" ;; esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
# shellcheck source-path=SCRIPTDIR source=codex-procs.sh
. "$SCRIPT_DIR/codex-procs.sh"
GCD="$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
  || die "refuse: $SCRIPT_DIR is not in a git checkout"
MAIN="$(cd "$GCD/.." && pwd -P)"
ROOT="$(canon "$MAIN/.claude/worktrees")"
W="$(canon "$ARG")" || die "refuse: cannot resolve $ARG"
[ -d "$W" ] || die "refuse: $W does not exist"

# 2. Caller inside the target.
if under "$(pwd -P)" "$W"; then die "refuse: the current directory is inside $W; cd to $MAIN first"; fi

# 3. Registered worktree directly under .claude/worktrees/, and the branch check.
[ "${W%/*}" = "$ROOT" ] || die "refuse: $W is not directly under $ROOT"
if under "$SCRIPT_DIR" "$W"; then die "refuse: run the main checkout's copy ($MAIN/.claude/scripts/remove-worktree.sh)"; fi
REG="" WT_BRANCH="" cur=""
while IFS= read -r line; do
  case "$line" in
    "worktree "*)
      cur="$(canon "${line#worktree }" || true)"
      if [ "$cur" = "$W" ]; then REG="${line#worktree }"; fi
      ;;
    "branch "*) if [ "$cur" = "$W" ]; then WT_BRANCH="${line#branch }"; fi ;;
  esac
done < <(git -C "$MAIN" worktree list --porcelain)
[ -n "$REG" ] || die "refuse: $W is not a registered worktree of $MAIN"
if [ -n "$BRANCH" ]; then
  [ "$WT_BRANCH" = "refs/heads/$BRANCH" ] \
    || die "refuse: $W has ${WT_BRANCH:-no branch} checked out, not refs/heads/$BRANCH"
  if git -C "$MAIN" merge-base --is-ancestor "refs/heads/$BRANCH" origin/main 2>/dev/null; then
    WHY="merged into origin/main"
  elif UP="$(git -C "$MAIN" rev-parse --abbrev-ref --symbolic-full-name "$BRANCH@{upstream}" 2>/dev/null)"; then
    WHY="has upstream $UP"
  else
    die "refuse: branch $BRANCH is not merged into origin/main and has no upstream; push it or delete it by hand"
  fi
fi

# 4. Collect jobs and processes.
DATA="${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/codex-openai-codex}"
STATE="$DATA/state"
if [ ! -d "$STATE" ] && [ "$NOSTATE" = 0 ]; then
  die "refuse: companion state $STATE is missing, so running jobs cannot be found (--force-no-state to go on without them)"
fi
TMP="$(mktemp -d "${TMPDIR:-/tmp}/remove-worktree.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
SNAPS="$TMP/snaps"
: >"$SNAPS"

# Active jobs under W: "id SEP workspaceRoot SEP pid SEP startedAt".
active_jobs() {
  local id st ws cwd pid started c
  codex_jobs "$STATE" | while IFS="$SEP" read -r id st ws cwd pid started; do
    is_active "$st" || continue
    for c in "$ws" "$cwd"; do
      [ -n "$c" ] || continue
      if under "$(canon "$c" || true)" "$W"; then
        printf '%s%s%s%s%s%s%s\n' "$id" "$SEP" "$ws" "$SEP" "$pid" "$SEP" "$started"
        break
      fi
    done
  done
}

# Brokers and task-workers under W: "kind SEP pid".
procs_under() {
  local kind pid cwd c
  codex_procs | while IFS="$SEP" read -r kind pid cwd _; do
    [ "$kind" != task ] || continue
    c="$(canon "$cwd" || true)"
    if [ -n "$c" ] && under "$c" "$W"; then printf '%s%s%s\n' "$kind" "$SEP" "$pid"; fi
  done
}

# Jobs to cancel: active jobs under W that either have no process yet or whose pid is their
# own companion process (job_proc). Any other pid is a stale record or a reused pid: the
# companion's cancel would signal it, so such a record is reported and left alone.
live_jobs() {
  local id ws pid started
  active_jobs | while IFS="$SEP" read -r id ws pid started; do
    if [ -z "$pid" ] || job_proc "$pid" "$started"; then
      printf '%s%s%s%s%s\n' "$id" "$SEP" "$ws" "$SEP" "$pid"
    fi
  done
}

TARGETS=""
while IFS="$SEP" read -r kind pid; do
  [ -n "$pid" ] || continue
  remember "$pid"
  TARGETS="$TARGETS $pid"
  echo "process $pid ($kind): $(snap "$pid" | cut -c1-200)"
done <<<"$(procs_under)"
while IFS="$SEP" read -r id ws pid started; do
  [ -n "$id" ] || continue
  if [ -z "$pid" ]; then
    echo "job $id (workspace $ws, no process yet): cancel"
  elif job_proc "$pid" "$started"; then
    echo "job $id (workspace $ws, pid $pid): cancel, then stop pid $pid"
    case " $TARGETS " in *" $pid "*) ;; *) remember "$pid"; TARGETS="$TARGETS $pid" ;; esac
  else
    echo "stale job record $id (workspace $ws): pid $pid is not its companion process; left alone"
  fi
done <<<"$(active_jobs)"
JOBS="$(live_jobs)"
[ -n "${TARGETS// /}" ] || [ -n "$JOBS" ] || echo "no Codex job or process for $W"

if [ "$DRY" = 1 ]; then
  echo "dry run: would stop:${TARGETS:- nothing}; remove worktree $REG${BRANCH:+; delete branch $BRANCH ($WHY)}"
  exit 0
fi

# 5. Cancel jobs through the companion, then signal the processes.
if [ -n "$JOBS" ]; then
  COMPANION="$(companion_path)"
  [ -n "$COMPANION" ] || die "codex-companion.mjs not found; cannot cancel jobs; nothing removed"
  while IFS="$SEP" read -r id ws _; do
    CLAUDE_PLUGIN_DATA="$DATA" env -u CODEX_COMPANION_SESSION_ID \
      node "$COMPANION" cancel "$id" --cwd "$ws" --json >/dev/null 2>&1 \
      || echo "warning: companion cancel $id failed"
  done <<<"$JOBS"
fi
# shellcheck disable=SC2086 # TARGETS is a space-separated pid list
[ -z "${TARGETS// /}" ] || stop_set $TARGETS

# 6. Confirm nothing is left.
LEFT=""
for p in $TARGETS; do
  if valid "$p"; then LEFT="$LEFT $p"; fi
done
LEFT_PROCS="$(procs_under | tr "$SEP" ' ' | tr '\n' ';')"
LEFT_JOBS="$(live_jobs | cut -d "$SEP" -f1 | tr '\n' ' ')"
if [ -n "$LEFT" ] || [ -n "$LEFT_PROCS" ] || [ -n "$LEFT_JOBS" ]; then
  die "still running for $W (pids:${LEFT:- none}; processes: ${LEFT_PROCS:-none}; jobs: ${LEFT_JOBS:-none}); nothing removed"
fi

# 7. Remove the worktree.
DIRTY="$(git -C "$W" status --short 2>/dev/null | wc -l | tr -d ' ')"
[ "$DIRTY" = 0 ] || echo "warning: discarding $DIRTY uncommitted path(s) in $W"
git -C "$MAIN" worktree remove --force "$REG"
echo "removed worktree $REG"

# 8. Delete the branch.
if [ -n "$BRANCH" ]; then
  git -C "$MAIN" branch -D "$BRANCH"
  echo "  ($BRANCH $WHY)"
fi
