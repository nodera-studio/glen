#!/usr/bin/env bash
# Fail-safe classifier for /implement-quick in glen.
#
# Usage: bash .claude/scripts/quick-classify.sh "<task description>"
# Exit 0 = the quick path is allowed. Any non-zero exit = route to /implement.
# Every uncertain case escalates: a missing task, a git error, a risk keyword, a
# protected path in the working-tree diff, or a diff over the size threshold.
set -uo pipefail

task="${1:-}"
if [ -z "$task" ]; then
  echo "escalate: no task description given" >&2; exit 2
fi

# 1. Risk keywords in the task. These are the surfaces where a small-looking change can
#    corrupt an omp session, leak a secret, or change glen's public contract.
risk='(write|append|truncate|delete).{0,20}(jsonl|transcript|session file)|omp (rpc|bridge|extension)|\brpc\b|bridge|extension|fork|secret|token|credential|unsafe|release|version bump|tag v|--json (shape|schema|format)|breaking|migration|dependency|crate (add|bump|upgrade)|cargo (add|update)'
if printf '%s' "$task" | grep -qiE "$risk"; then
  echo "escalate: task mentions a risk surface ($(printf '%s' "$task" | grep -oiE "$risk" | head -1))" >&2
  exit 3
fi

# 2. Protected paths and size of the current working-tree diff (staged + unstaged +
#    untracked). Before any edit the diff is usually empty and this leg passes; it is
#    re-run before committing, when the diff exists.
root="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "escalate: not in a git repo" >&2; exit 4; }
cd "$root" || exit 4
base="$(git merge-base HEAD origin/main 2>/dev/null || git rev-parse HEAD 2>/dev/null)" || { echo "escalate: cannot resolve a base commit" >&2; exit 4; }

changed="$( { git diff --name-only "$base"; git ls-files --others --exclude-standard; } 2>/dev/null | sort -u )" || { echo "escalate: git diff failed" >&2; exit 4; }

# Linux-only code is verified only by a CI run, so it never takes the quick path. Three
# checks, each on its own so one leg's "no match" can never mask another leg's match: the
# changed paths, the tracked diff, and each untracked file. A grep error escalates.
linux_msg="escalate: Linux-only code: review needs a CI run"
linux_only='target_os *= *"linux"|bwrap|unshare|netns|renameat2'
linux_paths='(^|/)omp-sandboxed-linux\.sh$|^src/sandbox\.rs$|^tests/linux_sandbox\.rs$'
if [ -n "$(printf '%s\n' "$changed" | grep -E "$linux_paths" || true)" ]; then
  echo "$linux_msg" >&2; exit 3
fi
diff_text="$(git diff "$base" 2>/dev/null)" || { echo "escalate: git diff failed" >&2; exit 4; }
if [[ "$diff_text" =~ $linux_only ]]; then
  echo "$linux_msg" >&2; exit 3
fi
while IFS= read -r -d '' f; do
  [ -d "$f" ] && continue
  grep -sIqE "$linux_only" -- "$f"; rc=$?
  if [ "$rc" -eq 0 ]; then echo "$linux_msg" >&2; exit 3; fi
  [ "$rc" -eq 1 ] || { echo "escalate: cannot scan untracked file ($f)" >&2; exit 4; }
done < <(git ls-files -z --others --exclude-standard 2>/dev/null)

protected='^(Cargo\.toml|Cargo\.lock|\.github/|lefthook\.yml|AGENTS\.md|CLAUDE\.md|\.claude/(settings\.json|hooks/|CLAUDE\.md))'
hit="$(printf '%s\n' "$changed" | grep -E "$protected" | head -1 || true)"
if [ -n "$hit" ]; then
  echo "escalate: protected path in diff ($hit)" >&2; exit 5
fi

files="$(printf '%s\n' "$changed" | grep -c . || true)"
lines="$(git diff --numstat "$base" 2>/dev/null | awk '{a+=$1; d+=$2} END {print a+d+0}')"
if [ "${files:-0}" -gt 6 ] || [ "${lines:-0}" -gt 200 ]; then
  echo "escalate: diff too large for the quick path (${files} files, ${lines} lines; limit 6 files / 200 lines)" >&2
  exit 6
fi

echo "quick path ok (${files:-0} files, ${lines:-0} lines changed)" >&2
exit 0
