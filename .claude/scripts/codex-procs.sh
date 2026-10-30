# codex-procs.sh — helpers shared by remove-worktree.sh and sweep-codex-orphans.sh. Source it.
# shellcheck shell=bash
#
# Processes are identified only by fixed argv shapes, never by text anywhere in a command line:
#   broker       node .../app-server-broker.mjs serve --endpoint E --cwd C --pid-file P
#   task-worker  node .../codex-companion.mjs task-worker --cwd C --job-id J
#   task         node .../codex-companion.mjs task ...   (argv beyond "task" is never read)
# Jobs come from the companion's own state: <state>/<slug>-<hash>/state.json and jobs/*.json.
# Before any signal, a process's `ps -o lstart=,command=` must equal the snapshot taken when it
# was listed (PID reuse guard). Process trees are never walked.

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

# Field separator for every record below: ASCII unit separator (a tab would collapse empty fields in read).
SEP=$'\x1f'

# codex_procs: one line per companion process, "kind SEP pid SEP cwd SEP job".
codex_procs() {
  local pid cmd
  ps -axww -o pid=,command= | while read -r pid cmd; do
    set -f
    # shellcheck disable=SC2086 # deliberate word split of the command line into argv
    set -- $cmd
    set +f
    [ $# -ge 3 ] || continue
    [ "${1##*/}" = node ] || continue
    if [ $# -eq 9 ] && [ "${2##*/}" = app-server-broker.mjs ] && [ "$3" = serve ] \
      && [ "$4" = --endpoint ] && [ "$6" = --cwd ] && [ "$8" = --pid-file ]; then
      printf 'broker%s%s%s%s%s\n' "$SEP" "$pid" "$SEP" "$7" "$SEP"
    elif [ $# -eq 7 ] && [ "${2##*/}" = codex-companion.mjs ] && [ "$3" = task-worker ] \
      && [ "$4" = --cwd ] && [ "$6" = --job-id ]; then
      printf 'task-worker%s%s%s%s%s%s\n' "$SEP" "$pid" "$SEP" "$5" "$SEP" "$7"
    elif [ "${2##*/}" = codex-companion.mjs ] && [ "$3" = task ]; then
      printf 'task%s%s%s%s\n' "$SEP" "$pid" "$SEP" "$SEP"
    fi
  done
}

# codex_jobs STATE_ROOT: every job, "id SEP status SEP workspaceRoot SEP cwd SEP pid SEP startedAt(epoch s)".
# A jobs/<id>.json record overrides the state.json entry (it is written first and is fuller).
codex_jobs() {
  [ -d "$1" ] || return 0
  node - "$1" <<'EOF'
const fs = require("fs");
const path = require("path");
const root = process.argv[2];
const jobs = new Map();
const add = (job) => {
  if (job && typeof job.id === "string") jobs.set(job.id, { ...(jobs.get(job.id) || {}), ...job });
};
const read = (f) => { try { return JSON.parse(fs.readFileSync(f, "utf8")); } catch { return null; } };
for (const d of fs.readdirSync(root)) {
  const dir = path.join(root, d);
  const state = read(path.join(dir, "state.json"));
  for (const j of (state && Array.isArray(state.jobs) ? state.jobs : [])) add(j);
  let files = [];
  try { files = fs.readdirSync(path.join(dir, "jobs")).filter((f) => f.endsWith(".json")); } catch {}
  for (const f of files) add(read(path.join(dir, "jobs", f)));
}
const clean = (v) => String(v ?? "").replace(/[\x1f\n\r]/g, " ");
for (const j of jobs.values()) {
  const started = Date.parse(j.startedAt ?? j.createdAt ?? "");
  const cwd = (j.request && j.request.cwd) ?? j.cwd ?? "";
  console.log([j.id, j.status, j.workspaceRoot, cwd, j.pid, Number.isFinite(started) ? Math.floor(started / 1000) : ""]
    .map(clean).join("\x1f"));
}
EOF
}

is_active() { [ "$1" = running ] || [ "$1" = queued ]; }

snap() { ps -o lstart=,command= -p "$1" 2>/dev/null || true; }

# remember PID: record its snapshot in $SNAPS.
remember() { printf '%s%s%s\n' "$1" "$SEP" "$(snap "$1")" >>"$SNAPS"; }

# valid PID: the process still has the start time and command recorded by remember.
valid() {
  local want now
  want="$(awk -F "$SEP" -v p="$1" '$1 == p { print substr($0, length($1) + 2); exit }' "$SNAPS")"
  now="$(snap "$1")"
  [ -n "$want" ] && [ "$now" = "$want" ]
}

# lstart_epoch PID: the process start time in epoch seconds.
lstart_epoch() {
  local l
  l="$(ps -o lstart= -p "$1" 2>/dev/null | tr -s ' ')" || return 1
  [ -n "$l" ] || return 1
  date -j -f '%a %b %d %T %Y' "$l" +%s 2>/dev/null || date -d "$l" +%s 2>/dev/null
}

# job_proc PID STARTED: PID is a live codex-companion task or task-worker that started no later
# than its job (STARTED, epoch s, plus 5 s). Otherwise the record is stale or the pid reused.
job_proc() {
  local s
  [ -n "$1" ] && [ -n "$2" ] || return 1
  set -f
  # shellcheck disable=SC2046 # deliberate word split of the command line into argv
  set -- "$1" "$2" $(ps -o command= -p "$1" 2>/dev/null || true)
  set +f
  [ $# -ge 5 ] && [ "${3##*/}" = node ] && [ "${4##*/}" = codex-companion.mjs ] || return 1
  case "$5" in task | task-worker) ;; *) return 1 ;; esac
  s="$(lstart_epoch "$1")" || return 1
  [ -n "$s" ] && [ "$s" -le $(($2 + 5)) ]
}

# stop_set PID...: SIGTERM each still-valid PID, wait up to 10 s for all of them to exit, then
# SIGKILL only survivors that still validate.
stop_set() {
  local p sent="" left
  for p in "$@"; do
    if valid "$p"; then
      if kill -TERM "$p" 2>/dev/null; then sent="$sent $p"; fi
    else
      echo "skip $p: start time or command changed since listing"
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

# companion_path: the newest installed codex-companion.mjs, or nothing.
companion_path() {
  local d
  d="$(find "$HOME/.claude/plugins/cache/openai-codex/codex" -mindepth 1 -maxdepth 1 -type d 2>/dev/null \
    | sort -V | tail -1 || true)"
  if [ -n "$d" ] && [ -f "$d/scripts/codex-companion.mjs" ]; then echo "$d/scripts/codex-companion.mjs"; fi
}
