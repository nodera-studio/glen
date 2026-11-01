#!/bin/bash
# suite-lock-smoke.sh — smoke test for .claude/scripts/suite-lock.sh (Bash 3.2, no network).
#
# Usage: /bin/bash .claude/scripts/tests/suite-lock-smoke.sh
# A temp target dir through CARGO_TARGET_DIR; the locked commands are `sh -c` sleeps, never cargo.
# Cases: (a) the command's exit status, no lock left; (b) two concurrent runs do not overlap;
# (c) a stale lock (dead pid, or a live pid with another start time) is taken over with a notice;
# (d) a live holder plus GLEN_SUITE_LOCK_WAIT=1 exits 75 naming the holder; (e) SIGTERM to a
# running suite-lock.sh releases the lock; (f) SIGTERM stops the command's descendants before the
# lock goes. Prints ok/FAIL per case and exits 1 on any FAIL.
set -u

HERE="$(cd "$(dirname "$0")" && pwd -P)"
LOCKSH="$HERE/../suite-lock.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/suite-lock-smoke.XXXXXX")" || exit 1
HOLDER=""
trap '[ -n "$HOLDER" ] && kill "$HOLDER" 2>/dev/null; rm -rf "$TMP"' EXIT
export CARGO_TARGET_DIR="$TMP/target"
LOCK="$CARGO_TARGET_DIR/.glen-suite.lock"
FAILS=0

