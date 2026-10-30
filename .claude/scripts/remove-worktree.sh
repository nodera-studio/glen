#!/usr/bin/env bash
# remove-worktree.sh — remove a .claude/worktrees/ worktree after stopping its Codex processes.
#
# Usage: bash "$MAIN/.claude/scripts/remove-worktree.sh" <worktree-path> [<branch>]
#
# In this order:
#   a. Stop every codex-companion broker (app-server-broker.mjs serve) and task process
#      (codex-companion.mjs task / task-worker) whose --cwd is exactly <worktree-path>, with
#      all its descendants (app-server, sandbox): SIGTERM, wait up to 10 s, then SIGKILL.
#      A task started without --cwd matches on its process cwd, also exactly. Never a prefix
#      match: .claude/worktrees/w5 does not match .claude/worktrees/w5b.
#   b. Confirm none remain; if any do, exit 1 and remove nothing.
#   c. git worktree remove --force <worktree-path> (discards uncommitted changes in it).
#   d. With <branch>, delete the local branch with git branch -D.
#
# Refuses any path that is not directly under the main checkout's .claude/worktrees/. Run the
# main checkout's copy of this script, not the copy inside the worktree being removed.
set -euo pipefail

die() { echo "remove-worktree: $*" >&2; exit 1; }

[ $# -ge 1 ] && [ $# -le 2 ] || die "usage: remove-worktree.sh <worktree-path> [<branch>]"
ARG="${1%/}"
BRANCH="${2:-}"

case "$ARG" in /*) ;; *) die "refuse: path must be absolute: $ARG" ;; esac
case "$ARG" in *[[:space:]]*) die "refuse: path contains whitespace: $ARG" ;; esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
GCD="$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
  || die "refuse: $SCRIPT_DIR is not in a git checkout"
MAIN="$(cd "$GCD/.." && pwd -P)"
ROOT="$MAIN/.claude/worktrees"

NAME="${ARG##*/}"
PARENT="$(cd "${ARG%/*}" 2>/dev/null && pwd -P)" || die "refuse: ${ARG%/*} does not exist"
case "$NAME" in '' | . | ..) die "refuse: bad worktree name in $ARG" ;; esac
[ "$PARENT" = "$ROOT" ] || die "refuse: $ARG is not directly under $ROOT"
WT="$ROOT/$NAME"
case "$SCRIPT_DIR/" in "$WT/"*) die "refuse: run the main checkout's copy ($MAIN/.claude/scripts/remove-worktree.sh)" ;; esac
case "$BRANCH" in main | master | HEAD) die "refuse: will not delete branch $BRANCH" ;; esac

cd "$MAIN"

# The worktree's cwd for a process: its first --cwd argument, else its process cwd.
proc_cwd() {
  if [ -L "/proc/$1/cwd" ]; then
    readlink "/proc/$1/cwd" 2>/dev/null || true
  else
    lsof -a -p "$1" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -1 || true
  fi
}

RE='^([^ ]*/)?node [^ ]*/(app-server-broker\.mjs serve|codex-companion\.mjs (task|task-worker))( |$)'

# Print the pid of every broker or task process whose cwd is exactly $ARG or $WT.
matching_pids() {
  local pid cmd cwd prev tok
  ps -axo pid=,command= | while read -r pid cmd; do
    [[ $cmd =~ $RE ]] || continue
    if [ "$pid" = "$$" ] || [ "$pid" = "$PPID" ]; then continue; fi
    cwd="" prev=""
    set -f
    for tok in $cmd; do
      if [ "$prev" = "--cwd" ]; then cwd="$tok"; break; fi
      prev="$tok"
    done
    set +f
    [ -n "$cwd" ] || cwd="$(proc_cwd "$pid")"
    cwd="${cwd%/}"
    if [ "$cwd" = "$ARG" ] || [ "$cwd" = "$WT" ]; then echo "$pid"; fi
  done
}

descendants() {
  local c
  for c in $(pgrep -P "$1" 2>/dev/null || true); do
    descendants "$c"
    echo "$c"
  done
}

alive() { kill -0 "$1" 2>/dev/null; }

# a. Stop matched processes and their descendants: TERM, wait, KILL.
ROOTS="$(matching_pids)"
TARGETS=""
for p in $ROOTS; do
  TARGETS="$TARGETS $(descendants "$p" | tr '\n' ' ') $p"
done
if [ -n "${TARGETS// /}" ]; then
  for p in $ROOTS; do
    echo "stopping $p: $(ps -o command= -p "$p" 2>/dev/null | cut -c1-160)"
  done
  for p in $TARGETS; do kill -TERM "$p" 2>/dev/null || true; done
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    left=""
    for p in $TARGETS; do alive "$p" && left="$left $p"; done
    [ -z "$left" ] && break
    sleep 1
  done
  for p in $TARGETS; do
    if alive "$p"; then
      echo "SIGKILL $p"
      kill -KILL "$p" 2>/dev/null || true
    fi
  done
  sleep 1
else
  echo "no codex-companion broker or task process for $WT"
fi

# b. Confirm none remain.
LEFT="$(matching_pids)"
for p in $TARGETS; do alive "$p" && LEFT="$LEFT $p"; done
[ -z "${LEFT// /}" ] || die "processes still running for $WT:$(echo " $LEFT" | tr '\n' ' '); nothing removed"

# c. Remove the worktree.
if git worktree list --porcelain | grep -qxF -e "worktree $WT" -e "worktree $ARG"; then
  DIRTY="$(git -C "$WT" status --short 2>/dev/null | wc -l | tr -d ' ')"
  [ "$DIRTY" = 0 ] || echo "warning: discarding $DIRTY uncommitted path(s) in $WT"
  git worktree remove --force "$WT"
  echo "removed worktree $WT"
elif [ -e "$WT" ]; then
  die "refuse: $WT exists but is not a registered worktree"
else
  git worktree prune
  echo "worktree $WT already gone (pruned)"
fi

# d. Optionally delete the local branch.
if [ -n "$BRANCH" ]; then
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    git branch -D "$BRANCH"
  else
    echo "no local branch $BRANCH"
  fi
fi
