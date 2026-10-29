#!/usr/bin/env bash
# PreToolUse(Bash) — capture-before-merge gate.
#
# Blocks `gh pr merge` until the agent has stored a memory MCP entry referencing
# this PR or branch IN THIS SESSION, so durable decisions/learnings are captured
# BEFORE a green PR merges (and its branch is later deleted). The code itself is
# never lost — it's in trunk history after the merge; this gate is about
# KNOWLEDGE. See your project's branching-model doc + the auto-mode wave-close step.
#
# Posture: FAIL OPEN. A capture gate must never wedge a legit merge:
#   - no jq / no command / not `gh pr merge`            -> allow (exit 0)
#   - no transcript path or unreadable                  -> allow
#   - transcript carries a memory_store we can't parse  -> allow (format drift)
# DENY only when the transcript is readable AND carries no memory_store at all,
# or a parseable memory_store set with none referencing this PR/branch. The
# convention + Stop-hook capture + nightly distiller are the backstop.
set -uo pipefail
command -v jq >/dev/null 2>&1 || exit 0
INPUT=$(cat 2>/dev/null) || exit 0
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$CMD" ] && exit 0

# Only gate an actual `gh pr merge` (not `gh pr merge --help`, not other gh verbs).
# Require both a `gh` command token AND a `pr merge` token — tolerant of global flags
# between them (e.g. `gh --repo X pr merge`).
printf '%s' "$CMD" | grep -qE '(^|[^[:alnum:]_-])gh([[:space:]]|$)' || exit 0
printf '%s' "$CMD" | grep -qE '(^|[[:space:]])pr[[:space:]]+merge([[:space:]]|$)' || exit 0
printf '%s' "$CMD" | grep -qE -- '(^|[[:space:]])(-h|--help)([[:space:]]|$)' && exit 0

deny() {
  jq -n --arg r "$1" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}' 2>/dev/null
  exit 0
}

TRANSCRIPT=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null)
[ -n "$TRANSCRIPT" ] && [ -f "$TRANSCRIPT" ] || exit 0   # can't verify -> allow

PR=$(printf '%s' "$CMD" | grep -oE 'merge[[:space:]]+#?[0-9]+' | grep -oE '[0-9]+' | head -1 || true)
[ -z "$PR" ] && PR=$(printf '%s' "$CMD" | grep -oE '/pull/[0-9]+' | grep -oE '[0-9]+' | head -1 || true)
BRANCH=$(git branch --show-current 2>/dev/null || true)

# Literal needles (grep -F, NOT regex) so a branch name with metacharacters (. + ( ...)
# can neither break the match nor satisfy it by accident. When the PR number is known,
# require a PR-SPECIFIC token (`PR #123` / `#123` / `/pull/123`) rather than a bare `123`
# (which could match an unrelated number in memory) — branch is the fallback only when no
# PR number was parsed (e.g. `gh pr merge` on the current branch). Nothing to match -> allow.
needles=()
if [ -n "$PR" ]; then
  needles+=(-e "PR #$PR" -e "#$PR" -e "/pull/$PR")
elif [ -n "$BRANCH" ]; then
  needles+=(-e "$BRANCH")
fi
[ ${#needles[@]} -eq 0 ] && exit 0

# Structured scan: memory_store tool_use inputs in the recent transcript tail.
parsed=$(tail -n 6000 "$TRANSCRIPT" 2>/dev/null \
  | jq -rc 'select(.type=="assistant") | (.message.content // [])[]? | select(.type=="tool_use" and (.name|test("memory_store"))) | (.input|tostring)' 2>/dev/null || true)
n_parsed=$(printf '%s\n' "$parsed" | grep -c . 2>/dev/null || true)

if [ "${n_parsed:-0}" -gt 0 ]; then
  # Parser works -> require a memory_store that references this PR/branch (literal match).
  printf '%s\n' "$parsed" | grep -qiF "${needles[@]}" && exit 0
  deny "Capture-before-merge: store the durable decisions/learnings for PR #${PR:-<n>} (branch '${BRANCH:-?}') to the memory MCP BEFORE merging — call mcp__memory__memory_store and reference this PR: put 'PR #${PR:-<n>}' (or the branch '${BRANCH:-?}' if there is no PR number) in the title/content/tags (a one-line episodic note is fine if nothing major shipped). Then re-run the merge. See your project's branching-model doc."
fi

# Parser found none -> disambiguate format drift (allow) from truly-no-capture (deny).
if tail -c 500000 "$TRANSCRIPT" 2>/dev/null | grep -qa 'memory_store'; then
  exit 0   # a memory_store is present but unparsed (format drift) -> fail open
fi
deny "Capture-before-merge: no memory MCP entry was stored this session, so merging PR #${PR:-<n>} (branch '${BRANCH:-?}') would lose its knowledge. Call mcp__memory__memory_store with the durable decisions/learnings (reference 'PR #${PR:-<n>}' or the branch; a one-line episodic note suffices), then re-run the merge. See your project's branching-model doc."
