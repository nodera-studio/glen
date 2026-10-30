# codex-procs.sh — helpers shared by remove-worktree.sh and sweep-codex-orphans.sh. Source it.
# shellcheck shell=bash
#
# The scripts signal brokers only; jobs, tasks and task-workers are never signalled. Ownership is
# decided from ONE process snapshot (snapshot_procs: `ps -axww -o pid=,lstart=,command=`). Before
# each signal the pid's current pid/lstart/command must equal its snapshot line, else it is
# skipped. Processes are identified only by fixed argv shapes, never by text elsewhere in a
# command line:
#   broker       node .../app-server-broker.mjs serve --endpoint E --cwd C --pid-file P
#   task-worker  node .../codex-companion.mjs task-worker --cwd C --job-id J
#   task         node .../codex-companion.mjs task ...   (argv beyond "task" is never read)
# Jobs come from the companion's own state, in both stores companion 1.0.6 uses (load_roots).
#
# Callers set SNAP (snapshot file), PROCS (codex_procs output) and JOBS (codex_jobs output).

# Field separator for every record: ASCII unit separator (a tab would collapse empty fields in read).
SEP=$'\x1f'

# canon PATH: canonical absolute path (pwd -P of the longest existing ancestor, plus the rest).
# Fails for a relative path or a non-existing remainder containing . or .. segments.
canon() {
  local p="${1%/}" rest=""
  case "$1" in /*) ;; *) return 1 ;; esac
  while [ -n "$p" ] && [ ! -d "$p" ]; do
    rest="/${p##*/}$rest"
    p="${p%/*}"
  done
  case "$rest/" in */./* | */../*) return 1 ;; esac
  p="$(cd "${p:-/}" && pwd -P)" || return 1
  if [ "$p" = / ]; then p=""; fi
  printf '%s\n' "${p}${rest:-}"
}

# under A B: A is B or below it (slash boundary).
under() {
  case "$1/" in "$2/"*) return 0 ;; esac
  return 1
}

# ps_records: normalise `ps -o pid=,lstart=,command=` lines on stdin to "pid SEP lstart SEP command".
ps_records() {
  local pid a b c d e cmd
  while read -r pid a b c d e cmd; do
    if [ -n "$pid" ]; then printf '%s%s%s %s %s %s %s%s%s\n' "$pid" "$SEP" "$a" "$b" "$c" "$d" "$e" "$SEP" "$cmd"; fi
  done
}

# snapshot_procs: the single process snapshot every ownership decision is made from, into $SNAP.
snapshot_procs() { ps -axww -o pid=,lstart=,command= | ps_records >"$SNAP"; }

# valid PID: the process still has the pid, start time and command of its snapshot line.
valid() {
  local want now
  want="$(awk -F "$SEP" -v p="$1" '$1 == p { print; exit }' "$SNAP")"
  now="$(ps -ww -o pid=,lstart=,command= -p "$1" 2>/dev/null | ps_records)"
  [ -n "$want" ] && [ "$now" = "$want" ]
}

# in_snapshot PID: PID is a process in the snapshot.
in_snapshot() { awk -F "$SEP" -v p="$1" '$1 == p { f = 1 } END { exit !f }' "$SNAP"; }

# proc_cwd PID: the process's current directory (for report and refusal checks only).
proc_cwd() {
  if [ -L "/proc/$1/cwd" ]; then
    readlink "/proc/$1/cwd" 2>/dev/null || true
  else
    lsof -a -p "$1" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -1 || true
  fi
}

# codex_procs: companion processes of the snapshot, "kind SEP pid SEP argv-cwd SEP job SEP proc-cwd".
codex_procs() {
  local pid cmd kind cwd job
  while IFS="$SEP" read -r pid _ cmd; do
    set -f
    # shellcheck disable=SC2086 # deliberate word split of the command line into argv
    set -- $cmd
    set +f
    [ $# -ge 3 ] || continue
    [ "${1##*/}" = node ] || continue
    if [ $# -eq 9 ] && [ "${2##*/}" = app-server-broker.mjs ] && [ "$3" = serve ] \
      && [ "$4" = --endpoint ] && [ "$6" = --cwd ] && [ "$8" = --pid-file ]; then
      kind=broker cwd="$7" job=""
    elif [ $# -eq 7 ] && [ "${2##*/}" = codex-companion.mjs ] && [ "$3" = task-worker ] \
      && [ "$4" = --cwd ] && [ "$6" = --job-id ]; then
      kind=task-worker cwd="$5" job="$7"
    elif [ "${2##*/}" = codex-companion.mjs ] && { [ "$3" = task ] || [ "$3" = task-worker ]; }; then
      kind="$3" cwd="" job=""
    else
      continue
    fi
    if [ "$kind" = broker ]; then
      printf '%s%s%s%s%s%s%s%s\n' "$kind" "$SEP" "$pid" "$SEP" "$cwd" "$SEP" "$job" "$SEP"
    else
      printf '%s%s%s%s%s%s%s%s%s\n' "$kind" "$SEP" "$pid" "$SEP" "$cwd" "$SEP" "$job" "$SEP" "$(proc_cwd "$pid")"
    fi
  done <"$SNAP"
}

# load_roots: set the array STATE_ROOTS to the companion's state stores that exist:
# $CLAUDE_PLUGIN_DATA/state (default ~/.claude/plugins/data/codex-openai-codex/state) and its
# fallback os.tmpdir()/codex-companion. Paths may contain spaces: always expand it quoted, as
# ${STATE_ROOTS[@]+"${STATE_ROOTS[@]}"} (bash 3.2 treats an empty array as unset under set -u).
load_roots() {
  local d
  STATE_ROOTS=()
  for d in "${CLAUDE_PLUGIN_DATA:-$HOME/.claude/plugins/data/codex-openai-codex}/state" \
    "$(cd "${TMPDIR:-/tmp}" 2>/dev/null && pwd)/codex-companion"; do
    if [ -d "$d" ]; then STATE_ROOTS+=("$d"); fi
  done
}

# codex_jobs ROOT...: every job, "root SEP id SEP status SEP workspaceRoot SEP cwd SEP pid".
# A jobs/<id>.json record overrides the state.json entry (it is written first and is fuller).
codex_jobs() {
  [ $# -gt 0 ] || return 0
  node - "$@" <<'EOF'
const fs = require("fs");
const path = require("path");
const read = (f) => { try { return JSON.parse(fs.readFileSync(f, "utf8")); } catch { return null; } };
const clean = (v) => String(v ?? "").replace(/[\x1f\n\r]/g, " ");
for (const root of process.argv.slice(2)) {
  const jobs = new Map();
  const add = (job) => {
    if (job && typeof job.id === "string") jobs.set(job.id, { ...(jobs.get(job.id) || {}), ...job });
  };
  let dirs = [];
  try { dirs = fs.readdirSync(root); } catch {}
  for (const d of dirs) {
    const dir = path.join(root, d);
    const state = read(path.join(dir, "state.json"));
    for (const j of (state && Array.isArray(state.jobs) ? state.jobs : [])) add(j);
    let files = [];
    try { files = fs.readdirSync(path.join(dir, "jobs")).filter((f) => f.endsWith(".json")); } catch {}
    for (const f of files) add(read(path.join(dir, "jobs", f)));
  }
  for (const j of jobs.values()) {
    const cwd = (j.request && j.request.cwd) ?? j.cwd ?? "";
    console.log([root, j.id, j.status, j.workspaceRoot, cwd, j.pid].map(clean).join("\x1f"));
  }
}
EOF
}

# live_jobs_at PATH: live jobs (queued, or running with a pid in the snapshot) whose
# workspaceRoot or cwd is at or below PATH, "root SEP id SEP workspaceRoot".
live_jobs_at() {
  local root id st ws cwd pid p c
  while IFS="$SEP" read -r root id st ws cwd pid; do
    case "$st" in
      queued) ;;
      running) if [ -z "$pid" ] || ! in_snapshot "$pid"; then continue; fi ;;
      *) continue ;;
    esac
    for p in "$ws" "$cwd"; do
      [ -n "$p" ] || continue
      c="$(canon "$p" || true)"
      if [ -n "$c" ] && under "$c" "$1"; then
        printf '%s%s%s%s%s\n' "$root" "$SEP" "$id" "$SEP" "$ws"
        break
      fi
    done
  done <"$JOBS"
}

