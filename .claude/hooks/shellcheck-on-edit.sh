#!/usr/bin/env bash
# PostToolUse (Edit|Write): lint edited shell scripts with shellcheck and surface
# findings back to Claude in the SAME turn, so shell bugs get fixed before the
# script is relied on. The enforcement hooks in this repo gate everything else,
# yet were themselves unlinted — this closes that gap.
#
# Fail-open posture (mirrors lefthook's gitleaks step + prettier-on-edit.sh):
#   - never blocks the edit OR the turn (PostToolUse runs AFTER the write; the file
#     is already saved — findings are injected as non-blocking additionalContext)
#   - degrades to a silent no-op when shellcheck is absent
#   - only warning+error severity is surfaced (style/info noise is skipped)
set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0 # jq drives parse + output → fail open without it

input=$(cat)
file=$(echo "$input" | jq -r '.tool_input.file_path // .tool_input.path // ""')

case "$file" in
  *.sh | *.bash) ;;
  *) exit 0 ;;
esac

[ -f "$file" ] || exit 0
command -v shellcheck >/dev/null 2>&1 || exit 0 # not installed → fail open

findings=$(shellcheck -S warning -f gcc "$file" 2>/dev/null) || true

if [ -n "$findings" ]; then
  note="shellcheck (warning+) findings in $file — fix before relying on this script:
$findings"
  # PostToolUse: inject as non-blocking additionalContext so findings surface to
  # Claude in the same turn WITHOUT interrupting it (decision:"block" can halt the
  # turn depending on version/continueOnBlock — additionalContext never does).
  # jq -n builds valid JSON regardless of quotes/newlines in the findings.
  jq -n --arg c "$note" \
    '{hookSpecificOutput: {hookEventName: "PostToolUse", additionalContext: $c}}'
fi
exit 0
