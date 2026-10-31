#!/usr/bin/env bash
# remove-worktree.sh — remove a .claude/worktrees/ worktree after stopping its Codex brokers.
#
# Usage: bash "$MAIN/.claude/scripts/remove-worktree.sh" [--dry-run] [--force-no-state] <worktree> [<branch>]
#
# It never cancels jobs and never signals a task or task-worker: cancel jobs first with the
# companion (it prints the commands). In this order:
#   1. Check the arguments.
#   2. Refuse if the caller's cwd (pwd -P) is inside the worktree.
#   3. Refuse unless the worktree is registered in this repo's `git worktree list --porcelain`
#      and sits directly under .claude/worktrees/, compared on canonical paths. With <branch>,
#      refuse unless the porcelain shows `branch refs/heads/<branch>` for this worktree and its
#      tip is an ancestor of origin/main, or of its upstream after fetching that upstream
#      (fully pushed, nothing ahead).
#   4. Take ONE process snapshot and read the companion state from both stores
#      ($CLAUDE_PLUGIN_DATA/state, default ~/.claude/plugins/data/codex-openai-codex/state, and
#      ${TMPDIR:-/tmp}/codex-companion; refuse if neither exists, unless --force-no-state).
#      A job's per-job file is authoritative over the state.json index, across both stores.
#      If a live job (queued, or running with a live companion task pid) has its workspaceRoot
#      or cwd at or below the worktree, wait 15 s once, take a new snapshot, re-read both stores
#      and refuse only if it is still live, listing each job's status, pid, the pid's snapshot
#      command and its cancel command; refuse if a task or task-worker process (fixed-argv
#      --cwd, or process cwd) is at or below it.
#   5. SIGTERM every broker of the snapshot whose fixed-position --cwd is at or below the
#      worktree, wait up to 10 s for all, SIGKILL the survivors. A pid whose pid, start time or
#      command differs from the snapshot is skipped.
#   6. Take a new snapshot and confirm no broker, task or live job is left; otherwise exit 1 and
#      remove nothing.
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
WHY=""
if [ -n "$BRANCH" ]; then
  [ "$WT_BRANCH" = "refs/heads/$BRANCH" ] \
    || die "refuse: $W has ${WT_BRANCH:-no branch} checked out, not refs/heads/$BRANCH"
  # Never trust a remote-tracking ref: ask the remote. The upstream branch must still exist
  # there (ls-remote) and hold the tip; otherwise the tip must be in a freshly fetched
  # origin/main.
  TIP="$(git -C "$MAIN" rev-parse "refs/heads/$BRANCH")"
  REMOTE="$(git -C "$MAIN" config "branch.$BRANCH.remote" || true)"
  MERGE="$(git -C "$MAIN" config "branch.$BRANCH.merge" || true)"
  UPSTATE="no upstream"
  if [ -n "$REMOTE" ] && [ "$REMOTE" != . ] && [ -n "$MERGE" ]; then
    SHA="$(git -C "$MAIN" ls-remote --exit-code "$REMOTE" "$MERGE" 2>/dev/null \
      | awk -v r="$MERGE" '$2 == r { print $1; exit }' || true)"
    if [ -z "$SHA" ]; then
      UPSTATE="upstream $REMOTE $MERGE no longer exists on the remote"
    elif ! git -C "$MAIN" fetch --quiet "$REMOTE" "$MERGE"; then
      UPSTATE="cannot fetch $REMOTE $MERGE"
    elif git -C "$MAIN" merge-base --is-ancestor "$TIP" "$SHA"; then
      WHY="its tip is on $REMOTE $MERGE ($SHA, checked with ls-remote)"
    else
      UPSTATE="it has commits not on its upstream $REMOTE $MERGE ($SHA)"
    fi
  fi
  if [ -z "$WHY" ]; then
    git -C "$MAIN" fetch --quiet origin "+refs/heads/main:refs/remotes/origin/main" \
      || die "refuse: branch $BRANCH: $UPSTATE, and origin main cannot be fetched; delete it by hand"
    git -C "$MAIN" merge-base --is-ancestor "$TIP" refs/remotes/origin/main \
      || die "refuse: branch $BRANCH: $UPSTATE, and its tip is not in origin/main; push it or delete it by hand"
    WHY="its tip is in freshly fetched origin/main"
  fi
fi

