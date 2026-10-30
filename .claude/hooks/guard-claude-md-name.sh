#!/usr/bin/env bash
# PreToolUse(Write) — claude.md naming guard.
#
# Claude Code auto-loads any file named CLAUDE.md under the working tree as nested
# project instructions. macOS's filesystem is case-insensitive, so an output file
# named `claude.md` (e.g. `.claude/plans/reviews/<wave>/claude.md`) is loaded too:
# a review was once injected as project instructions that way. Per-engine output
# files are named `<engine>-review.md` / `<engine>-plan.md` instead.
#
# Refuses a Write whose basename is `claude.md` in any case, except the instruction
# files that exist on purpose:
#   - <root>/CLAUDE.md and <root>/.claude/CLAUDE.md, where <root> is a git checkout
#     (the repo root or a worktree under .claude/worktrees/<name>/)
#   - ~/.claude/CLAUDE.md (the user's global instructions)
#   - any file that already exists on disk (an existing nested CLAUDE.md such as
#     ~/Developer/Apps/CLAUDE.md); only NEW claude.md names are refused
# The path is normalised lexically (`..`, `.`) before the decision. Write tool only:
# shell redirects and `mv` are covered by the prompts, not by this hook.
#
# Posture: FAIL OPEN on anything it cannot parse (no jq, no path); DENY only on a
# claude.md basename outside those paths.
set -uo pipefail
command -v jq >/dev/null 2>&1 || exit 0
INPUT=$(cat 2>/dev/null) || exit 0
FILE=$(printf '%s' "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null) || exit 0
[ -z "$FILE" ] && exit 0

base=$(basename "$FILE" | tr '[:upper:]' '[:lower:]')
[ "$base" = "claude.md" ] || exit 0

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
case "$FILE" in
  /*) abs=$FILE ;;
  *) abs="${CWD:-$PWD}/$FILE" ;;
esac
# Normalise `..` and `.` lexically so `reviews/../CLAUDE.md` is judged by where it lands.
if command -v python3 >/dev/null 2>&1; then
  abs=$(python3 -I -c 'import os, sys; print(os.path.normpath(sys.argv[1]))' "$abs" 2>/dev/null) || exit 0
fi

# Existing files (an intentional nested CLAUDE.md) are never blocked; only new names are.
[ -e "$abs" ] && exit 0

dir=$(dirname "$abs")
if [ "$(basename "$dir")" = ".claude" ]; then
  root=$(dirname "$dir")
else
  root=$dir
fi
# A git checkout root has a .git directory (main checkout) or .git file (worktree).
[ -e "$root/.git" ] && exit 0
[ -n "${HOME:-}" ] && [ "$abs" = "$HOME/.claude/CLAUDE.md" ] && exit 0

reason="Refused: '$FILE' is named claude.md. Claude Code loads any claude.md (any case, on macOS's case-insensitive filesystem) as a nested CLAUDE.md, so this file would be injected as project instructions. Name per-engine output <engine>-review.md or <engine>-plan.md (e.g. claude-review.md). Only <repo>/CLAUDE.md, <repo>/.claude/CLAUDE.md, ~/.claude/CLAUDE.md and CLAUDE.md files that already exist may carry that name."
jq -n --arg r "$reason" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}'
exit 0
