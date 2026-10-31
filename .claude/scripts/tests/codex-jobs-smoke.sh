#!/bin/bash
# codex-jobs-smoke.sh — codex_jobs / live_jobs_at (codex-procs.sh) against fixture stores.
#
# Usage: /bin/bash .claude/scripts/tests/codex-jobs-smoke.sh
# Bash 3.2, no network, no real companion: both companion stores ($CLAUDE_PLUGIN_DATA/state and
# $TMPDIR/codex-companion) are temp dirs, and the process snapshot ($SNAP) and companion process
# list ($PROCS) are fixture files with made-up pids. Prints ok/FAIL per case; exit 1 on any FAIL.
set -euo pipefail

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
LIB="$TEST_DIR/../codex-procs.sh"
SEP=$'\x1f'
WORK="$(mktemp -d "${TMPDIR:-/tmp}/codex-jobs-smoke.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

W="$WORK/wt"            # the worktree being checked
OUT="$WORK/elsewhere"   # a directory outside it
PLUGIN="$WORK/plugin"   # CLAUDE_PLUGIN_DATA; its store is $PLUGIN/state
TMPD="$WORK/tmp"        # TMPDIR; its store is $TMPD/codex-companion
P_STORE="$PLUGIN/state/wt-1"
T_STORE="$TMPD/codex-companion/wt-1"
mkdir -p "$W" "$OUT" "$P_STORE/jobs" "$T_STORE/jobs"
SNAP="$WORK/snap" PROCS="$WORK/procs" JOBS="$WORK/jobs.out" LIVE="$WORK/live.out"

# Index entries (state.json) and per-job files (jobs/<id>.json).
cat >"$P_STORE/state.json" <<EOF
{"jobs": [
  {"id": "job-a", "status": "running", "pid": 90001, "workspaceRoot": "$W"},
  {"id": "job-i", "status": "completed", "pid": null, "workspaceRoot": "$W"}
]}
EOF
cat >"$T_STORE/state.json" <<EOF
{"jobs": [
  {"id": "job-b", "status": "running", "pid": 90002, "workspaceRoot": "$W"},
  {"id": "job-c", "status": "running", "pid": 90003, "workspaceRoot": "$W"},
  {"id": "job-i", "status": "running", "pid": 90002, "workspaceRoot": "$W"}
]}
EOF
# (a) the file says completed with pid null; the index in the same dir still says running.
printf '{"id":"job-a","status":"completed","pid":null,"workspaceRoot":"%s","request":{"cwd":"%s"}}\n' "$W" "$W" >"$P_STORE/jobs/job-a.json"
# (b) the file is in the plugin store; the stale running index entry is in the TMPDIR store.
printf '{"id":"job-b","status":"completed","pid":null,"workspaceRoot":"%s"}\n' "$W" >"$P_STORE/jobs/job-b.json"
# (d) running, and its pid is the task-worker for this job.
printf '{"id":"job-d","status":"running","pid":90004,"workspaceRoot":"%s"}\n' "$W" >"$P_STORE/jobs/job-d.json"
# (e) queued.
printf '{"id":"job-e","status":"queued","pid":null,"workspaceRoot":"%s"}\n' "$W" >"$P_STORE/jobs/job-e.json"
# (g) running foreground review: the job's pid is the companion `review` process itself.
printf '{"id":"job-g","status":"running","pid":90005,"workspaceRoot":"%s"}\n' "$W" >"$P_STORE/jobs/job-g.json"
# (h) the same for adversarial-review.
printf '{"id":"job-h","status":"running","pid":90006,"workspaceRoot":"%s"}\n' "$W" >"$P_STORE/jobs/job-h.json"
# (i) no per-job file; the plugin store's index says completed, the TMPDIR store's (read later)
# still says running with a live task pid. The terminal entry wins.
# (f) queued, outside the worktree.
printf '{"id":"job-f","status":"queued","pid":null,"workspaceRoot":"%s","request":{"cwd":"%s"}}\n' "$OUT" "$OUT" >"$P_STORE/jobs/job-f.json"

