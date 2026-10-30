#!/usr/bin/env bash
# sweep-codex-orphans.sh — stop codex-companion processes left behind by finished runs and
# removed worktrees of this repo.
#
# Usage: bash .claude/scripts/sweep-codex-orphans.sh [--dry-run]
#
# Only processes tied to this repo are considered: the main checkout, or a worktree directly
# under its .claude/worktrees/. Other repos' processes are never touched.
#   - Broker (app-server-broker.mjs serve) under .claude/worktrees/ whose --cwd no longer exists.
#   - Task process (codex-companion.mjs task / task-worker) whose worktree no longer exists, or
#     whose job the companion's status reports as finished:
#       task-worker: `status <job-id>` is not queued or running;
#       task:        no queued or running job of that worktree (any Claude session) has its pid,
#                    and the process is older than 10 min (a starting task has no job record yet).
#     A task's worktree is the one holding its --cwd; without --cwd, the one holding its
#     process cwd, else the one worktree path its prompt names. A task whose worktree cannot be resolved, or whose status cannot be read,
#     is left alone: nothing is stopped by command-line pattern alone.
# Stop = SIGTERM to the process and its descendants, SIGKILL to survivors after 10 s.
set -euo pipefail

DRY=0
case "${1:-}" in --dry-run | -n) DRY=1 ;; "") ;; *) echo "usage: sweep-codex-orphans.sh [--dry-run]" >&2; exit 2 ;; esac

GCD="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
  || { echo "refuse: not in a git repo" >&2; exit 1; }
MAIN="$(cd "$GCD/.." && pwd -P)"
[ -n "$MAIN" ] && [ "$MAIN" != / ] || { echo "refuse: cannot resolve the main checkout" >&2; exit 1; }
ROOT="$MAIN/.claude/worktrees/"

CODEX_ROOT="$(find ~/.claude/plugins/cache/openai-codex/codex -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -1 || true)"
COMPANION="${CODEX_ROOT:+$CODEX_ROOT/scripts/codex-companion.mjs}"
[ -n "$COMPANION" ] && [ -f "$COMPANION" ] || COMPANION=""

proc_cwd() {
  if [ -L "/proc/$1/cwd" ]; then
    readlink "/proc/$1/cwd" 2>/dev/null || true
  else
    lsof -a -p "$1" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -1 || true
  fi
}

# ps etime ([[dd-]hh:]mm:ss) in seconds.
age_s() {
  local e d=0 h=0 m=0 s=0
  e="$(ps -o etime= -p "$1" 2>/dev/null | tr -d ' ')" || return 1
  [ -n "$e" ] || return 1
  case "$e" in *-*) d="${e%%-*}"; e="${e#*-}" ;; esac
  IFS=: read -r a b c <<<"$e"
  if [ -n "${c:-}" ]; then h="$a" m="$b" s="$c"; else m="$a" s="$b"; fi
  echo $(((10#$d * 86400) + (10#$h * 3600) + (10#$m * 60) + 10#$s))
}

# The worktree of this repo that contains a path (a .claude/worktrees/<name> root, or the main
# checkout), or nothing.
wt_of() {
  local rest
  case "$1/" in
    "$ROOT"?*) rest="${1#"$ROOT"}"; echo "$ROOT${rest%%/*}" ;;
    "$MAIN/"*) echo "$MAIN" ;;
  esac
}

# The single .claude/worktrees/<name> path a command line names, if exactly one.
prompt_wt() {
  local rest name found=""
  rest="$1"
  while [ "${rest#*"$ROOT"}" != "$rest" ]; do
    rest="${rest#*"$ROOT"}"
    name="${rest%%[/ \"\'\`),;:]*}"
    [ -n "$name" ] || continue
    if [ -z "$found" ]; then found="$ROOT$name"; elif [ "$found" != "$ROOT$name" ]; then return 0; fi
  done
  echo "$found"
}

descendants() {
  local c
  for c in $(pgrep -P "$1" 2>/dev/null || true); do
    descendants "$c"
    echo "$c"
  done
}

stop_tree() {
  local pid="$1" why="$2" t p
  echo "orphan $pid ($why): $(ps -o command= -p "$pid" 2>/dev/null | cut -c1-160)"
  [ "$DRY" = 1 ] && return 0
  t="$(descendants "$pid" | tr '\n' ' ') $pid"
  for p in $t; do kill -TERM "$p" 2>/dev/null || true; done
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
  done
  for p in $t; do
    if kill -0 "$p" 2>/dev/null; then kill -KILL "$p" 2>/dev/null || true; fi
  done
}

status_json() { env -u CODEX_COMPANION_SESSION_ID node "$COMPANION" status "$@" --json 2>/dev/null; }

BROKER_RE='^([^ ]*/)?node [^ ]*/app-server-broker\.mjs serve '
TASK_RE='^([^ ]*/)?node [^ ]*/codex-companion\.mjs (task|task-worker)( |$)'

ps -axo pid=,command= | while read -r pid cmd; do
  if [ "$pid" = "$$" ] || [ "$pid" = "$PPID" ]; then continue; fi
  if [[ $cmd =~ $BROKER_RE ]]; then kind=broker
  elif [[ $cmd =~ $TASK_RE ]]; then kind="${BASH_REMATCH[2]}"
  else continue
  fi
  cwd="" job="" prev=""
  set -f
  for tok in $cmd; do
    case "$prev" in --cwd) [ -n "$cwd" ] || cwd="$tok" ;; --job-id) [ -n "$job" ] || job="$tok" ;; esac
    prev="$tok"
  done
  set +f
  if [ "$kind" = broker ]; then
    cwd="${cwd%/}"
    case "$cwd" in "$ROOT"?*) [ -d "$cwd" ] || stop_tree "$pid" "broker, worktree gone: $cwd" ;; esac
    continue
  fi
  case "$cwd" in
    /*) cwd="$(wt_of "${cwd%/}")" ;;
    '') cwd="$(wt_of "$(proc_cwd "$pid")")"; [ -n "$cwd" ] || cwd="$(prompt_wt "$cmd")" ;;
    *) continue ;;
  esac
  [ -n "$cwd" ] || continue
  if [ ! -d "$cwd" ]; then stop_tree "$pid" "$kind, worktree gone: $cwd"; continue; fi
  [ -n "$COMPANION" ] || continue
  if [ "$kind" = task-worker ]; then
    [ -n "$job" ] || continue
    st="$(status_json "$job" --cwd "$cwd" | jq -r '.job.status // empty' 2>/dev/null || true)"
    case "$st" in completed | failed | cancelled) stop_tree "$pid" "task-worker, job $job $st" ;; esac
  else
    age="$(age_s "$pid" || true)"
    if [ -z "$age" ] || [ "$age" -lt 600 ]; then continue; fi
    n="$(status_json --cwd "$cwd" --all | jq --argjson p "$pid" '[.running[] | select(.pid == $p)] | length' 2>/dev/null || true)"
    if [ "$n" = 0 ]; then stop_tree "$pid" "task, no running job in $cwd, age ${age}s"; fi
  fi
done
exit 0