# 4. One snapshot; refuse on live jobs or task processes.
TMP="$(mktemp -d "${TMPDIR:-/tmp}/remove-worktree.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
SNAP="$TMP/snap" PROCS="$TMP/procs" JOBS="$TMP/jobs"
load_roots
if [ ${#STATE_ROOTS[@]} -eq 0 ] && [ "$NOSTATE" = 0 ]; then
  die "refuse: no companion state store exists, so live jobs cannot be checked (--force-no-state to go on without them)"
fi
snapshot_procs
codex_procs >"$PROCS"
codex_jobs ${STATE_ROOTS[@]+"${STATE_ROOTS[@]}"} >"$JOBS"

LIVE="$(live_jobs_at "$W")"
if [ -n "$LIVE" ]; then
  # The record may have raced the job's completion: re-check once from fresh state.
  echo "remove-worktree: live Codex job record(s) in $W; re-checking in 15 s" >&2
  sleep 15
  snapshot_procs
  codex_procs >"$PROCS"
  codex_jobs ${STATE_ROOTS[@]+"${STATE_ROOTS[@]}"} >"$JOBS"
  LIVE="$(live_jobs_at "$W")"
fi
if [ -n "$LIVE" ]; then
  {
    echo "remove-worktree: refuse: live Codex jobs in $W; cancel them first, then re-run:"
    echo "  COMPANION=<newest ~/.claude/plugins/cache/openai-codex/codex/<version>/scripts/codex-companion.mjs>"
    while IFS="$SEP" read -r root id ws st pid; do
      cmd="$(awk -F "$SEP" -v p="$pid" 'p != "" && $1 == p { print $3; exit }' "$SNAP" | cut -c1-200)"
      echo "  # job $id ($root): status $st, pid ${pid:-none}: ${cmd:-not in the snapshot}"
      echo "  $(cancel_hint "$root" "$id" "$ws")"
    done <<<"$LIVE"
  } >&2
  exit 1
fi
TASKS="$(tasks_at "$W")"
if [ -n "$TASKS" ]; then
  {
    echo "remove-worktree: refuse: Codex task processes in $W (stop them after checking; never by this script):"
    while IFS="$SEP" read -r kind pid c; do
      echo "  $kind $pid (cwd $c): $(awk -F "$SEP" -v p="$pid" '$1 == p { print $2 " " $3; exit }' "$SNAP" | cut -c1-200)"
    done <<<"$TASKS"
  } >&2
  exit 1
fi

BROKERS=""
while IFS="$SEP" read -r kind pid cwd _; do
  [ "$kind" = broker ] || continue
  c="$(canon "$cwd" || true)"
  if [ -n "$c" ] && under "$c" "$W"; then
    BROKERS="$BROKERS $pid"
    echo "broker $pid (cwd $cwd)"
  fi
done <"$PROCS"
[ -n "$BROKERS" ] || echo "no Codex broker for $W"

if [ "$DRY" = 1 ]; then
  echo "dry run: would stop:${BROKERS:- nothing}; remove worktree $REG${BRANCH:+; delete branch $BRANCH ($WHY)}"
  exit 0
fi

# 5. Stop the brokers.
# shellcheck disable=SC2086 # BROKERS is a space-separated pid list
[ -z "$BROKERS" ] || stop_set $BROKERS

# 6. Confirm nothing is left, from a new snapshot.
LEFT=""
for p in $BROKERS; do
  if valid "$p"; then LEFT="$LEFT $p"; fi
done
snapshot_procs
codex_procs >"$PROCS"
codex_jobs ${STATE_ROOTS[@]+"${STATE_ROOTS[@]}"} >"$JOBS"
while IFS="$SEP" read -r kind pid cwd _; do
  [ "$kind" = broker ] || continue
  c="$(canon "$cwd" || true)"
  if [ -n "$c" ] && under "$c" "$W"; then LEFT="$LEFT $pid"; fi
done <"$PROCS"
LEFT_TASKS="$(tasks_at "$W" | cut -d "$SEP" -f2 | tr '\n' ' ')"
LEFT_JOBS="$(live_jobs_at "$W" | cut -d "$SEP" -f2 | tr '\n' ' ')"
if [ -n "$LEFT" ] || [ -n "$LEFT_TASKS" ] || [ -n "$LEFT_JOBS" ]; then
  die "still running for $W (brokers:${LEFT:- none}; tasks: ${LEFT_TASKS:-none}; jobs: ${LEFT_JOBS:-none}); nothing removed"
fi

# 7. Remove the worktree.
DIRTY="$(git -C "$W" status --short 2>/dev/null | wc -l | tr -d ' ')"
[ "$DIRTY" = 0 ] || echo "warning: discarding $DIRTY uncommitted path(s) in $W"
git -C "$MAIN" worktree remove --force "$REG"
echo "removed worktree $REG"

# 8. Delete the branch.
if [ -n "$BRANCH" ]; then
  git -C "$MAIN" branch -D "$BRANCH"
  echo "  ($BRANCH: $WHY)"
fi