# Snapshot "pid SEP lstart SEP command" and companion processes "kind SEP pid SEP cwd SEP job SEP pcwd".
# 90001 and 90002 are live companion tasks, so only the per-job file can make (a) and (b) not live.
# 90003 is a reused pid: a plain sleep, not a companion process (c).
LS="Fri Oct 31 10:00:00 2026"
{
  printf '90001%s%s%snode /x/codex-companion.mjs task-worker --cwd %s --job-id job-a\n' "$SEP" "$LS" "$SEP" "$W"
  printf '90002%s%s%snode /x/codex-companion.mjs task --write\n' "$SEP" "$LS" "$SEP"
  printf '90003%s%s%ssleep 1000\n' "$SEP" "$LS" "$SEP"
  printf '90004%s%s%snode /x/codex-companion.mjs task-worker --cwd %s --job-id job-d\n' "$SEP" "$LS" "$SEP" "$W"
  printf '90005%s%s%snode /x/codex-companion.mjs review --base main --json\n' "$SEP" "$LS" "$SEP"
  printf '90006%s%s%snode /x/codex-companion.mjs adversarial-review --json\n' "$SEP" "$LS" "$SEP"
} >"$SNAP"
{
  printf 'task-worker%s90001%s%s%sjob-a%s%s\n' "$SEP" "$SEP" "$W" "$SEP" "$SEP" "$W"
  printf 'task%s90002%s%s%s%s\n' "$SEP" "$SEP" "$SEP" "$SEP" "$W"
  printf 'task-worker%s90004%s%s%sjob-d%s%s\n' "$SEP" "$SEP" "$W" "$SEP" "$SEP" "$W"
  printf 'review%s90005%s%s%s%s\n' "$SEP" "$SEP" "$SEP" "$SEP" "$W"
  printf 'adversarial-review%s90006%s%s%s%s\n' "$SEP" "$SEP" "$SEP" "$SEP" "$W"
} >"$PROCS"

# shellcheck disable=SC2016 # expanded by the inner shell
env CLAUDE_PLUGIN_DATA="$PLUGIN" TMPDIR="$TMPD" SNAP="$SNAP" PROCS="$PROCS" JOBS="$JOBS" \
  /bin/bash -c 'set -euo pipefail
    . "$1"
    load_roots
    [ ${#STATE_ROOTS[@]} -eq 2 ] || { echo "FAIL setup: expected 2 stores, got ${#STATE_ROOTS[@]}" >&2; exit 1; }
    codex_jobs "${STATE_ROOTS[@]}" >"$JOBS"
    W="$(canon "$2")"
    live_jobs_at "$W"
    codex_procs >"$3"' _ "$LIB" "$W" "$WORK/procs.gen" >"$LIVE"

FAILS=0
check() { # check LABEL EXPECT(live|not) ID
  local got=not
  if awk -F "$SEP" -v j="$3" '$2 == j { f = 1 } END { exit !f }' "$LIVE"; then got=live; fi
  if [ "$got" = "$2" ]; then echo "ok   $1"; else echo "FAIL $1 (expected $2, got $got)"; FAILS=$((FAILS + 1)); fi
}
check "(a) index running, file completed pid null, same dir -> not live" not job-a
check "(b) file completed in the plugin store, index running in the TMPDIR store -> not live" not job-b
B_LINES="$(awk -F "$SEP" '$2 == "job-b"' "$JOBS")"
if [ "$(printf '%s\n' "$B_LINES" | wc -l | tr -d ' ')" = 1 ] && [ "${B_LINES%%"$SEP"*}" = "$PLUGIN/state" ]; then
  echo "ok   (b) one job-b record, rooted in the plugin store"
else
  echo "FAIL (b) expected one job-b record rooted in $PLUGIN/state, got: $(printf '%s' "$B_LINES" | tr "$SEP" '|')"
  FAILS=$((FAILS + 1))
fi
check "(c) index running, no file, pid reused by a sleep -> not live" not job-c
check "(d) file running, pid is task-worker --job-id job-d -> live" live job-d
check "(e) file queued -> live" live job-e
check "(f) job outside the worktree -> not listed" not job-f
check "(g) file running, pid is a companion review process -> live" live job-g
check "(h) file running, pid is a companion adversarial-review process -> live" live job-h
check "(i) index completed in one store, running with a live pid in the other -> not live" not job-i

# codex_procs classifies the snapshot by fixed argv shape (the kinds live_jobs_at accepts).
kinds="$(awk -F "$SEP" '{ printf "%s:%s ", $2, $1 }' "$WORK/procs.gen")"
if [ "$kinds" = "90001:task-worker 90002:task 90004:task-worker 90005:review 90006:adversarial-review " ]; then
  echo "ok   codex_procs kinds: task-worker, task, review, adversarial-review; sleep ignored"
else
  echo "FAIL codex_procs kinds: got '$kinds'"
  FAILS=$((FAILS + 1))
fi

if [ "$FAILS" -gt 0 ]; then echo "$FAILS case(s) failed"; exit 1; fi
echo "all cases passed"