# tasks_at PATH: task and task-worker processes whose fixed-argv cwd or process cwd is at or
# below PATH, "kind SEP pid SEP cwd".
tasks_at() {
  local kind pid cwd job pcwd c
  while IFS="$SEP" read -r kind pid cwd job pcwd; do
    [ "$kind" != broker ] || continue
    for c in "$cwd" "$pcwd"; do
      [ -n "$c" ] || continue
      c="$(canon "$c" || true)"
      if [ -n "$c" ] && under "$c" "$1"; then
        printf '%s%s%s%s%s\n' "$kind" "$SEP" "$pid" "$SEP" "$c"
        break
      fi
    done
  done <"$PROCS"
}

# cancel_hint ROOT ID WORKSPACE: the companion command that cancels a job in that store.
cancel_hint() {
  local env_part
  case "$1" in
    */codex-companion) env_part="env -u CLAUDE_PLUGIN_DATA -u CODEX_COMPANION_SESSION_ID" ;;
    *) env_part="CLAUDE_PLUGIN_DATA=$(printf '%q' "${1%/state}") env -u CODEX_COMPANION_SESSION_ID" ;;
  esac
  # shellcheck disable=SC2016 # $COMPANION is printed literally, for the reader to set
  printf '%s node "$COMPANION" cancel %q --cwd %q --json\n' "$env_part" "$2" "$3"
}

# stop_set PID...: SIGTERM each still-valid PID, wait up to 10 s for all of them to exit, then
# SIGKILL only survivors that still validate.
stop_set() {
  local p sent="" left
  for p in "$@"; do
    if valid "$p"; then
      if kill -TERM "$p" 2>/dev/null; then sent="$sent $p"; fi
    else
      echo "skip $p: pid, start time or command differs from the snapshot"
    fi
  done
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    left=""
    for p in $sent; do
      if valid "$p"; then left="$left $p"; fi
    done
    [ -n "$left" ] || return 0
    sleep 1
  done
  for p in $sent; do
    if valid "$p"; then
      echo "SIGKILL $p"
      kill -KILL "$p" 2>/dev/null || true
    fi
  done
  sleep 1
}
