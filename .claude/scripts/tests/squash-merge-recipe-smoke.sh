#!/bin/bash
# squash-merge-recipe-smoke.sh — the "Squash-merged base" recipe in
# .claude/skills/auto-mode/references/loop-procedures.md (step 7) against a temp repo.
#
# Usage: /bin/bash .claude/scripts/tests/squash-merge-recipe-smoke.sh
# Bash 3.2, no network. The temp repos ignore the global and system git config, so no hook,
# signing key or alias of the owner's runs. History built per repo:
#   main:  seq 1 30 > f
#   waveA: line 5 -> A5 (this commit is CUT)
#   waveB: from CUT, A5 -> B5 and line 25 -> B25
#   main:  squash-merge of waveA (SQ), then line 15 -> M15 (and line 25 -> M25 in the variant)
# Cases: (a) the plain base conflicts; (b) the cut base merges clean (tree T); (c) the graft recipe
# merges to T with the ordinary parents and leaves refs/replace empty; (d) a real conflict left
# by the cut base exits 1 with only f conflicted, and refs/replace is still empty.
# Prints ok/FAIL per case; exit 1 on any FAIL.
set -euo pipefail

WORK="$(mktemp -d "${TMPDIR:-/tmp}/squash-merge-smoke.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=smoke GIT_AUTHOR_EMAIL=smoke@example.invalid
export GIT_COMMITTER_NAME=smoke GIT_COMMITTER_EMAIL=smoke@example.invalid
unset GIT_NO_REPLACE_OBJECTS GIT_DIR GIT_WORK_TREE

FAILS=0
ok() { echo "ok   $1"; }
fail() { echo "FAIL $1"; FAILS=$((FAILS + 1)); }
# expect <ok text> <FAIL text> <test args…>
expect() { local y="$1" n="$2"; shift 2; if [ "$@" ]; then ok "$y"; else fail "$n"; fi; }

# set_line <file> <line number> <text>
set_line() { awk -v n="$2" -v t="$3" 'NR == n { print t; next } { print }' "$1" >"$1.tmp" && mv "$1.tmp" "$1"; }

# build <repo dir> <main edits line 25: 0|1>; sets CUT and SQ, leaves waveB checked out.
build() {
  local r="$1"
  git init -q -b main "$r"
  seq 1 30 >"$r/f"
  git -C "$r" add f && git -C "$r" commit -q -m base
  git -C "$r" switch -q -c waveA
  set_line "$r/f" 5 A5 && git -C "$r" commit -q -am A5
  CUT="$(git -C "$r" rev-parse HEAD)"
  git -C "$r" switch -q -c waveB
  set_line "$r/f" 5 B5 && set_line "$r/f" 25 B25 && git -C "$r" commit -q -am B
  git -C "$r" switch -q main
  git -C "$r" merge -q --squash waveA >/dev/null && git -C "$r" commit -q -m "W-A (#1)"
  SQ="$(git -C "$r" rev-parse HEAD)"
  set_line "$r/f" 15 M15
  if [ "$2" = 1 ]; then set_line "$r/f" 25 M25; fi
  git -C "$r" commit -q -am M
  git -C "$r" switch -q waveB
}

replace_refs() { git -C "$1" for-each-ref refs/replace | wc -l | tr -d ' '; }

# recipe <repo dir>: the loop-procedures.md recipe, with origin/main as main; sets RC.
recipe() {
  local wt="$1" cut
  cut="$(git -C "$wt" merge-base HEAD waveA)"   # waveA's last tip stands in for headRefOid
  git -C "$wt" replace --graft "$SQ" "$SQ^" "$cut"
  if git -C "$wt" merge -q --no-edit main >/dev/null 2>&1; then RC=0; else RC=$?; fi
  git -C "$wt" replace -d "$SQ" >/dev/null
}

# --- clean variant: cases (a)-(c) ---
R="$WORK/clean"
build "$R" 0
OLD_HEAD="$(git -C "$R" rev-parse HEAD)"
MAIN_TIP="$(git -C "$R" rev-parse main)"

if git -C "$R" merge-tree --write-tree HEAD main >/dev/null 2>&1; then
  fail "(a) plain merge-tree base exited 0; expected a conflict"
else
  rc=$?
  if [ "$rc" -eq 1 ]; then ok "(a) plain base conflicts (exit 1)"; else fail "(a) plain merge-tree exited $rc, expected 1"; fi
fi

T=""
if T="$(git -C "$R" merge-tree --write-tree --merge-base="$CUT" HEAD main)"; then
  ok "(b) cut base merges clean (tree $T)"
else
  fail "(b) merge-tree --merge-base=CUT exited non-zero"
fi

recipe "$R"
if [ "$RC" -ne 0 ]; then
  fail "(c) graft recipe merge exited $RC, expected 0"
else
  tree="$(git -C "$R" rev-parse 'HEAD^{tree}')"
  parents="$(git -C "$R" rev-list --parents -n 1 HEAD | cut -d' ' -f2-)"
  want="$OLD_HEAD $MAIN_TIP"
  expect "(c) merge tree equals the cut-base tree" "(c) merge tree $tree != $T" "$tree" = "$T"
  expect "(c) parents are the old HEAD and main" "(c) parents '$parents', expected '$want'" "$parents" = "$want"
  if grep -qx B5 "$R/f" && grep -qx M15 "$R/f" && grep -qx B25 "$R/f" && ! grep -qx A5 "$R/f"; then
    ok "(c) f has B5, M15 and B25"
  else
    fail "(c) f lacks B5/M15/B25: $(tr '\n' ' ' <"$R/f")"
  fi
fi
expect "(c) refs/replace is empty" "(c) refs/replace not empty" "$(replace_refs "$R")" = 0

# --- conflict variant: case (d) ---
R="$WORK/conflict"
build "$R" 1
recipe "$R"
if [ "$RC" -eq 1 ]; then ok "(d) recipe exits 1 on a real conflict"; else fail "(d) recipe exited $RC, expected 1"; fi
conflicted="$(git -C "$R" diff --name-only --diff-filter=U)"
expect "(d) only f is conflicted" "(d) conflicted files: '$conflicted'" "$conflicted" = f
if grep -q '^<<<<<<<' "$R/f" && ! grep -qx A5 "$R/f" && grep -qx B5 "$R/f"; then
  ok "(d) the conflict is line 25 only (line 5 resolved to B5)"
else
  fail "(d) unexpected conflict content: $(tr '\n' ' ' <"$R/f")"
fi
expect "(d) refs/replace is empty" "(d) refs/replace not empty" "$(replace_refs "$R")" = 0

if [ "$FAILS" -gt 0 ]; then echo "$FAILS case(s) failed"; exit 1; fi
