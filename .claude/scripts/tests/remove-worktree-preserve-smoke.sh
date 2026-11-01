#!/bin/bash
# remove-worktree-preserve-smoke.sh — remove-worktree.sh keeps a worktree's working material.
#
# Usage: /bin/bash .claude/scripts/tests/remove-worktree-preserve-smoke.sh
# Bash 3.2, no network, no real companion: a temp repo carries copies of remove-worktree.sh and
# codex-procs.sh, and every run uses CLAUDE_PLUGIN_DATA and TMPDIR under the mktemp work dir with
# --force-no-state. Prints ok/FAIL per case; exit 1 on any FAIL.
set -euo pipefail

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
SCRIPTS="$TEST_DIR/.."
WORK="$(mktemp -d "${TMPDIR:-/tmp}/remove-worktree-preserve-smoke.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

REPO="$WORK/repo"
mkdir -p "$WORK/tmp" "$REPO/.claude/scripts"
G() { git -C "$REPO" -c user.name=smoke -c user.email=smoke@example.invalid \
  -c core.hooksPath=/dev/null -c commit.gpgsign=false "$@"; }
git init -q "$REPO"
printf '%s\n' .claude/worktrees/ .claude/evidence/ .claude/plans/ target/ >"$REPO/.gitignore"
cp "$SCRIPTS/remove-worktree.sh" "$SCRIPTS/codex-procs.sh" "$REPO/.claude/scripts/"
G add -A
G commit -q -m init
G worktree add -q "$REPO/.claude/worktrees/w1" -b t1 >/dev/null 2>&1
W1="$REPO/.claude/worktrees/w1"

# The main checkout has a different p.md and an identical same.md.
mkdir -p "$REPO/.claude/plans"
printf 'main plan\n' >"$REPO/.claude/plans/p.md"
printf 'same\n' >"$REPO/.claude/plans/same.md"
mkdir -p "$W1/.claude/evidence/w1" "$W1/.claude/plans" "$W1/.claude/reviews" "$W1/target"
printf 'shot\n' >"$W1/.claude/evidence/w1/shot.txt"
printf 'worktree plan\n' >"$W1/.claude/plans/p.md"
printf 'same\n' >"$W1/.claude/plans/same.md"
printf 'review\n' >"$W1/.claude/reviews/r.md"
printf 'build\n' >"$W1/target/x"
printf 'scratch\n' >"$W1/notes.txt"       # untracked outside the kept dirs: discarded and listed
printf 'changed\n' >>"$W1/.gitignore"      # a tracked change: discarded and listed

FAILS=0
ok() { echo "ok   $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
run() { # run ARGS...: sets OUT and RC
  RC=0
  OUT="$(cd "$REPO" && env CLAUDE_PLUGIN_DATA="$WORK/none" TMPDIR="$WORK/tmp" \
    /bin/bash "$REPO/.claude/scripts/remove-worktree.sh" --force-no-state "$@" 2>&1)" || RC=$?
}
has() { printf '%s\n' "$OUT" | grep -qF -- "$1"; }

# (a) --dry-run names the three new or different files and changes nothing.
run --dry-run "$W1"
n="$(printf '%s\n' "$OUT" | grep -c '^would preserve ' || true)"
if [ "$RC" = 0 ] && [ "$n" = 3 ] && has "would preserve .claude/evidence/w1/shot.txt -> $REPO/.claude/evidence/w1/shot.txt" \
  && has "would preserve .claude/reviews/r.md -> $REPO/.claude/reviews/r.md" \
  && has "would preserve .claude/plans/p.md -> $REPO/.claude/evidence/from-worktrees/w1-" \
  && ! has same.md && ! has target/x; then ok "(a) dry run lists shot.txt, r.md and p.md only"
else fail "(a) dry run: rc $RC, $n line(s): $OUT"; fi
if [ -d "$W1" ] && [ ! -e "$REPO/.claude/evidence" ] && [ ! -e "$REPO/.claude/reviews" ] \
  && [ "$(cat "$REPO/.claude/plans/p.md")" = "main plan" ]; then ok "(a) dry run changed nothing"
else fail "(a) dry run changed something"; fi

# (b) the real run preserves, never overwrites, skips duplicates and leaves target/ behind.
run "$W1"
if [ "$RC" = 0 ] && [ ! -e "$W1" ]; then ok "(b) worktree removed"; else fail "(b) rc $RC: $OUT"; fi
if [ "$(cat "$REPO/.claude/evidence/w1/shot.txt" 2>/dev/null)" = shot ] \
  && [ "$(cat "$REPO/.claude/reviews/r.md" 2>/dev/null)" = review ]; then ok "(b) shot.txt and r.md copied to the same path"
else fail "(b) shot.txt or r.md missing: $OUT"; fi
moved=""
for m in "$REPO"/.claude/evidence/from-worktrees/w1-*/.claude/plans/p.md; do [ -f "$m" ] && moved="$m"; done
if [ -n "$moved" ] && [ "$(cat "$moved")" = "worktree plan" ] \
  && [ "$(cat "$REPO/.claude/plans/p.md")" = "main plan" ]; then ok "(b) the different p.md went to from-worktrees/, main p.md unchanged"
else fail "(b) p.md: moved '${moved:-none}': $OUT"; fi
dup=""
for m in "$REPO"/.claude/evidence/from-worktrees/w1-*/.claude/plans/same.md; do [ -e "$m" ] && dup="$m"; done
if [ -z "$dup" ] && ! has same.md; then ok "(b) identical same.md not duplicated"
else fail "(b) same.md duplicated: $OUT"; fi
if [ ! -e "$REPO/target/x" ] && ! has target/x; then ok "(b) target/x not preserved"; else fail "(b) target/x preserved"; fi
if has "warning: discarding 2 uncommitted path(s) in $W1:" && has " M .gitignore" && has "?? notes.txt" \
  && ! has "?? .claude/reviews/r.md"; then ok "(b) discard warning lists .gitignore and notes.txt, not preserved files"
else fail "(b) discard warning: $OUT"; fi
if has "preserved .claude/evidence/w1/shot.txt -> "; then ok "(b) each copy printed"; else fail "(b) no preserved line: $OUT"; fi

# (c) a failed copy exits 1 and keeps the worktree.
G worktree add -q "$REPO/.claude/worktrees/w2" -b t2 >/dev/null 2>&1
W2="$REPO/.claude/worktrees/w2"
mkdir -p "$W2/.claude/evidence/w2"
printf 'shot2\n' >"$W2/.claude/evidence/w2/shot.txt"
mv "$REPO/.claude/evidence" "$WORK/evidence-b"
printf 'not a directory\n' >"$REPO/.claude/evidence"
run "$W2"
if [ "$RC" = 1 ] && [ -d "$W2" ] && has "could not preserve .claude/evidence/w2/shot.txt; nothing removed"; then
  ok "(c) failed copy: exit 1, worktree kept"
else fail "(c) rc $RC, worktree $([ -d "$W2" ] && echo kept || echo gone): $OUT"; fi

if [ "$FAILS" -gt 0 ]; then echo "$FAILS case(s) failed"; exit 1; fi
echo "all cases passed"
