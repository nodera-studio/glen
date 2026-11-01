#!/bin/bash
# clean-scratch-smoke.sh — clean-scratch.sh refusals, skips and removals, and the steer-bash rm advice.
#
# Usage: /bin/bash .claude/scripts/tests/clean-scratch-smoke.sh
# Bash 3.2, no network. TMPDIR is a temp dir under a mktemp work dir, the worktree check reads a
# temp repo (GLEN_CLEAN_REPO), and every holder process is a background sleep loop this test
# starts and kills. The real TMPDIR is never cleaned. Prints ok/FAIL per case; exit 1 on any FAIL.
set -euo pipefail

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
SCRIPT="$TEST_DIR/../clean-scratch.sh"
HOOK="$(cd "$TEST_DIR/../.." && pwd -P)/hooks/steer-bash.sh"
command -v jq >/dev/null 2>&1 || { echo "FAIL jq is required" >&2; exit 1; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/clean-scratch-smoke.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
HOLDERS=""
cleanup() {
  local p
  for p in $HOLDERS; do kill "$p" 2>/dev/null || true; done
  for p in $HOLDERS; do wait "$p" 2>/dev/null || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT

T="$WORK/tmp"        # the TMPDIR the script cleans
OUTSIDE="$WORK/outside"
REPO="$WORK/repo"    # GLEN_CLEAN_REPO, with glen-wt registered as a worktree
mkdir -p "$T" "$OUTSIDE/keep"
G() { git -C "$REPO" -c user.name=smoke -c user.email=smoke@example.invalid \
  -c core.hooksPath=/dev/null -c commit.gpgsign=false "$@"; }
git init -q "$REPO"
G commit -q --allow-empty -m init
G worktree add -q "$T/glen-wt" -b wt >/dev/null 2>&1

for d in glen-a glen-links-x glen-fresh glen-busy glen-cmd glen-omp-engine; do
  mkdir -p "$T/$d/sub"
  printf 'x\n' >"$T/$d/sub/f"
done
mkdir -p "$T/other-dir"
ln -s "$OUTSIDE" "$T/glen-link"
for d in glen-a glen-links-x glen-busy glen-cmd glen-omp-engine; do
  find "$T/$d" -exec touch -t 202610300000 {} +
done
touch "$T/glen-fresh/sub/f"

# Holders: a cwd inside glen-busy, a command line naming glen-cmd, a fake omp-engine.sh run.
printf 'while :; do sleep 1; done\n' >"$WORK/holder.sh"
cp "$WORK/holder.sh" "$WORK/omp-engine.sh"
(cd "$T/glen-busy" && exec sleep 60) &
HOLDERS="$HOLDERS $!"
/bin/bash "$WORK/holder.sh" "$T/glen-cmd" &
HOLDERS="$HOLDERS $!"
/bin/bash "$WORK/omp-engine.sh" &
HOLDERS="$HOLDERS $!"
# Wait (up to 20 s, for a loaded machine) until all three holders are visible to ps and lsof.
ready=0 tries=0
while [ "$tries" -lt 40 ]; do
  tries=$((tries + 1))
  # Snapshots plus here-strings: `big | grep -q` under pipefail fails once ps outgrows a pipe.
  cmds="$(ps -axww -o command=)"
  cw="$(lsof -a -d cwd -Fn 2>/dev/null || true)"
  if grep -qF "$T/glen-cmd" <<<"$cmds" && grep -qF "$WORK/omp-engine.sh" <<<"$cmds" \
    && grep -qxF "n$T/glen-busy" <<<"$cw"; then ready=1; break; fi
  sleep 0.5
done
[ "$ready" = 1 ] || { echo "FAIL setup: holder processes did not start" >&2; exit 1; }

FAILS=0
ok() { echo "ok   $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
run() { # run ARGS...: sets OUT and RC
  RC=0
  OUT="$(env TMPDIR="$T" GLEN_CLEAN_REPO="$REPO" /bin/bash "$SCRIPT" "$@" 2>&1)" || RC=$?
}
has() { printf '%s\n' "$OUT" | grep -qF -- "$1"; }

# (a) bad patterns exit 2 before anything is touched.
for bad in foo 'glen-../x'; do
  run --min-age 0 glen-a "$bad"
  if [ "$RC" = 2 ] && [ -d "$T/glen-a" ]; then ok "(a) '$bad' exits 2, glen-a untouched"
  else fail "(a) '$bad': rc $RC, glen-a $([ -d "$T/glen-a" ] && echo kept || echo gone): $OUT"; fi
done

# (b) --dry-run removes nothing and lists what would go.
run --dry-run 'glen-*'
if has "would remove $T/glen-a " && has "would remove $T/glen-links-x " \
  && [ -d "$T/glen-a" ] && [ -d "$T/glen-links-x" ]; then ok "(b) dry run lists glen-a and glen-links-x, removes nothing"
else fail "(b) dry run: rc $RC: $OUT"; fi

# (c) the real run removes the old idle entries and refuses or skips the rest.
run 'glen-*'
if [ "$RC" = 1 ]; then ok "(c) exit 1 (something refused)"; else fail "(c) exit $RC, want 1: $OUT"; fi
for d in glen-a glen-links-x; do
  if [ ! -e "$T/$d" ] && has "removed $T/$d "; then ok "(c) removed $d"; else fail "(c) $d not removed: $OUT"; fi
done
for d in glen-link glen-busy glen-cmd glen-omp-engine glen-wt; do
  if [ -e "$T/$d" ] && has "refuse: $T/$d: "; then ok "(c) refused $d ($(printf '%s\n' "$OUT" | sed -n "s#^refuse: $T/$d: ##p"))"
  else fail "(c) $d not refused: $OUT"; fi
done
if [ -e "$T/glen-fresh" ] && has "skip: $T/glen-fresh: "; then ok "(c) skipped glen-fresh"; else fail "(c) glen-fresh not skipped: $OUT"; fi
if [ -d "$T/other-dir" ] && [ -d "$OUTSIDE/keep" ]; then ok "(c) other-dir and the symlink's target remain"
else fail "(c) other-dir or $OUTSIDE/keep is gone"; fi

# (d) with the holders gone and --min-age 0, the rest goes.
for p in $HOLDERS; do kill "$p" 2>/dev/null || true; done
for p in $HOLDERS; do wait "$p" 2>/dev/null || true; done
HOLDERS=""
# A real omp-engine.sh run elsewhere on the machine keeps glen-omp-engine refused: leave it out then.
rest="glen-busy glen-cmd glen-omp-engine glen-fresh"
if grep -qF omp-engine.sh <<<"$(ps -axww -o command=)"; then
  rest="glen-busy glen-cmd glen-fresh"
  echo "note (d) an omp-engine.sh run is active on this machine; glen-omp-engine left out"
fi
# shellcheck disable=SC2086 # rest is a list of fixed names
run --min-age 0 $rest
gone=1
for d in $rest; do [ ! -e "$T/$d" ] || gone=0; done
if [ "$RC" = 0 ] && [ "$gone" = 1 ]; then ok "(d) holders stopped: $rest removed, exit 0"
else fail "(d) rc $RC: $OUT"; fi

# (f) no usable cwd snapshot (lsof fails, or lists nothing): every entry is refused and kept.
mkdir -p "$T/glen-nolsof/sub" "$WORK/lsof-fail" "$WORK/lsof-empty"
find "$T/glen-nolsof" -exec touch -t 202610300000 {} +
printf '#!/bin/sh\nexit 1\n' >"$WORK/lsof-fail/lsof"
printf '#!/bin/sh\nexit 0\n' >"$WORK/lsof-empty/lsof"
chmod +x "$WORK/lsof-fail/lsof" "$WORK/lsof-empty/lsof"
for f in fail empty; do
  RC=0
  OUT="$(env PATH="$WORK/lsof-$f:$PATH" TMPDIR="$T" GLEN_CLEAN_REPO="$REPO" /bin/bash "$SCRIPT" --min-age 0 glen-nolsof 2>&1)" || RC=$?
  if [ "$RC" = 1 ] && [ -d "$T/glen-nolsof" ] && has "refuse: $T/glen-nolsof: cannot check process cwds"; then
    ok "(f) lsof $f: glen-nolsof refused and kept"
  else fail "(f) lsof $f: rc $RC: $OUT"; fi
done

# (g) a TMPDIR outside the system temp roots exits 2 before anything is touched.
RC=0
OUT="$(env TMPDIR=/usr/bin GLEN_CLEAN_REPO="$REPO" /bin/bash "$SCRIPT" --dry-run 'glen-*' 2>&1)" || RC=$?
if [ "$RC" = 2 ] && has "not under a system temp root"; then ok "(g) TMPDIR=/usr/bin exits 2"
else fail "(g) TMPDIR=/usr/bin: rc $RC: $OUT"; fi

# (e) the hook points rm -rf of glen scratch at the helper and leaves other rm alone.
hook() { jq -n --arg c "$1" '{tool_name:"Bash",tool_input:{command:$c}}' | /bin/bash "$HOOK"; }
# shellcheck disable=SC2016 # literal hook input
H="$(hook 'rm -rf "$TMPDIR/glen-flake-target"')"
if printf '%s' "$H" | jq -e '.hookSpecificOutput.permissionDecision == "deny"' >/dev/null 2>&1 \
  && printf '%s' "$H" | grep -qF clean-scratch.sh; then ok "(e) rm -rf \$TMPDIR/glen-flake-target -> deny naming clean-scratch.sh"
else fail "(e) hook output for rm -rf \$TMPDIR/glen-flake-target: '$H'"; fi
H="$(hook 'rm -rf target/x')"
if [ -z "$H" ]; then ok "(e) rm -rf target/x -> no output"; else fail "(e) rm -rf target/x -> '$H'"; fi

if [ "$FAILS" -gt 0 ]; then echo "$FAILS case(s) failed"; exit 1; fi
echo "all cases passed"