ok() { echo "ok   $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }

# wait_owner: wait up to 10 s for a holder to write the owner file.
wait_owner() {
  local i=0
  while [ ! -s "$LOCK/owner" ] && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
  [ -s "$LOCK/owner" ]
}

# (a) the exit status is the command's, and the lock is released.
/bin/bash "$LOCKSH" sh -c 'exit 3' 2>"$TMP/a.err"
rc=$?
if [ "$rc" = 3 ] && [ ! -e "$LOCK" ]; then ok "(a) exit status passed through, lock released"
else fail "(a) exit=$rc (want 3), lock left: $([ -e "$LOCK" ] && echo yes || echo no)"; fi

# (b) two concurrent runs: the second starts after the first ends.
# shellcheck disable=SC2016  # expanded by the inner sh, not here
STAMP='date +%s >"$1.start"; sleep 2; date +%s >"$1.end"'
/bin/bash "$LOCKSH" sh -c "$STAMP" _ "$TMP/r1" 2>"$TMP/b1.err" &
p1=$!
sleep 0.5
/bin/bash "$LOCKSH" sh -c "$STAMP" _ "$TMP/r2" 2>"$TMP/b2.err" &
p2=$!
wait "$p1"; rc1=$?
wait "$p2"; rc2=$?
s1="$(cat "$TMP/r1.start" 2>/dev/null)"; e1="$(cat "$TMP/r1.end" 2>/dev/null)"
s2="$(cat "$TMP/r2.start" 2>/dev/null)"; e2="$(cat "$TMP/r2.end" 2>/dev/null)"
if [ "$rc1" = 0 ] && [ "$rc2" = 0 ] && [ -n "$s1" ] && [ -n "$e1" ] && [ -n "$s2" ] && [ -n "$e2" ] &&
  { [ "$s2" -ge "$e1" ] || [ "$s1" -ge "$e2" ]; } && grep -q 'waiting for' "$TMP/b2.err" "$TMP/b1.err"; then
  ok "(b) concurrent runs serialised (r1 $s1-$e1, r2 $s2-$e2)"
else
  fail "(b) rc=$rc1/$rc2 r1=$s1-$e1 r2=$s2-$e2 stderr=[$(cat "$TMP/b1.err" "$TMP/b2.err")]"
fi

# (c) a stale lock: its owner names a dead pid.
sh -c 'exit 0' &
dead=$!
wait "$dead"
mkdir -p "$LOCK"
printf 'pid=%s\nlstart=Thu Jan  1 00:00:00 1970\ncmd=cargo test\nsince=2026-11-01T08:00:00Z\n' "$dead" >"$LOCK/owner"
/bin/bash "$LOCKSH" sh -c 'exit 0' 2>"$TMP/c.err"
rc=$?
if [ "$rc" = 0 ] && grep -q "taking over stale lock (pid $dead" "$TMP/c.err" && [ ! -e "$LOCK" ]; then
  ok "(c) dead-pid lock taken over with the notice"
else fail "(c) exit=$rc stderr=[$(cat "$TMP/c.err")]"; fi

# (c2) a stale lock: a live pid (this shell) whose start time no longer matches.
mkdir -p "$LOCK"
printf 'pid=%s\nlstart=Thu Jan  1 00:00:00 1970\ncmd=cargo test\nsince=2026-11-01T08:00:00Z\n' "$$" >"$LOCK/owner"
/bin/bash "$LOCKSH" sh -c 'exit 0' 2>"$TMP/c2.err"
rc=$?
if [ "$rc" = 0 ] && grep -q "taking over stale lock (pid $$" "$TMP/c2.err" && [ ! -e "$LOCK" ]; then
  ok "(c2) reused-pid lock taken over with the notice"
else fail "(c2) exit=$rc stderr=[$(cat "$TMP/c2.err")]"; fi

# (d) a live holder plus GLEN_SUITE_LOCK_WAIT=1: exit 75 naming the holder.
/bin/bash "$LOCKSH" sh -c 'sleep 30' 2>"$TMP/holder.err" &
HOLDER=$!
if wait_owner; then
  env GLEN_SUITE_LOCK_WAIT=1 /bin/bash "$LOCKSH" sh -c 'exit 0' 2>"$TMP/d.err"
  rc=$?
  if [ "$rc" = 75 ] && grep -q "gave up after 1s.*held by pid $HOLDER " "$TMP/d.err" && [ -e "$LOCK/owner" ]; then
    ok "(d) wait cap exits 75 naming the holder"
  else fail "(d) exit=$rc stderr=[$(cat "$TMP/d.err")]"; fi
else
  fail "(d) the holder never wrote $LOCK/owner: [$(cat "$TMP/holder.err")]"
fi

# (e) SIGTERM to the running holder releases the lock.
kill -TERM "$HOLDER" 2>/dev/null
wait "$HOLDER"
rc=$?
HOLDER=""
if [ "$rc" = 143 ] && [ ! -e "$LOCK" ] && [ ! -e "$LOCK.takeover" ]; then
  ok "(e) SIGTERM released the lock"
else fail "(e) exit=$rc (want 143), lock left: $(find "$LOCK" "$LOCK.takeover" 2>/dev/null | tr '\n' ' ')"; fi

# (f) SIGTERM with a grandchild running (as cargo runs a test binary): the grandchild is gone
# once suite-lock.sh exits, and the lock with it.
# shellcheck disable=SC2016  # expanded by the inner sh, not here
/bin/bash "$LOCKSH" sh -c 'sleep 30 & echo $! >"$1"; wait' _ "$TMP/f.pid" 2>"$TMP/f.err" &
HOLDER=$!
i=0
while [ ! -s "$TMP/f.pid" ] && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
gc="$(cat "$TMP/f.pid" 2>/dev/null)"
kill -TERM "$HOLDER" 2>/dev/null
wait "$HOLDER"
rc=$?
HOLDER=""
if [ -n "$gc" ] && ! kill -0 "$gc" 2>/dev/null && [ "$rc" = 143 ] && [ ! -e "$LOCK" ]; then
  ok "(f) SIGTERM stopped the grandchild before releasing the lock"
else
  fail "(f) exit=$rc grandchild=${gc:-none} $(kill -0 "$gc" 2>/dev/null && echo alive || echo gone), lock left: $([ -e "$LOCK" ] && echo yes || echo no)"
  [ -n "$gc" ] && kill "$gc" 2>/dev/null
fi

/bin/bash "$LOCKSH" 2>"$TMP/u.err"
rc=$?
if [ "$rc" = 2 ] && grep -q usage "$TMP/u.err"; then ok "no command: usage, exit 2"
else fail "no command: exit=$rc stderr=[$(cat "$TMP/u.err")]"; fi

/bin/bash "$LOCKSH" -- sh -c 'echo ran "$1"' _ dashdash >"$TMP/d.out" 2>"$TMP/d.err"
rc=$?
if [ "$rc" = 0 ] && grep -qx "ran dashdash" "$TMP/d.out" && [ ! -e "$LOCK" ]; then ok "leading -- is dropped"
else fail "leading --: exit=$rc out=[$(cat "$TMP/d.out")] stderr=[$(cat "$TMP/d.err")]"; fi

/bin/bash "$LOCKSH" -- 2>"$TMP/u2.err"
rc=$?
if [ "$rc" = 2 ] && grep -q usage "$TMP/u2.err"; then ok "only --: usage, exit 2"
else fail "only --: exit=$rc stderr=[$(cat "$TMP/u2.err")]"; fi

if [ "$FAILS" -gt 0 ]; then
  echo "suite-lock-smoke: $FAILS FAIL"
  exit 1
fi
echo "suite-lock-smoke: all passed"
