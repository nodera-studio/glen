#!/bin/bash
# quick-classify-smoke.sh — smoke test for the Linux-only leg of .claude/scripts/quick-classify.sh
# (Bash 3.2, no network). Each case runs the classifier in a scratch git repo.
#
# Usage: /bin/bash .claude/scripts/tests/quick-classify-smoke.sh
# Cases: clean tree passes; a small non-Linux edit passes; a tracked cfg(target_os = "linux")
# line, a tracked `target_os="linux"` line, an untracked file with bwrap, a removed renameat2
# line, a tracked Linux line next to a benign untracked file, and an untracked
# omp-sandboxed-linux.sh with no Linux words each escalate with exit 3. Prints ok/FAIL per case
# and exits 1 on any FAIL.
set -u

HERE="$(cd "$(dirname "$0")" && pwd -P)"
CLASSIFY="$HERE/../quick-classify.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/quick-classify-smoke.XXXXXX")" || exit 1
cleanup() { case "$TMP" in */quick-classify-smoke.*) find "$TMP" -depth -delete ;; esac; }
trap cleanup EXIT
FAILS=0

g() { git -c user.name=smoke -c user.email=smoke@example.invalid -c commit.gpgsign=false \
  -c core.hooksPath=/dev/null "$@"; }

# setup <case>: a scratch repo with one committed file; prints its path.
setup() {
  local d="$TMP/$1"
  mkdir -p "$d/src"
  # The renameat2 line sits more than 3 lines above the end, so an append there is outside the
  # diff's context lines (the classifier also matches context lines, by design).
  printf '// renameat2 swap\npub fn a() {}\n#[cfg(target_os = "macos")]\npub fn b() {}\n\n\n' >"$d/src/lib.rs"
  g -C "$d" init -q
  g -C "$d" add -A
  g -C "$d" commit -q -m init
  printf '%s\n' "$d"
}

# check <name> <dir> <want-exit> <want-stderr-substring>
check() {
  local name="$1" dir="$2" want_rc="$3" want_err="$4" rc err
  (cd "$dir" && /bin/bash "$CLASSIFY" "fix a typo in the docs") 2>"$TMP/err"
  rc=$?
  err="$(cat "$TMP/err")"
  case "$err" in *"$want_err"*) ;; *) rc="stderr-mismatch:$rc" ;; esac
  if [ "$rc" = "$want_rc" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: exit=$rc (want $want_rc) stderr=[$err] (want *$want_err*)"
    FAILS=$((FAILS + 1))
  fi
}

LINUX_MSG="escalate: Linux-only code: review needs a CI run"

d="$(setup clean)"
check "clean tree passes" "$d" 0 "quick path ok"

d="$(setup plain)"
printf 'pub fn c() {}\n' >>"$d/src/lib.rs"
check "small non-Linux edit passes" "$d" 0 "quick path ok"

d="$(setup cfg-linux)"
printf '#[cfg(target_os = "linux")]\npub fn c() {}\n' >>"$d/src/lib.rs"
check "tracked cfg(target_os = \"linux\") escalates" "$d" 3 "$LINUX_MSG"

d="$(setup cfg-linux-tight)"
printf '#[cfg(all(unix, target_os="linux"))]\npub fn c() {}\n' >>"$d/src/lib.rs"
check "tracked target_os=\"linux\" escalates" "$d" 3 "$LINUX_MSG"

d="$(setup untracked-bwrap)"
printf 'bwrap --ro-bind / / -- true\n' >"$d/src/sandbox.sh"
check "untracked file with bwrap escalates" "$d" 3 "$LINUX_MSG"

d="$(setup removed-renameat2)"
printf 'pub fn a() {}\n#[cfg(target_os = "macos")]\npub fn b() {}\n\n\n' >"$d/src/lib.rs"
check "removed renameat2 line escalates" "$d" 3 "$LINUX_MSG"

# A benign untracked file must not mask a tracked Linux line (pipefail fail-open, batch 10).
d="$(setup linux-plus-untracked)"
printf '#[cfg(target_os = "linux")]\npub fn c() {}\n' >>"$d/src/lib.rs"
printf 'hello\n' >"$d/src/notes.txt"
check "tracked Linux line plus benign untracked file escalates" "$d" 3 "$LINUX_MSG"

d="$(setup linux-path)"
mkdir -p "$d/.claude/omp"
printf 'echo hi\n' >"$d/.claude/omp/omp-sandboxed-linux.sh"
check "Linux-only path without Linux words escalates" "$d" 3 "$LINUX_MSG"

if [ "$FAILS" -gt 0 ]; then
  echo "quick-classify-smoke: $FAILS FAIL"
  exit 1
fi
echo "quick-classify-smoke: all passed"
