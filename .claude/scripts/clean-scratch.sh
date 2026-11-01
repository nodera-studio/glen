#!/usr/bin/env bash
# clean-scratch.sh — delete glen scratch directly under $TMPDIR, refusing anything still in use.
#
# Usage: bash .claude/scripts/clean-scratch.sh [--dry-run] [--min-age <minutes>] <pattern>...
#
# The only sanctioned way for an agent to delete $TMPDIR scratch: `rm -rf` stays denied
# (settings.json, and hooks/steer-bash.sh points here). The allow rule is
# `Bash(bash .claude/scripts/clean-scratch.sh:*)`, so run it with that relative path from the repo
# or a worktree root. Each pattern is a basename glob that starts with glen- and has no / or ..
# (for example 'glen-*', 'glen-links-*', glen-flake-target), expanded only directly under the
# canonical ${TMPDIR:-/tmp} (T below). In this order:
#   1. Check the arguments: a pattern outside ^glen-[A-Za-z0-9._*?-]*$, or containing .., exits 2
#      before anything is touched.
#   2. Refuse to run (exit 2) when T is /, the canonical $HOME or an ancestor of it, or inside the
#      main checkout.
#   3. Take one snapshot: the main checkout's registered worktrees (`git worktree list
#      --porcelain`; GLEN_CLEAN_REPO, read only, replaces the main checkout for the smoke test),
#      every process cwd (`lsof -a -d cwd -Fn`) and every process command line (`ps -axww`).
#   4. For each entry, print `refuse: <path>: <reason>` and go on with the next when it is a
#      symlink; not directly under T after canon; a registered worktree, or holds one; a process
#      cwd, or holds one; named in a process command line (as a whole path, in T's canonical
#      spelling, without /private, or in $TMPDIR's own); glen-omp-engine or
#      glen-omp-engine-launcher while any process command line contains omp-engine.sh. Without
#      lsof every entry is refused ("cannot check process cwds").
#   5. Skip (`skip: <path>: ...`) an entry holding a file changed within --min-age minutes
#      (default 30; --min-age 0 turns the check off). A skip is not a refusal.
#   6. Otherwise remove it (`rm -rf`) and print `removed <path> (<size>)`, or `would remove ...`
#      under --dry-run, which changes nothing. End with a total.
# Exit: 0 = nothing refused, 1 = an entry was refused or could not be removed, 2 = bad arguments
# or an unsafe TMPDIR.
set -euo pipefail

die() { echo "clean-scratch: $*" >&2; exit 2; }
USAGE="usage: clean-scratch.sh [--dry-run] [--min-age <minutes>] <pattern>..."

# 1. Arguments.
DRY=0 MIN_AGE=30
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1 ;;
    --min-age) [ $# -ge 2 ] || die "$USAGE"; MIN_AGE="$2"; shift ;;
    --) shift; break ;;
    -*) die "$USAGE" ;;
    *) break ;;
  esac
  shift
done
case "$MIN_AGE" in '' | *[!0-9]*) die "--min-age takes whole minutes, not '$MIN_AGE'" ;; esac
[ $# -ge 1 ] || die "$USAGE"
RE='^glen-[A-Za-z0-9._*?-]*$'
for pat in "$@"; do
  case "$pat" in *..*) die "refuse: pattern contains ..: $pat" ;; esac
  [[ $pat =~ $RE ]] || die "refuse: a pattern is a glen-* basename glob without /: $pat"
done

# 2. T, $HOME and the main checkout.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
# shellcheck source-path=SCRIPTDIR source=codex-procs.sh
. "$SCRIPT_DIR/codex-procs.sh"
GCD="$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
  || die "refuse: $SCRIPT_DIR is not in a git checkout"
MAIN="$(cd "$GCD/.." && pwd -P)"
REPO="${GLEN_CLEAN_REPO:-$MAIN}"
RAW="${TMPDIR:-/tmp}"
T="$(cd "$RAW" && pwd -P)" || die "refuse: cannot resolve TMPDIR $RAW"
H="$(cd "$HOME" && pwd -P)" || die "refuse: cannot resolve HOME $HOME"
[ "$T" != / ] || die "refuse: TMPDIR resolves to /"
if under "$H" "$T"; then die "refuse: TMPDIR $T is \$HOME or an ancestor of it"; fi
if under "$T" "$MAIN"; then die "refuse: TMPDIR $T is inside the main checkout $MAIN"; fi

# 3. One snapshot.
WTS=""
while IFS= read -r wt; do
  WTS="$WTS$wt"$'\n'"$(canon "$wt" || true)"$'\n'
