#!/usr/bin/env bash
# suite-lock.sh — run a test suite while holding the cargo target dir's suite lock, so two suites
# never run at once in one target dir (a concurrent `cargo build --example` re-links example
# binaries a running suite uses).
#
# Usage: bash .claude/scripts/suite-lock.sh <command> [args…]
#   e.g. bash .claude/scripts/suite-lock.sh cargo test --locked
#
# The lock is the directory <target>/.glen-suite.lock/, <target> = $CARGO_TARGET_DIR if set, else
# <git toplevel of the cwd>/target. Its `owner` file holds the holder's pid, `ps -o lstart=`, the
# command and the UTC ISO time it took the lock. A second caller waits, printing the holder once a
# minute, up to GLEN_SUITE_LOCK_WAIT seconds (default 3600), then exits 75. A lock whose pid is
# gone, or whose pid's start time no longer matches, is stale: it is taken over with a one-line
# notice. The lock is released on exit, INT or TERM, only by the process that holds it.
#
# Exit status: the command's; 75 = gave up waiting; 2 = bad input (usage, no target dir).
set -u

die() { echo "suite-lock: $*" >&2; exit 2; }

[ $# -ge 1 ] || die "usage: suite-lock.sh <command> [args…]"
WAIT="${GLEN_SUITE_LOCK_WAIT:-3600}"
case "$WAIT" in '' | *[!0-9]*) die "GLEN_SUITE_LOCK_WAIT must be whole seconds: $WAIT" ;; esac

if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  TARGET="$CARGO_TARGET_DIR"
else
  TOP="$(git rev-parse --show-toplevel 2>/dev/null)" || die "not inside a git checkout and CARGO_TARGET_DIR is unset"
  TARGET="$TOP/target"
fi
mkdir -p "$TARGET" || die "cannot create the target dir: $TARGET"
LOCK="$TARGET/.glen-suite.lock"
GUARD="$LOCK.takeover"

lstart() { ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//'; }
field() { sed -n "s/^$1=//p" "$LOCK/owner" 2>/dev/null | head -n 1; }
holder() { echo "pid $(field pid) (started $(field lstart)) since $(field since): $(field cmd)"; }

OWNED=0
CHILD=""
release() {
  [ "$OWNED" = 1 ] && [ "$(field pid)" = "$$" ] || return 0
  rm -f "$LOCK/owner" && rmdir "$LOCK" 2>/dev/null
  OWNED=0
}
# shellcheck disable=SC2329  # invoked from the INT/TERM traps
on_signal() {
  [ -n "$CHILD" ] && kill -TERM "$CHILD" 2>/dev/null && wait "$CHILD" 2>/dev/null
  guard_drop
  release
  exit "$1"
}
trap 'guard_drop; release' EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

GUARDED=0
guard_drop() {
  [ "$GUARDED" = 1 ] || return 0
  rm -f "$GUARD/pid"
  rmdir "$GUARD" 2>/dev/null
  GUARDED=0
}
# guard_take: 0 when this process now holds the guard dir, which serialises taking and taking over
# the lock. A guard whose pid is gone, or that has had no pid for a minute, was left by a killed
# process and is removed (the caller retries on its next turn).
guard_take() {
  local pid
  if mkdir "$GUARD" 2>/dev/null; then
    GUARDED=1
    echo "$$" >"$GUARD/pid"
    return 0
  fi
  pid="$(cat "$GUARD/pid" 2>/dev/null)"
  if { [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; } ||
    { [ -z "$pid" ] && [ -n "$(find "$GUARD" -maxdepth 0 -mmin +1 2>/dev/null)" ]; }; then
    [ "$(cat "$GUARD/pid" 2>/dev/null)" = "$pid" ] && rm -f "$GUARD/pid" && rmdir "$GUARD" 2>/dev/null
  fi
  return 1
}

# try_lock (guard held): 0 = took the lock; 1 = a live holder has it; 2 = removed a stale lock.
# Every taker holds the guard, so the owner file cannot be replaced while this runs; a holder
# releasing meanwhile only empties it, and rmdir never removes a lock that has an owner file.
try_lock() {
  local pid start
  if mkdir "$LOCK" 2>/dev/null; then
    OWNED=1
    printf 'pid=%s\nlstart=%s\ncmd=%s\nsince=%s\n' "$$" "$(lstart $$)" "$*" \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$LOCK/owner" || die "cannot write $LOCK/owner"
    return 0
  fi
  pid="$(field pid)"
  if [ -n "$pid" ]; then
    start="$(lstart "$pid")"
    [ -n "$start" ] && [ "$start" = "$(field lstart)" ] && return 1
    echo "suite-lock: taking over stale lock ($(holder))" >&2
    rm -f "$LOCK/owner"
  elif [ -d "$LOCK" ]; then
    # No owner file while we hold the guard: its taker died between mkdir and writing it.
    echo "suite-lock: taking over stale lock (no owner file)" >&2
  fi
  rmdir "$LOCK" 2>/dev/null
  return 2
}

START="$(date +%s)"
LAST_NOTE=""
while :; do
  if guard_take; then
    try_lock "$@"
    r=$?
    guard_drop
    [ "$r" = 0 ] && break
    [ "$r" = 2 ] && continue
  fi
  now="$(date +%s)"
  if [ $((now - START)) -ge "$WAIT" ]; then
    echo "suite-lock: gave up after ${WAIT}s; lock $LOCK held by $(holder)" >&2
    exit 75
  fi
  if [ -z "$LAST_NOTE" ] || [ $((now - LAST_NOTE)) -ge 60 ]; then
    echo "suite-lock: waiting for $LOCK, held by $(holder)" >&2
    LAST_NOTE="$now"
  fi
  left=$((WAIT - (now - START)))
  sleep "$([ "$left" -lt 5 ] && echo "$left" || echo 5)"
done

# A background child plus `wait` lets the INT/TERM traps run while the command is still going;
# `<&0` keeps the caller's stdin (a background job would otherwise read /dev/null).
"$@" <&0 &
CHILD=$!
wait "$CHILD"
rc=$?
CHILD=""
release
exit "$rc"