done < <(git -C "$REPO" worktree list --porcelain | sed -n 's/^worktree //p')
[ -n "$WTS" ] || die "refuse: cannot list the worktrees of $REPO"
HAVE_LSOF=0 CWDS=""
if command -v lsof >/dev/null 2>&1; then
  HAVE_LSOF=1
  CWDS="$(lsof -a -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' || true)"
fi
CMDS="$(ps -axww -o command=)" || die "refuse: cannot list processes"

# spellings NAME: the paths a process may use for $T/NAME (canonical, without /private, $TMPDIR's).
spellings() {
  printf '%s\n' "$T/$1"
  case "$T" in /private/*) printf '%s\n' "${T#/private}/$1" ;; esac
  printf '%s\n%s\n' "${RAW%/}/$1" "${RAW%/}//$1"
}
# at_or_under LIST NAME: a line of LIST is a spelling of NAME or below one.
at_or_under() {
  SP="$(spellings "$2")" L="$1" awk 'BEGIN {
    n = split(ENVIRON["SP"], s, "\n"); m = split(ENVIRON["L"], l, "\n")
    for (i = 1; i <= m; i++) for (j = 1; j <= n; j++)
      if (s[j] != "" && (l[i] == s[j] || index(l[i], s[j] "/") == 1)) exit 0
    exit 1 }'
}
# named NAME: a process command line contains a spelling of NAME as a whole path component.
named() {
  SP="$(spellings "$1")" awk 'BEGIN { n = split(ENVIRON["SP"], s, "\n") }
    { for (j = 1; j <= n; j++) { if (s[j] == "") continue; r = $0
        while ((k = index(r, s[j])) > 0) {
          c = substr(r, k + length(s[j]), 1)
          if (c == "" || c !~ /[A-Za-z0-9._-]/) { found = 1; exit }
          r = substr(r, k + 1) } } }
    END { exit !found }' <<<"$CMDS"
}
# reason PATH NAME: why PATH must stay (empty when it may go).
reason() {
  local c
  if [ -L "$1" ]; then echo "a symlink"; return; fi
  c="$(canon "$1" || true)"
  if [ -z "$c" ] || [ "${c%/*}" != "$T" ]; then echo "not directly under $T"; return; fi
  if at_or_under "$WTS" "$2"; then echo "a git worktree of $REPO is at or under it"; return; fi
  if [ "$HAVE_LSOF" = 0 ]; then echo "cannot check process cwds (lsof is missing)"; return; fi
  if at_or_under "$CWDS" "$2"; then echo "a process cwd is at or under it"; return; fi
  if named "$2"; then echo "a process command line names it"; return; fi
  case "$2" in
    glen-omp-engine | glen-omp-engine-launcher)
      if grep -qF omp-engine.sh <<<"$CMDS"; then echo "an omp-engine.sh run is active"; return; fi ;;
  esac
}

# 4-6. Each entry.
shopt -s nullglob
REFUSED=0 FAILED=0 SKIPPED=0 COUNT=0 TOTAL=0
VERB=removed
[ "$DRY" = 0 ] || VERB="would remove"
for pat in "$@"; do
  matched=0
  # shellcheck disable=SC2231 # the checked pattern is expanded as a glob on purpose
  for p in "$T"/$pat; do
    matched=1
    name="${p##*/}"
    why="$(reason "$p" "$name")"
    if [ -n "$why" ]; then
      echo "refuse: $p: $why"
      REFUSED=$((REFUSED + 1))
      continue
    fi
    if [ "$MIN_AGE" -gt 0 ] \
      && [ -n "$(find "$p" -mmin "-$MIN_AGE" -print -quit 2>/dev/null || echo unreadable)" ]; then
      echo "skip: $p: changed within $MIN_AGE min (or not fully readable)"
      SKIPPED=$((SKIPPED + 1))
      continue
    fi
    kb="$(du -sk "$p" 2>/dev/null | awk '{ print $1 }' || true)"
    kb="${kb:-0}"
    if [ "$DRY" = 0 ] && ! rm -rf -- "$p"; then
      echo "failed: $p: rm -rf failed"
      FAILED=$((FAILED + 1))
      continue
    fi
    echo "$VERB $p (${kb} KB)"
    COUNT=$((COUNT + 1)) TOTAL=$((TOTAL + kb))
  done
  [ "$matched" = 1 ] || echo "no match: $pat"
done

PRE=""
[ "$DRY" = 0 ] || PRE="dry run: "
echo "${PRE}total: $VERB $COUNT entries (${TOTAL} KB); refused $REFUSED, skipped $SKIPPED, failed $FAILED"
if [ "$REFUSED" -gt 0 ] || [ "$FAILED" -gt 0 ]; then exit 1; fi
